//! 本体沙箱（AGENTS.md §7.8）：隔离运行提案本体，N tick 后返回结果。
//!
//! 硬禁：沙箱写入生产 D1/KV/R2。本模块用绑定名校验做守卫——
//! 沙箱只允许碰 *_SANDBOX 绑定，任何生产绑定名直接拒绝。

use worker::Env;

use crate::common::error::{AppError, AppResult};

/// 生产绑定名（wrangler.toml 中的 DB/KV/R2）。
const PRODUCTION_BINDINGS: &[&str] = &["DB", "KV", "R2"];

/// 校验绑定名是沙箱专用（DB_SANDBOX/KV_SANDBOX/R2_SANDBOX）。
pub fn ensure_sandbox_binding(binding: &str) -> AppResult<()> {
    if PRODUCTION_BINDINGS.contains(&binding) {
        return Err(AppError::Forbidden);
    }
    if !binding.ends_with("_SANDBOX") {
        return Err(AppError::InvalidInput(format!(
            "binding {binding} is not a sandbox binding"
        )));
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct SandboxResult {
    pub proposal_id: String,
    pub ticks_run: u64,
    pub passed: bool,
    pub notes: Vec<String>,
}

/// 跑沙箱：MVP 只做隔离校验与占位结果；真实 N tick 仿真在 M2 接入
/// （需要独立的 sandbox 存储与本体装载路径）。
pub async fn run_sandbox(env: &Env, proposal_id: &str, ticks: u64) -> AppResult<SandboxResult> {
    // 证明沙箱绑定存在且不是生产绑定
    ensure_sandbox_binding("DB_SANDBOX")?;
    let _ = env.d1("DB_SANDBOX")?;
    ensure_sandbox_binding("KV_SANDBOX")?;
    let _ = env.kv("KV_SANDBOX")?;
    ensure_sandbox_binding("R2_SANDBOX")?;
    let _ = env.bucket("R2_SANDBOX")?;

    Ok(SandboxResult {
        proposal_id: proposal_id.to_string(),
        ticks_run: ticks,
        passed: true,
        notes: vec!["MVP placeholder: isolation verified, simulation in M2".into()],
    })
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn sandbox_rejects_production_bindings() {
        for b in ["DB", "KV", "R2"] {
            assert!(matches!(ensure_sandbox_binding(b), Err(AppError::Forbidden)), "{b}");
        }
    }

    #[wasm_bindgen_test]
    fn sandbox_accepts_only_suffixed_bindings() {
        for b in ["DB_SANDBOX", "KV_SANDBOX", "R2_SANDBOX"] {
            assert!(ensure_sandbox_binding(b).is_ok(), "{b}");
        }
        assert!(ensure_sandbox_binding("PROD_DB").is_err());
        assert!(ensure_sandbox_binding("").is_err());
    }
}
