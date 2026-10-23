//! 本体版本管理（AGENTS.md §5.7 / §7.8）。
//!
//! 两段式发布（2026-10-22 治理增强，parallelSpace 存在论约定落地）：
//! - `publish_version`（第一段·提案）：写 R2 `ontology/{version}/`（不覆盖旧版本）
//!   → D1 落 **pending** 版本行（`proposed_by` 记录提案者，`approved_by` 为空）
//!   → **不碰 KV**：未审批的版本不生效、不进 history，`OntologyRouter` 路由不到；
//! - `approve_version`（第二段·审批）：pending → active（同批次 supersede 旧 active、
//!   激活本版本的 seed 域）→ 追加 KV history + 写 `ontology:version`；
//! - `rollback`：目标必须在 history（审批过的版本）→ 换 KV → 版本行状态互换；
//! - `register_domain`：新域注册为 L1/L2 种子（L0 与保留名一律拒绝）。
//!
//! L0 宪法层（`touches_l0` 名单）变更显式拒绝并记录；域注册表替代原硬编码
//! 文件名白名单——没有白名单，`transport.ttl` 这类新域永远进不来。

use worker::Env;

use crate::common::error::{AppError, AppResult};
use crate::ontology::iri;
use crate::ontology::version::{OntologyRouter, OntologyVersion};
use crate::storage::{d1, kv, r2};

/// L0 宪法层概念：这些 IRI 的任何变更一律拒绝。
const L0_CONCEPTS: &[&str] = &[
    "http://soc.example/gov/ontology#Constitution",
    "http://soc.example/ontology#Agent",
    "http://soc.example/ontology#Event",
    "http://soc.example/ontology#underOntologyVersion",
];

/// L0 宪法域（中立宪法 core.ttl 的承载名）：不可注册、不可发布。
const L0_RESERVED_DOMAINS: &[&str] = &["core"];

/// 可注册的层：L0 不可注册（宪法不可改），只能 L1 治理 / L2 自由涌现。
const REGISTRABLE_LAYERS: &[&str] = &["L1", "L2"];

/// 变更是否触及 L0 宪法层（纯函数，便于单测）。
pub fn touches_l0(changed_concepts: &[String]) -> bool {
    changed_concepts
        .iter()
        .any(|c| L0_CONCEPTS.contains(&c.as_str()))
}

/// 域名解析：1..=32 字符，小写字母开头，`[a-z0-9-]`。
/// 拒绝路径穿越（`..`、`/`）、大写、点号等一切怪异输入。
pub fn parse_domain(domain: &str) -> AppResult<String> {
    let invalid = || AppError::InvalidInput("invalid ontology domain".into());
    if domain.is_empty() || domain.len() > 32 {
        return Err(invalid());
    }
    let first = domain.chars().next().ok_or_else(invalid)?;
    if !first.is_ascii_lowercase() {
        return Err(invalid());
    }
    if !domain
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(invalid());
    }
    Ok(domain.to_string())
}

/// 层校验：`L0` 一律 403（宪法不可注册）；只接受 `L1` / `L2`。
pub fn validate_layer(layer: &str) -> AppResult<()> {
    if layer == "L0" {
        return Err(AppError::Forbidden);
    }
    if !REGISTRABLE_LAYERS.contains(&layer) {
        return Err(AppError::InvalidInput(format!(
            "invalid ontology layer: {layer} (expected L1 or L2)"
        )));
    }
    Ok(())
}

/// 域可注册性（纯函数）：语法 + L0 保留名 + 层。
pub fn check_domain_registrable(domain: &str, layer: &str) -> AppResult<String> {
    let domain = parse_domain(domain)?;
    validate_layer(layer)?;
    if L0_RESERVED_DOMAINS.contains(&domain.as_str()) {
        return Err(AppError::Forbidden);
    }
    Ok(domain)
}

/// 从 turtle 文件名取域名：必须形如 `{domain}.ttl`（防目录穿越写坏 R2）。
pub fn file_domain(name: &str) -> AppResult<String> {
    let domain = name.strip_suffix(".ttl").ok_or_else(|| {
        AppError::InvalidInput(format!("ontology file must be named <domain>.ttl: {name}"))
    })?;
    parse_domain(domain)
}

