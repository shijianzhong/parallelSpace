//! 事件总线：Queues 生产端（AGENTS.md §5.5）。
//!
//! 批量入队，每批最多 100 条（Queues 单批上限）；失败由调用方决定重试策略。

use worker::{Env, Queue};

use crate::common::error::AppResult;
use crate::agent_runtime::scheduler::AgentTask;

const BATCH_SIZE: usize = 100;

/// 把 Agent 任务批量入队 AGENT_TASKS。
pub async fn publish_agent_tasks(env: &Env, tasks: Vec<AgentTask>) -> AppResult<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let queue: Queue = env.queue("AGENT_TASKS")?;
    for chunk in tasks.chunks(BATCH_SIZE) {
        queue.send_batch(chunk.to_vec()).await?;
    }
    Ok(())
}
