//! 注册接口（AGENTS.md §8.1 的 MVP 子集）。
//!
//! 完整链路：账号会话 + 出身模板 + 自然语言叙述 → LLM 结构化为 PersonCard →
//! SHACL 校验 → 支付 → confirm_payment → pending → 治理员激活 → active。
//! 本模块负责第一步：账号/分身落库（created 状态）、输入消毒、审计。
//! 支付推进 pending 在 payment/credit.rs，激活在 api/agent.rs::activate。
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
    d1::create_agent(&db, &agent_id, &account_id, router.for_action().as_str()).await?;

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
