//! LLM 响应缓存（KV）。
//!
//! 硬禁：缓存键必须绑定本体版本（否则跨本体版本串味）。
//! 键格式见 storage::kv::llm_cache_key；命中缓存不重复调用真实 LLM（回放可复现）。

use worker::Env;

use crate::common::error::AppResult;
use crate::storage::kv;

/// FNV-1a 64 位。选它而不是 std DefaultHasher：跨进程/跨版本稳定，
/// 回放与线上必须算出同一个 prompt_hash。
pub fn prompt_hash(prompt: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in prompt.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

/// 查缓存。命中返回 Some(JSON 字符串)。
pub async fn get_cached(
    env: &Env,
    prompt: &str,
    ontology_version: &str,
    model_version: &str,
    temperature_millis: i64,
    seed: i64,
) -> AppResult<Option<String>> {
    let kv_store = env.kv("KV")?;
    let key = kv::llm_cache_key(
        &prompt_hash(prompt),
        ontology_version,
        model_version,
        temperature_millis,
        seed,
    );
    // 存的是裸字符串（put(String)），必须用 text() 读——用 .json::<String>()
    // 会对裸串做 JSON.parse 直接抛错（README「三个坑」第 3 条 / kv.rs 既定配对规则）。
    Ok(kv_store.get(&key).text().await?)
}

/// 写缓存（TTL 7 天，SAD §6.2）。
/// 只允许写入**真实上游**的响应；占位提案禁止写入（会毒化缓存，见 gateway.rs）。
/// 当前无调用方：真实上游接入（M2）时由 gateway 在拿到真实响应后调用。
pub async fn put_cached(
    env: &Env,
    prompt: &str,
    ontology_version: &str,
    model_version: &str,
    temperature_millis: i64,
    seed: i64,
    response_json: &str,
) -> AppResult<()> {
    let kv_store = env.kv("KV")?;
    let key = kv::llm_cache_key(
        &prompt_hash(prompt),
        ontology_version,
        model_version,
        temperature_millis,
        seed,
    );
    kv_store
        .put(&key, response_json.to_string())?
        .expiration_ttl(7 * 24 * 3600)
        .execute()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn prompt_hash_is_stable_and_deterministic() {
        let a = prompt_hash("同一个 prompt");
        let b = prompt_hash("同一个 prompt");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[wasm_bindgen_test]
    fn prompt_hash_discriminates() {
        assert_ne!(prompt_hash("a"), prompt_hash("b"));
        assert_ne!(prompt_hash(""), prompt_hash(" "));
    }
}
