//! 分身查询与激活：GET /api/agent/:id、POST /api/agent/:id/activate。
//!
//! 激活是注册主线的收尾（此前缺失，见 docs/GAPS_AND_PLAN.md G1）：
//! 注册 → created；支付首次确认 → pending（payment/credit.rs 推进）；
//! 治理员带 PersonCard 激活 → active。`created → active` 直跳仍是硬禁。
//! G5：注册时已落库的 PersonCard（agents.person_card_rdf）可省略请求入参，
//! 激活优先用请求入参、缺省用已存 card；两者都无保持原报错语义。

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
            "person_card_rdf": agent.person_card_rdf,
        }))?),
        None => Err(AppError::NotFound(format!("agent {agent_id}"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ActivateInput {
    /// PersonCard 的 Turtle 负载（soc:Person 实例，过 PersonCardShape 校验）。
    /// 可省略：省略（或空白）时用注册落库的已存 card（G5）；都无则报错。
    #[serde(default)]
    pub person_card_rdf: Option<String>,
    /// 出生落点（G8）：locations 表中已存在的地点 id，激活时写入
    /// agents.location_id（core:locatedAt）。可省略——locatedAt 是可选
    /// 属性，无公理要求分身必须有位置；给了但地点不存在 → 400。
    #[serde(default)]
    pub location_id: Option<String>,
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

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    let agent = d1::get_agent(&db, &agent_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("agent {agent_id}")))?;
    // PersonCard 来源（G5）：请求入参优先，缺省用注册落库的已存 card；
    // 两者都无保持原报错语义（400 person_card_rdf required）。
    let person_card_rdf = input
        .person_card_rdf
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .or_else(|| {
            agent
                .person_card_rdf
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
        })
        .ok_or_else(|| AppError::InvalidInput("person_card_rdf required".into()))?
        .to_string();
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

    // 出生落点（G8）：给了 location_id 就必须指向真实存在的地点，
    // 不造位置、不静默忽略——与 PersonCard 一样在迁移前完成全部校验。
    let location_id = input
        .location_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(loc) = &location_id {
        if d1::get_location(&db, loc).await?.is_none() {
            return Err(AppError::InvalidInput(format!("unknown location_id: {loc}")));
        }
    }

    let validation = crate::ontology::shacl::validate_person_card(&person_card_rdf)?;
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

    // 出生落点写入（G8）：状态迁移成功后才落位置，避免给未激活分身写位置。
    if let Some(loc) = &location_id {
        d1::set_agent_location(&db, &agent_id, loc).await?;
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
            "location_id": location_id,
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
        "location_id": location_id,
    }))?)
}
