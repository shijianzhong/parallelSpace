//! 积分入账（幂等核心）。
//!
//! 硬规则（AGENTS.md §6/自检清单）：
//! - 所有积分变更必须走本模块的 confirm_payment，禁止直接改 credit_accounts.balance；
//! - 幂等：`UPDATE orders ... WHERE id=? AND status='pending'`，影响行数=1 才入账；
//! - 事件 ontology_version 由 OntologyRouter 注入，调用方传不进来了；
//! - 治理员确认需 admin 校验 + 二次核对金额和订单号（api/payment.rs 负责）。

use worker::Env;

use crate::common::error::{AppError, AppResult};
use crate::common::types::now_rfc3339;
use crate::event::log;
use crate::ontology::version::OntologyRouter;
use crate::storage::{d1, kv};

pub struct ConfirmResult {
    pub order_id: String,
    pub account_id: String,
    pub credits: i64,
    pub new_balance: i64,
    pub already_confirmed: bool,
}

/// 确认支付并幂等入账。
///
/// 二次核对：调用方（治理接口）必须已校验金额与订单号，本函数复核订单存在且
/// 处于 pending；重复确认返回 already_confirmed=true（不报错、不重复入账）。
pub async fn confirm_payment(
    env: &Env,
    order_id: &str,
    channel_txn_id: &str,
    confirmed_by: &str,
) -> AppResult<ConfirmResult> {
    if order_id.is_empty() || channel_txn_id.is_empty() || confirmed_by.is_empty() {
        return Err(AppError::InvalidInput("missing required fields".into()));
    }

    let db = env.d1("DB")?;
    let router: OntologyRouter = OntologyRouter::from_env(env).await?;
    let ontology_version = router.for_action().as_str().to_string();

    // 1. 读订单
    let order = d1::get_order(&db, order_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("order {order_id}")))?;

    // 2. 已支付直接返回（幂等读路径）
    if order.status == "paid" {
        let balance = current_balance(&db, env, &order.account_id).await?;
        return Ok(ConfirmResult {
            order_id: order_id.to_string(),
            account_id: order.account_id,
            credits: order.credits,
            new_balance: balance,
            already_confirmed: true,
        });
    }
    if order.status != "pending" {
        return Err(AppError::Conflict(format!("order status is {}", order.status)));
    }

    // 3. 渠道流水号查重：手工支付场景最现实的重复入账路径是「同一微信流水号
    //    被填到另一笔待付订单」，所以必须在落库前拦住。
    if let Some(other) = d1::find_order_by_channel_txn(&db, &order.payment_channel, channel_txn_id)
        .await?
    {
        return Err(AppError::Conflict(format!(
            "channel_txn_id already used by order {other}"
        )));
    }

    // 4. 条件更新（并发下只有一个调用方能更新成功）
    let updated = d1::mark_order_paid(&db, order_id, channel_txn_id, confirmed_by).await?;
    if !updated {
        let balance = current_balance(&db, env, &order.account_id).await?;
        return Ok(ConfirmResult {
            order_id: order_id.to_string(),
            account_id: order.account_id,
            credits: order.credits,
            new_balance: balance,
            already_confirmed: true,
        });
    }

    // 5. 写流水 + upsert 账户（batch 原子）
    let txn_id = ulid::Ulid::generate().to_string();
    let event_id = ulid::Ulid::generate().to_string();
    let now = now_rfc3339();
    let new_balance = d1::apply_credit_purchase(
        &db,
        &txn_id,
        &order.account_id,
        order.credits,
        order_id,
        &event_id,
        &now,
    )
    .await?;

    // 6. econ:CreditPurchased 事件（R2 append-only + D1 索引）
    let payload = serde_json::json!({
        "order_id": order_id,
        "account_id": order.account_id,
        "amount_cents": order.amount_cents,
        "credits": order.credits,
        "payment_channel": order.payment_channel,
        "channel_txn_id": channel_txn_id,
        "confirmed_by": confirmed_by,
        "new_balance": new_balance,
    });
    let tick = crate::agent_runtime::scheduler::current_tick();
    log::append(
        env,
        crate::ontology::iri::CREDIT_PURCHASED,
        &event_id,
        &ontology_version,
        tick,
        Some(&order.account_id),
        &payload,
    )
    .await?;

    // 7. 刷新余额缓存
    if let Ok(kv_store) = env.kv("KV") {
        kv::put_credit_balance_cache(&kv_store, &order.account_id, new_balance).await?;
    }

    Ok(ConfirmResult {
        order_id: order_id.to_string(),
        account_id: order.account_id,
        credits: order.credits,
        new_balance,
        already_confirmed: false,
    })
}

async fn current_balance(
    db: &worker::D1Database,
    env: &Env,
    account_id: &str,
) -> AppResult<i64> {
    if let Ok(kv_store) = env.kv("KV") {
        if let Some(cached) = kv::get_credit_balance_cache(&kv_store, account_id).await? {
            return Ok(cached);
        }
    }
    Ok(d1::get_credit_account(db, account_id)
        .await?
        .map(|a| a.balance)
        .unwrap_or(0))
}
