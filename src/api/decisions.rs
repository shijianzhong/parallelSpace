//! 决策收件箱（G15，docs/GAPS_AND_PLAN.md 第十三/十四节）。
//!
//! 离线代理 Defer 的落地形态：所有者不在场时，分身的不可逆动作生成
//! PendingDecision 挂起待批（见 api::action::proxy_gate），所有者在本
//! 模块的接口里批准/驳回。红线（AGENTS.md §5.9）：
//! - 批准必须以所有者身份把存储载荷重新走正常 action 入口
//!   （validate_action + action::execute_validated_action），禁止凭载荷
//!   绕过校验直落事件；校验不过置 rejected 并回显 violations。
//! - 挂起 72 小时未决策自动驳回（expired）：读取收件箱 / 审批尝试 /
//!   调度 tick 三处惰性过期；批准已过期项返回 410。
//! - 只批/驳：不提供修改待决动作载荷的入口。

use worker::{Env, Request, Response, RouteContext};

use crate::api::action::{execute_validated_action, extract_rdf_payload};
use crate::api::require_param;
use crate::api::session::require_session;
use crate::common::error::{AppError, AppResult};
use crate::common::types::now_rfc3339;
use crate::governance::validator::{validate_action, ActionRequest};
use crate::storage::d1::{self, PendingDecisionRow};

/// 待决项挂起时限（小时）：用户 2026-10-23 拍板 72 小时自动驳回。
pub const DECISION_TTL_HOURS: i64 = 72;

/// 自动驳回的落库原因（expired 的 reject_reason）。
pub const AUTO_REJECT_REASON: &str = "超过 72 小时未决策，系统自动驳回";

/// 待决项到期时刻 = 创建时刻 + 72h（RFC3339）。
/// 解析失败时原样返回入参（不伪造时间），由 is_expired 的字符串比较兜底。
pub fn expires_at_for(created_at_rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(created_at_rfc3339) {
        Ok(t) => (t.with_timezone(&chrono::Utc) + chrono::Duration::hours(DECISION_TTL_HOURS))
            .to_rfc3339(),
        Err(_) => created_at_rfc3339.to_string(),
    }
}

/// 是否已到期。时间戳同为 RFC3339 UTC 串，字典序与时序一致（与订单过期
/// 清理 expire_stale_orders 的字符串比较同一口径）。
pub fn is_expired(expires_at: &str, now_rfc3339: &str) -> bool {
    now_rfc3339 >= expires_at
}

/// 待决项在「当前时刻」下的裁决门分类（纯函数，审批/读取路径共用）。
#[derive(Debug, PartialEq, Eq)]
pub enum DecisionGate {
    /// 仍在待决且未到期，可裁决。
    Open,
    /// 待决但已过 72h：须先置 expired，审批返回已过期错误。
    Expired,
    /// 已被裁决（含已自动驳回），重复操作返回 409。内含当前状态。
    Decided(String),
}

/// 按状态与到期时刻分类一条待决项。
pub fn classify(status: &str, expires_at: &str, now_rfc3339: &str) -> DecisionGate {
    if status != "pending" {
        return DecisionGate::Decided(status.to_string());
    }
    if is_expired(expires_at, now_rfc3339) {
        DecisionGate::Expired
    } else {
        DecisionGate::Open
    }
}

/// 把 ActionRequest 序列化为待决项载荷（完整提交体，批准重放用）。
pub fn serialize_payload(action: &ActionRequest) -> String {
    serde_json::json!({
        "agent_id": action.agent_id,
        "action_type": action.action_type,
        "target_id": action.target_id,
        "params": action.params,
    })
    .to_string()
}

/// 解析待决项载荷回 ActionRequest。载荷损坏视为内部错误——它是我们自己
/// 在挂起时写入的，读不回来说明数据被破坏，不得静默降级执行。
pub fn parse_payload(payload: &str) -> AppResult<ActionRequest> {
    serde_json::from_str(payload)
        .map_err(|e| AppError::Internal(format!("stored decision payload unreadable: {e}")))
}

/// 把一条已到期的 pending 置 expired + 审计（惰性过期的单条落点）。
/// 返回是否真的发生了迁移（条件更新未命中说明已被并发裁决）。
async fn expire_decision(env: &Env, row: &PendingDecisionRow) -> AppResult<bool> {
    let db = env.d1("DB")?;
    let changed = d1::transition_pending_decision(
        &db,
        &row.id,
        "expired",
        None,
        &now_rfc3339(),
        Some(AUTO_REJECT_REASON),
    )
    .await?;
    if changed {
        crate::governance::audit::record(
            env,
            "decision.expired",
            serde_json::json!({
                "pending_decision_id": row.id,
                "agent_id": row.agent_id,
                "account_id": row.account_id,
                "action_type": row.action_type,
                "expires_at": row.expires_at,
            }),
        )
        .await?;
    }
    Ok(changed)
}

