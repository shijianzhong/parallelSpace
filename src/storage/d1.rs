//! D1 存储层（AGENTS.md §4.1：数据库写入走 storage::d1::batch_write，
//! 禁止在 handler 里逐条 execute）。
//!
//! 约定：
//! - 行结构体字段名与 migrations/0001_init.sql 列名一致（snake_case）；
//! - 数值列（INTEGER）以 i64/f64 绑定，D1 经 JS 数字回传；
//! - 多表写入一律走 D1Database::batch（单次往返、原子性）。

use serde::{Deserialize, Serialize};
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
    /// 注册时落库的 PersonCard（RDF/Turtle，已过 SHACL）；未携带则为 None（G5）。
    pub person_card_rdf: Option<String>,
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
pub async fn get_account(db: &D1Database, id: &str) -> AppResult<Option<AccountRow>> {
    let stmt = db
        .prepare("SELECT id, email, created_at, risk_score FROM accounts WHERE id = ?1")
        .bind(&[s(id)])?;
    Ok(stmt.first(None).await?)
}

/// 查登录身份绑定的账号（G7 user_identities）。
pub async fn get_identity_account(
    db: &D1Database,
    provider: &str,
    subject: &str,
) -> AppResult<Option<String>> {
    let stmt = db
        .prepare("SELECT account_id FROM user_identities WHERE provider = ?1 AND provider_subject = ?2")
        .bind(&[s(provider), s(subject)])?;
    #[derive(Deserialize)]
    struct IdentityAccountRow {
        account_id: String,
    }
    let row: Option<IdentityAccountRow> = stmt.first(None).await?;
    Ok(row.map(|r| r.account_id))
}

/// 绑定登录身份到账号（已存在同 provider+subject 时不覆盖，防抢绑）。
pub async fn link_identity(
    db: &D1Database,
    account_id: &str,
    provider: &str,
    subject: &str,
    email: &str,
    now: &str,
) -> AppResult<()> {
    db.prepare(
        "INSERT INTO user_identities (account_id, provider, provider_subject, email, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(provider, provider_subject) DO NOTHING",
    )
    .bind(&[s(account_id), s(provider), s(subject), s(email), s(now)])?
    .run()
    .await?;
    Ok(())
}

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

