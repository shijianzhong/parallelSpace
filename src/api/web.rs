//! 前端入口（试运行 MVP）：零构建原生 HTML/CSS/JS 单页应用。
//!
//! 路线选择：静态文件放在仓库根 `web/` 目录，编译期 `include_str!` 内嵌
//! 进 Worker，由本模块路由直接返回。没有选 wrangler `[assets]` 绑定：
//! 当前构建链以 worker-build 产出的 `build/worker/shim.mjs` 为入口，
//! assets 绑定需额外的 run_worker_first 配置且本地/线上行为需另行验证；
//! 内嵌路线零配置、wrangler dev 与线上一致、smoke 可直接断言。
//! 代价：改前端需重新构建 Worker（试运行阶段可接受）。
//!
//! 页面本身不做服务端鉴权（静态壳公开），受保护数据全部经 /api/* 的
//! 会话校验：前端启动先调 /api/me，401 即跳 /login。世界层（地图 /
//! 时间线背后的数据接口）均需会话或既有校验，不对访客开放数据。
//! 地图不用 MapLibre（AGENTS.md §3：M2 规划），前端用 SVG 直接画
//! /api/map 返回的 GeoJSON Point/Polygon 坐标。

use worker::{Response, RouteContext};

use crate::common::error::AppResult;

const INDEX_HTML: &str = include_str!("../../web/index.html");
const APP_CSS: &str = include_str!("../../web/app.css");
const APP_JS: &str = include_str!("../../web/app.js");

fn serve(content: &'static str, content_type: &str) -> AppResult<Response> {
    let mut resp = Response::ok(content)?;
    resp.headers_mut().set("content-type", content_type)?;
    resp.headers_mut().set("cache-control", "no-cache")?;
    Ok(resp)
}

/// 应用壳：/、/login、/create、/home、/timeline、/funds、/map 共用同一
/// HTML，前端 JS 按 location.pathname 选择视图。
pub async fn index(_req: worker::Request, _ctx: RouteContext<()>) -> AppResult<Response> {
    serve(INDEX_HTML, "text/html; charset=utf-8")
}

/// 样式表：GET /app.css。
pub async fn css(_req: worker::Request, _ctx: RouteContext<()>) -> AppResult<Response> {
    serve(APP_CSS, "text/css; charset=utf-8")
}

/// 前端脚本：GET /app.js。
pub async fn js(_req: worker::Request, _ctx: RouteContext<()>) -> AppResult<Response> {
    serve(APP_JS, "text/javascript; charset=utf-8")
}