/// 扫某账号的到期未决项并逐条过期（收件箱读取路径的惰性过期）。
async fn expire_account_decisions(env: &Env, account_id: &str) -> AppResult<usize> {
    let db = env.d1("DB")?;
    let rows =
        d1::list_expired_pending_decisions(&db, Some(account_id), &now_rfc3339(), 200).await?;
    let mut n = 0;
    for row in &rows {
        if expire_decision(env, row).await? {
            n += 1;
        }
    }
    Ok(n)
}

/// 全局过期 sweep（调度 tick 顺带执行，沿用订单过期清理的同位钩子，
/// 不新造调度器）。返回本次置 expired 的条数。
pub async fn sweep_expired_decisions(env: &Env) -> AppResult<usize> {
    let db = env.d1("DB")?;
    let rows = d1::list_expired_pending_decisions(&db, None, &now_rfc3339(), 200).await?;
    let mut n = 0;
    for row in &rows {
        if expire_decision(env, row).await? {
            n += 1;
        }
    }
    Ok(n)
}

/// 读一条待决项并核对归属：不存在 404，他人 403（列表按账号隔离，
/// 单项操作同样不许跨账号——与 G7 归属校验同一口径）。
async fn load_owned_decision(env: &Env, account_id: &str, id: &str) -> AppResult<PendingDecisionRow> {
    let db = env.d1("DB")?;
    let row = d1::get_pending_decision(&db, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("pending decision not found: {id}")))?;
    if row.account_id != account_id {
        // 他人待决项：403（错误体不带细节，与全仓 Forbidden 口径一致）。
        return Err(AppError::Forbidden);
    }
    Ok(row)
}

/// 收件箱里一条待决项的回显形态：载荷只给摘要（params 去掉 rdf 大字段，
/// 以 has_rdf 标记），完整载荷不下发前端。
fn decision_json(row: &PendingDecisionRow) -> serde_json::Value {
    let parsed = parse_payload(&row.payload).ok();
    let (target_id, params_summary, has_rdf) = match &parsed {
        Some(a) => {
            let mut p = a.params.clone();
            let has_rdf = p
                .get("rdf")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if let Some(obj) = p.as_object_mut() {
                obj.remove("rdf");
            }
            (a.target_id.clone(), p, has_rdf)
        }
        None => (None, serde_json::Value::Null, false),
    };
    serde_json::json!({
        "id": row.id,
        "agent_id": row.agent_id,
        "action_type": row.action_type,
        "target_id": target_id,
        "params": params_summary,
        "has_rdf": has_rdf,
        "status": row.status,
        "created_at": row.created_at,
        "expires_at": row.expires_at,
        "decided_at": row.decided_at,
        "decided_by": row.decided_by,
        "event_id": row.event_id,
        "reject_reason": row.reject_reason,
    })
}

/// GET /api/me/decisions：本人分身的待决与历史（含已过期）。
/// 读取前先对本账号做惰性过期，保证列表里不出现「已过期仍标 pending」。
pub async fn list_decisions(req: Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = require_session(&req, &env).await?;
    expire_account_decisions(&env, &account_id).await?;
    let db = env.d1("DB")?;
    let rows = d1::list_pending_decisions_by_account(&db, &account_id, 100).await?;
    let decisions: Vec<serde_json::Value> = rows.iter().map(decision_json).collect();
    Ok(Response::from_json(&serde_json::json!({ "decisions": decisions }))?)
}

