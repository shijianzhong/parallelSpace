//! 分身查询：GET /api/agent/:id。

use worker::{Response, RouteContext};

use crate::api::require_param;
use crate::common::error::{AppError, AppResult};
use crate::storage::d1;

pub async fn get_agent(_req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let agent_id = require_param(&ctx, "id")?;
    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    match d1::get_agent(&db, &agent_id).await? {
        Some(agent) => Ok(Response::from_json(&serde_json::json!({
            "id": agent.id,
            "account_id": agent.account_id,
            "status": agent.status,
            "location_id": agent.location_id,
            "ontology_version": agent.ontology_version,
        }))?),
        None => Err(AppError::NotFound(format!("agent {agent_id}"))),
    }
}
