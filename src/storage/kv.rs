//! KV 存储层：热状态 / 配额 / LLM 缓存 / 杀死开关 / 本体版本（SAD §6.2 键设计）。

use serde::{Deserialize, Serialize};
use worker::KvStore;

use crate::common::error::AppResult;

/// 分身热状态（KV agent:{id}:hot，TTL 1h）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotState {
    pub money: f64,
    pub food: f64,
    pub energy: f64,
    pub reputation: f64,
}

impl Default for HotState {
    fn default() -> Self {
        // 与 migrations/0001_init.sql 的 agent_state 默认值一致
        Self {
            money: 0.0,
            food: 0.0,
            energy: 100.0,
            reputation: 0.0,
        }
    }
}

/// 读热状态；未命中返回默认值（不视为错误）。
pub async fn get_hot_state(kv: &KvStore, agent_id: &str) -> AppResult<HotState> {
    let key = format!("agent:{agent_id}:hot");
    match kv.get(&key).json::<HotState>().await? {
        Some(state) => Ok(state),
        None => Ok(HotState::default()),
    }
}

/// 写热状态（TTL 1h）。D1 持久化由 tick 收尾批量完成，KV 只是热缓存。
pub async fn put_hot_state(kv: &KvStore, agent_id: &str, state: &HotState) -> AppResult<()> {
    let key = format!("agent:{agent_id}:hot");
    kv.put(&key, state)?.expiration_ttl(3600).execute().await?;
    Ok(())
}

/// 配额键：quota:{date}:{agent_id}（SAD §6.2）。
pub fn quota_key(date: &str, agent_id: &str) -> String {
    format!("quota:{date}:{agent_id}")
}

/// 当日已用 token 数；键不存在返回 0。
pub async fn get_quota_used(kv: &KvStore, date: &str, agent_id: &str) -> AppResult<i64> {
    match kv.get(&quota_key(date, agent_id)).json::<i64>().await? {
        Some(v) => Ok(v),
        None => Ok(0),
    }
}

/// 累加配额并写回（TTL 当日余量，MVP 固定 25h 兜底过期）。
pub async fn add_quota_used(
    kv: &KvStore,
    date: &str,
    agent_id: &str,
    tokens: i64,
) -> AppResult<i64> {
    let key = quota_key(date, agent_id);
    let used = get_quota_used(kv, date, agent_id).await? + tokens;
    kv.put(&key, used)?.expiration_ttl(25 * 3600).execute().await?;
    Ok(used)
}

/// LLM 缓存键（硬禁：缓存键必须绑定本体版本）。
/// llm:cache:{prompt_hash}:{ontology_version}:{model_version}:{temp}:{seed}
pub fn llm_cache_key(
    prompt_hash: &str,
    ontology_version: &str,
    model_version: &str,
    temperature_millis: i64,
    seed: i64,
) -> String {
    format!("llm:cache:{prompt_hash}:{ontology_version}:{model_version}:{temperature_millis}:{seed}")
}

/// 杀死开关（config:kill_switch）：账单超阈值时暂停非核心逻辑。
pub async fn kill_switch_on(kv: &KvStore) -> AppResult<bool> {
    Ok(kv
        .get("config:kill_switch")
        .json::<bool>()
        .await?
        .unwrap_or(false))
}

/// 当前本体版本（KV ontology:version）。
///
/// 坑（2026-10-22 rollback 500 的根因）：`put(String)` 经 `ToRawKvValue` 存的是
/// **裸字符串**（无 JSON 引号，见 worker-0.8.7 kv/mod.rs 的 `impl for str`），
/// 而 `.json::<String>()` 会让 JS 侧对裸串做 `JSON.parse` 直接抛错。
/// 数值/布尔能碰巧工作（"42"/"true" 是合法 JSON），字符串必须用 `text()` 读。
/// 写读两侧就此对称；本函数在 rollback 接线前从未被调用，bug 一直潜伏。
pub async fn get_ontology_version(kv: &KvStore) -> AppResult<Option<String>> {
    Ok(kv.get("ontology:version").text().await?)
}

/// 本体版本历史（KV ontology:version:history，只追加 JSON 数组）。
pub async fn get_ontology_history(kv: &KvStore) -> AppResult<Vec<String>> {
    Ok(kv
        .get("ontology:version:history")
        .json::<Vec<String>>()
        .await?
        .unwrap_or_default())
}

pub async fn append_ontology_history(kv: &KvStore, entry: &str) -> AppResult<()> {
    let mut history = get_ontology_history(kv).await?;
    history.push(entry.to_string());
    kv.put("ontology:version:history", history)?.execute().await?;
    Ok(())
}

/// 积分余额缓存（credit:account:{account_id}，TTL 10m）。
pub async fn get_credit_balance_cache(kv: &KvStore, account_id: &str) -> AppResult<Option<i64>> {
    Ok(kv
        .get(&format!("credit:account:{account_id}"))
        .json::<i64>()
        .await?)
}

pub async fn put_credit_balance_cache(
    kv: &KvStore,
    account_id: &str,
    balance: i64,
) -> AppResult<()> {
    kv.put(&format!("credit:account:{account_id}"), balance)?
        .expiration_ttl(600)
        .execute()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn llm_cache_key_binds_all_dimensions() {
        let k = llm_cache_key("abc123", "1.0.0", "gpt-x@2026-01", 700, 42);
        assert_eq!(
            k,
            "llm:cache:abc123:1.0.0:gpt-x@2026-01:700:42"
        );
        // 本体版本不同 ⇒ 缓存键必须不同（回放正确性）
        let k2 = llm_cache_key("abc123", "2.0.0", "gpt-x@2026-01", 700, 42);
        assert_ne!(k, k2);
        // 温度/种子不同 ⇒ 键不同（可复现性）
        assert_ne!(k, llm_cache_key("abc123", "1.0.0", "gpt-x@2026-01", 800, 42));
        assert_ne!(k, llm_cache_key("abc123", "1.0.0", "gpt-x@2026-01", 700, 43));
    }

    #[wasm_bindgen_test]
    fn quota_key_shape() {
        assert_eq!(quota_key("2026-10-22", "agent-1"), "quota:2026-10-22:agent-1");
    }

    #[wasm_bindgen_test]
    fn hot_state_defaults_match_migration() {
        let d = HotState::default();
        assert_eq!((d.money, d.food, d.energy, d.reputation), (0.0, 0.0, 100.0, 0.0));
    }
}