/// POST /api/me/decisions/:id/approve：批准并立即执行存储的动作。
///
/// 流程：归属校验 → 过期判定（已过期先置 expired 并返回 410）→ 条件
/// 认领（pending 到 approved，重复/并发批准只有一个命中）→ 载荷重走
/// 正常 action 入口（validate_action + execute_validated_action）→
/// 成功回填 event_id 并写 decision.approved 审计。
///
/// 校验不过时置 rejected 终局并以 422 回显 violations。落盘段的输入类
/// 拒绝（如 Move 目的地已不存在）同校验失败处理；基础设施失败则回滚
/// 认领回 pending，批准只在事件真正落盘后才算数。
pub async fn approve_decision(req: Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = require_session(&req, &env).await?;
    let id = require_param(&ctx, "id")?;
    let db = env.d1("DB")?;
    let row = load_owned_decision(&env, &account_id, &id).await?;

    match classify(&row.status, &row.expires_at, &now_rfc3339()) {
        // 已被自动驳回的项再批准：仍回已过期错误（410），与从未裁决的
        // approved/rejected（409）区分——前者的「决定」不是人做的。
        DecisionGate::Decided(status) if status == "expired" => {
            return Err(AppError::DecisionExpired(id));
        }
        DecisionGate::Decided(status) => {
            return Err(AppError::Conflict(format!("decision already {status}")));
        }
        DecisionGate::Expired => {
            expire_decision(&env, &row).await?;
            return Err(AppError::DecisionExpired(id));
        }
        DecisionGate::Open => {}
    }

    // 认领前先解析载荷：载荷损坏直接报错、状态不动（不留半裁决项）。
    let action = parse_payload(&row.payload)?;
    if action.agent_id != row.agent_id {
        return Err(AppError::Internal(
            "stored decision payload agent mismatch".into(),
        ));
    }

    let claimed = d1::transition_pending_decision(
        &db,
        &id,
        "approved",
        Some(&account_id),
        &now_rfc3339(),
        None,
    )
    .await?;
    if !claimed {
        let status = d1::get_pending_decision(&db, &id)
            .await?
            .map(|r| r.status)
            .unwrap_or_else(|| "missing".to_string());
        return Err(AppError::Conflict(format!("decision already {status}")));
    }

    // 以所有者身份重走正常 action 入口：RDF 提取 → SHACL/L3 校验 → 落盘。
    let rdf_payload = extract_rdf_payload(&action);
    let result = validate_action(&env, &action, &rdf_payload).await?;
    if !result.ok {
        d1::reject_claimed_decision(&db, &id, "validation_failed_on_approve").await?;
        crate::governance::audit::record(
            &env,
            "decision.rejected",
            serde_json::json!({
                "pending_decision_id": id,
                "agent_id": row.agent_id,
                "account_id": account_id,
                "reason": "validation_failed_on_approve",
                "violations": result.violations,
            }),
        )
        .await?;
        let resp = Response::from_json(&serde_json::json!({
            "decision_id": id,
            "status": "rejected",
            "violations": result.violations,
        }))?;
        return Ok(resp.with_status(422));
    }

    match execute_validated_action(&env, &action, &rdf_payload, result.inferred.len()).await {
        Ok(body) => {
            let event_id = body
                .get("event_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            d1::set_pending_decision_event(&db, &id, &event_id).await?;
            crate::governance::audit::record(
                &env,
                "decision.approved",
                serde_json::json!({
                    "pending_decision_id": id,
                    "agent_id": row.agent_id,
                    "account_id": account_id,
                    "event_id": event_id,
                }),
            )
            .await?;
            Ok(Response::from_json(&serde_json::json!({
                "decision_id": id,
                "status": "approved",
                "event_id": event_id,
            }))?)
        }
        Err(e) if matches!(e, AppError::InvalidInput(_)) => {
            // 落盘段的输入类拒绝（如 Move 目的地在挂起期间被删）与校验失败
            // 同级：置 rejected 终局，避免所有者反复批一个永远落不了的动作。
            let reason = format!("execution_rejected: {e}");
            d1::reject_claimed_decision(&db, &id, &reason).await?;
            crate::governance::audit::record(
                &env,
                "decision.rejected",
                serde_json::json!({
                    "pending_decision_id": id,
                    "agent_id": row.agent_id,
                    "account_id": account_id,
                    "reason": "execution_rejected",
                    "detail": e.to_string(),
                }),
            )
            .await?;
            Err(e)
        }
        Err(e) => {
            // 基础设施失败：回滚认领回 pending，所有者可重试。
            let _ = d1::revert_decision_claim(&db, &id).await;
            Err(e)
        }
    }
}

/// POST /api/me/decisions/:id/reject：驳回待决项（不执行、只留痕）。
/// 已过期项（含已被自动驳回的）返回 410；已人工裁决项返回 409。
pub async fn reject_decision(req: Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let env = ctx.env.clone();
    let account_id = require_session(&req, &env).await?;
    let id = require_param(&ctx, "id")?;
    let db = env.d1("DB")?;
    let row = load_owned_decision(&env, &account_id, &id).await?;

    match classify(&row.status, &row.expires_at, &now_rfc3339()) {
        DecisionGate::Decided(status) if status == "expired" => {
            return Err(AppError::DecisionExpired(id));
        }
        DecisionGate::Decided(status) => {
            return Err(AppError::Conflict(format!("decision already {status}")));
        }
        DecisionGate::Expired => {
            expire_decision(&env, &row).await?;
            return Err(AppError::DecisionExpired(id));
        }
        DecisionGate::Open => {}
    }

    let changed = d1::transition_pending_decision(
        &db,
        &id,
        "rejected",
        Some(&account_id),
        &now_rfc3339(),
        Some("所有者驳回"),
    )
    .await?;
    if !changed {
        let status = d1::get_pending_decision(&db, &id)
            .await?
            .map(|r| r.status)
            .unwrap_or_else(|| "missing".to_string());
        return Err(AppError::Conflict(format!("decision already {status}")));
    }
    crate::governance::audit::record(
        &env,
        "decision.rejected",
        serde_json::json!({
            "pending_decision_id": id,
            "agent_id": row.agent_id,
            "account_id": account_id,
            "reason": "owner_rejected",
        }),
    )
    .await?;
    Ok(Response::from_json(&serde_json::json!({
        "decision_id": id,
        "status": "rejected",
    }))?)
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;
    use crate::governance::validator::ActionType;

    #[wasm_bindgen_test]
    fn expires_at_is_created_plus_72h() {
        assert_eq!(
            expires_at_for("2026-10-23T08:00:00+00:00"),
            "2026-10-26T08:00:00+00:00"
        );
        // 跨月同样成立
        assert_eq!(
            expires_at_for("2026-10-30T23:30:00+00:00"),
            "2026-11-02T23:30:00+00:00"
        );
        // 解析不了的输入原样返回，不伪造时间
        assert_eq!(expires_at_for("not-a-time"), "not-a-time");
    }

    #[wasm_bindgen_test]
    fn expiry_boundary_is_inclusive() {
        let exp = "2026-10-26T08:00:00+00:00";
        assert!(!is_expired(exp, "2026-10-26T07:59:59+00:00"));
        // 到点即过期（now == expires_at 视为已自动驳回）
        assert!(is_expired(exp, exp));
        assert!(is_expired(exp, "2026-10-26T08:00:01+00:00"));
    }

    #[wasm_bindgen_test]
    fn classify_covers_open_expired_decided() {
        let now = "2026-10-24T08:00:00+00:00";
        assert_eq!(
            classify("pending", "2026-10-26T08:00:00+00:00", now),
            DecisionGate::Open
        );
        assert_eq!(
            classify("pending", "2026-10-24T07:00:00+00:00", now),
            DecisionGate::Expired
        );
        // 已裁决项不再看时间：状态优先
        assert_eq!(
            classify("approved", "2026-10-20T08:00:00+00:00", now),
            DecisionGate::Decided("approved".to_string())
        );
        assert_eq!(
            classify("expired", "2026-10-20T08:00:00+00:00", now),
            DecisionGate::Decided("expired".to_string())
        );
    }

    fn trade_action() -> ActionRequest {
        ActionRequest {
            agent_id: "agent-1".into(),
            action_type: ActionType::Trade,
            target_id: Some("agent-2".into()),
            params: serde_json::json!({ "amount": 10, "rdf": "<urn:a> a <urn:B> ." }),
        }
    }

    #[wasm_bindgen_test]
    fn payload_roundtrip_preserves_full_submission() {
        // 批准重放依赖载荷无损往返：agent / 类型 / target / params（含 rdf）
        let a = trade_action();
        let stored = serialize_payload(&a);
        // action_type 必须是 snake_case 串，与 ActionRequest 的 Deserialize 对齐
        assert!(stored.contains("\"action_type\":\"trade\""), "{stored}");
        let back = parse_payload(&stored).expect("payload must parse");
        assert_eq!(back.agent_id, a.agent_id);
        assert_eq!(back.action_type, a.action_type);
        assert_eq!(back.target_id, a.target_id);
        assert_eq!(back.params, a.params);
    }

    #[wasm_bindgen_test]
    fn payload_without_target_roundtrips_as_none() {
        let mut a = trade_action();
        a.target_id = None;
        let back = parse_payload(&serialize_payload(&a)).expect("payload must parse");
        assert_eq!(back.target_id, None);
    }

    #[wasm_bindgen_test]
    fn corrupt_payload_is_an_error_not_a_panic() {
        assert!(parse_payload("{not json").is_err());
        assert!(parse_payload("{\"agent_id\":1}").is_err());
    }
}
