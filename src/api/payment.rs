//! 支付确认（治理员手动通道）：POST /api/payment/confirm。
//!
//! 安全（AGENTS.md §4.4）：admin 角色校验（x-admin-token 对 env ADMIN_TOKEN），
//! 二次核对金额与订单号（body 携带 expected_amount_cents，与订单不一致即拒）。
//! 入账一律走 payment::credit::confirm_payment（幂等）。

use serde::Deserialize;
use worker::{Response, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::payment::credit;
use crate::payment::manual_wechat::ManualWechatProvider;
use crate::payment::packages;
use crate::payment::provider::{Order, PaymentProvider};

#[derive(Debug, Deserialize)]
pub struct CreateOrderInput {
    pub account_id: String,
    /// 套餐 id（服务端定价；客户端不得传金额）
    pub package_id: String,
}

/// 下单：POST /api/payment/order。
///
/// 金额与积分一律由 `payment::packages` 决定，body 里的金额字段会被忽略
/// （没有该字段），从根上堵住「改价购买」。
pub async fn create_order(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let input: CreateOrderInput = json_body(&mut req).await?;
    if input.account_id.is_empty() {
        return Err(AppError::InvalidInput("account_id required".into()));
    }

    let pkg = packages::find_package(&input.package_id)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown package: {}", input.package_id)))?;

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    // 配置先校验：QR 地址缺失就不该落订单，否则留一堆永远付不了的孤儿订单
    let qr_url = env
        .var("MANUAL_WECHAT_QR_URL")
        .map(|v| v.to_string())
        .unwrap_or_default();
    if qr_url.is_empty() {
        return Err(AppError::Internal(
            "MANUAL_WECHAT_QR_URL not configured".into(),
        ));
    }

    // 账号必须存在：否则会造出无法确认的孤儿订单（虽然外键也会挡，但错误不可读）
    if !crate::storage::d1::account_exists(&db, &input.account_id).await? {
        return Err(AppError::NotFound(format!("account {}", input.account_id)));
    }

    let order_id = ulid::Ulid::generate().to_string();
    crate::storage::d1::create_order(
        &db,
        &order_id,
        &input.account_id,
        pkg.amount_cents,
        pkg.credits,
        "manual_wechat",
    )
    .await?;

    let provider = ManualWechatProvider::new(qr_url);
    let intent = provider
        .create_order(&Order {
            id: order_id.clone(),
            account_id: input.account_id.clone(),
            amount_cents: pkg.amount_cents,
            credits: pkg.credits,
            status: "pending".into(),
            payment_channel: "manual_wechat".into(),
            channel_txn_id: None,
        })
        .await?;

    crate::governance::audit::record(
        &env,
        "payment.order_created",
        serde_json::json!({
            "order_id": order_id,
            "account_id": input.account_id,
            "package_id": pkg.id,
            "amount_cents": pkg.amount_cents,
            "credits": pkg.credits,
        }),
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "order_id": intent.order_id,
        "package_id": pkg.id,
        "amount_cents": pkg.amount_cents,
        "credits": pkg.credits,
        "qr_code_url": intent.qr_code_url,
        "expires_at": intent.expires_at,
    }))?)
}

#[derive(Debug, Deserialize)]
pub struct ConfirmInput {
    pub order_id: String,
    pub channel_txn_id: String,
    /// 治理员二次核对：期望金额（分），与订单不一致拒绝
    pub expected_amount_cents: Option<i64>,
    pub note: Option<String>,
}

