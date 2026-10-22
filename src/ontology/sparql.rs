//! 参数化 SPARQL 构建（AGENTS.md §4.1：SPARQL 查询用参数化构建，禁止字符串拼接用户输入）。
//!
//! 策略：
//! - 标识符（IRI）必须过 [`crate::ontology::iri::iri_allowed`] 白名单；
//! - 字面量经 [`escape_literal`] 转义后进入 `VALUES` 子句；
//! - 数值先经 `f64`/`u32` 解析与范围校验，绝不接受原始字符串直接入句。
//!
//! 范围说明：L3 完整 SPARQL 1.1 查询由外部 open-ontologies 服务执行，
//! 本模块只覆盖 Worker 内需要拼装的少量模板查询。

use crate::common::error::{AppError, AppResult};
use crate::ontology::iri;

/// 一条参数化查询。`params` 记录实际进入语句的参数，仅供日志与审计。
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterizedQuery {
    pub sparql: String,
    pub params: Vec<String>,
}

/// 转义 SPARQL 字符串字面量：反斜杠、双引号、换行、回车、制表符。
pub fn escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// 校验 IRI 属于白名单命名空间，返回原串（不可变借用交给调用方转 String）。
fn checked_iri(iri_str: &str) -> AppResult<&str> {
    if !iri::iri_allowed(iri_str) {
        return Err(AppError::InvalidInput(format!(
            "IRI outside ontology whitelist: {iri_str}"
        )));
    }
    Ok(iri_str)
}

/// 校验经纬度范围（WGS84）。
fn checked_lon_lat(lon: f64, lat: f64) -> AppResult<()> {
    if !lon.is_finite() || !lat.is_finite() || !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err(AppError::InvalidInput(format!(
            "coordinate out of range: ({lon}, {lat})"
        )));
    }
    Ok(())
}

/// 查询某状态的 Agent 及其位置（地图图层 / 上下文构建用）。
/// status 为 None 时不带状态过滤；limit 夹在 1..=1000。
pub fn select_agents(status: Option<&str>, limit: u32) -> AppResult<ParameterizedQuery> {
    let limit = limit.clamp(1, 1000);
    let (filter, params) = match status {
        Some(s) => {
            let st = crate::common::types::AgentStatus::parse(s)
                .ok_or_else(|| AppError::InvalidInput(format!("unknown agent status: {s}")))?;
            (
                format!(
                    "?agent {} \"{}\" .",
                    iri::STATUS,
                    escape_literal(st.as_str())
                ),
                vec![st.as_str().to_string()],
            )
        }
        None => (String::new(), Vec::new()),
    };

    let sparql = format!(
        "PREFIX soc: <{SOC}>\nPREFIX sge: <{SGE}>\nSELECT ?agent ?name ?wkt WHERE {{\n  ?agent a soc:Agent ;\n         {DISPLAY_NAME} ?name ;\n         {LOCATED_AT} ?loc .\n  ?loc {HAS_WKT} ?wkt .\n  {filter}\n}} LIMIT {limit}",
        SOC = iri::SOC_NS,
        SGE = iri::GEO_NS,
        DISPLAY_NAME = iri::DISPLAY_NAME,
        LOCATED_AT = iri::LOCATED_AT,
        HAS_WKT = iri::SGE_HAS_WKT,
    );
    Ok(ParameterizedQuery { sparql, params })
}

/// 查询 bbox 内的 Agent。bbox 先在 Rust 侧校验，过滤在取数后进行
/// （本体只存 WKT，SPARQL 侧几何过滤依赖 L3 引擎，见 AGENTS.md §2 地图约束）。
pub fn agents_in_bbox(
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    limit: u32,
) -> AppResult<ParameterizedQuery> {
    checked_lon_lat(min_lon, min_lat)?;
    checked_lon_lat(max_lon, max_lat)?;
    if min_lon > max_lon || min_lat > max_lat {
        return Err(AppError::InvalidInput(
            "bbox min corner must be <= max corner".into(),
        ));
    }
    let limit = limit.clamp(1, 1000);
    let sparql = format!(
        "PREFIX soc: <{SOC}>\nPREFIX sge: <{SGE}>\nSELECT ?agent ?name ?wkt WHERE {{\n  ?agent a soc:Agent ;\n         {DISPLAY_NAME} ?name ;\n         {LOCATED_AT} ?loc .\n  ?loc {HAS_WKT} ?wkt .\n}} LIMIT {limit}",
        SOC = iri::SOC_NS,
        SGE = iri::GEO_NS,
        DISPLAY_NAME = iri::DISPLAY_NAME,
        LOCATED_AT = iri::LOCATED_AT,
        HAS_WKT = iri::SGE_HAS_WKT,
    );
    Ok(ParameterizedQuery {
        sparql,
        params: vec![
            min_lon.to_string(),
            min_lat.to_string(),
            max_lon.to_string(),
            max_lat.to_string(),
        ],
    })
}

