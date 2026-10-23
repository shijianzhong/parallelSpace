//! 本体沙箱（AGENTS.md §7.8 / §8.7，G9 真沙箱）：隔离运行提案本体，
//! 用当前世界状态快照跑 N tick 规则仿真后如实判定。
//!
//! 硬禁：沙箱写入生产 D1/KV/R2。本模块对生产侧只有**只读**访问——
//! 取回提案 Turtle（R2）、读版本行与世界快照（D1/KV）；仿真全程在内存中
//! 推进，结果不落正式事件流、不落任何存储，同步返回治理员。
//! 隔离守卫保留原语义：*_SANDBOX 绑定必须存在，生产绑定名直接拒绝。
//!
//! 判定（passed 不再无条件，四路任一失败即 failed，明细进 notes）：
//! 1. 装载：提案 Turtle 与内嵌基线本体一轮 OWL 2 RL（与 analyze_impact
//!    同一条 Owl2RlReasoner 路径），解析/装载失败即 failed；
//! 2. 一致性：装载后出现 owl:Nothing 实例即不一致；
//! 3. SHACL：提案图过全部内嵌形状（person-card / charter / transaction），
//!    任一违规即 failed；
//! 4. 仿真（本体一致才跑）：世界快照（active/idle 分身的 KV 热状态——线上
//!    决策循环的真实状态源）先过初始不变量检查，再逐 tick 用与线上**同一**
//!    纯函数 `decision::rule_step` 推进，每 tick 后复查不变量（四项资源
//!    有限且非负、能量 ≤ 100），任一打破即 failed 并注明 tick 与 agent。
//!
//! 近似点（如实标注）：
//! - 仿真只覆盖现有规则决策循环（能量推进）；LLM 决策路径未接上游
//!   （G12），不在仿真范围内；organizations / positions 无 tick 规则，
//!   只作快照背景计数，不参与推进。
//! - SHACL 形状以实例类为 target，TBox 型提案通常无命中；提案若携带
//!   实例数据则会被形状真实校验。完整实例层校验待 G14。
//! - `proposal_id` 即已发布的本体版本标识（与 analyze_impact 同口径），
//!   典型用法是审批前对 pending 版本跑沙箱。

use worker::Env;

use crate::agent_runtime::decision::rule_step;
use crate::common::error::{AppError, AppResult};
use crate::governance::ontology_admin;
use crate::ontology::reason::Owl2RlReasoner;
use crate::ontology::shacl;
use crate::storage::kv::HotState;
use crate::storage::{d1, kv};

/// 生产绑定名（wrangler.toml 中的 DB/KV/R2）。
const PRODUCTION_BINDINGS: &[&str] = &["DB", "KV", "R2"];

/// 沙箱单次仿真的 tick 上限（试运行量级：几十个 agent、少量 ticks；
/// 超限直接拒绝，不静默截断、不假报）。
pub const MAX_SANDBOX_TICKS: u64 = 1000;

/// notes 中最多逐条列出的 SHACL 违规数（超出只记总数，避免响应膨胀）。
const MAX_VIOLATION_NOTES: usize = 10;

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
    /// 实际完成的仿真 tick 数（装载失败或本体不一致时为 0——本体自相矛盾
    /// 则仿真无意义；SHACL 违规不挡仿真，判定联合；仿真中途不变量被打破
    /// 时为打破前已完成的 tick 数）。
    pub ticks_run: u64,
    pub passed: bool,
    /// 提案装载进内嵌基线本体后是否一致（OWL 2 RL 口径）；装载失败时为 false。
    pub consistent: bool,
    /// 提案图过全部内嵌 SHACL 形状的违规总数（仅统计形状违规本身；
    /// 图解析/形状加载失败看 notes）。
    pub shacl_violations: u64,
    /// 参与仿真的快照 agent 数。
    pub agents_simulated: u64,
    /// 判定明细：失败原因逐条在前，快照与仿真背景在后。
    pub notes: Vec<String>,
}

