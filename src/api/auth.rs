//! 注册接口（AGENTS.md §8.1 的 MVP 子集）。
//!
//! 完整链路：账号会话 + 出身模板 + 自然语言叙述 → LLM 结构化为 PersonCard →
//! SHACL 校验 → 支付 → confirm_payment → pending → 治理员激活 → active。
//! 本模块负责第一步：账号/分身落库（created 状态）、输入消毒、审计。
//! 支付推进 pending 在 payment/credit.rs，激活在 api/agent.rs::activate。
//! G5：`POST /api/agent` 可选携带 PersonCard，先过 SHACL 才随分身落库
//! （agents.person_card_rdf，migration 0005）；激活时优先用已存 card。
//! TODO(M2)：LLM 结构化 PersonCard（llm/gateway.rs 就绪后接入）。
//!
//! 硬规则：未支付不得激活成分身（lifecycle 状态门 + 激活接口的 paid 订单校验）。

use serde::Deserialize;
use worker::{Response, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::common::logging;
use crate::governance::validator::sanitize_text;
use crate::ontology::version::OntologyRouter;
use crate::storage::d1;

#[derive(Debug, Deserialize)]
pub struct RegisterInput {
    pub email: String,
    /// 自然语言出身叙述（LLM 结构化的原料；MVP 先入库留痕）
    pub narrative: String,
}

pub async fn register(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let input: RegisterInput = json_body(&mut req).await?;
    let email = sanitize_text(&input.email, 254).to_lowercase();
    if !email.contains('@') || email.len() < 3 {
        return Err(AppError::InvalidInput("invalid email".into()));
    }
    let narrative = sanitize_text(&input.narrative, 4000);
    if narrative.is_empty() {
        return Err(AppError::InvalidInput("narrative required".into()));
    }

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    // 账号：已存在则复用（幂等注册）
    let account_id = match d1::get_account_by_email(&db, &email).await? {
        Some(a) => a.id,
        None => {
            let id = ulid::Ulid::generate().to_string();
            d1::create_account(&db, &id, &email).await?;
            id
        }
    };

    // 分身：一账号一个分身（MVP），状态 created（未支付）
    let router = OntologyRouter::from_env(&env).await?;
    let agent_id = ulid::Ulid::generate().to_string();
    d1::create_agent(&db, &agent_id, &account_id, router.for_action().as_str(), None).await?;

    crate::governance::audit::record(
        &env,
        "auth.register",
        serde_json::json!({ "account_id": account_id, "agent_id": agent_id }),
    )
    .await?;
    logging::info(
        "register_created",
        serde_json::json!({ "agent_id": agent_id, "status": "created" }),
    );

    Ok(Response::from_json(&serde_json::json!({
        "account_id": account_id,
        "agent_id": agent_id,
        "status": "created",
        "next": "payment_required",
    }))?)
}

// ---------- G7：邮箱验证码登录 + 会话后建分身 + /api/me ----------

use crate::api::session::{self, OtpStart};
use crate::common::types::now_rfc3339;
use crate::storage::kv;

#[derive(Debug, Deserialize)]
pub struct EmailStartInput {
    pub email: String,
}

fn normalize_email(raw: &str) -> AppResult<String> {
    let email = sanitize_text(raw, 254).to_lowercase();
    if !email.contains('@') || email.len() < 3 {
        return Err(AppError::InvalidInput("invalid email".into()));
    }
    Ok(email)
}

/// 找回或创建账号，并绑定 email 身份（OTP 与 OAuth 共用落点）。
pub(crate) async fn resolve_or_create_account(
    env: &worker::Env,
    provider: &str,
    subject: &str,
    email: &str,
) -> AppResult<String> {
    let db = env.d1("DB")?;
    if let Some(acc) = d1::get_identity_account(&db, provider, subject).await? {
        return Ok(acc);
    }
    let account_id = match d1::get_account_by_email(&db, email).await? {
        Some(a) => a.id,
        None => {
            let id = ulid::Ulid::generate().to_string();
            d1::create_account(&db, &id, email).await?;
            id
        }
    };
    d1::link_identity(&db, &account_id, provider, subject, email, &now_rfc3339()).await?;
    Ok(account_id)
}

pub(crate) async fn respond_with_session(
    env: &worker::Env,
    account_id: &str,
    extra: serde_json::Value,
) -> AppResult<Response> {
    let token = session::issue_session(env, account_id).await?;
    let db = env.d1("DB")?;
    let agent = d1::get_agent_by_account(&db, account_id).await?;
    let mut body = serde_json::json!({
        "account_id": account_id,
        "token": token,
        "has_agent": agent.is_some(),
        "agent_id": agent.as_ref().map(|a| a.id.clone()),
        "agent_status": agent.as_ref().map(|a| a.status.clone()),
    });
    if let (Some(obj), Some(ext)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    let mut resp = Response::from_json(&body)?;
    resp.headers_mut()
        .set("set-cookie", &session::session_cookie(&token))?;
    Ok(resp)
}

/// 发送登录验证码：POST /api/auth/email/start。
pub async fn email_start(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let input: EmailStartInput = json_body(&mut req).await?;
    let email = normalize_email(&input.email)?;
    let env = ctx.env.clone();
    match session::start_otp(&env, &email).await? {
        OtpStart::Sent => Ok(Response::from_json(&serde_json::json!({ "sent": true }))?),
        OtpStart::LogOnly(code) => {
            let mut body = serde_json::json!({ "sent": true, "mail_mode": "log_only" });
            if session::dev_echo_allowed(&env, true) {
                body["dev_code"] = serde_json::Value::String(code);
            }
            Ok(Response::from_json(&body)?)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EmailVerifyInput {
    pub email: String,
    pub code: String,
}

/// 校验登录验证码并下发会话：POST /api/auth/email/verify。
pub async fn email_verify(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let input: EmailVerifyInput = json_body(&mut req).await?;
    let email = normalize_email(&input.email)?;
    let env = ctx.env.clone();
    session::verify_otp(&env, &email, input.code.trim()).await?;
    let account_id = resolve_or_create_account(&env, "email", &email, &email).await?;
    crate::governance::audit::record(
        &env,
        "auth.login",
        serde_json::json!({ "account_id": account_id, "provider": "email" }),
    )
    .await?;
    respond_with_session(&env, &account_id, serde_json::json!({})).await
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentInput {
    pub narrative: String,
    /// 可选 PersonCard（RDF/Turtle）：携带时先过 SHACL，通过才随分身落库（G5）；
    /// 不带保持兼容（激活时再由请求传入，或届时仍无卡则按激活接口语义报错）。
    #[serde(default)]
    pub person_card_rdf: Option<String>,
}

/// 登录后创建分身（出生态）：POST /api/agent（需会话）。
/// 一账号一分身（MVP）；已有分身返回 409。
/// 携带 PersonCard 时先过 SHACL：违规 → 422 + violations 明细，且不建分身。
pub async fn create_agent(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = session::require_session(&req, &env).await?;
    let input: CreateAgentInput = json_body(&mut req).await?;
    let narrative = sanitize_text(&input.narrative, 4000);
    if narrative.is_empty() {
        return Err(AppError::InvalidInput("narrative required".into()));
    }
    // PersonCard 落库门（G5）：空白视为未携带；携带则必须过 SHACL 才落库。
    let person_card_rdf = input
        .person_card_rdf
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    if let Some(card) = person_card_rdf {
        let validation = crate::ontology::shacl::validate_person_card(card)?;
        if !validation.ok {
            let resp = Response::from_json(&serde_json::json!({
                "created": false,
                "violations": validation.violations,
            }))?;
            return Ok(resp.with_status(422));
        }
    }
    let db = env.d1("DB")?;
    if d1::get_agent_by_account(&db, &account_id).await?.is_some() {
        return Err(AppError::Conflict("account already has an agent".into()));
    }
    let router = OntologyRouter::from_env(&env).await?;
    let agent_id = ulid::Ulid::generate().to_string();
    d1::create_agent(
        &db,
        &agent_id,
        &account_id,
        router.for_action().as_str(),
        person_card_rdf,
    )
    .await?;
    crate::governance::audit::record(
        &env,
        "auth.agent_created",
        serde_json::json!({ "account_id": account_id, "agent_id": agent_id }),
    )
    .await?;
    Ok(Response::from_json(&serde_json::json!({
        "account_id": account_id,
        "agent_id": agent_id,
        "status": "created",
        "person_card_stored": person_card_rdf.is_some(),
        "next": "payment_required",
    }))?)
}

/// 当前账号总览：GET /api/me（需会话）。首页分身卡的数据源。
pub async fn me(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = session::require_session(&req, &env).await?;
    let db = env.d1("DB")?;
    let account = d1::get_account(&db, &account_id)
        .await?
        .ok_or_else(|| AppError::NotFound("account".into()))?;
    let credits = d1::get_credit_account(&db, &account_id)
        .await?
        .map(|a| a.balance)
        .unwrap_or(0);
    let agent_json = match d1::get_agent_by_account(&db, &account_id).await? {
        Some(a) => {
            let hot = match env.kv("KV") {
                Ok(store) => kv::get_hot_state(&store, &a.id).await.ok(),
                Err(_) => None,
            };
            serde_json::json!({
                "id": a.id,
                "status": a.status,
                "person_card_rdf": a.person_card_rdf,
                "hot_state": hot.map(|h| serde_json::json!({
                    "money": h.money, "food": h.food, "energy": h.energy, "reputation": h.reputation
                })),
            })
        }
        None => serde_json::Value::Null,
    };
    Ok(Response::from_json(&serde_json::json!({
        "account_id": account.id,
        "email": account.email,
        "credits": credits,
        "agent": agent_json,
    }))?)
}
