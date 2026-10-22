//! 全局错误类型与 HTTP 映射。
//!
//! 规范：错误用 thiserror 定义 AppError，HTTP 映射到合适状态码（AGENTS.md §4.1）。
//! 请求路径禁止 unwrap()/expect()。

use thiserror::Error;
use worker::Response;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("ontology version not found: {0}")]
    OntologyVersionNotFound(String),
    #[error("payment already confirmed")]
    PaymentAlreadyConfirmed,
    #[error("payment order expired")]
    PaymentOrderExpired,
    #[error("db error: {0}")]
    Db(String),
    #[error("ontology engine error: {0}")]
    OntologyEngine(String),
    #[error("llm error: {0}")]
    Llm(String),
    /// Workers 平台绑定/运行时错误（js-sys 侧）。对外统一 500，细节只进日志。
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<worker::Error> for AppError {
    fn from(e: worker::Error) -> Self {
        AppError::Runtime(e.to_string())
    }
}

/// KV 操作错误（workers-rs 的 KvStore 用独立错误类型）→ Db。
impl From<worker::KvError> for AppError {
    fn from(e: worker::KvError) -> Self {
        AppError::Db(format!("kv: {e}"))
    }
}

impl AppError {
    /// (HTTP 状态码, 机器可读错误码)。
    pub fn http_mapping(&self) -> (u16, &'static str) {
        match self {
            AppError::Unauthorized => (401, "UNAUTHORIZED"),
            AppError::Forbidden => (403, "FORBIDDEN"),
            AppError::NotFound(_) => (404, "NOT_FOUND"),
            AppError::InvalidInput(_) => (400, "INVALID_INPUT"),
            AppError::ValidationFailed(_) => (422, "VALIDATION_FAILED"),
            AppError::Conflict(_) => (409, "CONFLICT"),
            AppError::QuotaExceeded => (429, "QUOTA_EXCEEDED"),
            AppError::OntologyVersionNotFound(_) => (400, "ONTOLOGY_VERSION_NOT_FOUND"),
            AppError::PaymentAlreadyConfirmed => (409, "PAYMENT_ALREADY_CONFIRMED"),
            AppError::PaymentOrderExpired => (410, "PAYMENT_ORDER_EXPIRED"),
            AppError::Db(_)
            | AppError::OntologyEngine(_)
            | AppError::Llm(_)
            | AppError::Runtime(_)
            | AppError::Internal(_) => (500, "INTERNAL"),
        }
    }

    /// 转为 HTTP 响应：统一 JSON 体 `{"error": "<CODE>", "message": "..."}`。
    ///
    /// 两条约束：
    /// - 5xx 一律只回通用文案，细节（DB 语句 / LLM prompt / 引擎地址）只进日志，
    ///   否则错误响应会成为内部信息的泄露通道；
    /// - JSON 化是为了让前端和其它 API 一致解析，不必对错误走纯文本分支。
    pub fn to_response(&self) -> Response {
        let (status, code) = self.http_mapping();
        let message = if status >= 500 {
            "internal error".to_string()
        } else {
            self.to_string()
        };
        let body = serde_json::json!({ "error": code, "message": message });
        match Response::from_json(&body) {
            Ok(resp) => resp.with_status(status),
            // 兜底：JSON 都造不出来时就退回纯文本，状态码保持正确
            Err(_) => Response::error(code, status).unwrap_or_else(|_| {
                worker::ResponseBuilder::new().with_status(status).empty()
            }),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn http_mapping_covers_all_variants() {
        assert_eq!(AppError::Unauthorized.http_mapping(), (401, "UNAUTHORIZED"));
        assert_eq!(AppError::Forbidden.http_mapping(), (403, "FORBIDDEN"));
        assert_eq!(
            AppError::NotFound("x".into()).http_mapping(),
            (404, "NOT_FOUND")
        );
        assert_eq!(
            AppError::InvalidInput("x".into()).http_mapping(),
            (400, "INVALID_INPUT")
        );
        assert_eq!(
            AppError::ValidationFailed("x".into()).http_mapping(),
            (422, "VALIDATION_FAILED")
        );
        assert_eq!(AppError::Conflict("x".into()).http_mapping(), (409, "CONFLICT"));
        assert_eq!(AppError::QuotaExceeded.http_mapping(), (429, "QUOTA_EXCEEDED"));
        assert_eq!(
            AppError::OntologyVersionNotFound("v2".into()).http_mapping(),
            (400, "ONTOLOGY_VERSION_NOT_FOUND")
        );
        assert_eq!(
            AppError::PaymentAlreadyConfirmed.http_mapping(),
            (409, "PAYMENT_ALREADY_CONFIRMED")
        );
        assert_eq!(
            AppError::PaymentOrderExpired.http_mapping(),
            (410, "PAYMENT_ORDER_EXPIRED")
        );
        for e in [
            AppError::Db("x".into()),
            AppError::OntologyEngine("x".into()),
            AppError::Llm("x".into()),
            AppError::Runtime("x".into()),
            AppError::Internal("x".into()),
        ] {
            assert_eq!(e.http_mapping(), (500, "INTERNAL"), "{e}");
        }
    }

    #[wasm_bindgen_test]
    fn worker_error_converts_to_runtime_variant() {
        // worker::Error::Internal 接收 JsValue；用 JsError 构造纯字符串错误即可覆盖转换路径
        let e = AppError::from(worker::Error::JsError("boom".into()));
        assert!(matches!(e, AppError::Runtime(_)));
        assert_eq!(e.http_mapping().0, 500);
    }
}
