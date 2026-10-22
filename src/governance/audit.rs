//! 审计日志：所有行动、LLM 调用、本体变更、支付确认的 append-only 记录
//! （PRD §10 / AGENTS.md §4.4）。R2 audit/{date}/{ulid}.json + 结构化 stdout。

use worker::Env;

use crate::common::error::AppResult;
use crate::common::types::now_rfc3339;
use crate::storage::r2;

/// 写一条审计记录。action 形如 "action.submit" / "payment.confirm" / "ontology.publish"。
pub async fn record(env: &Env, action: &str, fields: serde_json::Value) -> AppResult<String> {
    let audit_id = ulid::Ulid::generate().to_string();
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let record = serde_json::json!({
        "id": audit_id,
        "action": action,
        "at": now_rfc3339(),
        "fields": fields,
    });
    let bucket = env.bucket("R2")?;
    r2::put_json(&bucket, &format!("audit/{date}/{audit_id}.json"), &record).await?;
    crate::common::logging::info("audit", record);
    Ok(audit_id)
}
