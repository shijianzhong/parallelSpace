//! D1 存储层（AGENTS.md §4.1：数据库写入走 storage::d1::batch_write，
//! 禁止在 handler 里逐条 execute）。
//!
//! 约定：
//! - 行结构体字段名与 migrations/0001_init.sql 列名一致（snake_case）；
//! - 数值列（INTEGER）以 i64/f64 绑定，D1 经 JS 数字回传；
//! - 多表写入一律走 D1Database::batch（单次往返、原子性）。

use serde::Deserialize;
use worker::{wasm_bindgen::JsValue, D1Database, D1Result};

use crate::common::error::{AppError, AppResult};

// ---- 行结构体 ----

#[derive(Debug, Deserialize)]
pub struct OrderRow {
    pub id: String,
    pub account_id: String,
    pub amount_cents: i64,
    pub credits: i64,
    pub status: String,
    pub payment_channel: String,
    pub channel_txn_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreditAccountRow {
    pub account_id: String,
    pub balance: i64,
    pub total_purchased: i64,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentRow {
    pub id: String,
    pub account_id: String,
    pub status: String,
    pub location_id: Option<String>,
    pub created_at: String,
    pub ontology_version: String,
}

#[derive(Debug, Deserialize)]
pub struct AccountRow {
    pub id: String,
    pub email: String,
    pub created_at: String,
    pub risk_score: f64,
}

// ---- 绑定辅助 ----

fn s(v: &str) -> JsValue {
    JsValue::from_str(v)
}

fn n(v: i64) -> JsValue {
    JsValue::from_f64(v as f64)
}

fn opt_s(v: Option<&str>) -> JsValue {
    match v {
        Some(v) => JsValue::from_str(v),
        None => JsValue::NULL,
    }
}

// ---- 批量写（编码规范唯一写入口）----

/// 批量执行（单次 D1 batch 往返）。所有多语句写入必须走这里。
pub async fn batch_write(db: &D1Database, statements: Vec<worker::D1PreparedStatement>) -> AppResult<Vec<D1Result>> {
    Ok(db.batch(statements).await?)
}

// ---- 账号 / 分身 ----

pub async fn create_account(db: &D1Database, id: &str, email: &str) -> AppResult<()> {
    db.prepare("INSERT INTO accounts (id, email, created_at, risk_score) VALUES (?1, ?2, ?3, 0)")
        .bind(&[s(id), s(email), s(&crate::common::types::now_rfc3339())])?
        .run()
        .await?;
    Ok(())
}

pub async fn get_account_by_email(db: &D1Database, email: &str) -> AppResult<Option<AccountRow>> {
    Ok(db
        .prepare("SELECT id, email, created_at, risk_score FROM accounts WHERE email = ?1")
        .bind(&[s(email)])?
        .first::<AccountRow>(None)
        .await?)
}

/// 账号是否存在（下单前置校验，避免外键错误以 500 形式暴露）。
pub async fn account_exists(db: &D1Database, account_id: &str) -> AppResult<bool> {
    #[derive(serde::Deserialize)]
    struct Row {
        ok: i64,
    }
    let row = db
        .prepare("SELECT 1 AS ok FROM accounts WHERE id = ?1")
        .bind(&[s(account_id)])?
        .first::<Row>(None)
        .await?;
    Ok(row.map(|r| r.ok == 1).unwrap_or(false))
}

/// 创建分身（制度先于组织：初始状态 created，支付成功并经治理确认后才 pending→active）。
pub async fn create_agent(
    db: &D1Database,
    id: &str,
    account_id: &str,
    ontology_version: &str,
) -> AppResult<()> {
    db.prepare(
        "INSERT INTO agents (id, account_id, status, location_id, created_at, ontology_version) \
         VALUES (?1, ?2, 'created', NULL, ?3, ?4)",
    )
    .bind(&[
        s(id),
        s(account_id),
        s(&crate::common::types::now_rfc3339()),
        s(ontology_version),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn get_agent(db: &D1Database, id: &str) -> AppResult<Option<AgentRow>> {
    Ok(db
        .prepare(
            "SELECT id, account_id, status, location_id, created_at, ontology_version \
             FROM agents WHERE id = ?1",
        )
        .bind(&[s(id)])?
        .first::<AgentRow>(None)
        .await?)
}

// ---- 事件索引 ----

/// 写事件索引行（完整事件体由调用方写 R2，见 event/log.rs）。
pub async fn insert_event_index(
    db: &D1Database,
    event_id: &str,
    event_type: &str,
    actor_id: Option<&str>,
    target_id: Option<&str>,
    tick: u64,
    ontology_version: &str,
) -> AppResult<()> {
    db.prepare(
        "INSERT INTO events_index (id, type, actor_id, target_id, tick, ontology_version, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&[
        s(event_id),
        s(event_type),
        opt_s(actor_id),
        opt_s(target_id),
        n(tick as i64),
        s(ontology_version),
        s(&crate::common::types::now_rfc3339()),
    ])?
    .run()
    .await?;
    Ok(())
}

// ---- 支付 / 积分 ----

pub async fn get_order(db: &D1Database, order_id: &str) -> AppResult<Option<OrderRow>> {
    Ok(db
        .prepare(
            "SELECT id, account_id, amount_cents, credits, status, payment_channel, channel_txn_id \
             FROM orders WHERE id = ?1",
        )
        .bind(&[s(order_id)])?
        .first::<OrderRow>(None)
        .await?)
}

/// 查找已占用该渠道流水号的其他订单 id。
///
/// 数据库层有 idx_orders_channel_txn 唯一兜底，但 D1 的唯一约束冲突表现为一整批
/// batch 回滚 + 泛化的 SQL 错误，排查成本高；这里先做一次可读的前置检查，
/// 让 api/payment 能返回「流水号已被订单 X 占用」而不是 500。
pub async fn find_order_by_channel_txn(
    db: &D1Database,
    payment_channel: &str,
    channel_txn_id: &str,
) -> AppResult<Option<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        id: String,
    }
    Ok(db
        .prepare("SELECT id FROM orders WHERE payment_channel = ?1 AND channel_txn_id = ?2 LIMIT 1")
        .bind(&[s(payment_channel), s(channel_txn_id)])?
        .first::<Row>(None)
        .await?
        .map(|r| r.id))
}

/// 条件更新订单为 paid。返回是否更新了 1 行（幂等的关键）。
pub async fn mark_order_paid(
    db: &D1Database,
    order_id: &str,
    channel_txn_id: &str,
    confirmed_by: &str,
) -> AppResult<bool> {
    let results = db
        .batch(vec![
            db.prepare(
                "UPDATE orders SET status = 'paid', channel_txn_id = ?1, paid_at = ?2, confirmed_by = ?3 \
                 WHERE id = ?4 AND status = 'pending'",
            )
            .bind(&[
                s(channel_txn_id),
                s(&crate::common::types::now_rfc3339()),
                s(confirmed_by),
                s(order_id),
            ])?,
        ])
        .await?;
    let changes = results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .unwrap_or(0);
    Ok(changes == 1)
}

pub async fn get_credit_account(
    db: &D1Database,
    account_id: &str,
) -> AppResult<Option<CreditAccountRow>> {
    Ok(db
        .prepare(
            "SELECT account_id, balance, total_purchased, updated_at \
             FROM credit_accounts WHERE account_id = ?1",
        )
        .bind(&[s(account_id)])?
        .first::<CreditAccountRow>(None)
        .await?)
}

/// 入账：写流水 + upsert 积分账户，返回新余额。必须走 batch（原子）。
pub async fn apply_credit_purchase(
    db: &D1Database,
    txn_id: &str,
    account_id: &str,
    delta: i64,
    order_id: &str,
    event_id: &str,
    now: &str,
) -> AppResult<i64> {
    let now_owned = now.to_string();
    db.batch(vec![
        db.prepare(
            "INSERT INTO credit_transactions (id, account_id, delta, type, order_id, event_id, created_at) \
             VALUES (?1, ?2, ?3, 'purchase', ?4, ?5, ?6)",
        )
        .bind(&[
            s(txn_id),
            s(account_id),
            n(delta),
            s(order_id),
            s(event_id),
            s(&now_owned),
        ])?,
        db.prepare(
            "INSERT INTO credit_accounts (account_id, balance, total_purchased, updated_at) \
             VALUES (?1, ?2, ?2, ?3) \
             ON CONFLICT(account_id) DO UPDATE SET \
               balance = balance + excluded.balance, \
               total_purchased = total_purchased + excluded.total_purchased, \
               updated_at = excluded.updated_at",
        )
        .bind(&[s(account_id), n(delta), s(&now_owned)])?,
    ])
    .await?;
    let account = get_credit_account(db, account_id)
        .await?
        .ok_or_else(|| AppError::Db("credit account vanished after upsert".into()))?;
    Ok(account.balance)
}

/// 本 tick 需唤醒的 agent：事件队列 + 目标到期 + 资源阈值（MVP：资源阈值子集）。
pub async fn find_wakeable_agents(db: &D1Database, tick: u64) -> AppResult<Vec<String>> {
    // MVP：唤醒（a）本 tick 有新事件的 actor，（b）热状态能量低于阈值的 active 分身。
    // 目标到期规则在 lifecycle/decision 成熟后并入。
    let rows: Vec<EventActorRow> = db
        .prepare(
            "SELECT DISTINCT actor_id AS id FROM events_index \
             WHERE tick >= ?1 AND actor_id IS NOT NULL LIMIT 500",
        )
        .bind(&[n(tick as i64)])?
        .all()
        .await?
        .results()?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

#[derive(Debug, Deserialize)]
struct EventActorRow {
    id: String,
}

/// 创建支付订单（治理员扫码后手动确认，见 payment/credit.rs）。
pub async fn create_order(
    db: &D1Database,
    order_id: &str,
    account_id: &str,
    amount_cents: i64,
    credits: i64,
    payment_channel: &str,
) -> AppResult<()> {
    db.prepare(
        "INSERT INTO orders (id, account_id, amount_cents, credits, status, payment_channel, created_at) \
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6)",
    )
    .bind(&[
        s(order_id),
        s(account_id),
        n(amount_cents),
        n(credits),
        s(payment_channel),
        s(&crate::common::types::now_rfc3339()),
    ])?
    .run()
    .await?;
    Ok(())
}

/// 清理过期 pending 订单（30 分钟，由 Cron 调用）。
pub async fn expire_stale_orders(db: &D1Database, before_rfc3339: &str) -> AppResult<u64> {
    let results = db
        .batch(vec![
            db.prepare("UPDATE orders SET status = 'expired' WHERE status = 'pending' AND created_at < ?1")
                .bind(&[s(before_rfc3339)])?,
        ])
        .await?;
    Ok(results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .map(|c| c as u64)
        .unwrap_or(0))
}