/// 世界状态快照（只读）：参与仿真的 agent 热状态 + 结构化存储背景计数。
#[derive(Debug, Clone)]
pub struct WorldSnapshot {
    /// (agent_id, 热状态)；热状态取自生产 KV（决策循环的真实状态源），
    /// 未命中用默认值——与线上 `run_decision_cycle` 的读法一致。
    pub agents: Vec<(String, HotState)>,
    pub organizations: u64,
    pub positions: u64,
}

/// 状态不变量检查：四项资源有限且非负、能量不超过 100。
/// 返回第一条被打破的不变量描述（带 agent 与字段），全过返回 None。
fn invariant_violation(agent_id: &str, state: &HotState) -> Option<String> {
    for (field, value) in [
        ("money", state.money),
        ("food", state.food),
        ("energy", state.energy),
        ("reputation", state.reputation),
    ] {
        if !value.is_finite() {
            return Some(format!("agent {agent_id}: {field} is not finite ({value})"));
        }
        if value < 0.0 {
            return Some(format!("agent {agent_id}: {field} is negative ({value})"));
        }
    }
    if state.energy > 100.0 {
        return Some(format!(
            "agent {agent_id}: energy exceeds 100 ({})",
            state.energy
        ));
    }
    None
}

/// 沙箱判定核心（纯函数，不碰 Env）：装载 → 一致性 → SHACL → 快照仿真 → 判定。
/// 单测直接调用本函数覆盖好/坏提案两路。
pub fn evaluate_proposal(
    proposal_id: &str,
    turtle: &str,
    snapshot: &WorldSnapshot,
    ticks: u64,
) -> SandboxResult {
    let agents_simulated = snapshot.agents.len() as u64;

    // 1. 真装载：内嵌基线本体 + 提案文本一轮 OWL 2 RL（analyze_impact 同路径）
    let reasoner = match Owl2RlReasoner::build(turtle) {
        Ok(r) => r,
        Err(e) => {
            return SandboxResult {
                proposal_id: proposal_id.to_string(),
                ticks_run: 0,
                passed: false,
                consistent: false,
                shacl_violations: 0,
                agents_simulated,
                notes: vec![format!("proposal failed to load: {e}")],
            };
        }
    };

    let mut failures: Vec<String> = Vec::new();

    // 2. 一致性：图中出现 owl:Nothing 实例即不一致
    let consistent = reasoner.is_consistent();
    if !consistent {
        failures.push(
            "ontology inconsistency: baseline + proposal entail owl:Nothing instance(s)".into(),
        );
    }

    // 3. SHACL 全量形状：提案图过全部内嵌形状
    let mut shacl_violations: u64 = 0;
    match shacl::Graph::parse_turtle(turtle) {
        Ok(graph) => match shacl::load_shapes() {
            Ok(shapes) => {
                let violations = shacl::validate(&graph, &shapes);
                shacl_violations = violations.len() as u64;
                for v in violations.iter().take(MAX_VIOLATION_NOTES) {
                    failures.push(format!("shacl violation at {}: {}", v.path, v.message));
                }
                if violations.len() > MAX_VIOLATION_NOTES {
                    failures.push(format!(
                        "shacl violations truncated in notes: {} total",
                        violations.len()
                    ));
                }
            }
            Err(e) => failures.push(format!("shacl shapes failed to load: {e}")),
        },
        Err(e) => failures.push(format!("shacl graph parse failed: {e}")),
    }

    // 4. 快照仿真：初始不变量 → 逐 tick 同规则推进 → 每 tick 后复查不变量。
    // 本体不一致时跳过（自相矛盾的本体下推进没有语义），ticks_run 保持 0。
    let mut states: Vec<(String, HotState)> = snapshot.agents.clone();
    let mut ticks_run: u64 = 0;
    let mut sim_failure: Option<String> = None;
    if consistent {
        for (id, st) in &states {
            if let Some(v) = invariant_violation(id, st) {
                sim_failure = Some(format!("snapshot invariant broken before tick 1: {v}"));
                break;
            }
        }
        if sim_failure.is_none() {
            for tick in 1..=ticks {
                for (_, st) in states.iter_mut() {
                    let (_, next) = rule_step(st);
                    *st = next;
                }
                ticks_run = tick;
                for (id, st) in &states {
                    if let Some(v) = invariant_violation(id, st) {
                        sim_failure = Some(format!("invariant broken after tick {tick}: {v}"));
                        break;
                    }
                }
                if sim_failure.is_some() {
                    break;
                }
            }
        }
        if let Some(f) = sim_failure {
            failures.push(f);
        }
    }

    let mut notes: Vec<String> = failures.clone();
    notes.push(format!(
        "snapshot: {} agent(s), {} organization(s), {} position(s); \
         simulated {ticks_run}/{ticks} tick(s) with production rule_step; \
         consistent={consistent}, shacl_violations={shacl_violations}",
        snapshot.agents.len(),
        snapshot.organizations,
        snapshot.positions
    ));

    SandboxResult {
        proposal_id: proposal_id.to_string(),
        ticks_run,
        passed: failures.is_empty(),
        consistent,
        shacl_violations,
        agents_simulated,
        notes,
    }
}

