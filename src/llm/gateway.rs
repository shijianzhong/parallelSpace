//! LLM 网关（AGENTS.md §5.4 / §7.6）。
//!
//! 链路：配额检查（KV）→ 缓存命中（prompt_hash + ontology_version + model_version
//! + temp + seed）→ 外部 LLM API → 解析为 ActionProposal（失败拒绝并记审计）。
//!
//! MVP 边界：
//! - 外部 API 地址由 env LLM_API_URL 注入；未配置时拒绝调用（禁默认 BYOK）；
//! - Workers AI 仅作降级且仅 SimpleSocial（本 scaffold 未接，留 TODO 标记）；
//! - 每次调用记录 call_id/model/model_version/temperature/seed/hashes/tick。

use serde::{Deserialize, Serialize};
use worker::Env;

use crate::common::error::{AppError, AppResult};
use crate::common::logging;
use crate::llm::cache;
use crate::llm::router::{route, LlmTask, ModelTier};
use crate::ontology::version::OntologyRouter;

/// 每分身每日 token 预算（KV quota 硬限制）。
pub const DAILY_TOKEN_BUDGET: i64 = 50_000;

#[derive(Debug, Deserialize)]
pub struct LlmRequest {
    pub agent_id: String,
    pub task: LlmTask,
    pub context: String,
    pub max_tokens: u32,
}

/// 结构化行动提案：LLM 唯一被允许的输出形态（解析失败即拒绝）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionProposal {
    pub action: String,
    pub target_id: Option<String>,
    pub params: serde_json::Value,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
pub struct LlmResponse {
    pub proposal: ActionProposal,
    pub model: String,
    pub model_version: String,
    pub cached: bool,
}

/// 模型档位 → 具体模型名（平台统一提供，禁默认 BYOK）。
fn model_name(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Small => "platform-small",
        ModelTier::Medium => "platform-medium",
        ModelTier::Large => "platform-large",
    }
}

/// 提案入口。配额超限返回 QuotaExceeded；无上游返回 Internal（调用方降级到规则）。
pub async fn propose(env: &Env, req: &LlmRequest) -> AppResult<LlmResponse> {
    let router = OntologyRouter::from_env(env).await?;
    let ontology_version = router.for_action().as_str().to_string();
    let tier = route(req.task);
    let model = model_name(tier);
    let temperature_millis: i64 = 700;
    let seed: i64 = 0;

    // 1. 配额（KV quota:{date}:{agent_id}）
    let kv_store = env.kv("KV")?;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let used = crate::storage::kv::get_quota_used(&kv_store, &date, &req.agent_id).await?;
    if used >= DAILY_TOKEN_BUDGET {
        return Err(AppError::QuotaExceeded);
    }

    // 2. 缓存（键绑定本体版本）
    if let Some(hit) = cache::get_cached(
        env,
        &req.context,
        &ontology_version,
        model,
        temperature_millis,
        seed,
    )
    .await?
    {
        let proposal: ActionProposal = serde_json::from_str(&hit)
            .map_err(|e| AppError::Llm(format!("cached proposal parse: {e}")))?;
        return Ok(LlmResponse {
            proposal,
            model: model.to_string(),
            model_version: "cached".into(),
            cached: true,
        });
    }

    // 3. 外部 API（未配置则明确失败，由调用方降级）
    let api_url = env
        .var("LLM_API_URL")
        .map(|v| v.to_string())
        .unwrap_or_default();
    if api_url.is_empty() {
        return Err(AppError::Llm(
            "LLM_API_URL not configured; fall back to rule-based decision".into(),
        ));
    }

    // 4. 调用 + 记录（TODO(M2): 接入真实上游；当前返回占位提案）
    //    记录字段与 llm_calls 表对齐（AGENTS.md §4.3）。
    let call_id = ulid::Ulid::generate().to_string();
    logging::info(
        "llm_call",
        serde_json::json!({
            "call_id": call_id,
            "agent_id": req.agent_id,
            "task": format!("{:?}", req.task),
            "model": model,
            "model_version": model,
            "temperature_millis": temperature_millis,
            "seed": seed,
            "prompt_hash": cache::prompt_hash(&req.context),
            "response_hash": cache::prompt_hash("placeholder:idle"),
            "validation_result": "placeholder_not_validated",
            "tick": crate::agent_runtime::scheduler::current_tick(),
            "ontology_version": ontology_version,
        }),
    );

    // 配额预扣：按本次 max_tokens 计入当日用量，让入口的配额检查真实生效
    // （此前 add_quota_used 无调用方，used 恒为 0、配额永不触发——GAPS G2）。
    crate::storage::kv::add_quota_used(&kv_store, &date, &req.agent_id, req.max_tokens as i64)
        .await?;

    // MVP 占位提案：证明管道（配额→缓存→路由→提案结构）打通；
    // 真实上游接入前，行动默认走规则路径。
    // ⚠️ 占位提案**不写缓存**：写进去会按 7 天 TTL 毒化同一 prompt 的后续真实
    // 响应（命中缓存后永远返回 idle）——GAPS G2。
    let proposal = ActionProposal {
        action: "idle".into(),
        target_id: None,
        params: serde_json::json!({}),
        rationale: "llm upstream not wired in MVP".into(),
    };

    Ok(LlmResponse {
        proposal,
        model: model.to_string(),
        model_version: model.into(),
        cached: false,
    })
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn proposal_requires_structured_shape() {
        // LLM 输出必须是 ActionProposal；任意文本解析失败
        assert!(serde_json::from_str::<ActionProposal>("自由文本").is_err());
        let p: ActionProposal =
            serde_json::from_str(r#"{"action":"trade","params":{},"rationale":"x"}"#).unwrap();
        assert_eq!(p.action, "trade");
        assert!(p.target_id.is_none());
    }

    #[wasm_bindgen_test]
    fn model_tiers_are_distinct() {
        assert_ne!(model_name(ModelTier::Small), model_name(ModelTier::Large));
    }
}