/// 创建分身（初始状态 created；支付首次确认推进 pending，治理员激活后 active）。
/// `person_card_rdf` 为注册时携带并已过 SHACL 的 PersonCard（G5），无则传 None。
pub async fn create_agent(
    db: &D1Database,
    id: &str,
    account_id: &str,
    ontology_version: &str,
    person_card_rdf: Option<&str>,
) -> AppResult<()> {
    db.prepare(
        "INSERT INTO agents (id, account_id, status, location_id, created_at, ontology_version, person_card_rdf) \
         VALUES (?1, ?2, 'created', NULL, ?3, ?4, ?5)",
    )
    .bind(&[
        s(id),
        s(account_id),
        s(&crate::common::types::now_rfc3339()),
        s(ontology_version),
        opt_s(person_card_rdf),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn get_agent(db: &D1Database, id: &str) -> AppResult<Option<AgentRow>> {
    Ok(db
        .prepare(
            "SELECT id, account_id, status, location_id, created_at, ontology_version, person_card_rdf \
             FROM agents WHERE id = ?1",
        )
        .bind(&[s(id)])?
        .first::<AgentRow>(None)
        .await?)
}

/// 取账号名下的分身（MVP 一账号一分身）。
pub async fn get_agent_by_account(
    db: &D1Database,
    account_id: &str,
) -> AppResult<Option<AgentRow>> {
    Ok(db
        .prepare(
            "SELECT id, account_id, status, location_id, created_at, ontology_version, person_card_rdf \
             FROM agents WHERE account_id = ?1 ORDER BY created_at ASC LIMIT 1",
        )
        .bind(&[s(account_id)])?
        .first::<AgentRow>(None)
        .await?)
}

/// 条件迁移分身状态：仅当当前状态恰为 `from` 时才改为 `to`，返回是否迁移了 1 行。
/// 与 `mark_order_paid` 同款写法：状态机的合法性由调用方先用
/// `lifecycle::ensure_transition` 校验，本函数负责并发下的原子抢占——
/// 两个并发迁移只有一个能成功，另一个拿到 false 由调用方转 409。
pub async fn transition_agent_status(
    db: &D1Database,
    agent_id: &str,
    from: &str,
    to: &str,
) -> AppResult<bool> {
    let results = db
        .batch(vec![
            db.prepare("UPDATE agents SET status = ?1 WHERE id = ?2 AND status = ?3")
                .bind(&[s(to), s(agent_id), s(from)])?,
        ])
        .await?;
    let changes = results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .unwrap_or(0);
    Ok(changes == 1)
}

/// 账号是否有已支付订单（激活前置条件：未支付不得激活）。
pub async fn account_has_paid_order(db: &D1Database, account_id: &str) -> AppResult<bool> {
    #[derive(serde::Deserialize)]
    struct Row {
        ok: i64,
    }
    let row = db
        .prepare("SELECT 1 AS ok FROM orders WHERE account_id = ?1 AND status = 'paid' LIMIT 1")
        .bind(&[s(account_id)])?
        .first::<Row>(None)
        .await?;
    Ok(row.map(|r| r.ok == 1).unwrap_or(false))
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

#[derive(Debug, Deserialize)]
pub struct RedeemRow {
    pub id: String,
    pub account_id: String,
    pub credits: i64,
    pub amount_cents: i64,
    pub status: String,
}

/// 条件扣减积分余额（`balance >= amount` 才扣）。返回是否扣成功。
///
/// 转让 / 兑换冻结的第一步：先用这一条把余额占住，失败即中止后续入账，
/// 避免「先入账后发现余额不足」的造币路径（与 mark_order_paid 同一模式）。
pub async fn conditional_debit_credits(
    db: &D1Database,
    account_id: &str,
    amount: i64,
    now: &str,
) -> AppResult<bool> {
    let stmt = db
        .prepare(
            "UPDATE credit_accounts SET balance = balance - ?2, updated_at = ?3 \
             WHERE account_id = ?1 AND balance >= ?2",
        )
        .bind(&[s(account_id), n(amount), s(now)])?;
    let results = db.batch(vec![stmt]).await?;
    let changed = results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .unwrap_or(0);
    Ok(changed == 1)
}

/// 余额上调（不计 total_purchased）：用于 agent 收益入账与兑换驳回释放。
/// delta 必须为正（调用方保证）；写流水与改余额同一 batch。
pub async fn apply_credit_adjustment(
    db: &D1Database,
    txn_id: &str,
    account_id: &str,
    delta: i64,
    txn_type: &str,
    event_id: &str,
    now: &str,
) -> AppResult<i64> {
    db.batch(vec![
        db.prepare(
            "INSERT INTO credit_transactions (id, account_id, delta, type, order_id, event_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
        )
        .bind(&[
            s(txn_id),
            s(account_id),
            n(delta),
            s(txn_type),
            s(event_id),
            s(now),
        ])?,
        db.prepare(
            "INSERT INTO credit_accounts (account_id, balance, total_purchased, updated_at) \
             VALUES (?1, ?2, 0, ?3) \
             ON CONFLICT(account_id) DO UPDATE SET \
               balance = balance + excluded.balance, \
               updated_at = excluded.updated_at",
        )
        .bind(&[s(account_id), n(delta), s(now)])?,
    ])
    .await?;
    let account = get_credit_account(db, account_id)
        .await?
        .ok_or_else(|| AppError::Db("credit account vanished after adjustment".into()))?;
    Ok(account.balance)
}

/// 转让的入账侧：给收款方上调余额 + 成对流水（out/in 共享 event_id）同一 batch。
/// 扣减侧已由 conditional_debit_credits 先行占住余额，本函数只做入账与记账。
#[allow(clippy::too_many_arguments)]
pub async fn apply_credit_transfer_in(
    db: &D1Database,
    out_txn_id: &str,
    in_txn_id: &str,
    from_account: &str,
    to_account: &str,
    amount: i64,
    event_id: &str,
    now: &str,
) -> AppResult<(i64, i64)> {
    db.batch(vec![
        db.prepare(
            "INSERT INTO credit_transactions (id, account_id, delta, type, order_id, event_id, created_at) \
             VALUES (?1, ?2, ?3, 'transfer_out', NULL, ?4, ?5)",
        )
        .bind(&[
            s(out_txn_id),
            s(from_account),
            n(-amount),
            s(event_id),
            s(now),
        ])?,
        db.prepare(
            "INSERT INTO credit_transactions (id, account_id, delta, type, order_id, event_id, created_at) \
             VALUES (?1, ?2, ?3, 'transfer_in', NULL, ?4, ?5)",
        )
        .bind(&[
            s(in_txn_id),
            s(to_account),
            n(amount),
            s(event_id),
            s(now),
        ])?,
        db.prepare(
            "INSERT INTO credit_accounts (account_id, balance, total_purchased, updated_at) \
             VALUES (?1, ?2, 0, ?3) \
             ON CONFLICT(account_id) DO UPDATE SET \
               balance = balance + excluded.balance, \
               updated_at = excluded.updated_at",
        )
        .bind(&[s(to_account), n(amount), s(now)])?,
    ])
    .await?;
    let from = get_credit_account(db, from_account)
        .await?
        .ok_or_else(|| AppError::Db("from credit account vanished".into()))?;
    let to = get_credit_account(db, to_account)
        .await?
        .ok_or_else(|| AppError::Db("to credit account vanished".into()))?;
    Ok((from.balance, to.balance))
}

/// 建兑换申请行 + 冻结流水，同一 batch。余额已由 conditional_debit_credits 扣减（冻结）。
#[allow(clippy::too_many_arguments)]
pub async fn create_redeem_request(
    db: &D1Database,
    request_id: &str,
    hold_txn_id: &str,
    account_id: &str,
    credits: i64,
    amount_cents: i64,
    event_id: &str,
    now: &str,
) -> AppResult<()> {
    db.batch(vec![
        db.prepare(
            "INSERT INTO redeem_requests (id, account_id, credits, amount_cents, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        )
        .bind(&[
            s(request_id),
            s(account_id),
            n(credits),
            n(amount_cents),
            s(now),
        ])?,
        db.prepare(
            "INSERT INTO credit_transactions (id, account_id, delta, type, order_id, event_id, created_at) \
             VALUES (?1, ?2, ?3, 'redeem_hold', ?4, ?5, ?6)",
        )
        .bind(&[
            s(hold_txn_id),
            s(account_id),
            n(-credits),
            s(request_id),
            s(event_id),
            s(now),
        ])?,
    ])
    .await?;
    Ok(())
}

pub async fn get_redeem_request(db: &D1Database, id: &str) -> AppResult<Option<RedeemRow>> {
    let stmt = db
        .prepare("SELECT id, account_id, credits, amount_cents, status FROM redeem_requests WHERE id = ?1")
        .bind(&[s(id)])?;
    Ok(stmt.first(None).await?)
}

/// 条件裁决兑换申请（仅 pending 可裁决）。返回是否命中（防重复 settle/并发双裁）。
pub async fn decide_redeem_request(
    db: &D1Database,
    id: &str,
    new_status: &str,
    decided_by: &str,
    payout_ref: Option<&str>,
    now: &str,
) -> AppResult<bool> {
    let stmt = db
        .prepare(
            "UPDATE redeem_requests SET status = ?2, decided_by = ?3, decided_at = ?4, payout_ref = ?5 \
             WHERE id = ?1 AND status = 'pending'",
        )
        .bind(&[
            s(id),
            s(new_status),
            s(decided_by),
            s(now),
            payout_ref.map(JsValue::from_str).unwrap_or(JsValue::NULL),
        ])?;
    let results = db.batch(vec![stmt]).await?;
    let changed = results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .unwrap_or(0);
    Ok(changed == 1)
}

/// 本 tick 需唤醒的 agent（并集去重，上限 500）：
/// (a) 本 tick 有新事件的 actor（事件驱动）；
/// (b) 状态为 active / idle 的分身（周期心跳）——没有 (b)，世界在无用户行动时
///     会因「没事件 → 不唤醒 → 不产生事件」自举冻死。
/// 注：热状态（能量）在 KV，D1 侧不可见，原「低能量唤醒」设想无法在此实现；
/// 低能量恢复由 decision 在心跳唤醒后自行处理。`agents` 表即权威状态源，
/// 孤儿表 `agent_state` 不参与唤醒判定。
pub async fn find_wakeable_agents(db: &D1Database, tick: u64) -> AppResult<Vec<String>> {
    let rows: Vec<EventActorRow> = db
        .prepare(
            "SELECT id FROM ( \
               SELECT DISTINCT actor_id AS id FROM events_index \
                 WHERE tick >= ?1 AND actor_id IS NOT NULL \
               UNION \
               SELECT id FROM agents WHERE status IN ('active', 'idle') \
             ) LIMIT 500",
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
    let results = batch_write(
        db,
        vec![
            db.prepare("UPDATE orders SET status = 'expired' WHERE status = 'pending' AND created_at < ?1")
                .bind(&[s(before_rfc3339)])?,
        ],
    )
    .await?;
    Ok(results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .map(|c| c as u64)
        .unwrap_or(0))
}

// ---- 本体域注册表 / 版本索引（两段式发布，2026-10-22） ----

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomainRow {
    pub domain: String,
    pub layer: String,
    pub status: String,
    pub description: Option<String>,
    pub registered_by: String,
    pub created_at: String,
    pub activated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OntologyVersionRow {
    pub version: String,
    pub published_at: String,
    pub prior_version: Option<String>,
    /// pending（提案未审）/ active（生效）/ superseded（被新版本取代）/ rolled_back（被回滚离开）
    pub status: String,
    pub proposed_by: Option<String>,
    pub approved_by: Option<String>,
}

pub async fn domain_exists(db: &D1Database, domain: &str) -> AppResult<bool> {
    #[derive(serde::Deserialize)]
    struct Row {
        ok: i64,
    }
    let row = db
        .prepare("SELECT 1 AS ok FROM ontology_domains WHERE domain = ?1")
        .bind(&[s(domain)])?
        .first::<Row>(None)
        .await?;
    Ok(row.map(|r| r.ok == 1).unwrap_or(false))
}

pub async fn get_domain(db: &D1Database, domain: &str) -> AppResult<Option<DomainRow>> {
    Ok(db
        .prepare(
            "SELECT domain, layer, status, description, registered_by, created_at, activated_at \
             FROM ontology_domains WHERE domain = ?1",
        )
        .bind(&[s(domain)])?
        .first::<DomainRow>(None)
        .await?)
}

pub async fn list_domains(db: &D1Database) -> AppResult<Vec<DomainRow>> {
    let rows: Vec<DomainRow> = db
        .prepare(
            "SELECT domain, layer, status, description, registered_by, created_at, activated_at \
             FROM ontology_domains ORDER BY domain",
        )
        .all()
        .await?
        .results()?;
    Ok(rows)
}

/// 注册新域（种子态）。domain_exists 前置检查，避免 D1 唯一冲突整批回滚成 500。
pub async fn register_domain(
    db: &D1Database,
    domain: &str,
    layer: &str,
    description: Option<&str>,
    registered_by: &str,
) -> AppResult<()> {
    if domain_exists(db, domain).await? {
        return Err(AppError::Conflict(format!("domain {domain} already registered")));
    }
    db.prepare(
        "INSERT INTO ontology_domains (domain, layer, status, description, registered_by, created_at) \
         VALUES (?1, ?2, 'seed', ?3, ?4, ?5)",
    )
    .bind(&[
        s(domain),
        s(layer),
        opt_s(description),
        s(registered_by),
        s(&crate::common::types::now_rfc3339()),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn list_versions(db: &D1Database) -> AppResult<Vec<OntologyVersionRow>> {
    let rows: Vec<OntologyVersionRow> = db
        .prepare(
            "SELECT version, published_at, prior_version, status, proposed_by, approved_by \
             FROM ontology_versions ORDER BY published_at DESC, version DESC",
        )
        .all()
        .await?
        .results()?;
    Ok(rows)
}

pub async fn get_version(db: &D1Database, version: &str) -> AppResult<Option<OntologyVersionRow>> {
    Ok(db
        .prepare(
            "SELECT version, published_at, prior_version, status, proposed_by, approved_by \
             FROM ontology_versions WHERE version = ?1",
        )
        .bind(&[s(version)])?
        .first::<OntologyVersionRow>(None)
        .await?)
}

/// 某版本包含的域清单（审批事件负载 + 激活种子域用）。
pub async fn get_version_domains(db: &D1Database, version: &str) -> AppResult<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        domain: String,
    }
    let rows: Vec<Row> = db
        .prepare("SELECT domain FROM ontology_version_domains WHERE version = ?1 ORDER BY domain")
        .bind(&[s(version)])?
        .all()
        .await?
        .results()?;
    Ok(rows.into_iter().map(|r| r.domain).collect())
}

/// 落 pending 版本行 + 版本↔域关系（批量，单次往返）。
pub async fn insert_pending_version(
    db: &D1Database,
    version: &str,
    prior_version: &str,
    proposed_by: &str,
    domains: &[String],
) -> AppResult<()> {
    let mut stmts = vec![db
        .prepare(
            "INSERT INTO ontology_versions (version, published_at, prior_version, status, proposed_by) \
             VALUES (?1, ?2, ?3, 'pending', ?4)",
        )
        .bind(&[
            s(version),
            s(&crate::common::types::now_rfc3339()),
            s(prior_version),
            s(proposed_by),
        ])?];
    for d in domains {
        stmts.push(
            db.prepare("INSERT OR IGNORE INTO ontology_version_domains (version, domain) VALUES (?1, ?2)")
                .bind(&[s(version), s(d)])?,
        );
    }
    batch_write(db, stmts).await?;
    Ok(())
}

/// 审批：pending → active；同批次把此前的 active 版本置 superseded、
/// 把本版本涉及的 seed 域提升为 active（随种子激活）。
/// 返回 false 表示版本不存在或不在 pending（调用方转 409）。
pub async fn approve_version_row(db: &D1Database, version: &str, approved_by: &str) -> AppResult<bool> {
    let now = crate::common::types::now_rfc3339();
    let results = batch_write(
        db,
        vec![
            db.prepare(
                "UPDATE ontology_versions SET status = 'active', approved_by = ?1 \
                 WHERE version = ?2 AND status = 'pending'",
            )
            .bind(&[s(approved_by), s(version)])?,
            db.prepare(
                "UPDATE ontology_versions SET status = 'superseded' \
                 WHERE status = 'active' AND version <> ?1",
            )
            .bind(&[s(version)])?,
            db.prepare(
                "UPDATE ontology_domains SET status = 'active', activated_at = ?1 \
                 WHERE status = 'seed' AND domain IN \
                 (SELECT domain FROM ontology_version_domains WHERE version = ?2)",
            )
            .bind(&[s(&now), s(version)])?,
        ],
    )
    .await?;
    let changes = results
        .first()
        .and_then(|r| r.meta().ok().flatten())
        .and_then(|m| m.changes)
        .unwrap_or(0);
    Ok(changes == 1)
}

/// 回滚时的版本行状态互换：被离开的 active → rolled_back，目标 → active。
pub async fn swap_active_version(db: &D1Database, from_version: &str, to_version: &str) -> AppResult<()> {
    batch_write(
        db,
        vec![
            db.prepare(
                "UPDATE ontology_versions SET status = 'rolled_back' \
                 WHERE version = ?1 AND status = 'active'",
            )
            .bind(&[s(from_version)])?,
            db.prepare("UPDATE ontology_versions SET status = 'active' WHERE version = ?1")
                .bind(&[s(to_version)])?,
        ],
    )
    .await?;
    Ok(())
}
