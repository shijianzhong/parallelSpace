//! 积分入账（幂等核心）。
//!
//! 硬规则（AGENTS.md §6/自检清单）：
//! - 所有积分变更必须走本模块的函数（confirm_payment / transfer_credits / credit_agent_earnings / redeem 流程，见 §5.7），禁止直接改 credit_accounts.balance；
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

    // 7. 分身状态推进 created → pending（注册主线的中间站，见 GAPS G1）：
    // 本函数只在首次入账路径走到这里（重放走 already_confirmed 提前返回），
    // 天然幂等。推进失败不回滚已落账的支付（各步本就分批提交、非单事务），
    // 只记日志——激活接口凭 paid 订单可自愈补走这一步（api/agent.rs）。
    match d1::get_agent_by_account(&db, &order.account_id).await {
        Ok(Some(agent)) if agent.status == "created" => {
            match d1::transition_agent_status(&db, &agent.id, "created", "pending").await {
                Ok(true) => crate::common::logging::info(
                    "agent_created_to_pending",
                    serde_json::json!({ "agent_id": agent.id, "order_id": order_id }),
                ),
                Ok(false) => {}
                Err(e) => crate::common::logging::error(
                    "agent_pending_transition_failed",
                    serde_json::json!({ "agent_id": agent.id, "error": e.to_string() }),
                ),
            }
        }
        Ok(_) => {}
        Err(e) => crate::common::logging::error(
            "agent_lookup_for_pending_failed",
            serde_json::json!({ "account_id": order.account_id, "error": e.to_string() }),
        ),
    }

    // 8. 刷新余额缓存
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

// ---------- 转让 / agent 收益 / 兑换（AGENTS §5.7，2026-10-23 定调开放） ----------

fn tick_now() -> u64 {
    crate::agent_runtime::scheduler::current_tick()
}

async fn version_now(env: &Env) -> AppResult<String> {
    let router = OntologyRouter::from_env(env).await?;
    Ok(router.for_action().as_str().to_string())
}

async fn refresh_cache(env: &Env, account_id: &str, balance: i64) {
    if let Ok(kv_store) = env.kv("KV") {
        let _ = kv::put_credit_balance_cache(&kv_store, account_id, balance).await;
    }
}

/// 转让入参校验（纯函数，可单测）。
pub fn validate_transfer(from: &str, to: &str, amount: i64) -> AppResult<()> {
    if from.is_empty() || to.is_empty() {
        return Err(AppError::InvalidInput("missing account id".into()));
    }
    if from == to {
        return Err(AppError::InvalidInput(
            "cannot transfer to self".into(),
        ));
    }
    if amount <= 0 {
        return Err(AppError::InvalidInput("amount must be positive".into()));
    }
    Ok(())
}

pub struct TransferResult {
    pub from_balance: i64,
    pub to_balance: i64,
}

/// 账号间转让积分：条件扣减占住余额 → 事件 → 入账+成对流水同一 batch。
/// 总量守恒；`total_purchased` 不受影响（转让不是购买）。
pub async fn transfer_credits(
    env: &Env,
    from_account: &str,
    to_account: &str,
    amount: i64,
) -> AppResult<TransferResult> {
    validate_transfer(from_account, to_account, amount)?;
    let db = env.d1("DB")?;
    if !d1::account_exists(&db, from_account).await?
        || !d1::account_exists(&db, to_account).await?
    {
        return Err(AppError::NotFound("account not found".into()));
    }
    let now = now_rfc3339();
    // 1. 条件扣减（余额不足即中止，未动分毫）
    if !d1::conditional_debit_credits(&db, from_account, amount, &now).await? {
        return Err(AppError::Conflict("insufficient balance".into()));
    }
    // 2. 事件（append-only）
    let event_id = ulid::Ulid::generate().to_string();
    if let Err(e) = log::append(
        env,
        &crate::ontology::iri::econ("CreditsTransferred"),
        &event_id,
        &version_now(env).await?,
        tick_now(),
        None,
        &serde_json::json!({"fromAccount": from_account, "toAccount": to_account, "amount": amount}),
    )
    .await
    {
        // 已扣未入账是最坏中间态：大声记日志（治理侧据此对账修复），不静默吞
        worker::console_error!(
            "transfer {event_id}: debited {from_account} but event append failed: {e}"
        );
        return Err(e);
    }
    // 3. 入账 + 成对流水
    let (from_balance, to_balance) = d1::apply_credit_transfer_in(
        &db,
        &ulid::Ulid::generate().to_string(),
        &ulid::Ulid::generate().to_string(),
        from_account,
        to_account,
        amount,
        &event_id,
        &now,
    )
    .await?;
    refresh_cache(env, from_account, from_balance).await;
    refresh_cache(env, to_account, to_balance).await;
    Ok(TransferResult {
        from_balance,
        to_balance,
    })
}

