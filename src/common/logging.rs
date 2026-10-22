//! 结构化 JSON 日志（AGENTS.md §2：worker::console_log! / console_error!，结构化 JSON）。

use serde_json::json;

/// 写一条结构化日志。level: info/warn/error。
/// 生产环境可被 Tail Worker 按行消费；本地 wrangler dev 直接打印。
pub fn log(level: &str, event: &str, fields: serde_json::Value) {
    let line = json!({
        "level": level,
        "event": event,
        "fields": fields,
    });
    match level {
        "error" => worker::console_error!("{}", line),
        "warn" => worker::console_log!("{}", line),
        _ => worker::console_log!("{}", line),
    }
}

pub fn info(event: &str, fields: serde_json::Value) {
    log("info", event, fields);
}

pub fn warn(event: &str, fields: serde_json::Value) {
    log("warn", event, fields);
}

pub fn debug(event: &str, fields: serde_json::Value) {
    log("debug", event, fields);
}

pub fn error(event: &str, fields: serde_json::Value) {
    log("error", event, fields);
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn log_line_is_valid_json_shape() {
        // 只验证序列化形状，不触发 console（wasm 下无 console 也不 panic）
        let line = json!({
            "level": "info",
            "event": "tick_done",
            "fields": {"woken": 3},
        });
        let s = line.to_string();
        assert!(s.contains("\"level\":\"info\""));
        assert!(s.contains("\"event\":\"tick_done\""));
        assert!(s.contains("\"woken\":3"));
    }
}
