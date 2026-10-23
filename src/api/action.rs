//! 行动提交：POST /api/action（AGENTS.md §5.1）。
//!
//! 链路：L1 参数 → 查热状态 → L2 SHACL（governance::validator::validate_action）
//! → 离线代理闸门（G4 接线，见 proxy_gate）→ 合法则写事件 → 异步 L3
//! （引擎未配置时已在 validate_action 内降级）。
//! LLM 提案路径：decision.rs 产出 ActionProposal 后同样走本入口校验。

use worker::{Response, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::governance::validator::{validate_action, ActionRequest};
use crate::ontology::version::OntologyRouter;

/// 从 params 里取 RDF（Turtle）负载；缺失时退化成占位 JSON 而不是报错。
///
/// 这条降级路径必须由测试钉住：静默丢掉 rdf 会让 L2 形状校验形同虚设，
/// 所以占位串里显式写明 "no rdf payload"，审计时能一眼看出哪些行动没走校验。
/// Move 动作的目的地（G8）：优先 `params.location_id`，其次通用
/// `target_id`。仅 Move 返回目的地；其他动作类型即使带了 location_id
/// 也不改位置（位置变更的语义只挂在 Move 上）。空串视为未给。
fn extract_move_destination(action: &ActionRequest) -> Option<String> {
    if action.action_type != crate::governance::validator::ActionType::Move {
        return None;
    }
    action
        .params
        .get("location_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            action
                .target_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

pub(crate) fn extract_rdf_payload(action: &ActionRequest) -> String {
    action
        .params
        .get("rdf")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            serde_json::to_string(&serde_json::json!({
                "note": "no rdf payload; L2 shape checks skipped",
                "agent_id": action.agent_id,
            }))
            .unwrap_or_default()
        })
}

pub async fn submit(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let action: ActionRequest = json_body(&mut req).await?;
    let env = ctx.env.clone();

    // G7 归属校验（过渡模式）：带会话必须与分身归属一致； REQUIRE_AUTH=true 时强制会话
    crate::api::session::enforce_ownership(&req, &env, &action.agent_id).await?;

    // RDF 负载：MVP 由 params.rdf 携带（Turtle）
    let rdf_payload = extract_rdf_payload(&action);

    let result = validate_action(&env, &action, &rdf_payload).await?;
    if !result.ok {
        return Ok(Response::from_json(&serde_json::json!({
            "accepted": false,
            "violations": result.violations,
        }))?);
    }

    // G4 离线代理闸门：所有者不在场时按 offline_proxy::decide 拦截
    // 不可逆动作（Defer 挂起待批 / Deny 直接拒绝）。放在 L2 校验之后，
    // 保证既有 SHACL 拒绝语义（violations 形态）不被闸门改变。
    if let Some(blocked) = proxy_gate(&req, &env, &action).await? {
        return Ok(blocked);
    }

    let body = execute_validated_action(&env, &action, &rdf_payload, result.inferred.len()).await?;
    Ok(Response::from_json(&body)?)
}

/// 校验通过后的落盘段：在线提交与 G15 决策批准重放共用同一函数。
///
/// 批准路径禁止自造落盘逻辑（AGENTS.md §5.9）：待决动作被批准时必须经
/// `validate_action` + 本函数落盘，保证校验与落盘语义与在线提交完全
/// 一致。返回成功响应体（accepted / event_id / inferred_count /
/// location_id），批准方从中取 event_id 回填待决项。
pub(crate) async fn execute_validated_action(
    env: &worker::Env,
    action: &ActionRequest,
    rdf_payload: &str,
    inferred_count: usize,
) -> AppResult<serde_json::Value> {
    // G8 Move 落点：给了目的地就必须指向真实存在的地点（不存在 → 400，
    // 在落事件前拒绝，避免事件流里出现指向虚空的位置变更）。Move 不带
    // 目的地时只记事件、不改位置（locatedAt 可选，无公理要求每次 Move
    // 都携带可指称落点）。
    let move_destination = extract_move_destination(action);
    if let Some(dest) = &move_destination {
        let db = env.d1("DB")?;
        if crate::storage::d1::get_location(&db, dest).await?.is_none() {
            return Err(AppError::InvalidInput(format!(
                "unknown location_id: {dest}"
            )));
        }
    }

    // 写事件（append-only）
    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "agent_id": action.agent_id,
        "action_type": action.action_type.to_string(),
        "target_id": action.target_id,
        "params": action.params,
        "rdf": rdf_payload,
    });
    crate::event::log::append(
        env,
        "http://soc.example/ontology#Action",
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(&action.agent_id),
        &payload,
    )
    .await?;

    // G8：Move 事件落盘后更新位置指针（事件是事实源，位置是其投影；
    // 分身不存在时 set 返回 false，只记日志不回滚事件——事件已成立）。
    let mut applied_location: Option<String> = None;
    if let Some(dest) = &move_destination {
        let db = env.d1("DB")?;
        if crate::storage::d1::set_agent_location(&db, &action.agent_id, dest).await? {
            applied_location = Some(dest.clone());
        } else {
            crate::common::logging::warn(
                "move_location_not_applied",
                serde_json::json!({ "agent_id": action.agent_id, "location_id": dest }),
            );
        }
    }

    crate::governance::audit::record(
        env,
        "action.submit",
        serde_json::json!({
            "agent_id": action.agent_id,
            "event_id": event_id,
            "action_type": action.action_type.to_string(),
        }),
    )
    .await?;

    Ok(serde_json::json!({
        "accepted": true,
        "event_id": event_id,
        "inferred_count": inferred_count,
        "location_id": applied_location,
    }))
}

