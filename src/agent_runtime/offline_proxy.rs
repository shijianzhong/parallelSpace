//! 离线代理：分身离线期间代管决策的边界（PRD §7）。
//!
//! 硬禁：离线代理不得自动同意婚恋/背叛/重大契约。
//! 代理只能做维持性动作（移动/吃饭/简单工作），不可逆决策一律留待本人。

use crate::governance::validator::ActionType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyDecision {
    /// 代理可直接执行
    Allow,
    /// 留待本人上线处理
    DeferToOwner,
    /// 直接拒绝（写入审计）
    Deny,
}

impl ProxyDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyDecision::Allow => "allow",
            ProxyDecision::DeferToOwner => "defer_to_owner",
            ProxyDecision::Deny => "deny",
        }
    }
}

/// 离线代理对某动作的裁决。纯函数，便于单测。
pub fn decide(action: ActionType) -> ProxyDecision {
    match action {
        // 维持性生活动作
        ActionType::Move | ActionType::Work | ActionType::Learn => ProxyDecision::Allow,
        // 社交类可代理发起，但不可代理接受约束性后果
        ActionType::Chat => ProxyDecision::Allow,
        // 以下涉及不可逆或高影响承诺，留待本人
        ActionType::Trade
        | ActionType::Ally
        | ActionType::Employ
        | ActionType::Vote
        | ActionType::Legislate => ProxyDecision::DeferToOwner,
        // 背叛/违约类代理一律拒绝
        ActionType::Breach => ProxyDecision::Deny,
    }
}

/// 婚恋/重大契约在 MVP 尚无独立 ActionType，用契约类别名识别，
/// 走同一条 DeferToOwner 规则（契约由 api/org 与治理流程创建）。
pub fn contract_class_deferred(contract_class: &str) -> bool {
    matches!(
        contract_class,
        "marriage" | "betrayal" | "major_contract" | "loan"
    )
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn survival_actions_allowed() {
        assert_eq!(decide(ActionType::Move), ProxyDecision::Allow);
        assert_eq!(decide(ActionType::Work), ProxyDecision::Allow);
        assert_eq!(decide(ActionType::Learn), ProxyDecision::Allow);
    }

    #[wasm_bindgen_test]
    fn irreversible_decisions_deferred() {
        for a in [
            ActionType::Trade,
            ActionType::Ally,
            ActionType::Employ,
            ActionType::Vote,
            ActionType::Legislate,
        ] {
            assert_eq!(decide(a), ProxyDecision::DeferToOwner, "{a:?}");
        }
    }

    #[wasm_bindgen_test]
    fn betrayal_denied() {
        assert_eq!(decide(ActionType::Breach), ProxyDecision::Deny);
    }

    #[wasm_bindgen_test]
    fn major_contracts_deferred() {
        assert!(contract_class_deferred("marriage"));
        assert!(contract_class_deferred("major_contract"));
        assert!(!contract_class_deferred("groceries"));
    }
}
