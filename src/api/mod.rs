//! API 层：HTTP 路由处理函数。薄层——业务在 ontology/ / governance/ / payment/ 各模块。

pub mod action;
pub mod admin;
pub mod agent;
pub mod auth;
pub mod map;
pub mod org;
pub mod oauth;
pub mod payment;
pub mod session;

use worker::{Request, Response, RouteContext};

use crate::common::error::{AppError, AppResult};

/// 统一的 JSON 错误响应。
pub fn error_response(e: AppError) -> Response {
    e.to_response()
}

/// 把 AppResult handler 适配成 Router 需要的 worker::Result（保留 HTTP 状态码）。
pub fn into_router_result(r: AppResult<Response>) -> worker::Result<Response> {
    match r {
        Ok(resp) => Ok(resp),
        Err(e) => Ok(e.to_response()),
    }
}

/// 从 RouteContext 取路径参数，缺失即 400。
pub fn require_param(ctx: &RouteContext<()>, key: &str) -> AppResult<String> {
    ctx.param(key)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("missing path param: {key}")))
}

/// 读 JSON body；解析失败转 400。
pub async fn json_body<T: serde::de::DeserializeOwned>(req: &mut Request) -> AppResult<T> {
    req.json()
        .await
        .map_err(|e| AppError::InvalidInput(format!("invalid json body: {e}")))
}
