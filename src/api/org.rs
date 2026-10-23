//! 组织创建：POST /api/org（组织是 L2 seed 域概念；其章程前置是该域自身规则，非 L0 公理）。

use serde::Deserialize;
use worker::{wasm_bindgen::JsValue, Response, RouteContext};

use crate::api::json_body;
use crate::common::error::{AppError, AppResult};
use crate::common::types::now_rfc3339;
use crate::ontology::version::OntologyRouter;

#[derive(Debug, Deserialize)]
pub struct CreateOrgInput {
    pub name: String,
    /// 章程 ID（必须先经治理流程创建 gov:OrganizationCharter 实例）
    pub charter_id: String,
    /// 创始人账号（G4 补 G7 遗留）：带会话时以会话账号为准且必须与
    /// 此字段一致（防伪造）；无会话时按过渡模式取此字段（可空）。
    #[serde(default)]
    pub founder_account_id: Option<String>,
}

pub async fn create(mut req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let input: CreateOrgInput = json_body(&mut req).await?;
    let name = crate::governance::validator::sanitize_text(&input.name, 128);
    if name.is_empty() {
        return Err(AppError::InvalidInput("name required".into()));
    }
    if input.charter_id.is_empty() {
        // org 域自身规则：没有章程不允许建组织（仅约束本域，不上升为通用公理）
        return Err(AppError::InvalidInput(
            "charter_id required: organizations are instantiated from charters".into(),
        ));
    }

    let env = ctx.env.clone();

    // 创始人绑定（G4 补 G7 遗留）：归属校验自此有落点。
    // - 带会话：创始人 = 会话账号；入参声称的创始人不一致 → 403（不许冒名建组织）。
    // - 无会话：与 session::enforce_ownership 同一过渡模式——REQUIRE_AUTH=true
    //   时 401；否则取入参 founder_account_id（可空，落 NULL 表示创始人未知）。
    let founder_account_id: Option<String> =
        match crate::api::session::resolve_session(&req, &env).await? {
            Some(account_id) => {
                if let Some(claimed) = &input.founder_account_id {
                    if claimed != &account_id {
                        return Err(AppError::Forbidden);
                    }
                }
                Some(account_id)
            }
            None => {
                let require = env
                    .var("REQUIRE_AUTH")
                    .map(|v| v.to_string() == "true")
                    .unwrap_or(false);
                if require {
                    return Err(AppError::Unauthorized);
                }
                input.founder_account_id.clone()
            }
        };

    let db = env.d1("DB")?;
    let router = OntologyRouter::from_env(&env).await?;
    let org_id = ulid::Ulid::generate().to_string();

    db.prepare(
        "INSERT INTO organizations (id, name, charter_id, founder_account_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&[
        JsValue::from_str(&org_id),
        JsValue::from_str(&name),
        JsValue::from_str(&input.charter_id),
        founder_account_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&now_rfc3339()),
    ])?
    .run()
    .await?;

    let payload = serde_json::json!({
        "org_id": org_id,
        "name": name,
        "charter_id": input.charter_id,
        "founder_account_id": founder_account_id,
    });
    let event_id = ulid::Ulid::generate().to_string();
    crate::event::log::append(
        &env,
        "http://soc.example/ontology#OrganizationCreated",
        &event_id,
        router.for_action().as_str(),
        crate::agent_runtime::scheduler::current_tick(),
        None,
        &payload,
    )
    .await?;

    Ok(Response::from_json(&serde_json::json!({
        "org_id": org_id,
        "event_id": event_id,
        "founder_account_id": founder_account_id,
    }))?)
}
