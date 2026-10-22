//! Durable Objects：Agent 状态协调 / WebSocket（SAD §10.1）。
//!
//! MVP：DO 提供状态读写与 WebSocket 升级占位；广播在 M2 接入。
//! 热点分片：按 agent_id 命名取 stub，避免单对象过热。

use worker::{durable_object, wasm_bindgen, DurableObject, Env, Method, Request, Response, Result, State, WebSocketPair};

#[durable_object]
pub struct AgentStateDO {
    state: State,
}

impl DurableObject for AgentStateDO {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    /// GET /state 读状态；POST /state 写状态；GET /ws 升级 WebSocket。
    async fn fetch(&self, mut req: Request) -> Result<Response> {
        match (req.method(), req.path().as_str()) {
            (Method::Get, "/state") => match self.state.storage().get::<String>("state").await? {
                Some(s) => Response::ok(s),
                None => Response::ok("{}"),
            },
            (Method::Post, "/state") => {
                let body = req.text().await?;
                self.state.storage().put("state", body).await?;
                Response::ok("ok")
            }
            (Method::Get, "/ws") => {
                // MVP 占位：接受连接后原样挂起，由事件总线在 M2 驱动广播。
                let pair = WebSocketPair::new()?;
                self.state.accept_web_socket(&pair.server);
                Response::from_websocket(pair.client)
            }
            _ => Response::error("not found", 404),
        }
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
}
