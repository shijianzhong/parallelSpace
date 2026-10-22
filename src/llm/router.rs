//! LLM 任务路由（AGENTS.md §5.4）：
//! SimpleSocial→小模型；Negotiation→中；Legislation/Arbitration→大。
//! Workers AI 仅降级，且仅 SimpleSocial。

use crate::governance::validator::ActionType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmTask {
    /// 常规社交/简单决策
    SimpleSocial,
    /// 交易/缔约/谈判
    Negotiation,
    /// 立法
    Legislation,
    /// 仲裁
    Arbitration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Small,
    Medium,
    Large,
}

impl ModelTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelTier::Small => "small",
            ModelTier::Medium => "medium",
            ModelTier::Large => "large",
        }
    }
}

/// 行动类型 → LLM 任务分级。
pub fn task_for_action(action: ActionType) -> LlmTask {
    match action {
        ActionType::Move | ActionType::Work | ActionType::Chat | ActionType::Learn => {
            LlmTask::SimpleSocial
        }
        ActionType::Trade | ActionType::Ally | ActionType::Employ => LlmTask::Negotiation,
        ActionType::Vote | ActionType::Legislate => LlmTask::Legislation,
        ActionType::Breach => LlmTask::Arbitration,
    }
}

/// 任务 → 模型档位。纯函数，便于单测。
pub fn route(task: LlmTask) -> ModelTier {
    match task {
        LlmTask::SimpleSocial => ModelTier::Small,
        LlmTask::Negotiation => ModelTier::Medium,
        LlmTask::Legislation | LlmTask::Arbitration => ModelTier::Large,
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn routing_matrix_matches_spec() {
        assert_eq!(route(LlmTask::SimpleSocial), ModelTier::Small);
        assert_eq!(route(LlmTask::Negotiation), ModelTier::Medium);
        assert_eq!(route(LlmTask::Legislation), ModelTier::Large);
        assert_eq!(route(LlmTask::Arbitration), ModelTier::Large);
    }

    #[wasm_bindgen_test]
    fn action_to_task_mapping() {
        assert_eq!(task_for_action(ActionType::Move), LlmTask::SimpleSocial);
        assert_eq!(task_for_action(ActionType::Trade), LlmTask::Negotiation);
        assert_eq!(task_for_action(ActionType::Legislate), LlmTask::Legislation);
        assert_eq!(task_for_action(ActionType::Breach), LlmTask::Arbitration);
    }
}
