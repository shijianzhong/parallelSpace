//! 治理管理接口：本体版本发布 / 回滚（AGENTS.md §7.8）。
//!
//! admin 校验 + 审计；L0 变更显式拒绝；发布不覆盖旧版本目录。

use serde::Deserialize;
use worker::{Response, Result, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::governance::ontology_admin;

#[derive(Debug, Deserialize)]
pub struct PublishInput {
    pub version: String,
    /// 本次变更涉及的概念 IRI（L0 名单命中即拒绝）
    pub changed_concepts: Vec<String>,
    pub prior_version: String,
    /// turtle 文件内容（MVP 只收 soc/gov/econ 三个；文件名固定，防目录穿越）
    pub turtle_files: Vec<TurtleFile>,
}

#[derive(Debug, Deserialize)]
pub struct TurtleFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct RollbackInput {
    pub target_version: String,
    pub reason: String,
}

/// 文件名白名单：只允许已知本体文件名，防路径穿越写坏 R2。
fn check_file_name(name: &str) -> bool {
    matches!(
        name,
        "soc.ttl" | "gov.ttl" | "econ.ttl" | "geo.ttl" | "time.ttl"
    )
}

fn check_admin(req: &worker::Request, ctx: &RouteContext<()>) -> Result<()> {
    let expected = ctx
        .env
        .secret("ADMIN_TOKEN")
        .map(|s| s.to_string())
        .unwrap_or_default();
    if expected.is_empty() {
        return Err(worker::Error::RustError("admin token not configured".into()));
    }
    let provided = req
        .headers()
        .get("x-admin-token")
        .ok()
        .flatten()
        .unwrap_or_default();
    if provided != expected {
        return Err(worker::Error::RustError("bad admin token".into()));
    }
    Ok(())
}

pub async fn publish_ontology(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: PublishInput = json_body(&mut req).await?;

    // L0 宪法层变更显式拒绝
    if ontology_admin::touches_l0(&input.changed_concepts) {
        return Err(AppError::Forbidden);
    }
    for f in &input.turtle_files {
        if !check_file_name(&f.name) {
            return Err(AppError::InvalidInput(format!(
                "unknown ontology file: {}",
                f.name
            )));
        }
    }

    let env = ctx.env.clone();
    let files: Vec<(String, Vec<u8>)> = input
        .turtle_files
        .into_iter()
        .map(|f| (f.name, f.content.into_bytes()))
        .collect();
    ontology_admin::publish_version(
        &env,
        &input.version,
        files,
        &input.prior_version,
        "admin",
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "published": input.version,
        "prior_version": input.prior_version,
    }))?)
}

pub async fn rollback_ontology(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: RollbackInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    ontology_admin::rollback(&env, &input.target_version, &input.reason, "admin").await?;
    Ok(Response::from_json(&serde_json::json!({
        "rolled_back_to": input.target_version,
    }))?)
}
