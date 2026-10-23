//! Durable Objects：Agent 状态协调 / WebSocket 世界事件广播（SAD §10.1，G8 接通）。
//!
//! 两种用法共用一个类：
//! - 按 agent_id 命名的 stub：单分身状态读写（GET/POST /state），天然分片；
//! - 名为 [`WORLD_HUB_NAME`] 的单例 hub：WebSocket 订阅与广播。客户端经
//!   lib.rs 的 `GET /api/ws` 转发连入；事件落 append-only 日志后由
//!   `event::log::append` 调 [`notify_world`] 把事件信封 POST 到 hub 的
//!   /broadcast，hub 再推给全部在线连接。
//!
//! 约束：事件的唯一事实源是 append-only 事件流（R2 + D1 索引）；WS 只是
//! 分发通道——广播失败只记日志、绝不回滚或阻塞事件落盘；无连接时广播
//! 是 no-op。连接走 WebSocket hibernation（accept_web_socket），DO 被回收
//! 后连接保留、消息到达时再唤醒。
//! 已知边界：hub 是单 DO 实例，试运行量级足够；连接不做鉴权（与地图
//! 图层同为公开只读面），订阅者只能收、不能借此写世界。

use serde_json::json;
use worker::{
    durable_object, wasm_bindgen, DurableObject, Env, Method, Request, RequestInit, Response,
    Result, State, WebSocket, WebSocketIncomingMessage, WebSocketPair,
};

use crate::common::logging;

/// 世界事件广播 hub 的 DO 名称（单例）。
pub const WORLD_HUB_NAME: &str = "world";

/// hub 内部广播端点（仅 DO stub 内部调用，不对外路由）。
const BROADCAST_PATH: &str = "/broadcast";

/// 判断请求路径是否是 WebSocket 升级（外部经 /api/ws 转发进来，
/// 路径保持原样；DO 内部直连时是 /ws）。
pub fn is_ws_path(path: &str) -> bool {
    path.ends_with("/ws")
}

/// 把一条已落盘的事件记录广播给 hub 的在线订阅者（best-effort）。
///
/// 在事件写入 R2 + D1 索引之后调用；DO 绑定缺失、hub 不可达、请求失败
/// 一律只记 warn 日志后返回，绝不让广播影响事件主流程。
pub async fn notify_world(env: &Env, event_record_json: &str) {
    let envelope = json!({ "kind": "event", "event": serde_json::from_str::<serde_json::Value>(event_record_json).unwrap_or(serde_json::Value::Null) }).to_string();
    let outcome = async {
        let ns = env.durable_object("DO_STATE")?;
        let stub = ns.id_from_name(WORLD_HUB_NAME)?.get_stub()?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        init.with_body(Some(wasm_bindgen::JsValue::from_str(&envelope)));
        let req = Request::new_with_init("http://do.internal/broadcast", &init)?;
        stub.fetch_with_request(req).await
    }
    .await;
    if let Err(e) = outcome {
        logging::warn(
            "ws_broadcast_failed",
            json!({ "error": e.to_string() }),
        );
    }
}

#[durable_object]
pub struct AgentStateDO {
    state: State,
}

impl DurableObject for AgentStateDO {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    /// GET /state 读状态；POST /state 写状态；
    /// GET */ws 升级 WebSocket（hibernation）；POST /broadcast 向全部
    /// 在线连接广播请求体（仅 hub 实例有连接，per-agent 实例上是 no-op）。
    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let path = req.path();
        match (req.method(), path.as_str()) {
            (Method::Get, "/state") => match self.state.storage().get::<String>("state").await? {
                Some(s) => Response::ok(s),
                None => Response::ok("{}"),
            },
            (Method::Post, "/state") => {
                let body = req.text().await?;
                self.state.storage().put("state", body).await?;
                Response::ok("ok")
            }
            (Method::Get, p) if is_ws_path(p) => {
                let pair = WebSocketPair::new()?;
                self.state.accept_web_socket(&pair.server);
                // 订阅确认：让客户端能区分「连上了」与「在等第一条事件」。
                pair.server
                    .send_with_str(json!({ "kind": "connected" }).to_string())?;
                Response::from_websocket(pair.client)
            }
            (Method::Post, BROADCAST_PATH) => {
                let body = req.text().await?;
                let delivered = self.broadcast(&body);
                Response::from_json(&json!({ "delivered": delivered }))
            }
            _ => Response::error("not found", 404),
        }
    }

    /// hibernation 唤醒后的入站消息：只处理探活 ping，其余忽略
    /// （订阅是连接即全量，不设主题订阅协议）。
    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        if let WebSocketIncomingMessage::String(text) = message {
            if text.trim() == "ping" {
                ws.send_with_str(json!({ "kind": "pong" }).to_string())?;
            }
        }
        Ok(())
    }

    /// 连接关闭：hibernation 运行时自行摘除死连接，这里无状态可清。
    async fn websocket_close(
        &self,
        _ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        Ok(())
    }
}

impl AgentStateDO {
    /// 向本实例全部在线 WS 连接推送一条文本，返回成功送达数。
    /// 单个连接发送失败不影响其余连接（死连接由运行时回收）。
    fn broadcast(&self, text: &str) -> usize {
        let mut delivered = 0;
        for ws in self.state.get_websockets() {
            if ws.send_with_str(text).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }
}

/// 按 agent_id 生成 DO 名称（一个分身一个 DO 对象，天然分片）。
pub fn stub_name_for_agent(agent_id: &str) -> String {
    agent_id.to_string()
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn stub_name_is_agent_id() {
        assert_eq!(stub_name_for_agent("agent-1"), "agent-1");
    }

    #[wasm_bindgen_test]
    fn ws_path_matching_accepts_external_and_internal() {
        assert!(is_ws_path("/api/ws")); // lib.rs 转发后路径保持原样
        assert!(is_ws_path("/ws"));
        assert!(!is_ws_path("/state"));
        assert!(!is_ws_path("/broadcast"));
    }
}