/// 按 ID 查询 Agent 的属性（ID 作为转义字面量，绝不拼进 IRI）。
pub fn agent_by_id(agent_id: &str) -> AppResult<ParameterizedQuery> {
    if agent_id.is_empty() || agent_id.len() > 64 {
        return Err(AppError::InvalidInput("invalid agent id".into()));
    }
    let sparql = format!(
        "PREFIX soc: <{SOC}>\nSELECT ?p ?o WHERE {{\n  ?agent {DISPLAY_NAME} ?name .\n  FILTER(?name = \"{id}\")\n  ?agent ?p ?o .\n}} LIMIT 500",
        SOC = iri::SOC_NS,
        DISPLAY_NAME = iri::DISPLAY_NAME,
        id = escape_literal(agent_id),
    );
    Ok(ParameterizedQuery {
        sparql,
        params: vec![agent_id.to_string()],
    })
}

/// 查询某个类的实例（IRI 必须过白名单）。
pub fn instances_of_class(class_iri: &str, limit: u32) -> AppResult<ParameterizedQuery> {
    let class_iri = checked_iri(class_iri)?;
    let limit = limit.clamp(1, 1000);
    let sparql = format!(
        "PREFIX rdf: <{RDF}>\nSELECT ?s WHERE {{ ?s rdf:type <{class_iri}> . }} LIMIT {limit}",
        RDF = iri::RDF_NS,
    );
    Ok(ParameterizedQuery {
        sparql,
        params: vec![class_iri.to_string()],
    })
}

/// 查询某 tick 之后的事件（回放 / 审计用）。
pub fn events_since_tick(tick: u64, limit: u32) -> AppResult<ParameterizedQuery> {
    let limit = limit.clamp(1, 1000);
    let sparql = format!(
        "PREFIX soc: <{SOC}>\nPREFIX ste: <{STE}>\nSELECT ?event ?actor WHERE {{\n  ?event a soc:Event ;\n         {TICK} ?t .\n  FILTER(?t >= {tick})\n  OPTIONAL {{ ?event {PERFORMED} ?actor }}\n}} LIMIT {limit}",
        SOC = iri::SOC_NS,
        STE = iri::TIME_NS,
        TICK = iri::STE_SOCIAL_TICK,
        PERFORMED = iri::PERFORMED,
    );
    Ok(ParameterizedQuery {
        sparql,
        params: vec![tick.to_string()],
    })
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn escape_literal_handles_injection_attempts() {
        assert_eq!(escape_literal("a\"b"), "a\\\"b");
        assert_eq!(escape_literal("a\\b"), "a\\\\b");
        assert_eq!(escape_literal("a\nb"), "a\\nb");
        assert_eq!(escape_literal("x\") UNION { ?s ?p ?o } #"), "x\\\") UNION { ?s ?p ?o } #");
        assert_eq!(escape_literal("正常中文"), "正常中文");
    }

    #[wasm_bindgen_test]
    fn select_agents_filters_by_status_and_clamps_limit() {
        let q = select_agents(Some("active"), 99999).unwrap();
        assert!(q.sparql.contains("\"active\""));
        assert!(q.sparql.contains("LIMIT 1000"));
        assert_eq!(q.params, vec!["active".to_string()]);

        let q = select_agents(None, 0).unwrap();
        assert!(!q.sparql.contains("soc:status"));
        assert!(q.sparql.contains("LIMIT 1"));
    }

    #[wasm_bindgen_test]
    fn select_agents_rejects_unknown_status() {
        assert!(select_agents(Some("flying"), 10).is_err());
    }

    #[wasm_bindgen_test]
    fn bbox_validates_ranges() {
        assert!(agents_in_bbox(0.0, 0.0, 10.0, 10.0, 10).is_ok());
        assert!(agents_in_bbox(200.0, 0.0, 10.0, 10.0, 10).is_err());
        assert!(agents_in_bbox(10.0, 0.0, 0.0, 10.0, 10).is_err());
        assert!(agents_in_bbox(f64::NAN, 0.0, 10.0, 10.0, 10).is_err());
    }

    #[wasm_bindgen_test]
    fn agent_by_id_rejects_empty_and_overlong() {
        assert!(agent_by_id("").is_err());
        assert!(agent_by_id(&"x".repeat(65)).is_err());
        let q = agent_by_id("01J...").unwrap();
        assert!(q.sparql.contains("\"01J...\""));
    }

    #[wasm_bindgen_test]
    fn instances_of_class_enforces_iri_whitelist() {
        assert!(instances_of_class(iri::PERSON, 10).is_ok());
        assert!(instances_of_class("http://evil.example/x", 10).is_err());
    }

    #[wasm_bindgen_test]
    fn events_since_tick_inlines_validated_number_only() {
        let q = events_since_tick(42, 10).unwrap();
        assert!(q.sparql.contains("?t >= 42"));
        assert!(!q.sparql.contains("42\""));
    }
}
