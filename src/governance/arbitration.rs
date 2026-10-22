//! 治理：仲裁案件（PRD §9 / SAD §7）。
//!
//! MVP：案件落 D1 事件索引 + R2 append-only 日志；裁决流程（双方举证、
//! NPC 陪审）在 M2 接入。

use serde::Deserialize;
use worker::Env;

use crate::common::error::{AppError, AppResult};
use crate::common::types::now_rfc3339;
use crate::ontology::version::OntologyRouter;

#[derive(Debug, Deserialize)]
pub struct ArbitrationCaseInput {
    pub claimant_id: String,
    pub respondent_id: String,
    pub contract_id: Option<String>,
    pub claim: String,
}

/// 立案：生成仲裁案件事件。双方必须存在且不相同。
pub async fn file_case(env: &Env, input: &ArbitrationCaseInput) -> AppResult<String> {
    if input.claimant_id.is_empty() || input.respondent_id.is_empty() {
        return Err(AppError::InvalidInput("both parties required".into()));
    }
    if input.claimant_id == input.respondent_id {
        return Err(AppError::InvalidInput("claimant == respondent".into()));
    }
    let claim = crate::governance::validator::sanitize_text(&input.claim, 2000);
    if claim.is_empty() {
        return Err(AppError::InvalidInput("claim empty".into()));
    }

    let router = OntologyRouter::from_env(env).await?;
    let case_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "case_id": case_id,
        "claimant_id": input.claimant_id,
        "respondent_id": input.respondent_id,
        "contract_id": input.contract_id,
        "claim": claim,
        "status": "filed",
        "filed_at": now_rfc3339(),
    });
    crate::event::log::append(
        env,
        "http://soc.example/gov/ontology#ArbitrationCase",
        &case_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(&input.claimant_id),
        &payload,
    )
    .await?;
    Ok(case_id)
}