/// agent 社会收益结算到所属账号（type='agent_earnings'，不计购买额）。
/// G16 结算引擎落地后由其内部调用；当前由 admin 代办接口触发同一函数。
pub async fn credit_agent_earnings(
    env: &Env,
    agent_id: &str,
    credits: i64,
    source_ref: &str,
) -> AppResult<i64> {
    if credits <= 0 {
        return Err(AppError::InvalidInput("credits must be positive".into()));
    }
    let db = env.d1("DB")?;
    let agent = d1::get_agent(&db, agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("agent {agent_id}")))?;
    let event_id = ulid::Ulid::generate().to_string();
    log::append(
        env,
        &crate::ontology::iri::econ("AgentEarningsCredited"),
        &event_id,
        &version_now(env).await?,
        tick_now(),
        Some(agent_id),
        &serde_json::json!({"agentId": agent_id, "accountId": agent.account_id, "credits": credits, "sourceRef": source_ref}),
    )
    .await?;
    let now = now_rfc3339();
    let new_balance = d1::apply_credit_adjustment(
        &db,
        &ulid::Ulid::generate().to_string(),
        &agent.account_id,
        credits,
        "agent_earnings",
        &event_id,
        &now,
    )
    .await?;
    refresh_cache(env, &agent.account_id, new_balance).await;
    Ok(new_balance)
}

pub struct RedeemResult {
    pub request_id: String,
    pub account_id: String,
    pub credits: i64,
    pub amount_cents: i64,
    pub status: String,
    pub new_balance: i64,
}

/// 发起兑换：条件扣减冻结 → 建申请（pending）+ redeem_hold 流水。
/// 打款在 settle 前不发生；驳回由 reject_redeem 释放冻结。
pub async fn request_redeem(env: &Env, account_id: &str, credits: i64) -> AppResult<RedeemResult> {
    if credits <= 0 {
        return Err(AppError::InvalidInput("credits must be positive".into()));
    }
    let db = env.d1("DB")?;
    if !d1::account_exists(&db, account_id).await? {
        return Err(AppError::NotFound("account not found".into()));
    }
    let amount_cents = super::packages::redeem_amount_cents(credits);
    let now = now_rfc3339();
    if !d1::conditional_debit_credits(&db, account_id, credits, &now).await? {
        return Err(AppError::Conflict("insufficient balance".into()));
    }
    let request_id = ulid::Ulid::generate().to_string();
    let event_id = ulid::Ulid::generate().to_string();
    log::append(
        env,
        &crate::ontology::iri::econ("CreditsRedeemRequested"),
        &event_id,
        &version_now(env).await?,
        tick_now(),
        None,
        &serde_json::json!({"requestId": request_id, "accountId": account_id, "credits": credits, "amountCents": amount_cents}),
    )
    .await?;
    d1::create_redeem_request(
        &db,
        &request_id,
        &ulid::Ulid::generate().to_string(),
        account_id,
        credits,
        amount_cents,
        &event_id,
        &now,
    )
    .await?;
    // 冻结扣减后以 D1 为准读新余额（缓存可能仍是旧值，随后刷新）
    let new_balance = d1::get_credit_account(&db, account_id)
        .await?
        .map(|a| a.balance)
        .unwrap_or(0);
    refresh_cache(env, account_id, new_balance).await;
    Ok(RedeemResult {
        request_id,
        account_id: account_id.to_string(),
        credits,
        amount_cents,
        status: "pending".into(),
        new_balance,
    })
}

