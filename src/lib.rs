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
        .get_async("/api/agent/:id", |req, ctx| async move {
            api::into_router_result(api::agent::get_agent(req, ctx).await)
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
        // 支付：下单（服务端定价）+ 治理员手动确认
        .post_async("/api/payment/order", |req, ctx| async move {
            api::into_router_result(api::payment::create_order(req, ctx).await)
        })
        .post_async("/api/payment/confirm", |req, ctx| async move {
            api::into_router_result(api::payment::confirm(req, ctx).await)
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
        });

    router.run(req, env).await
}
