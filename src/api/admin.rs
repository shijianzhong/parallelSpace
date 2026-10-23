//! 治理管理接口：域注册 / 本体版本发布（提案）/ 审批 / 回滚（AGENTS.md §7.8）。
//!
//! admin 校验 + 审计；L0 变更显式拒绝；发布不覆盖旧版本目录。
//! 两段式发布：publish 落 pending（提案），approve 才生效——未审批的版本
//! 不写 KV、不进 history，OntologyRouter 路由不到。域注册表替代原硬编码
//! 文件名白名单：新域（如 transport）注册为 L1/L2 种子后即可提案发布。

use serde::Deserialize;
use worker::{Response, Result, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::governance::ontology_admin;
use crate::storage::d1;

#[derive(Debug, Deserialize)]
pub struct PublishInput {
    pub version: String,
    /// 本次变更涉及的概念 IRI（L0 名单命中即拒绝）
    pub changed_concepts: Vec<String>,
    pub prior_version: String,
    /// 提案者（治理员，或未来的 LLM/agent 提案执行者）；审批前版本不生效
    pub proposed_by: String,
    /// turtle 文件内容；文件名必须是已注册域的 {domain}.ttl
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

#[derive(Debug, Deserialize)]
pub struct RegisterDomainInput {
    pub domain: String,
    /// 'L1' | 'L2'；'L0' 一律拒绝（宪法不可注册）
    pub layer: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApproveInput {
    pub approved_by: String,
}

#[derive(Debug, Deserialize)]
pub struct SandboxRunInput {
    pub proposal_id: String,
    pub ticks: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateLocationInput {
    /// 地点 id（core:Location 的可指称名）；省略时生成 ULID。
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    /// GeoJSON Polygon 外环 `[[lon,lat],...]`；省略则只有代表点。
    #[serde(default)]
    pub polygon: Option<serde_json::Value>,
}

pub(crate) fn check_admin(req: &worker::Request, ctx: &RouteContext<()>) -> Result<()> {
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

/// 第一段·提案：落 pending，不生效。
pub async fn publish_ontology(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: PublishInput = json_body(&mut req).await?;

    // L0 宪法层变更显式拒绝
    if ontology_admin::touches_l0(&input.changed_concepts) {
        return Err(AppError::Forbidden);
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
        &input.proposed_by,
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "proposed": input.version,
        "status": "pending",
        "prior_version": input.prior_version,
    }))?)
}

/// 第二段·审批：pending → active，写 KV ontology:version，激活 seed 域。
pub async fn approve_ontology(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let version = crate::api::require_param(&ctx, "version")?;
    let input: ApproveInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    ontology_admin::approve_version(&env, &version, &input.approved_by).await?;
    Ok(Response::from_json(&serde_json::json!({
        "approved": version,
        "status": "active",
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

/// 注册新本体域（L1/L2 种子）。L0 与保留名一律拒绝。
pub async fn register_domain(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: RegisterDomainInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    ontology_admin::register_domain(
        &env,
        &input.domain,
        &input.layer,
        input.description.as_deref(),
        "admin",
    )
    .await?;
    Ok(Response::from_json(&serde_json::json!({
        "registered": input.domain,
        "layer": input.layer,
        "status": "seed",
    }))?)
}

/// 域注册表一览（含种子/激活/退役状态）。
pub async fn list_domains(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let db = ctx.env.d1("DB")?;
    let rows = d1::list_domains(&db).await?;
    Ok(Response::from_json(&rows)?)
}

/// 本体版本一览（pending/active/superseded/rolled_back）。
pub async fn list_versions(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let db = ctx.env.d1("DB")?;
    let rows = d1::list_versions(&db).await?;
    Ok(Response::from_json(&rows)?)
}

/// 提案影响分析（G4 接线）：临时装载提案本体 + 按类近似统计受影响实例数。
/// 近似口径见 governance::ontology_admin::analyze_impact 的注释与报告 notes。
pub async fn analyze_impact(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let version = crate::api::require_param(&ctx, "version")?;
    let env = ctx.env.clone();
    let report = ontology_admin::analyze_impact(&env, &version).await?;
    Ok(Response::from_json(&report)?)
}

/// 跑本体沙箱（G4 接线、G9 真沙箱）：proposal_id 为已发布的本体版本，
/// governance::sandbox 内真装载提案 + 一致性 + SHACL 全量形状 + 世界
/// 快照 N tick 规则仿真后如实判定（passed 不再无条件）。
pub async fn run_sandbox(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: SandboxRunInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let result = crate::governance::sandbox::run_sandbox(&env, &input.proposal_id, input.ticks).await?;
    Ok(Response::from_json(&result)?)
}

/// 仲裁立案（G4 接线）：案件落事件日志，裁决流程在 M2。
pub async fn file_arbitration_case(
    mut req: worker::Request,
    ctx: RouteContext<()>,
) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: crate::governance::arbitration::ArbitrationCaseInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let case_id = crate::governance::arbitration::file_case(&env, &input).await?;
    Ok(Response::from_json(&serde_json::json!({
        "case_id": case_id,
        "status": "filed",
    }))?)
}

/// 创建地点（G8 位置数据模型）：地点是可指称位置（core:Location），
/// 坐标只是渲染投影、不进本体公理（AGENTS.md §8.5）。地点只能经治理
/// 通道创建——系统不预置任何地点数据，地图在无地点时返回空集。
/// id 已存在 → 409；坐标出 WGS84 范围或 polygon 不合法 → 400。
pub async fn create_location(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let input: CreateLocationInput = json_body(&mut req).await?;
    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    let name = crate::governance::validator::sanitize_text(&input.name, 128);
    if name.is_empty() {
        return Err(AppError::InvalidInput("location name required".into()));
    }
    crate::api::map::validate_lon_lat(input.lon, input.lat)?;
    let polygon_json = match &input.polygon {
        Some(raw) => {
            let ring = crate::api::map::normalize_polygon(raw)?;
            Some(serde_json::to_string(&ring).map_err(|e| AppError::Internal(e.to_string()))?)
        }
        None => None,
    };
    let id = input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ulid::Ulid::generate().to_string());
    if d1::get_location(&db, &id).await?.is_some() {
        return Err(AppError::Conflict(format!("location {id} already exists")));
    }
    d1::create_location(&db, &id, &name, input.lon, input.lat, polygon_json.as_deref()).await?;
    crate::governance::audit::record(
        &env,
        "location.created",
        serde_json::json!({ "location_id": id, "name": name }),
    )
    .await?;

    let loc = d1::get_location(&db, &id)
        .await?
        .ok_or_else(|| AppError::Internal("location vanished after insert".into()))?;
    Ok(Response::from_json(&serde_json::json!({
        "location_id": loc.id,
        "name": loc.name,
        "geometry": crate::api::map::location_geometry(&loc),
    }))?)
}

/// 事件回放（G4 接线）：?from_tick=&to_tick=，报告含版本不一致计数
/// （硬禁：用当前本体解释旧事件却不报语义差异，见 event::replay）。
pub async fn replay_events(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    check_admin(&req, &ctx).map_err(|_| AppError::Unauthorized)?;
    let url = req.url()?;
    let params: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();
    let parse_tick = |key: &str| -> AppResult<u64> {
        params
            .get(key)
            .ok_or_else(|| AppError::InvalidInput(format!("missing query param: {key}")))?
            .parse::<u64>()
            .map_err(|_| AppError::InvalidInput(format!("query param {key} must be u64")))
    };
    let from_tick = parse_tick("from_tick")?;
    let to_tick = parse_tick("to_tick")?;
    let env = ctx.env.clone();
    let report = crate::event::replay(&env, from_tick, to_tick).await?;
    Ok(Response::from_json(&report)?)
}