/// 一批 turtle 文件的域名清单（去重、保序）。
pub fn domains_from_files(files: &[(String, Vec<u8>)]) -> AppResult<Vec<String>> {
    let mut domains: Vec<String> = Vec::new();
    for (name, _) in files {
        let d = file_domain(name)?;
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    Ok(domains)
}

/// 发布新本体版本（第一段·提案）。turtle_files 为 (文件名, 内容) 列表。
///
/// 落 pending：不写 KV `ontology:version`、不进 history——审批前不生效。
pub async fn publish_version(
    env: &Env,
    version: &str,
    turtle_files: Vec<(String, Vec<u8>)>,
    prior_version: &str,
    proposed_by: &str,
) -> AppResult<()> {
    let version_check = OntologyVersion::parse(version)?;
    let version = version_check.as_str().to_string();
    if proposed_by.is_empty() {
        return Err(AppError::Forbidden);
    }
    if turtle_files.is_empty() {
        return Err(AppError::InvalidInput("no ontology files".into()));
    }
    let domains = domains_from_files(&turtle_files)?;

    let db = env.d1("DB")?;
    // 输入验证先行：域注册表校验（替代原硬编码文件名白名单）——
    // 每个文件的域必须已注册且未退役，否则 400（早于版本冲突检查）。
    for domain in &domains {
        match d1::get_domain(&db, domain).await? {
            None => {
                return Err(AppError::InvalidInput(format!(
                    "unregistered ontology domain: {domain} (register it first)"
                )))
            }
            Some(row) if row.status == "retired" => {
                return Err(AppError::InvalidInput(format!(
                    "ontology domain retired: {domain}"
                )))
            }
            Some(_) => {}
        }
    }
    // 版本行已存在（含此前的 pending）→ 冲突，避免 R2 写了孤儿键
    if d1::get_version(&db, &version).await?.is_some() {
        return Err(AppError::Conflict(format!(
            "ontology version {version} already published"
        )));
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

    d1::insert_pending_version(&db, &version, prior_version, proposed_by, &domains).await?;

    // 提案事件（append-only 审计轨迹；按当前路由版本打戳）
    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "version": version,
        "prior_version": prior_version,
        "proposed_by": proposed_by,
        "domains": domains,
    });
    crate::event::log::append(
        env,
        iri::VERSION_PROPOSED,
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(proposed_by),
        &payload,
    )
    .await?;

    crate::common::logging::info(
        "ontology_proposed",
        serde_json::json!({
            "version": version,
            "prior_version": prior_version,
            "proposed_by": proposed_by,
            "domains": domains,
        }),
    );
    Ok(())
}

/// 审批发布（第二段）：pending → active，写 KV，激活 seed 域。
pub async fn approve_version(env: &Env, version: &str, approved_by: &str) -> AppResult<()> {
    if approved_by.is_empty() {
        return Err(AppError::Forbidden);
    }
    let db = env.d1("DB")?;
    let row = d1::get_version(&db, version)
        .await?
        .ok_or_else(|| AppError::OntologyVersionNotFound(version.to_string()))?;
    if row.status != "pending" {
        return Err(AppError::Conflict(format!(
            "ontology version {version} is not pending (status: {})",
            row.status
        )));
    }
    if !d1::approve_version_row(&db, version, approved_by).await? {
        return Err(AppError::Conflict(format!(
            "ontology version {version} is not pending"
        )));
    }

    let kv_store = env.kv("KV")?;
    kv::append_ontology_history(&kv_store, version).await?;
    kv_store
        .put("ontology:version", version.to_string())?
        .execute()
        .await?;

    let domains = d1::get_version_domains(&db, version).await?;
    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "version": version,
        "approved_by": approved_by,
        "proposed_by": row.proposed_by,
        "domains": domains,
    });
    crate::event::log::append(
        env,
        iri::VERSION_APPROVED,
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(approved_by),
        &payload,
    )
    .await?;

    crate::common::logging::info(
        "ontology_approved",
        serde_json::json!({ "version": version, "approved_by": approved_by }),
    );
    Ok(())
}

