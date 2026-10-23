//! 决策循环：单个 agent 被唤醒后的一轮处理（AGENTS.md §5.5 / §7.4）。
//!
//! MVP 规则化：能量低 → 休息事件；否则维持 idle。复杂决策走 LLM 的路径
//! 在 llm/gateway.rs 就绪后接入（路由与配额已实现）。

use worker::Env;

use crate::agent_runtime::scheduler::AgentTask;
use crate::common::error::AppResult;
use crate::common::logging;
use crate::common::types::now_rfc3339;
use crate::ontology::version::OntologyRouter;
use crate::storage::kv::{self, HotState};

/// 规则决策的单步推进（纯函数）：能量低于 20 → 休息恢复 +10（上限 100）；
/// 否则维持 idle，能量 -1（下限 0）。返回 (动作名, 推进后状态)。
///
/// 抽成纯函数供沙箱（governance::sandbox）复用同一套规则做离线仿真——
/// 沙箱与线上决策循环必须同规则，否则沙箱判定没有意义。
pub fn rule_step(state: &HotState) -> (&'static str, HotState) {
    let mut next = state.clone();
    let action = if next.energy < 20.0 {
        next.energy = (next.energy + 10.0).min(100.0);
        "rest"
    } else {
        next.energy = (next.energy - 1.0).max(0.0);
        "idle"
    };
    (action, next)
}

/// 一轮决策循环。幂等：同一 agent 同一 tick 重复执行结果一致
/// （Queue 批级重试依赖这一点）。
pub async fn run_decision_cycle(env: &Env, task: &AgentTask) -> AppResult<()> {
    let kv_store = env.kv("KV")?;

    // 1. 读热状态（KV 未命中走默认值）
    let state = kv::get_hot_state(&kv_store, &task.agent_id).await?;

    // 2. 规则决策（MVP，与沙箱共用 rule_step）
    let (action, state) = rule_step(&state);

    // 3. 写回热状态
    kv::put_hot_state(&kv_store, &task.agent_id, &state).await?;

    // 4. 决策事件（append-only；R2 全量 + D1 索引由 event::log::append 一并完成）
    let router = OntologyRouter::from_env(env).await?;
    let event_id = ulid::Ulid::generate().to_string();
    let payload = serde_json::json!({
        "agent_id": task.agent_id,
        "action": action,
        "reason": task.reason,
        "energy_after": state.energy,
    });
    crate::event::log::append(
        env,
        crate::ontology::iri::AGENT_DECISION,
        &event_id,
        router.for_action().as_str(),
        task.tick,
        Some(&task.agent_id),
        &payload,
    )
    .await?;

    logging::debug(
        "agent_decision",
        serde_json::json!({
            "agent_id": task.agent_id,
            "tick": task.tick,
            "action": action,
            "updated_at": now_rfc3339(),
        }),
    );
    Ok(())
}
