//! 调度器：Cron tick + Queues 消费（AGENTS.md §5.5 / §7.4）。
//!
//! 硬规则：
//! - 状态持久、计算离散：无常驻循环，agent 按 tick/事件唤醒；
//! - 每 tick 结束批量写 D1，禁止逐条 execute；
//! - 常规行为规则化，复杂决策才走 llm::gateway::propose；
//! - 记录 tick 耗时、唤醒数、LLM 调用数、校验失败数。

use worker::{console_error, event, Context, Env, MessageBatch, ScheduleContext, ScheduledEvent};

use crate::common::error::AppResult;
use crate::common::logging;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AgentTask {
    pub agent_id: String,
    pub tick: u64,
    pub reason: String,
}

/// 一个 tick 的墙钟长度（秒）。wrangler.toml 的 crons 必须与之对齐。
pub const TICK_SECONDS: u64 = 300;

/// 5 分钟 = 1 tick 的换算。抽成纯函数以便单测（不依赖挂钟）。
pub fn tick_from_secs(epoch_secs: u64) -> u64 {
    epoch_secs / TICK_SECONDS
}

/// 当前社会 tick：每 5 分钟一个，从 Unix 纪元起算。
pub fn current_tick() -> u64 {
    tick_from_secs(chrono::Utc::now().timestamp().max(0) as u64)
}

/// Cron 入口（wrangler.toml: crons = ["*/5 * * * *"]）。
#[event(scheduled)]
pub async fn tick(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let started = chrono::Utc::now();
    if let Err(e) = run_tick(env).await {
        console_error!("tick failed: {}", e);
        logging::error(
            "tick_failed",
            serde_json::json!({ "error": e.to_string() }),
        );
        return;
    }
    let elapsed_ms = (chrono::Utc::now() - started).num_milliseconds();
    logging::info(
        "tick_done",
        serde_json::json!({ "elapsed_ms": elapsed_ms }),
    );
}

async fn run_tick(env: Env) -> AppResult<()> {
    let db = env.d1("DB")?;
    let kv = env.kv("KV")?;
    let tick = current_tick();

    // 杀死开关：预算超阈值时跳过非核心 tick
    if crate::storage::kv::kill_switch_on(&kv).await? {
        logging::warn("tick_skipped_kill_switch", serde_json::json!({ "tick": tick }));
        return Ok(());
    }

    // 1. 找出本 tick 需唤醒的 agent
    let candidates = crate::storage::d1::find_wakeable_agents(&db, tick).await?;
    let woken = candidates.len();

    // 2. 批量入队
    crate::event::bus::publish_agent_tasks(
        &env,
        candidates
            .into_iter()
            .map(|agent_id| AgentTask {
                agent_id,
                tick,
                reason: "tick".into(),
            })
            .collect(),
    )
    .await?;

    // 3. 清理过期订单（30 分钟）
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(chrono::Duration::minutes(
            crate::payment::provider::ORDER_TTL_MINUTES,
        ))
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let expired = crate::storage::d1::expire_stale_orders(&db, &cutoff).await?;

    // 4. 更新 tick 指标（KV）
    kv.put("last_tick", tick.to_string())?.execute().await?;

    logging::info(
        "tick_metrics",
        serde_json::json!({ "tick": tick, "woken": woken, "orders_expired": expired }),
    );
    Ok(())
}

/// Queue 入口：消费 AGENT_TASKS。
/// 注意：worker 0.8 的 Message 只提供批级 ack/retry，失败整批重试
/// （决策循环是幂等的：同一 agent 同 tick 重放结果一致）。
#[event(queue)]
pub async fn handle_agent_task(
    batch: MessageBatch<AgentTask>,
    env: Env,
    _ctx: Context,
) -> worker::Result<()> {
    let mut any_failed = false;
    for msg in batch.messages()? {
        let task = msg.body();
        if let Err(e) = crate::agent_runtime::decision::run_decision_cycle(&env, task).await {
            console_error!("agent {} decision failed: {}", task.agent_id, e);
            logging::error(
                "agent_decision_failed",
                serde_json::json!({
                    "agent_id": task.agent_id,
                    "tick": task.tick,
                    "error": e.to_string(),
                }),
            );
            any_failed = true;
        }
    }
    if any_failed {
        batch.retry_all();
    } else {
        batch.ack_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn tick_is_five_minute_epoch() {
        assert_eq!(tick_from_secs(0), 0);
        assert_eq!(tick_from_secs(299), 0);
        assert_eq!(tick_from_secs(300), 1);
        assert_eq!(tick_from_secs(301), 1);
        assert_eq!(tick_from_secs(1800), 6);
        // 2026-10-22 ≈ 1.79e9 秒
        assert_eq!(tick_from_secs(1_790_000_000), 5_966_666);
    }

    #[wasm_bindgen_test]
    fn current_tick_in_plausible_range() {
        let t = current_tick();
        // 5_900_000 tick ≈ 2026-01；上限给到 2035 年，越界说明取时源出错了
        assert!(t > 5_900_000, "tick too small: {t}");
        assert!(t < 6_900_000, "tick too large: {t}");
    }

    #[wasm_bindgen_test]
    fn agent_task_roundtrips_through_queue_payload() {
        let task = AgentTask {
            agent_id: "urn:agent:1".into(),
            tick: tick_from_secs(1_790_000_000),
            reason: "tick".into(),
        };
        let json = serde_json::to_string(&task).expect("serialize");
        let back: AgentTask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.agent_id, task.agent_id);
        assert_eq!(back.tick, task.tick);
        assert_eq!(back.reason, task.reason);
    }
}
