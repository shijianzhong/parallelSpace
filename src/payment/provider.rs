//! 支付提供方抽象（AGENTS.md §5.6）。
//!
//! 短期手动（ManualWechatProvider）+ 长期自动回调共用同一幂等入账管道
//! payment::credit::confirm_payment，禁止绕过它直接改积分。

use serde::{Deserialize, Serialize};

use crate::common::error::AppResult;

/// 支付订单（与 D1 orders 行对应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub account_id: String,
    pub amount_cents: i64,
    pub credits: i64,
    pub status: String,
    pub payment_channel: String,
    pub channel_txn_id: Option<String>,
}

/// 下单结果：收款码 + 过期时间。
#[derive(Debug, Clone)]
pub struct PaymentIntent {
    pub qr_code_url: String,
    pub expires_at: String,
    pub order_id: String,
}

/// 渠道回调数据（自动通道用；手动通道没有自动回调）。
#[derive(Debug, Clone)]
pub struct CallbackData {
    pub order_id: String,
    pub channel_txn_id: String,
    pub paid_amount_cents: i64,
}

/// 订单有效期（分钟）。到期由 Cron 清理（AGENTS.md §7.7）。
pub const ORDER_TTL_MINUTES: i64 = 30;

#[async_trait::async_trait(?Send)]
pub trait PaymentProvider {
    /// 创建渠道侧支付意图。
    async fn create_order(&self, order: &Order) -> AppResult<PaymentIntent>;
    /// 验签渠道回调。手动通道恒失败（没有自动回调）。
    async fn verify_callback(&self, payload: &[u8], signature: &str) -> AppResult<CallbackData>;
}
