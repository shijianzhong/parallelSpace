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
use crate::storage::kv;

/// 一轮决策循环。幂等：同一 agent 同一 tick 重复执行结果一致
/// （Queue 批级重试依赖这一点）。
pub async fn run_decision_cycle(env: &Env, task: &AgentTask) -> AppResult<()> {
    let kv_store = env.kv("KV")?;

    // 1. 读热状态（KV 未命中走默认值）
    let mut state = kv::get_hot_state(&kv_store, &task.agent_id).await?;

    // 2. 规则决策（MVP）：能量低于 20 → 休息恢复
    let action = if state.energy < 20.0 {
        state.energy = (state.energy + 10.0).min(100.0);
        "rest"
    } else {
        state.energy = (state.energy - 1.0).max(0.0);
        "idle"
    };

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
