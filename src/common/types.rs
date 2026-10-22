//! 跨模块共享的基础类型。
//!
//! 注意：`Violation` / `ValidationResult` 放在这里而不是 governance/validator.rs，
//! 因为 ontology/shacl.rs 的 SHACL 报告与行动校验共用同一结构，避免两份定义漂移
//! （a.md 曾把它们定义在 validator.rs， scaffold 合并到 common，语义不变）。

use serde::{Deserialize, Serialize};

/// 社会 tick：与 UTC 时间并列的离散社会时间（每 5 分钟一个 tick）。
pub type Tick = u64;

/// UTC RFC3339 时间戳。
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 分身生命周期状态。取值与 ontology/shacl/person-card.ttl 的 sh:in 列表保持一致，
/// 两端必须同步修改（OWL 管语义、SHACL 管形状，这里是 SHACL 的 Rust 侧镜像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Created,
    Pending,
    Active,
    Idle,
    Dormant,
    Offline,
    Suspended,
    Deceased,
    Archived,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Created => "created",
            AgentStatus::Pending => "pending",
            AgentStatus::Active => "active",
            AgentStatus::Idle => "idle",
            AgentStatus::Dormant => "dormant",
            AgentStatus::Offline => "offline",
            AgentStatus::Suspended => "suspended",
            AgentStatus::Deceased => "deceased",
            AgentStatus::Archived => "archived",
        }
    }

    /// 解析状态字符串；非法值返回 None（调用方转 InvalidInput）。
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "created" => AgentStatus::Created,
            "pending" => AgentStatus::Pending,
            "active" => AgentStatus::Active,
            "idle" => AgentStatus::Idle,
            "dormant" => AgentStatus::Dormant,
            "offline" => AgentStatus::Offline,
            "suspended" => AgentStatus::Suspended,
            "deceased" => AgentStatus::Deceased,
            "archived" => AgentStatus::Archived,
            _ => return None,
        })
    }
}

/// 单条校验违规：path 为约束涉及的属性 IRI（或形状路径），message 面向人类。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub path: String,
    pub message: String,
}

/// 校验结果。ok 为真时 violations 为空；inferred 为 L2/L3 推理产出的推论三元组字符串。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub ok: bool,
    pub violations: Vec<Violation>,
    pub inferred: Vec<String>,
}

impl ValidationResult {
    pub fn ok_empty() -> Self {
        Self {
            ok: true,
            violations: Vec::new(),
            inferred: Vec::new(),
        }
    }

    pub fn with_violations(violations: Vec<Violation>) -> Self {
        Self {
            ok: violations.is_empty(),
            violations,
            inferred: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn agent_status_roundtrip_matches_shacl_list() {
        // 与 person-card.ttl 的 sh:in 列表逐项对齐
        let all = [
            "created",
            "pending",
            "active",
            "idle",
            "dormant",
            "offline",
            "suspended",
            "deceased",
            "archived",
        ];
        for s in all {
            let st = AgentStatus::parse(s).expect(s);
            assert_eq!(st.as_str(), s);
        }
        assert!(AgentStatus::parse("flying").is_none());
    }

    #[wasm_bindgen_test]
    fn validation_result_ok_flag_tracks_violations() {
        assert!(ValidationResult::ok_empty().ok);
        let r = ValidationResult::with_violations(vec![Violation {
            path: "p".into(),
            message: "m".into(),
        }]);
        assert!(!r.ok);
        assert_eq!(r.violations.len(), 1);
    }

    #[wasm_bindgen_test]
    fn now_rfc3339_is_parseable() {
        let t = now_rfc3339();
        assert!(chrono::DateTime::parse_from_rfc3339(&t).is_ok(), "{t}");
    }

    /// wasm 运行时冒烟：chrono::Utc::now() 需 wasmbind feature，
    /// ulid::Ulid::generate() 需 rand→getrandom 的 wasm_js 通路。
    /// 任一条没接好都会在这里 panic，而不是在生产 tick 里panic。
    #[wasm_bindgen_test]
    fn wasm_runtime_time_and_ulid_work() {
        assert!(!ulid::Ulid::generate().to_string().is_empty());
        assert!(!now_rfc3339().is_empty());
    }
}