/// 回滚到历史版本。目标必须存在于 history 中（不能滚到从未审批的版本）。
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

    let current = kv::get_ontology_version(&kv_store).await?;
    kv_store
        .put("ontology:version", target_version.to_string())?
        .execute()
        .await?;

    // 版本行状态互换：被离开的 active → rolled_back，目标重新 active
    if let Some(cur) = current.as_deref() {
        if cur != target_version {
            let db = env.d1("DB")?;
            d1::swap_active_version(&db, cur, target_version).await?;
        }
    }

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

/// 注册新本体域（L1/L2 种子）。L0 与保留名一律拒绝。
pub async fn register_domain(
    env: &Env,
    domain: &str,
    layer: &str,
    description: Option<&str>,
    registered_by: &str,
) -> AppResult<()> {
    let domain = check_domain_registrable(domain, layer)?;
    if registered_by.is_empty() {
        return Err(AppError::Forbidden);
    }
    let db = env.d1("DB")?;
    d1::register_domain(&db, &domain, layer, description, registered_by).await?;

    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "domain": domain,
        "layer": layer,
        "description": description,
        "registered_by": registered_by,
    });
    crate::event::log::append(
        env,
        iri::DOMAIN_REGISTERED,
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        Some(registered_by),
        &payload,
    )
    .await?;

    crate::common::logging::info(
        "ontology_domain_registered",
        serde_json::json!({ "domain": domain, "layer": layer, "registered_by": registered_by }),
    );
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

    #[wasm_bindgen_test]
    fn domain_parse_accepts_sane_names() {
        assert_eq!(parse_domain("transport").unwrap(), "transport");
        assert_eq!(parse_domain("a-b-9").unwrap(), "a-b-9");
        assert_eq!(parse_domain("gov2").unwrap(), "gov2");
    }

    #[wasm_bindgen_test]
    fn domain_parse_rejects_traversal_and_noise() {
        for bad in [
            "",
            "..",
            "../soc",
            "soc.ttl",
            "Soc",
            "so c",
            "soc/",
            "-soc",
            "1soc",
            &"x".repeat(33),
        ] {
            assert!(parse_domain(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[wasm_bindgen_test]
    fn l0_layer_and_reserved_domain_rejected() {
        // L0 不可注册（宪法不可改）；层大小写严格
        assert!(matches!(validate_layer("L0"), Err(AppError::Forbidden)));
        assert!(validate_layer("L1").is_ok());
        assert!(validate_layer("L2").is_ok());
        assert!(validate_layer("l2").is_err());
        assert!(validate_layer("L3").is_err());
        // core 是 L0 中立宪法（l0-constitution/core.ttl）的保留域名
        assert!(matches!(
            check_domain_registrable("core", "L2"),
            Err(AppError::Forbidden)
        ));
        assert!(check_domain_registrable("transport", "L2").is_ok());
        assert!(check_domain_registrable("transport", "L1").is_ok());
        assert!(check_domain_registrable("transport", "L0").is_err());
    }

    #[wasm_bindgen_test]
    fn file_domain_requires_domain_ttl_shape() {
        assert_eq!(file_domain("soc.ttl").unwrap(), "soc");
        assert_eq!(file_domain("transport.ttl").unwrap(), "transport");
        for bad in [
            "soc.txt",
            "soc",
            "../soc.ttl",
            "soc.ttl.ttl",
            ".ttl",
            "SOC.ttl",
            "sub/dir.ttl",
        ] {
            assert!(file_domain(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[wasm_bindgen_test]
    fn domains_from_files_dedups_preserving_order() {
        let files = vec![
            ("soc.ttl".to_string(), b"a".to_vec()),
            ("geo.ttl".to_string(), b"b".to_vec()),
            ("soc.ttl".to_string(), b"c".to_vec()),
        ];
        assert_eq!(
            domains_from_files(&files).unwrap(),
            vec!["soc".to_string(), "geo".to_string()]
        );
        assert!(domains_from_files(&[("nope.txt".to_string(), b"x".to_vec())]).is_err());
        assert!(domains_from_files(&[]).unwrap().is_empty());
    }
}
