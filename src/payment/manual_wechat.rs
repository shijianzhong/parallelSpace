//! 手动微信支付通道：返回固定收款码，等待治理员在后台确认。
//!
//! 幂等入账统一走 payment::credit::confirm_payment；本模块只负责造意图。

use crate::common::error::AppResult;
use crate::payment::provider::{CallbackData, Order, ORDER_TTL_MINUTES, PaymentIntent, PaymentProvider};

pub struct ManualWechatProvider {
    /// 治理员收款码图片 URL（固定一个，账号维度对账由治理台账负责）。
    pub qr_code_url: String,
}

impl ManualWechatProvider {
    pub fn new(qr_code_url: impl Into<String>) -> Self {
        Self {
            qr_code_url: qr_code_url.into(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl PaymentProvider for ManualWechatProvider {
    async fn create_order(&self, order: &Order) -> AppResult<PaymentIntent> {
        let expires_at = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::minutes(ORDER_TTL_MINUTES))
            .map(|t| t.to_rfc3339())
            .ok_or_else(|| {
                crate::common::error::AppError::Internal("order expiry overflow".into())
            })?;
        Ok(PaymentIntent {
            qr_code_url: self.qr_code_url.clone(),
            expires_at,
            order_id: order.id.clone(),
        })
    }

    async fn verify_callback(&self, _payload: &[u8], _signature: &str) -> AppResult<CallbackData> {
        // 手动通道没有自动回调：确认只能由治理员在 admin 接口触发
        Err(crate::common::error::AppError::Forbidden)
    }
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    fn order() -> Order {
        Order {
            id: "o1".into(),
            account_id: "a1".into(),
            amount_cents: 1000,
            credits: 100,
            status: "pending".into(),
            payment_channel: "manual_wechat".into(),
            channel_txn_id: None,
        }
    }

    #[wasm_bindgen_test]
    async fn create_order_returns_qr_and_expiry() {
        let p = ManualWechatProvider::new("https://example.com/qr.png");
        let intent = p.create_order(&order()).await.expect("intent");
        assert_eq!(intent.qr_code_url, "https://example.com/qr.png");
        assert_eq!(intent.order_id, "o1");
        assert!(chrono::DateTime::parse_from_rfc3339(&intent.expires_at).is_ok());
    }

    #[wasm_bindgen_test]
    async fn verify_callback_always_forbidden() {
        let p = ManualWechatProvider::new("https://example.com/qr.png");
        assert!(p.verify_callback(b"{}", "sig").await.is_err());
    }
}