/// 离线代理闸门（G4 接线，agent_runtime::offline_proxy；G15 起 Defer = 挂起待批）。
///
/// 「所有者在线」的定义：请求携带的会话账号正是分身的归属账号——
/// 本人亲自提交时，决定权在本人，代理规则不介入（归属一致性已由
/// 上游 enforce_ownership 保证，这里再独立核对一次，不依赖调用顺序）。
/// 所有者不在场（无会话，或分身不存在）时，提交视为离线代理行为，
/// 按 decide() 裁决：维持性动作（Move/Work/Learn/Chat）放行；
/// 不可逆动作（Trade/Ally/Employ/Vote/Legislate）Defer；背叛类 Deny。
///
/// Defer 的落地（G15，AGENTS.md §5.9）：分身存在时生成 PendingDecision
/// 落库 + 审计 `decision.pending_created`，响应保持 `accepted:false` +
/// `proxy_decision:"defer_to_owner"` 并带 `pending_decision_id`，所有者在
/// 决策收件箱批/驳（72 小时未决自动驳回）。分身不存在时无归属者可挂起，
/// 维持 G4 路径：拒绝 + `action.proxy_deferred` 审计兜底。
/// Deny 保持直接拒绝（审计 `action.proxy_denied`），不进收件箱。
async fn proxy_gate(
    req: &worker::Request,
    env: &worker::Env,
    action: &ActionRequest,
) -> AppResult<Option<Response>> {
    use crate::agent_runtime::offline_proxy::{decide, ProxyDecision};

    let db = env.d1("DB")?;
    let agent = crate::storage::d1::get_agent(&db, &action.agent_id).await?;
    let owner_present = match crate::api::session::resolve_session(req, env).await? {
        Some(account_id) => agent
            .as_ref()
            .map(|a| a.account_id == account_id)
            .unwrap_or(false),
        None => false,
    };
    if owner_present {
        return Ok(None);
    }

    let decision = decide(action.action_type);
    match decision {
        ProxyDecision::Allow => Ok(None),
        ProxyDecision::Deny => {
            crate::governance::audit::record(
                env,
                "action.proxy_denied",
                serde_json::json!({
                    "agent_id": action.agent_id,
                    "action_type": action.action_type.to_string(),
                    "proxy_decision": decision.as_str(),
                }),
            )
            .await?;
            Ok(Some(Response::from_json(&serde_json::json!({
                "accepted": false,
                "proxy_decision": decision.as_str(),
                "reason": "owner offline: offline proxy must never approve this action class",
            }))?))
        }
        ProxyDecision::DeferToOwner => {
            let Some(agent) = agent else {
                // 分身不存在：无归属者可挂起待批，维持 G4 的拒绝 + 审计兜底。
                crate::governance::audit::record(
                    env,
                    "action.proxy_deferred",
                    serde_json::json!({
                        "agent_id": action.agent_id,
                        "action_type": action.action_type.to_string(),
                        "proxy_decision": decision.as_str(),
                    }),
                )
                .await?;
                return Ok(Some(Response::from_json(&serde_json::json!({
                    "accepted": false,
                    "proxy_decision": decision.as_str(),
                    "reason": "owner offline: irreversible action deferred to owner; agent not found, no pending decision created",
                }))?));
            };
            // G15 挂起待批：落 PendingDecision + 创建审计，响应带待决项 id。
            let created_at = crate::common::types::now_rfc3339();
            let expires_at = crate::api::decisions::expires_at_for(&created_at);
            let decision_id = ulid::Ulid::generate().to_string();
            let audit_id = crate::governance::audit::record(
                env,
                "decision.pending_created",
                serde_json::json!({
                    "agent_id": action.agent_id,
                    "account_id": agent.account_id,
                    "action_type": action.action_type.to_string(),
                    "proxy_decision": decision.as_str(),
                    "pending_decision_id": decision_id,
                    "expires_at": expires_at,
                }),
            )
            .await?;
            crate::storage::d1::create_pending_decision(
                &db,
                &decision_id,
                &action.agent_id,
                &agent.account_id,
                &action.action_type.to_string(),
                &crate::api::decisions::serialize_payload(action),
                Some(&audit_id),
                &created_at,
                &expires_at,
            )
            .await?;
            Ok(Some(Response::from_json(&serde_json::json!({
                "accepted": false,
                "proxy_decision": decision.as_str(),
                "reason": "owner offline: irreversible action held as pending decision for owner approval (auto-rejects after 72h)",
                "pending_decision_id": decision_id,
            }))?))
        }
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;
    use crate::ontology::shacl;

    #[wasm_bindgen_test]
    fn shacl_engine_is_reachable_from_action_path() {
        assert!(shacl::load_shapes().is_ok());
    }

    fn action_with_params(params: serde_json::Value) -> ActionRequest {
        ActionRequest {
            agent_id: "urn:agent:1".into(),
            action_type: crate::governance::validator::ActionType::Trade,
            target_id: None,
            params,
        }
    }

    #[wasm_bindgen_test]
    fn rdf_payload_is_taken_verbatim_when_present() {
        let ttl = "@prefix soc: <http://soc.example/ontology#> .\n<urn:a:1> a soc:Agent .\n";
        let a = action_with_params(serde_json::json!({ "rdf": ttl }));
        assert_eq!(extract_rdf_payload(&a), ttl);
    }

    #[wasm_bindgen_test]
    fn missing_rdf_payload_is_annotated_not_silent() {
        let a = action_with_params(serde_json::json!({ "amount": 10 }));
        let got = extract_rdf_payload(&a);
        // 占位串必须自带说明 + 带上 agent_id，便于审计哪些行动绕过了 L2
        assert!(got.contains("no rdf payload"), "{got}");
        assert!(got.contains("urn:agent:1"), "{got}");
        assert!(serde_json::from_str::<serde_json::Value>(&got).is_ok(), "{got}");
    }

    #[wasm_bindgen_test]
    fn non_string_rdf_field_falls_back_to_placeholder() {
        // 客户端传 {"rdf": {"nested": true}} 时不该 panic，走同一条降级路径
        let a = action_with_params(serde_json::json!({ "rdf": { "nested": true } }));
        assert!(extract_rdf_payload(&a).contains("no rdf payload"));
    }

    fn move_action(params: serde_json::Value, target_id: Option<&str>) -> ActionRequest {
        ActionRequest {
            agent_id: "urn:agent:1".into(),
            action_type: crate::governance::validator::ActionType::Move,
            target_id: target_id.map(str::to_string),
            params,
        }
    }

    #[wasm_bindgen_test]
    fn move_destination_prefers_params_location_id() {
        let a = move_action(
            serde_json::json!({ "location_id": "loc-a" }),
            Some("loc-b"),
        );
        assert_eq!(extract_move_destination(&a).as_deref(), Some("loc-a"));
    }

    #[wasm_bindgen_test]
    fn move_destination_falls_back_to_target_id() {
        let a = move_action(serde_json::json!({}), Some(" loc-b "));
        assert_eq!(extract_move_destination(&a).as_deref(), Some("loc-b"));
        // 两处都不给 → 无目的地（只记事件、不改位置）
        let a = move_action(serde_json::json!({ "location_id": "  " }), None);
        assert_eq!(extract_move_destination(&a), None);
    }

    #[wasm_bindgen_test]
    fn non_move_action_has_no_destination() {
        // Trade 即使带 location_id 也不改位置：位置变更语义只挂 Move
        let a = action_with_params(serde_json::json!({ "location_id": "loc-a" }));
        assert_eq!(extract_move_destination(&a), None);
    }
}