/// 审批通过并确认已人工打款：仅状态流转（钱在申请时已冻结扣减）。
pub async fn settle_redeem(env: &Env, request_id: &str, payout_ref: &str, decided_by: &str) -> AppResult<RedeemResult> {
    if payout_ref.is_empty() || decided_by.is_empty() {
        return Err(AppError::InvalidInput(
            "payout_ref and decided_by required".into(),
        ));
    }
    let db = env.d1("DB")?;
    let req = d1::get_redeem_request(&db, request_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("redeem request {request_id}")))?;
    if req.status != "pending" {
        return Err(AppError::Conflict(format!(
            "redeem request status is {}",
            req.status
        )));
    }
    let now = now_rfc3339();
    if !d1::decide_redeem_request(&db, request_id, "paid", decided_by, Some(payout_ref), &now).await? {
        return Err(AppError::Conflict(
            "redeem request already decided".into(),
        ));
    }
    let event_id = ulid::Ulid::generate().to_string();
    log::append(
        env,
        &crate::ontology::iri::econ("CreditsRedeemPaid"),
        &event_id,
        &version_now(env).await?,
        tick_now(),
        None,
        &serde_json::json!({"requestId": request_id, "accountId": req.account_id, "credits": req.credits, "amountCents": req.amount_cents, "payoutRef": payout_ref}),
    )
    .await?;
    let balance = d1::get_credit_account(&db, &req.account_id)
        .await?
        .map(|a| a.balance)
        .unwrap_or(0);
    Ok(RedeemResult {
        request_id: request_id.to_string(),
        account_id: req.account_id,
        credits: req.credits,
        amount_cents: req.amount_cents,
        status: "paid".into(),
        new_balance: balance,
    })
}

/// 驳回兑换：状态流转 + 释放冻结（redeem_release 流水把积分加回）。
pub async fn reject_redeem(env: &Env, request_id: &str, decided_by: &str) -> AppResult<RedeemResult> {
    if decided_by.is_empty() {
        return Err(AppError::InvalidInput("decided_by required".into()));
    }
    let db = env.d1("DB")?;
    let req = d1::get_redeem_request(&db, request_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("redeem request {request_id}")))?;
    if req.status != "pending" {
        return Err(AppError::Conflict(format!(
            "redeem request status is {}",
            req.status
        )));
    }
    let now = now_rfc3339();
    if !d1::decide_redeem_request(&db, request_id, "rejected", decided_by, None, &now).await? {
        return Err(AppError::Conflict(
            "redeem request already decided".into(),
        ));
    }
    let event_id = ulid::Ulid::generate().to_string();
    log::append(
        env,
        &crate::ontology::iri::econ("CreditsRedeemRejected"),
        &event_id,
        &version_now(env).await?,
        tick_now(),
        None,
        &serde_json::json!({"requestId": request_id, "accountId": req.account_id, "credits": req.credits}),
    )
    .await?;
    let new_balance = d1::apply_credit_adjustment(
        &db,
        &ulid::Ulid::generate().to_string(),
        &req.account_id,
        req.credits,
        "redeem_release",
        &event_id,
        &now,
    )
    .await?;
    refresh_cache(env, &req.account_id, new_balance).await;
    Ok(RedeemResult {
        request_id: request_id.to_string(),
        account_id: req.account_id,
        credits: req.credits,
        amount_cents: req.amount_cents,
        status: "rejected".into(),
        new_balance,
    })
}

#[cfg(test)]
mod transfer_tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::validate_transfer;

    #[wasm_bindgen_test]
    fn transfer_validation() {
        assert!(validate_transfer("a", "b", 10).is_ok());
        assert!(validate_transfer("a", "a", 10).is_err());
        assert!(validate_transfer("a", "b", 0).is_err());
        assert!(validate_transfer("a", "b", -5).is_err());
        assert!(validate_transfer("", "b", 10).is_err());
    }
}
