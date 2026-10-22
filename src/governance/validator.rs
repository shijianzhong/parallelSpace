//! 行动校验：LLM 提案、规则守门（AGENTS.md §1/§5）。
//!
//! 分层：L2 本地 SHACL（ontology/shacl.rs，< 50ms）→ 合法则异步/同步 L3
//! 外部本体引擎 OWL 2 DL（本模块 validate_remote）。L3 未配置或失败时
//! 降级返回 L2 结果并记 warning（SAD §5「失败降级到本地 reasonable 子集校验」）。

use std::fmt;

use serde::{Deserialize, Serialize};
use worker::{js_sys, Env, Fetch, Method, Request, RequestInit};

use crate::common::error::{AppError, AppResult};
use crate::common::logging;
use crate::common::types::ValidationResult;
use crate::ontology::shacl;

/// RDF 负载大小上限（256KB）：防超大输入打爆 L2 解析。
pub const MAX_RDF_PAYLOAD_BYTES: usize = 256 * 1024;

/// 行动提交请求（AGENTS.md §5.1）。
#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub agent_id: String,
    pub action_type: ActionType,
    pub target_id: Option<String>,
    pub params: serde_json::Value,
}

/// 行动类型。与 LLM 行动提案 schema、llm::router 的任务分级对应。
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Move,
    Work,
    Trade,
    Chat,
    Ally,
    Employ,
    Vote,
    Legislate,
    Breach,
    Learn,
}

impl fmt::Display for ActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ActionType::Move => "move",
            ActionType::Work => "work",
            ActionType::Trade => "trade",
            ActionType::Chat => "chat",
            ActionType::Ally => "ally",
            ActionType::Employ => "employ",
            ActionType::Vote => "vote",
            ActionType::Legislate => "legislate",
            ActionType::Breach => "breach",
            ActionType::Learn => "learn",
        };
        f.write_str(s)
    }
}

/// 行动响应：校验通过后由 api::action 写事件并返回。
#[derive(Debug, Serialize)]
pub struct ActionResponse {
    pub accepted: bool,
    pub event_id: String,
    pub violations: Vec<crate::common::types::Violation>,
}

/// 输入消毒（AGENTS.md §4.4）：剔除控制字符、按字符截断。
/// 所有用户输入先过这里再进业务层。
pub fn sanitize_text(input: &str, max_len: usize) -> String {
    input
        .trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(max_len)
        .collect()
}

/// L2 + L3 行动校验入口。
pub async fn validate_action(
    env: &Env,
    action: &ActionRequest,
    rdf_payload: &str,
) -> AppResult<ValidationResult> {
    if rdf_payload.len() > MAX_RDF_PAYLOAD_BYTES {
        return Err(AppError::InvalidInput("rdf payload too large".into()));
    }
    if action.agent_id.is_empty() {
        return Err(AppError::InvalidInput("agent_id required".into()));
    }

    // L2：本地 SHACL 快速拒绝
    let local = shacl::validate_local(rdf_payload, action.action_type)?;
    if !local.ok {
        return Ok(local);
    }

    // L3：外部本体引擎（可选）。未配置或失败 → 降级为 L2 结果。
    let engine_url = env
        .var("ONTOLOGY_ENGINE_URL")
        .map(|v| v.to_string())
        .unwrap_or_default();
    if engine_url.is_empty() {
        return Ok(local);
    }
    match validate_remote(&engine_url, rdf_payload).await {
        Ok(remote) => Ok(remote),
        Err(e) => {
            logging::warn(
                "l3_engine_fallback_to_l2",
                serde_json::json!({ "error": e.to_string() }),
            );
            Ok(local)
        }
    }
}

/// 调用外部 open-ontologies 的 /validate 端点（SAD §4）。
async fn validate_remote(engine_url: &str, rdf_payload: &str) -> AppResult<ValidationResult> {
    let url = format!("{}/validate", engine_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&serde_json::json!({ "rdf": rdf_payload }))
        .map_err(|e| AppError::Internal(format!("serialize l3 request: {e}")))?;

    let mut init = RequestInit::new();
    init.method = Method::Post;
    let bytes = js_sys::Uint8Array::from(body.as_slice());
    init.body = Some(bytes.into());
    let req = Request::new_with_init(&url, &init)?;
    let mut resp = Fetch::Request(req).send().await?;
    if resp.status_code() != 200 {
        return Err(AppError::OntologyEngine(format!(
            "l3 engine returned {}",
            resp.status_code()
        )));
    }
    let payload: serde_json::Value = resp.json().await?;
    let ok = payload
        .get("ok")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| AppError::OntologyEngine("l3 response missing ok".into()))?;
    let violations: Vec<crate::common::types::Violation> = payload
        .get("violations")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(ValidationResult {
        ok: ok && violations.is_empty(),
        violations,
        inferred: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn sanitize_strips_control_chars_and_truncates() {
        assert_eq!(sanitize_text("a\u{7}b", 10), "ab");
        assert_eq!(sanitize_text("a\nb\tc", 10), "a\nb\tc");
        assert_eq!(sanitize_text(" héllo", 10), "héllo"); // é 是合法字符
        assert_eq!(sanitize_text("abcdef", 3), "abc");
        assert_eq!(sanitize_text("", 5), "");
    }

    #[wasm_bindgen_test]
    fn action_type_serde_snake_case() {
        let json = serde_json::to_string(&ActionType::Legislate).unwrap();
        assert_eq!(json, "\"legislate\"");
        let t: ActionType = serde_json::from_str("\"trade\"").unwrap();
        assert_eq!(t, ActionType::Trade);
        assert!(serde_json::from_str::<ActionType>("\"Trade\"").is_err());
    }

    #[wasm_bindgen_test]
    fn action_type_display() {
        assert_eq!(ActionType::Breach.to_string(), "breach");
    }
}
