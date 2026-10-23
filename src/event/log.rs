//! append-only 事件日志（R2 全量 + D1 索引）。
//!
//! 规则（AGENTS.md §4.3）：
//! - 事件 ID 用 ULID；ontology_version 由调用方从 OntologyRouter 取得后传入
//!   （router 是唯一版本来源，本模块不自行决定版本）；
//! - R2 键 events/{tick}/{event_id}.json 全局唯一，天然 append-only；
//! - D1 events_index 只做检索，不存事件体。

use serde::Serialize;
use worker::{Bucket, Env};

use crate::common::error::AppResult;
use crate::common::types::now_rfc3339;
use crate::storage::{d1, r2};

/// 一条社会事件的 R2 存储形态。
#[derive(Debug, Serialize)]
pub struct EventRecord {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub tick: u64,
    pub ontology_version: String,
    pub actor_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

/// 写事件：R2 全量 + D1 索引。两个存储无法原子，先 R2 后 D1 索引
/// （索引缺失可由 replay 补偿，事件体丢失不可补偿）。
pub async fn append(
    env: &Env,
    event_type: &str,
    event_id: &str,
    ontology_version: &str,
    tick: u64,
    actor_id: Option<&str>,
    payload: &serde_json::Value,
) -> AppResult<()> {
    let record = EventRecord {
        id: event_id.to_string(),
        event_type: event_type.to_string(),
        tick,
        ontology_version: ontology_version.to_string(),
        actor_id: actor_id.map(str::to_string),
        payload: payload.clone(),
        created_at: now_rfc3339(),
    };

    let bucket: Bucket = env.bucket("R2")?;
    r2::put_json(&bucket, &r2::event_key(tick, event_id), &record).await?;

    let db = env.d1("DB")?;
    d1::insert_event_index(
        &db,
        event_id,
        event_type,
        actor_id,
        None,
        tick,
        ontology_version,
    )
    .await?;

    // G8：事件已落盘后向 WebSocket hub 广播（best-effort 分发通道，
    // 不是第二事实源；失败只记日志，见 storage::do_state::notify_world）。
    if let Ok(record_json) = serde_json::to_string(&record) {
        crate::storage::do_state::notify_world(env, &record_json).await;
    }
    Ok(())
}
