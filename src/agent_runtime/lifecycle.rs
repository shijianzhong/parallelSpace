//! 分身生命周期状态机（与 common/types.rs 的 AgentStatus 对应）。
//!
//! 制度先于组织：created（已建账号）→ pending（已支付待治理确认）→ active。
//! 迁移表是纯函数，便于单测；写库由 api/agent.rs 负责。

use crate::common::error::{AppError, AppResult};
use crate::common::types::AgentStatus;

/// 是否允许 from → to 的迁移。
pub fn can_transition(from: AgentStatus, to: AgentStatus) -> bool {
    use AgentStatus::*;
    match (from, to) {
        // 注册主线
        (Created, Pending) => true,   // 支付成功、治理确认前
        (Pending, Active) => true,    // 治理确认 + 实例化完成
        (Created, Active) => false,   // 未支付不得直接激活（硬规则）
        // 运行期
        (Active, Idle) => true,
        (Idle, Active) => true,
        (Active, Offline) => true,
        (Offline, Active) => true,
        (Active, Dormant) => true,
        (Dormant, Active) => true,
        // 治理
        (Active, Suspended) => true,
        (Suspended, Active) => true,  // 解除封禁
        (Idle, Suspended) => true,
        (Dormant, Suspended) => true,
        // 终态
        (Active, Deceased) => true,
        (Idle, Deceased) => true,
        (Dormant, Deceased) => true,
        (Suspended, Deceased) => true,
        (Deceased, Archived) => true,
        // 其余一律拒绝（含 Archived 的任何出口、Suspended→Idle 等）
        _ => false,
    }
}

/// 校验并返回目标状态；非法迁移报 Conflict。
pub fn ensure_transition(from: AgentStatus, to: AgentStatus) -> AppResult<AgentStatus> {
    if can_transition(from, to) {
        Ok(to)
    } else {
        Err(AppError::Conflict(format!(
            "illegal agent status transition {} -> {}",
            from.as_str(),
            to.as_str()
        )))
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn created_to_active_without_payment_rejected() {
        assert!(!can_transition(AgentStatus::Created, AgentStatus::Active));
    }

    #[wasm_bindgen_test]
    fn happy_path_allowed() {
        assert!(can_transition(AgentStatus::Created, AgentStatus::Pending));
        assert!(can_transition(AgentStatus::Pending, AgentStatus::Active));
    }

    #[wasm_bindgen_test]
    fn terminal_states_have_no_exit() {
        assert!(!can_transition(AgentStatus::Archived, AgentStatus::Active));
        assert!(can_transition(AgentStatus::Deceased, AgentStatus::Archived));
        assert!(!can_transition(AgentStatus::Archived, AgentStatus::Deceased));
    }

    #[wasm_bindgen_test]
    fn suspension_flow() {
        assert!(can_transition(AgentStatus::Active, AgentStatus::Suspended));
        assert!(can_transition(AgentStatus::Suspended, AgentStatus::Active));
        assert!(!can_transition(AgentStatus::Suspended, AgentStatus::Idle));
    }

    #[wasm_bindgen_test]
    fn ensure_transition_reports_conflict() {
        let e = ensure_transition(AgentStatus::Created, AgentStatus::Active).unwrap_err();
        assert!(matches!(e, AppError::Conflict(_)));
    }
}