/// admin 令牌校验（fail closed）：未配置 ADMIN_TOKEN 或 header 不匹配一律拒绝。
/// 校验在 confirm() 内联完成，避免 secret 在辅助函数里多次读取。
pub async fn confirm(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    // admin 校验（header 方式）
    let expected = ctx
        .env
        .secret("ADMIN_TOKEN")
        .map(|s| s.to_string())
        .unwrap_or_default();
    if expected.is_empty() {
        return Err(AppError::Forbidden);
    }
    let provided = req
        .headers()
        .get("x-admin-token")
        .ok()
        .flatten()
        .unwrap_or_default();
    if provided != expected {
        return Err(AppError::Unauthorized);
    }

    let input: ConfirmInput = json_body(&mut req).await?;
    if input.order_id.is_empty() || input.channel_txn_id.is_empty() {
        return Err(AppError::InvalidInput(
            "order_id and channel_txn_id required".into(),
        ));
    }

    let env = ctx.env.clone();

    // 二次核对金额
    if let Some(expected_cents) = input.expected_amount_cents {
        let db = env.d1("DB")?;
        let order = crate::storage::d1::get_order(&db, &input.order_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("order {}", input.order_id)))?;
        if order.amount_cents != expected_cents {
            crate::governance::audit::record(
                &env,
                "payment.confirm_rejected_amount_mismatch",
                serde_json::json!({
                    "order_id": input.order_id,
                    "expected_cents": expected_cents,
                    "actual_cents": order.amount_cents,
                }),
            )
            .await?;
            return Err(AppError::Conflict(
                "amount mismatch: expected_cents does not match order".into(),
            ));
        }
    }

    let result = credit::confirm_payment(&env, &input.order_id, &input.channel_txn_id, "admin")
        .await?;

    crate::governance::audit::record(
        &env,
        "payment.confirm",
        serde_json::json!({
            "order_id": result.order_id,
            "account_id": result.account_id,
            "credits": result.credits,
            "new_balance": result.new_balance,
            "already_confirmed": result.already_confirmed,
            "note": input.note,
        }),
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "order_id": result.order_id,
        "account_id": result.account_id,
        "credits": result.credits,
        "new_balance": result.new_balance,
        "already_confirmed": result.already_confirmed,
    }))?)
}

// ---------- 转让 / 收益 / 兑换（AGENTS §5.7；当前无用户认证 G7，全部 admin 代办） ----------

fn check_admin_headers(req: &worker::Request, ctx: &RouteContext<()>) -> AppResult<String> {
    crate::api::admin::check_admin(req, ctx).map_err(|_| AppError::Unauthorized)?;
    Ok("payment-admin".to_string())
}

fn json_ok(value: serde_json::Value) -> AppResult<Response> {
    Ok(Response::from_json(&value)?)
}

#[derive(Debug, Deserialize)]
pub struct TransferInput {
    pub from_account_id: String,
    pub to_account_id: String,
    pub amount: i64,
}

