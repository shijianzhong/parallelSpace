//! 行动提交：POST /api/action（AGENTS.md §5.1）。
//!
//! 链路：L1 参数 → 查热状态 → L2 SHACL（governance::validator::validate_action）
//! → 合法则写事件 → 异步 L3（引擎未配置时已在 validate_action 内降级）。
//! LLM 提案路径：decision.rs 产出 ActionProposal 后同样走本入口校验。

use worker::{Response, RouteContext};

use crate::api::json_body;
use crate::common::error::AppResult;
use crate::governance::validator::{validate_action, ActionRequest};
use crate::ontology::version::OntologyRouter;

/// 从 params 里取 RDF（Turtle）负载；缺失时退化成占位 JSON 而不是报错。
///
/// 这条降级路径必须由测试钉住：静默丢掉 rdf 会让 L2 形状校验形同虚设，
/// 所以占位串里显式写明 "no rdf payload"，审计时能一眼看出哪些行动没走校验。
fn extract_rdf_payload(action: &ActionRequest) -> String {
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

    // 写事件（append-only）
    let router = OntologyRouter::from_env(&env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "agent_id": action.agent_id,
        "action_type": action.action_type.to_string(),
        "target_id": action.target_id,
        "params": action.params,
        "rdf": rdf_payload,
    });
    crate::event::log::append(
        &env,
        "http://soc.example/ontology#Action",
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(&action.agent_id),
        &payload,
    )
    .await?;

    crate::governance::audit::record(
        &env,
        "action.submit",
        serde_json::json!({
            "agent_id": action.agent_id,
            "event_id": event_id,
            "action_type": action.action_type.to_string(),
        }),
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "accepted": true,
        "event_id": event_id,
        "inferred_count": result.inferred.len(),
    }))?)
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
}
