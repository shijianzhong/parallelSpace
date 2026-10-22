//! R2 存储层：append-only 事件日志 / LLM 调用记录 / 本体版本快照（SAD §6.3）。

use worker::Bucket;

use crate::common::error::AppResult;

/// 事件对象键：events/{tick}/{event_id}.json（append-only，不覆盖）。
pub fn event_key(tick: u64, event_id: &str) -> String {
    format!("events/{tick}/{event_id}.json")
}

/// LLM 调用记录键：llm/{date}/{call_id}.json。
pub fn llm_call_key(date: &str, call_id: &str) -> String {
    format!("llm/{date}/{call_id}.json")
}

/// 本体版本快照键：ontology/{version}/{file}（硬禁：不覆盖旧版本目录）。
pub fn ontology_key(version: &str, file: &str) -> String {
    format!("ontology/{version}/{file}")
}

/// 写 JSON 对象。同键重复写直接报错——R2 语义上是覆盖，
/// 但事件/记录必须是 append-only，调用方保证键唯一（ULID）。
pub async fn put_json(bucket: &Bucket, key: &str, value: &impl serde::Serialize) -> AppResult<()> {
    let body = serde_json::to_vec(value)
        .map_err(|e| crate::common::error::AppError::Internal(format!("serialize r2 object: {e}")))?;
    bucket.put(key, body).execute().await?;
    Ok(())
}

/// 读 JSON 对象。
pub async fn get_json<T: serde::de::DeserializeOwned>(
    bucket: &Bucket,
    key: &str,
) -> AppResult<Option<T>> {
    match bucket.get(key).execute().await? {
        Some(object) => {
            let body = object.body().ok_or_else(|| {
                crate::common::error::AppError::Internal(format!("r2 object {key} has no body"))
            })?;
            let bytes = body.bytes().await?;
            let value = serde_json::from_slice(&bytes).map_err(|e| {
                crate::common::error::AppError::Internal(format!("parse r2 object {key}: {e}"))
            })?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn key_layouts_match_sad() {
        assert_eq!(event_key(42, "01J"), "events/42/01J.json");
        assert_eq!(llm_call_key("2026-10-22", "01J"), "llm/2026-10-22/01J.json");
        assert_eq!(ontology_key("1.0.0", "soc.ttl"), "ontology/1.0.0/soc.ttl");
    }
}