/// 积分转让（admin 代办）：POST /api/payment/transfer。
pub async fn transfer(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin_headers(&req, &ctx)?;
    let input: TransferInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let r = credit::transfer_credits(&env, &input.from_account_id, &input.to_account_id, input.amount)
        .await?;
    json_ok(serde_json::json!({
        "fromAccountId": input.from_account_id,
        "toAccountId": input.to_account_id,
        "amount": input.amount,
        "fromBalance": r.from_balance,
        "toBalance": r.to_balance,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AgentEarningsInput {
    pub agent_id: String,
    pub credits: i64,
    pub source_ref: String,
}

/// agent 收益入账（admin 代办；G16 引擎落地后内部调用同一函数）：
/// POST /api/admin/payment/agent-earnings。
pub async fn agent_earnings(
    mut req: worker::Request,
    ctx: RouteContext<()>,
) -> AppResult<Response> {
    check_admin_headers(&req, &ctx)?;
    let input: AgentEarningsInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let new_balance = credit::credit_agent_earnings(&env, &input.agent_id, input.credits, &input.source_ref).await?;
    json_ok(serde_json::json!({
        "agentId": input.agent_id,
        "credits": input.credits,
        "newBalance": new_balance,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RedeemInput {
    pub account_id: String,
    pub credits: i64,
}

/// 发起兑换（admin 代办，申请即冻结）：POST /api/admin/payment/redeem。
pub async fn redeem_request(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin_headers(&req, &ctx)?;
    let input: RedeemInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let r = credit::request_redeem(&env, &input.account_id, input.credits).await?;
    json_ok(serde_json::json!({
        "requestId": r.request_id,
        "accountId": r.account_id,
        "credits": r.credits,
        "amountCents": r.amount_cents,
        "status": r.status,
        "newBalance": r.new_balance,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RedeemSettleInput {
    pub payout_ref: String,
}

/// 兑换结算（确认已人工打款）：POST /api/admin/payment/redeem/:id/settle。
pub async fn redeem_settle(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let decided_by = check_admin_headers(&req, &ctx)?;
    let id = ctx.param("id").map(|s| s.to_string()).unwrap_or_default();
    let input: RedeemSettleInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let r = credit::settle_redeem(&env, &id, &input.payout_ref, &decided_by).await?;
    json_ok(serde_json::json!({
        "requestId": r.request_id,
        "status": r.status,
        "amountCents": r.amount_cents,
        "newBalance": r.new_balance,
    }))
}

/// 兑换驳回（释放冻结）：POST /api/admin/payment/redeem/:id/reject。
pub async fn redeem_reject(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let decided_by = check_admin_headers(&req, &ctx)?;
    let id = ctx.param("id").map(|s| s.to_string()).unwrap_or_default();
    let env = ctx.env.clone();
    let r = credit::reject_redeem(&env, &id, &decided_by).await?;
    json_ok(serde_json::json!({
        "requestId": r.request_id,
        "status": r.status,
        "newBalance": r.new_balance,
    }))
}

// ---------- 用户自助资金操作（需会话，G7 后开放；复用 §5.7 账本函数） ----------

/// 我的积分流水：GET /api/me/transactions?limit=50（需会话，只读）。
/// 余额与 /api/me 同源（credit_accounts），流水来自 append-only 的
/// credit_transactions（§5.7 三条资金路径的 type 都在其中）。
pub async fn me_transactions(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = crate::api::session::require_session(&req, &env).await?;
    let limit: u32 = req
        .url()?
        .query_pairs()
        .find(|(k, _)| k == "limit")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let db = env.d1("DB")?;
    let balance = crate::storage::d1::get_credit_account(&db, &account_id)
        .await?
        .map(|a| a.balance)
        .unwrap_or(0);
    let rows = crate::storage::d1::list_credit_transactions(&db, &account_id, limit).await?;
    let transactions: Vec<serde_json::Value> = rows
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "delta": t.delta,
                "type": t.txn_type,
                "order_id": t.order_id,
                "event_id": t.event_id,
                "created_at": t.created_at,
            })
        })
        .collect();
    json_ok(serde_json::json!({
        "account_id": account_id,
        "balance": balance,
        "transactions": transactions,
    }))
}

#[derive(Debug, Deserialize)]
pub struct MeTransferInput {
    pub to_account_id: String,
    pub amount: i64,
}

/// 自助转让：POST /api/me/transfer（出账方固定为会话账号，不可代转）。
pub async fn me_transfer(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let from = crate::api::session::require_session(&req, &env).await?;
    let input: MeTransferInput = json_body(&mut req).await?;
    let r = credit::transfer_credits(&env, &from, &input.to_account_id, input.amount).await?;
    json_ok(serde_json::json!({
        "fromAccountId": from,
        "toAccountId": input.to_account_id,
        "amount": input.amount,
        "fromBalance": r.from_balance,
        "toBalance": r.to_balance,
    }))
}

#[derive(Debug, Deserialize)]
pub struct MeRedeemInput {
    pub credits: i64,
}

/// 自助兑换申请：POST /api/me/redeem（冻结后仍走 admin 审批 settle/reject）。
pub async fn me_redeem(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = crate::api::session::require_session(&req, &env).await?;
    let input: MeRedeemInput = json_body(&mut req).await?;
    let r = credit::request_redeem(&env, &account_id, input.credits).await?;
    json_ok(serde_json::json!({
        "requestId": r.request_id,
        "accountId": r.account_id,
        "credits": r.credits,
        "amountCents": r.amount_cents,
        "status": r.status,
        "newBalance": r.new_balance,
    }))
}
