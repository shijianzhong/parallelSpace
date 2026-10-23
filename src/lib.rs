//! parallelSpace —— OWL 本体驱动的多智能体社会模拟（Cloudflare Workers + workers-rs）。
//!
//! 模块划分遵循 AGENTS.md §3（编码提示词第三节），新增模块需先改 AGENTS.md。
//!
//! 事件入口：
//! - `fetch`：HTTP API（Router 分发到 api::*）
//! - `tick`：Cron 每 5 分钟一个社会 tick（agent_runtime/scheduler.rs）
//! - `handle_agent_task`：AGENT_TASKS 队列消费（agent_runtime/scheduler.rs）
//!
//! 本体版本由 OntologyRouter 统一注入，任何handler 不得自行决定事件版本。

use worker::{event, Context, Env, Request, Response, Result, Router};

pub mod agent_runtime;
pub mod api;
pub mod common;
pub mod event;
pub mod governance;
pub mod llm;
pub mod ontology;
pub mod payment;
pub mod storage;

/// HTTP 入口。
#[event(fetch)]
pub async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let router = Router::new()
        // 运维
        .get_async("/health", |_, _| async move {
            Response::ok(concat!("parallelspace/", env!("CARGO_PKG_VERSION")))
        })
        // 账号与分身
        .post_async("/api/auth/register", |req, ctx| async move {
            api::into_router_result(api::auth::register(req, ctx).await)
        })
        .post_async("/api/auth/email/start", |req, ctx| async move {
            api::into_router_result(api::auth::email_start(req, ctx).await)
        })
        .post_async("/api/auth/email/verify", |req, ctx| async move {
            api::into_router_result(api::auth::email_verify(req, ctx).await)
        })
        .get_async("/api/auth/oauth/:provider/start", |req, ctx| async move {
            api::into_router_result(api::oauth::start(req, ctx).await)
        })
        .get_async("/api/auth/oauth/:provider/callback", |req, ctx| async move {
            api::into_router_result(api::oauth::callback(req, ctx).await)
        })
        .post_async("/api/agent", |req, ctx| async move {
            api::into_router_result(api::auth::create_agent(req, ctx).await)
        })
        .get_async("/api/me", |req, ctx| async move {
            api::into_router_result(api::auth::me(req, ctx).await)
        })
        .post_async("/api/me/transfer", |req, ctx| async move {
            api::into_router_result(api::payment::me_transfer(req, ctx).await)
        })
        .post_async("/api/me/redeem", |req, ctx| async move {
            api::into_router_result(api::payment::me_redeem(req, ctx).await)
        })
        .get_async("/api/agent/:id", |req, ctx| async move {
            api::into_router_result(api::agent::get_agent(req, ctx).await)
        })
        .post_async("/api/agent/:id/activate", |req, ctx| async move {
            api::into_router_result(api::agent::activate(req, ctx).await)
        })
        // 行动
        .post_async("/api/action", |req, ctx| async move {
            api::into_router_result(api::action::submit(req, ctx).await)
        })
        // 组织
        .post_async("/api/org", |req, ctx| async move {
            api::into_router_result(api::org::create(req, ctx).await)
        })
        // 地图图层
        .get_async("/api/map/agents", |req, ctx| async move {
            api::into_router_result(api::map::agents_layer(req, ctx).await)
        })
        .get_async("/api/map/locations", |req, ctx| async move {
            api::into_router_result(api::map::locations_layer(req, ctx).await)
        })
        // WebSocket 世界事件订阅（G8）：转发到 DO 广播 hub（单例），
        // 升级与 hibernation 语义在 storage::do_state 内处理。
        .get_async("/api/ws", |req, ctx| async move {
            let ns = ctx.env.durable_object("DO_STATE")?;
            let stub = ns
                .id_from_name(crate::storage::do_state::WORLD_HUB_NAME)?
                .get_stub()?;
            stub.fetch_with_request(req).await
        })
        // 支付：下单（服务端定价）+ 治理员手动确认
        .post_async("/api/payment/order", |req, ctx| async move {
            api::into_router_result(api::payment::create_order(req, ctx).await)
        })
        .post_async("/api/payment/confirm", |req, ctx| async move {
            api::into_router_result(api::payment::confirm(req, ctx).await)
        })
        .post_async("/api/payment/transfer", |req, ctx| async move {
            api::into_router_result(api::payment::transfer(req, ctx).await)
        })
        .post_async("/api/admin/payment/agent-earnings", |req, ctx| async move {
            api::into_router_result(api::payment::agent_earnings(req, ctx).await)
        })
        .post_async("/api/admin/payment/redeem", |req, ctx| async move {
            api::into_router_result(api::payment::redeem_request(req, ctx).await)
        })
        .post_async("/api/admin/payment/redeem/:id/settle", |req, ctx| async move {
            api::into_router_result(api::payment::redeem_settle(req, ctx).await)
        })
        .post_async("/api/admin/payment/redeem/:id/reject", |req, ctx| async move {
            api::into_router_result(api::payment::redeem_reject(req, ctx).await)
        })
        // 治理管理：域注册表 + 两段式发布（提案 pending → 审批 active）+ 回滚
        .post_async("/api/admin/ontology/publish", |req, ctx| async move {
            api::into_router_result(api::admin::publish_ontology(req, ctx).await)
        })
        .post_async("/api/admin/ontology/rollback", |req, ctx| async move {
            api::into_router_result(api::admin::rollback_ontology(req, ctx).await)
        })
        .post_async("/api/admin/ontology/domains", |req, ctx| async move {
            api::into_router_result(api::admin::register_domain(req, ctx).await)
        })
        .get_async("/api/admin/ontology/domains", |req, ctx| async move {
            api::into_router_result(api::admin::list_domains(req, ctx).await)
        })
        .get_async("/api/admin/ontology/versions", |req, ctx| async move {
            api::into_router_result(api::admin::list_versions(req, ctx).await)
        })
        .post_async("/api/admin/ontology/versions/:version/approve", |req, ctx| async move {
            api::into_router_result(api::admin::approve_ontology(req, ctx).await)
        })
        // G4 接线：提案影响分析 / 沙箱 / 仲裁立案 / 事件回放（均为 admin 通道）
        .get_async("/api/admin/ontology/versions/:version/impact", |req, ctx| async move {
            api::into_router_result(api::admin::analyze_impact(req, ctx).await)
        })
        .post_async("/api/admin/sandbox/run", |req, ctx| async move {
            api::into_router_result(api::admin::run_sandbox(req, ctx).await)
        })
        .post_async("/api/admin/arbitration/cases", |req, ctx| async move {
            api::into_router_result(api::admin::file_arbitration_case(req, ctx).await)
        })
        .get_async("/api/admin/replay", |req, ctx| async move {
            api::into_router_result(api::admin::replay_events(req, ctx).await)
        })
        // G8 位置数据模型：地点创建（治理通道，系统不预置地点数据）
        .post_async("/api/admin/locations", |req, ctx| async move {
            api::into_router_result(api::admin::create_location(req, ctx).await)
        });

    router.run(req, env).await
}
