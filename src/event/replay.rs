//! 事件回放（可观测与调试，SAD §10）。
//!
//! 硬禁：回放时用当前本体解释旧事件却不报告语义差异。
//! 因此本模块对每条事件核对 underOntologyVersion 与当前版本，
//! 不一致时计入 VersionMismatch，由调用方决定是否继续。

use serde::Deserialize;
use worker::{Bucket, Env};

use crate::common::error::{AppError, AppResult};
use crate::ontology::version::OntologyRouter;

/// 回放范围内的轻量事件记录（只含审计字段，不含 payload）。
#[derive(Debug, Deserialize)]
pub struct EventRecordLite {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub tick: u64,
    pub ontology_version: String,
}

#[derive(Debug)]
pub struct ReplayReport {
    pub from_tick: u64,
    pub to_tick: u64,
    pub events: Vec<EventRecordLite>,
    /// 事件版本与回放时当前本体不一致的事件数（必须显式报告）。
    pub version_mismatches: usize,
}

/// 回放 [from_tick, to_tick] 的事件（R2 list + get）。
/// 事件量大时由调用方分页；MVP 限制单次扫描上限。
const MAX_SCAN: u32 = 1000;

/// 范围校验（纯函数，便于单测）。
pub fn validate_range(from_tick: u64, to_tick: u64) -> AppResult<()> {
    if to_tick < from_tick {
        return Err(AppError::InvalidInput(
            "to_tick must be >= from_tick".into(),
        ));
    }
    Ok(())
}

pub async fn replay(env: &Env, from_tick: u64, to_tick: u64) -> AppResult<ReplayReport> {
    validate_range(from_tick, to_tick)?;
    let bucket: Bucket = env.bucket("R2")?;
    let router = OntologyRouter::from_env(env).await?;

    let mut events = Vec::new();
    let mut version_mismatches = 0usize;
    for tick in from_tick..=to_tick {
        let prefix = format!("events/{tick}/");
        let listed = bucket.list().prefix(prefix).limit(MAX_SCAN).execute().await?;
        for obj in listed.objects() {
            let key = obj.key();
            match crate::storage::r2::get_json::<EventRecordLite>(&bucket, &key).await? {
                Some(ev) => {
                    if ev.ontology_version != router.current.as_str() {
                        version_mismatches += 1;
                    }
                    events.push(ev);
                }
                None => continue,
            }
        }
    }
    Ok(ReplayReport {
        from_tick,
        to_tick,
        events,
        version_mismatches,
    })
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;
    use crate::common::types::now_rfc3339;

    #[wasm_bindgen_test]
    fn validate_range_rejects_inverted() {
        assert!(validate_range(5, 4).is_err());
        assert!(validate_range(5, 5).is_ok());
        assert!(validate_range(5, 9).is_ok());
        // 错误信息里必须写明原因：回放是本体的审计入口，不能给含糊拒绝
        match validate_range(5, 4) {
            Err(AppError::InvalidInput(msg)) => assert!(msg.contains("to_tick")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn event_record_lite_deserializes_renamed_type() {
        let json = r#"{"id":"e1","type":"soc:AgentDecision","tick":7,"ontology_version":"1.0.0"}"#;
        let rec: EventRecordLite = serde_json::from_str(json).expect("deserialize");
        assert_eq!(rec.id, "e1");
        assert_eq!(rec.event_type, "soc:AgentDecision");
        assert_eq!(rec.tick, 7);
        assert_eq!(rec.ontology_version, "1.0.0");

        // 版本字段缺失即拒绝：filter_map 静默跳过会掩盖审计漏洞
        let bad = r#"{"id":"e1","type":"soc:AgentDecision","tick":7}"#;
        assert!(serde_json::from_str::<EventRecordLite>(bad).is_err());
    }

    #[wasm_bindgen_test]
    fn now_is_rfc3339() {
        assert!(chrono::DateTime::parse_from_rfc3339(&now_rfc3339()).is_ok());
    }
}
