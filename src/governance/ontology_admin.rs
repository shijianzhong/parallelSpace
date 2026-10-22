//! 本体版本管理（AGENTS.md §5.7 / §7.8）。
//!
//! - publish_version：写 R2 ontology/{version}/（不覆盖旧版本）→ 更新 D1
//!   ontology_versions → 追加 KV ontology:version:history；
//! - rollback：校验目标版本存在 → 更新 KV → 写 gov:RolledBack 事件；
//! - L0 宪法层（gov:Constitution 及其直接依赖）变更显式拒绝并记录。

use worker::Env;

use crate::common::error::{AppError, AppResult};
use crate::ontology::iri;
use crate::ontology::version::OntologyRouter;
use crate::storage::{kv, r2};

/// L0 宪法层概念：这些 IRI 的任何变更一律拒绝。
const L0_CONCEPTS: &[&str] = &[
    "http://soc.example/gov/ontology#Constitution",
    "http://soc.example/ontology#Agent",
    "http://soc.example/ontology#Event",
    "http://soc.example/ontology#underOntologyVersion",
];

/// 变更是否触及 L0 宪法层（纯函数，便于单测）。
pub fn touches_l0(changed_concepts: &[String]) -> bool {
    changed_concepts
        .iter()
        .any(|c| L0_CONCEPTS.contains(&c.as_str()))
}

/// 发布新本体版本。turtle_files 为 (文件名, 内容) 列表。
pub async fn publish_version(
    env: &Env,
    version: &str,
    turtle_files: Vec<(String, Vec<u8>)>,
    prior_version: &str,
    approved_by: &str,
) -> AppResult<()> {
    let version_check = crate::ontology::version::OntologyVersion::parse(version)?;
    let version = version_check.as_str().to_string();
    if approved_by.is_empty() {
        return Err(AppError::Forbidden);
    }
    if turtle_files.is_empty() {
        return Err(AppError::InvalidInput("no ontology files".into()));
    }

    let bucket = env.bucket("R2")?;
    // 不覆盖旧版本：先探测目标键是否已存在
    for (name, content) in &turtle_files {
        let key = r2::ontology_key(&version, name);
        if bucket.head(&key).await?.is_some() {
            return Err(AppError::Conflict(format!(
                "ontology version {version} already published"
            )));
        }
        bucket.put(key.clone(), content.clone()).execute().await?;
    }

    let db = env.d1("DB")?;
    db.prepare(
        "INSERT INTO ontology_versions (version, published_at, prior_version, status) \
         VALUES (?1, ?2, ?3, 'active') \
         ON CONFLICT(version) DO UPDATE SET status = 'active'",
    )
    .bind(&[
        worker::wasm_bindgen::JsValue::from_str(&version),
        worker::wasm_bindgen::JsValue::from_str(&crate::common::types::now_rfc3339()),
        worker::wasm_bindgen::JsValue::from_str(prior_version),
    ])?
    .run()
    .await?;

    let kv_store = env.kv("KV")?;
    kv::append_ontology_history(&kv_store, &version).await?;
    kv_store
        .put("ontology:version", version.clone())?
        .execute()
        .await?;

    crate::common::logging::info(
        "ontology_published",
        serde_json::json!({
            "version": version,
            "prior_version": prior_version,
            "approved_by": approved_by,
        }),
    );
    Ok(())
}

/// 回滚到历史版本。目标必须存在于 history 中（不能滚到从未发布的版本）。
pub async fn rollback(
    env: &Env,
    target_version: &str,
    reason: &str,
    operator: &str,
) -> AppResult<()> {
    if operator.is_empty() {
        return Err(AppError::Forbidden);
    }
    let kv_store = env.kv("KV")?;
    let history = kv::get_ontology_history(&kv_store).await?;
    if !history.iter().any(|v| v == target_version) {
        return Err(AppError::OntologyVersionNotFound(target_version.to_string()));
    }

    kv_store
        .put("ontology:version", target_version.to_string())?
        .execute()
        .await?;

    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "target_version": target_version,
        "reason": reason,
        "operator": operator,
    });
    crate::event::log::append(
        env,
        iri::ROLLED_BACK,
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(operator),
        &payload,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn l0_changes_rejected() {
        assert!(touches_l0(&[
            "http://soc.example/gov/ontology#Constitution".to_string()
        ]));
        assert!(touches_l0(&[
            "http://soc.example/ontology#Event".to_string()
        ]));
        // L1/L2 概念不在 L0 名单
        assert!(!touches_l0(&[
            "http://soc.example/gov/ontology#Law".to_string()
        ]));
        assert!(!touches_l0(&[]));
    }
}
