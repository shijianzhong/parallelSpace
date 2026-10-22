//! 本体版本与 OntologyRouter。
//!
//! 规则（AGENTS.md §4.3 / §5.7）：
//! - 事件写入时 ontology_version 由 OntologyRouter 注入，不可由调用方传入；
//! - Router 只认 current + previous 两个版本，更早的事件查询直接报错；
//! - L0 宪法层不可变，L1 制度层走治理流程（governance/ontology_admin.rs）。

use crate::common::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OntologyVersion(pub String);

impl OntologyVersion {
    /// 版本号：非空、<= 32 字符。MVP 不强制 semver，治理流程负责约束。
    pub fn parse(s: &str) -> AppResult<Self> {
        if s.is_empty() || s.len() > 32 {
            return Err(AppError::InvalidInput("invalid ontology version".into()));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 双版本路由器：行动用 current，事件按自带版本号路由到 current 或 previous。
pub struct OntologyRouter {
    pub current: OntologyVersion,
    pub previous: Option<OntologyVersion>,
}

impl OntologyRouter {
    pub fn new(current: OntologyVersion, previous: Option<OntologyVersion>) -> Self {
        Self { current, previous }
    }

    /// 从环境构造：KV ontology:version 为当前版本，history 的倒数第二个为 previous；
    /// KV 未初始化时回退 env var ONTOLOGY_VERSION，再回退 "1.0.0"（启动兜底）。
    pub async fn from_env(env: &worker::Env) -> AppResult<Self> {
        let kv = env.kv("KV")?;
        let history = crate::storage::kv::get_ontology_history(&kv).await?;
        let current_str = if let Some(last) = history.last() {
            last.clone()
        } else {
            env.var("ONTOLOGY_VERSION")
                .map(|v| v.to_string())
                .unwrap_or_else(|_| "1.0.0".to_string())
        };
        let previous = if history.len() >= 2 {
            Some(OntologyVersion::parse(&history[history.len() - 2])?)
        } else {
            None
        };
        Ok(Self {
            current: OntologyVersion::parse(&current_str)?,
            previous,
        })
    }

    /// 新行动一律绑定当前版本。
    pub fn for_action(&self) -> &OntologyVersion {
        &self.current
    }

    /// 历史事件按记录时的版本解释；current/previous 之外的版本拒绝服务
    /// （防止用错版本解释旧事件，见硬禁「回放时用当前本体解释旧事件却不报告语义差异」）。
    pub fn for_event(&self, event_version: &str) -> AppResult<&OntologyVersion> {
        if event_version == self.current.0 {
            return Ok(&self.current);
        }
        if let Some(prev) = self.previous.as_ref() {
            if event_version == prev.0 {
                return Ok(prev);
            }
        }
        Err(AppError::OntologyVersionNotFound(event_version.to_string()))
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    fn router() -> OntologyRouter {
        OntologyRouter::new(
            OntologyVersion::parse("2.0.0").unwrap(),
            Some(OntologyVersion::parse("1.0.0").unwrap()),
        )
    }

    #[wasm_bindgen_test]
    fn parse_rejects_empty_and_too_long() {
        assert!(OntologyVersion::parse("").is_err());
        assert!(OntologyVersion::parse(&"x".repeat(33)).is_err());
        assert!(OntologyVersion::parse("1.0.0").is_ok());
    }

    #[wasm_bindgen_test]
    fn for_action_uses_current() {
        assert_eq!(router().for_action().as_str(), "2.0.0");
    }

    #[wasm_bindgen_test]
    fn for_event_accepts_current_and_previous_only() {
        let r = router();
        assert_eq!(r.for_event("2.0.0").unwrap().as_str(), "2.0.0");
        assert_eq!(r.for_event("1.0.0").unwrap().as_str(), "1.0.0");
        assert!(matches!(
            r.for_event("0.9.0"),
            Err(AppError::OntologyVersionNotFound(_))
        ));
    }
}
