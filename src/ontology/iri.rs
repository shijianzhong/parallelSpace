//! IRI 常量集中地（AGENTS.md §4.1：所有 IRI 用 ontology/iri.rs 常量，禁止散落字符串字面量）。
//! 命名空间与 ontology/*.ttl 中的 @prefix 声明一一对应。

pub const SOC_NS: &str = "http://soc.example/ontology#";
pub const GOV_NS: &str = "http://soc.example/gov/ontology#";
pub const ECON_NS: &str = "http://soc.example/econ/ontology#";
pub const GEO_NS: &str = "http://soc.example/geo/ontology#";
pub const TIME_NS: &str = "http://soc.example/time/ontology#";
pub const GEO_SPARQL_NS: &str = "http://www.opengis.net/ont/geosparql#";
pub const OWL_TIME_NS: &str = "http://www.w3.org/2006/time#";
/// L0 中立宪法（l0-constitution/core.ttl）的命名空间。
pub const CORE_NS: &str = "http://parallelspace.org/ontology/core#";

pub const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";
pub const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

pub fn soc(local: &str) -> String {
    format!("{SOC_NS}{local}")
}
pub fn gov(local: &str) -> String {
    format!("{GOV_NS}{local}")
}
pub fn econ(local: &str) -> String {
    format!("{ECON_NS}{local}")
}
pub fn sge(local: &str) -> String {
    format!("{GEO_NS}{local}")
}
pub fn ste(local: &str) -> String {
    format!("{TIME_NS}{local}")
}
pub fn core(local: &str) -> String {
    format!("{CORE_NS}{local}")
}

// ---- 核心类 ----
pub const AGENT: &str = "http://soc.example/ontology#Agent";
pub const PERSON: &str = "http://soc.example/ontology#Person";
pub const NPC: &str = "http://soc.example/ontology#NPC";
pub const ORGANIZATION: &str = "http://soc.example/ontology#Organization";
pub const POSITION: &str = "http://soc.example/ontology#Position";
pub const EVENT: &str = "http://soc.example/ontology#Event";
pub const RELATIONSHIP: &str = "http://soc.example/ontology#Relationship";
pub const CREDIT_PURCHASED: &str = "http://soc.example/econ/ontology#CreditPurchased";
pub const ROLLED_BACK: &str = "http://soc.example/gov/ontology#RolledBack";
/// 两段式发布：提案（pending，未生效）。
pub const VERSION_PROPOSED: &str = "http://soc.example/gov/ontology#VersionProposed";
/// 两段式发布：审批通过（生效）。
pub const VERSION_APPROVED: &str = "http://soc.example/gov/ontology#VersionApproved";
/// 新本体域注册（L1/L2 种子）。
pub const DOMAIN_REGISTERED: &str = "http://soc.example/gov/ontology#DomainRegistered";
/// 决策循环事件类型（soc.ttl 中声明为 soc:Event 子类）。
pub const AGENT_DECISION: &str = "http://soc.example/ontology#AgentDecision";

// ---- 核心属性 ----
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const MEMBER_OF: &str = "http://soc.example/ontology#memberOf";
pub const HOLDS_POSITION: &str = "http://soc.example/ontology#holdsPosition";
pub const OWNS: &str = "http://soc.example/ontology#owns";
pub const PARTY_TO: &str = "http://soc.example/ontology#partyTo";
pub const LOCATED_AT: &str = "http://soc.example/ontology#locatedAt";
pub const HAS_SKILL: &str = "http://soc.example/ontology#hasSkill";
pub const HAS_GOAL: &str = "http://soc.example/ontology#hasGoal";
pub const DISPLAY_NAME: &str = "http://soc.example/ontology#displayName";
pub const STATUS: &str = "http://soc.example/ontology#status";
pub const PERFORMED: &str = "http://soc.example/ontology#performed";
pub const AFFECTED: &str = "http://soc.example/ontology#affected";
pub const UNDER_ONTOLOGY_VERSION: &str = "http://soc.example/ontology#underOntologyVersion";
pub const GOV_HAS_CHARTER: &str = "http://soc.example/gov/ontology#hasCharter";
pub const GOV_DEFINES_POSITION: &str = "http://soc.example/gov/ontology#definesPosition";
pub const ECON_AMOUNT: &str = "http://soc.example/econ/ontology#amount";
pub const ECON_TRANSACTION: &str = "http://soc.example/econ/ontology#Transaction";
pub const SGE_HAS_WKT: &str = "http://soc.example/geo/ontology#hasWKT";
pub const STE_SOCIAL_TICK: &str = "http://soc.example/time/ontology#socialTick";

// ---- 推理用 ----
pub const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
pub const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";

/// 本体命名空间白名单：任何拼进 SPARQL / 负载的 IRI 必须属于这些命名空间
/// （AGENTS.md §4.4：RDF 输入限制三元组数量和 IRI 白名单）。
pub const ALLOWED_NAMESPACES: &[&str] = &[
    SOC_NS,
    GOV_NS,
    ECON_NS,
    GEO_NS,
    TIME_NS,
    GEO_SPARQL_NS,
    OWL_TIME_NS,
    CORE_NS,
    RDF_NS,
    RDFS_NS,
    OWL_NS,
    XSD_NS,
];

/// IRI 是否属于白名单命名空间。拒绝一切外部 IRI（含用户可控注入）。
pub fn iri_allowed(iri: &str) -> bool {
    ALLOWED_NAMESPACES.iter().any(|ns| iri.starts_with(ns))
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn namespace_helpers_build_absolute_iris() {
        assert_eq!(soc("Person"), "http://soc.example/ontology#Person");
        assert_eq!(gov("Law"), "http://soc.example/gov/ontology#Law");
        assert_eq!(econ("Transaction"), "http://soc.example/econ/ontology#Transaction");
        assert_eq!(ste("socialTick"), "http://soc.example/time/ontology#socialTick");
        assert_eq!(core("Agent"), "http://parallelspace.org/ontology/core#Agent");
    }

    #[wasm_bindgen_test]
    fn iri_whitelist_accepts_ours_rejects_foreign() {
        assert!(iri_allowed(PERSON));
        assert!(iri_allowed(&core("ChangeProposal")));
        assert!(iri_allowed(GOV_HAS_CHARTER));
        assert!(iri_allowed("http://www.w3.org/2001/XMLSchema#decimal"));
        assert!(!iri_allowed("http://evil.example/ontology#Person"));
        assert!(!iri_allowed("file:///etc/passwd"));
        assert!(!iri_allowed(""));
    }
}