/// 跑沙箱：隔离守卫 → 解析提案版本 → 取回 Turtle → 读世界快照 → 判定。
///
/// 对生产侧全程只读；版本不存在返回 OntologyVersionNotFound（400，
/// 与 analyze_impact 同口径）；判定类失败（装载/一致性/SHACL/不变量）
/// 不走 Err，而是如实落在 SandboxResult.passed = false 与 notes 明细里。
pub async fn run_sandbox(env: &Env, proposal_id: &str, ticks: u64) -> AppResult<SandboxResult> {
    // 证明沙箱绑定存在且不是生产绑定
    ensure_sandbox_binding("DB_SANDBOX")?;
    let _ = env.d1("DB_SANDBOX")?;
    ensure_sandbox_binding("KV_SANDBOX")?;
    let _ = env.kv("KV_SANDBOX")?;
    ensure_sandbox_binding("R2_SANDBOX")?;
    let _ = env.bucket("R2_SANDBOX")?;

    if ticks > MAX_SANDBOX_TICKS {
        return Err(AppError::InvalidInput(format!(
            "ticks {ticks} exceeds sandbox limit {MAX_SANDBOX_TICKS}"
        )));
    }

    // 提案解析：proposal_id 即已发布的本体版本（与 analyze_impact 同口径）
    let db = env.d1("DB")?;
    let row = d1::get_version(&db, proposal_id)
        .await?
        .ok_or_else(|| AppError::OntologyVersionNotFound(proposal_id.to_string()))?;
    let domains = d1::get_version_domains(&db, proposal_id).await?;
    let turtle = ontology_admin::load_version_turtle(env, proposal_id, &domains).await?;

    // 世界快照（生产侧只读）：active/idle 分身 + 其 KV 热状态 + 背景计数
    let kv_store = env.kv("KV")?;
    let mut agents = Vec::new();
    for id in d1::list_simulatable_agents(&db).await? {
        let state = kv::get_hot_state(&kv_store, &id).await?;
        agents.push((id, state));
    }
    let snapshot = WorldSnapshot {
        agents,
        organizations: d1::count_organizations(&db).await?,
        positions: d1::count_positions(&db).await?,
    };

    let mut result = evaluate_proposal(proposal_id, &turtle, &snapshot, ticks);
    result.notes.insert(
        0,
        format!(
            "proposal version {} (status: {}), domains: {}",
            proposal_id,
            row.status,
            domains.join(", ")
        ),
    );
    Ok(result)
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

    fn snapshot_with(states: Vec<(&str, HotState)>) -> WorldSnapshot {
        WorldSnapshot {
            agents: states
                .into_iter()
                .map(|(id, st)| (id.to_string(), st))
                .collect(),
            organizations: 2,
            positions: 3,
        }
    }

    fn state(energy: f64) -> HotState {
        HotState {
            energy,
            ..HotState::default()
        }
    }

    const GOOD_TURTLE: &str = r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://parallelspace.org/ontology/core#TransportMode> a owl:Class ;
    rdfs:comment "L2 emergence candidate: named ways of moving" .
"#;

    #[wasm_bindgen_test]
    fn good_proposal_passes_with_real_simulation() {
        let snap = snapshot_with(vec![("a1", state(50.0)), ("a2", state(10.0))]);
        let r = evaluate_proposal("v-good", GOOD_TURTLE, &snap, 3);
        assert!(r.passed, "notes: {:?}", r.notes);
        assert!(r.consistent);
        assert_eq!(r.shacl_violations, 0);
        assert_eq!(r.ticks_run, 3);
        assert_eq!(r.agents_simulated, 2);
    }

    #[wasm_bindgen_test]
    fn inconsistent_proposal_fails_before_simulation() {
        // 实例被声明为 owl:Nothing ⇒ 与基线装载后不一致（reasonable 判法）
        let bad = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<urn:x:bad> a owl:Nothing .\n";
        let snap = snapshot_with(vec![("a1", state(50.0))]);
        let r = evaluate_proposal("v-bad", bad, &snap, 3);
        assert!(!r.passed);
        assert!(!r.consistent);
        assert_eq!(r.ticks_run, 0);
        assert!(
            r.notes.iter().any(|n| n.contains("inconsistency")),
            "notes: {:?}",
            r.notes
        );
    }

    #[wasm_bindgen_test]
    fn unparsable_proposal_fails_as_load_error() {
        let snap = snapshot_with(vec![]);
        let r = evaluate_proposal("v-broken", "<<<not turtle", &snap, 3);
        assert!(!r.passed);
        assert_eq!(r.ticks_run, 0);
        assert!(
            r.notes.iter().any(|n| n.contains("failed to load")),
            "notes: {:?}",
            r.notes
        );
    }

    #[wasm_bindgen_test]
    fn shacl_violating_proposal_fails() {
        // 提案携带实例数据：soc:Person 的 status 不在形状枚举内且缺必填字段
        let bad = r#"
@prefix soc: <http://soc.example/ontology#> .
<urn:p:1> a soc:Person ;
    soc:status "flying" .
"#;
        let snap = snapshot_with(vec![("a1", state(50.0))]);
        let r = evaluate_proposal("v-shacl-bad", bad, &snap, 2);
        assert!(!r.passed, "notes: {:?}", r.notes);
        assert!(r.consistent, "shacl failure must not taint consistency");
        assert!(r.shacl_violations > 0);
        assert_eq!(r.ticks_run, 2, "simulation still runs; verdict is joint");
    }

    #[wasm_bindgen_test]
    fn illegal_snapshot_state_fails_invariant_check() {
        // 快照里能量 150 超过上限：不变量在 tick 1 之前即被打破
        let snap = snapshot_with(vec![("a1", state(150.0))]);
        let r = evaluate_proposal("v-good", GOOD_TURTLE, &snap, 3);
        assert!(!r.passed);
        assert_eq!(r.ticks_run, 0);
        assert!(
            r.notes.iter().any(|n| n.contains("invariant broken")),
            "notes: {:?}",
            r.notes
        );
    }

    #[wasm_bindgen_test]
    fn simulation_uses_production_rule_step() {
        // 与 decision::rule_step 同语义：50 → idle 三 tick 到 47；
        // 10 → rest 到 20 → idle 两 tick 到 18。用 evaluate 的判定侧面验证：
        // 低能量 agent 经 rest 回升后仍满足不变量且全程 passed。
        let snap = snapshot_with(vec![("low", state(10.0)), ("mid", state(50.0))]);
        let r = evaluate_proposal("v-good", GOOD_TURTLE, &snap, 3);
        assert!(r.passed, "notes: {:?}", r.notes);
        // rule_step 本身的边界（与 decision.rs 内联旧实现逐项对齐）
        assert_eq!(rule_step(&state(19.0)).0, "rest");
        assert_eq!(rule_step(&state(19.0)).1.energy, 29.0);
        assert_eq!(rule_step(&state(20.0)).0, "idle");
        assert_eq!(rule_step(&state(95.0)).1.energy, 94.0);
        assert_eq!(rule_step(&state(0.0)).1.energy, 10.0);
    }
}
