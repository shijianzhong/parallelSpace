//! 分身查询与激活：GET /api/agent/:id、POST /api/agent/:id/activate。
//!
//! 激活是注册主线的收尾（此前缺失，见 docs/GAPS_AND_PLAN.md G1）：
//! 注册 → created；支付首次确认 → pending（payment/credit.rs 推进）；
//! 治理员带 PersonCard 激活 → active。`created → active` 直跳仍是硬禁。

use serde::Deserialize;
use worker::{Response, RouteContext};

use crate::api::{json_body, require_param};
use crate::common::error::{AppError, AppResult};
use crate::common::types::AgentStatus;
use crate::ontology::version::OntologyRouter;
use crate::storage::d1;

pub async fn get_agent(_req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let agent_id = require_param(&ctx, "id")?;
    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    match d1::get_agent(&db, &agent_id).await? {
        Some(agent) => Ok(Response::from_json(&serde_json::json!({
            "id": agent.id,
            "account_id": agent.account_id,
            "status": agent.status,
            "location_id": agent.location_id,
            "ontology_version": agent.ontology_version,
        }))?),
        None => Err(AppError::NotFound(format!("agent {agent_id}"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ActivateInput {
    /// PersonCard 的 Turtle 负载（soc:Person 实例，过 PersonCardShape 校验）。
    pub person_card_rdf: String,
}

/// 激活分身：pending → active（治理员执行）。
///
/// 前置（任一不满足即拒绝，不改状态）：
/// - admin token 校验（与治理接口共用 `api::admin::check_admin`）；
/// - 分身存在且状态恰为 pending（created 未支付 → 409；已 active 重复激活 → 409）；
/// - 分身账号名下有 paid 订单（纵深防御：pending 本应只由支付推进产生，
///   但状态被其他路径改动时仍不能绕过支付）；
/// - PersonCard 过 SHACL（违规 → 422 + violations 明细）。
///
/// 落地走条件迁移，并发下只有一个请求能成功。
pub async fn activate(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    crate::api::admin::check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let agent_id = require_param(&ctx, "id")?;
    let input: ActivateInput = json_body(&mut req).await?;
    if input.person_card_rdf.trim().is_empty() {
        return Err(AppError::InvalidInput("person_card_rdf required".into()));
    }

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    let agent = d1::get_agent(&db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("agent {agent_id}")))?;
    let from = AgentStatus::parse(&agent.status)
        .ok_or_else(|| AppError::Internal(format!("unknown agent status {}", agent.status)))?;
    // 状态门：激活只接受 pending 的分身，其余状态统一 409 并报出当前状态。
    // 注：lifecycle 的迁移表允许 idle→active 这类运行期恢复，那不是激活语义，
    // 不能直接拿 ensure_transition(→Active) 当激活门。
    // 一个例外（自愈）：状态停在 created 但账号已有 paid 订单——说明支付确认时
    // 的推进（credit.rs）曾失败或订单是历史数据。此处凭支付证据先补走
    // created → pending，再继续激活，整条链仍严格经过 pending、不直跳 active。
    if from == AgentStatus::Created {
        if !d1::account_has_paid_order(&db, &agent.account_id).await? {
            return Err(AppError::Conflict(format!(
                "agent {agent_id} is created and account {} has no paid order; payment required before activation",
                agent.account_id
            )));
        }
        let repaired = d1::transition_agent_status(
            &db,
            &agent_id,
            AgentStatus::Created.as_str(),
            AgentStatus::Pending.as_str(),
        )
        .await?;
        if !repaired {
            return Err(AppError::Conflict(format!(
                "agent {agent_id} status changed concurrently; activation aborted"
            )));
        }
        crate::common::logging::warn(
            "agent_status_repaired_created_to_pending",
            serde_json::json!({ "agent_id": agent_id }),
        );
    } else if from != AgentStatus::Pending {
        return Err(AppError::Conflict(format!(
            "agent {agent_id} is {}, must be pending to activate",
            agent.status
        )));
    }
    crate::agent_runtime::lifecycle::ensure_transition(AgentStatus::Pending, AgentStatus::Active)?;

    if !d1::account_has_paid_order(&db, &agent.account_id).await? {
        return Err(AppError::Conflict(format!(
            "account {} has no paid order; payment required before activation",
            agent.account_id
        )));
    }

    let validation = crate::ontology::shacl::validate_person_card(&input.person_card_rdf)?;
    if !validation.ok {
        let resp = Response::from_json(&serde_json::json!({
            "activated": false,
            "violations": validation.violations,
        }))?;
        return Ok(resp.with_status(422));
    }

    let moved = d1::transition_agent_status(
        &db,
        &agent_id,
        AgentStatus::Pending.as_str(),
        AgentStatus::Active.as_str(),
    )
    .await?;
    if !moved {
        return Err(AppError::Conflict(format!(
            "agent {agent_id} status changed concurrently; activation aborted"
        )));
    }

    // AgentActivated 事件（append-only；版本由 OntologyRouter 注入）
    let router = OntologyRouter::from_env(&env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let event_type = crate::ontology::iri::soc("AgentActivated");
    crate::event::log::append(
        &env,
        &event_type,
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(&agent_id),
        &serde_json::json!({
            "agent_id": agent_id,
            "account_id": agent.account_id,
            "from": "pending",
            "to": "active",
        }),
    )
    .await?;
    crate::governance::audit::record(
        &env,
        "agent.activated",
        serde_json::json!({ "agent_id": agent_id, "event_id": event_id }),
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "agent_id": agent_id,
        "status": "active",
        "event_id": event_id,
    }))?)
}
