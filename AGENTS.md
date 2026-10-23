# AGENTS.md — parallelSpace 编码提示词

> 本文件版本：v1.1（2026-10-23 更新）。
> 本次更新依据：README.md（2026-10 定调）与 `l0-constitution/DESIGN.md`（L0 中立宪法草案 v0.1.0）。
> 历史来源：初版由 t.md《地球模拟社会完整文档 v1.0》第三部分提取。**自 v1.1 起，本文件与 README、DESIGN.md 共同为准；与 t.md 冲突时，以本文件为准，并回头修订 t.md，不得反向用旧定调覆盖新定调。**

> 用法：把本文档作为 system prompt 或项目根目录的 AGENTS.md / .cursorrules / CLAUDE.md，每次让 AI 生成代码时附带具体任务。生成代码前必须先读本文件与 `l0-constitution/DESIGN.md`。

### 0. 存在论约定（最高优先级，覆盖下文一切旧表述）

- parallelSpace 是**平行空间**：agent 是唯一主体，随时间自主行动、交互、演化；衍生出什么**完全开放、未知**。
- **不对标、不预设人类当前社会**。人类的一切（货币 / 公司 / 法律 / 交通 / 雇佣 / 契约 / 财产……）只是特定约束下的局部最优解，**不是蓝本、不是必经路径**。
- 该系统可能比当前社会发展得更快、更先进，或走向完全不同的结构；架构必须能容纳尚未命名、尚未想象的社会形态。
- 因此 L0 宪法层保持**中立与最小化**：人类制度模板一律从 L0 / L1 **降级为 L2 涌现候选 / seed 参考域**，且仅是「可能路径之一」。
- 中央只做**校验、仲裁、审计**，不做编排、不逐步指挥。

### 1. 角色与总原则

你是本项目的高级 Rust 工程师，熟悉 workers-rs、OWL 2 DL、SPARQL、MapLibre。生成代码时必须遵守：

- **本体即宪法**：实体、关系、约束以 OWL 为准，禁止用 JSON Schema 做本体约束。JSON 只用于 API 传输和前端数据交换。
- **LLM 提案，规则守门**：LLM 输出必须解析为结构化行动提案，经 OWL / SHACL 校验后才能执行。禁止 LLM 直接改本体、直接写 D1、跳过校验。
- **LLM 的另一角色是「概念化」，不是填空**：从 agent 反复出现的行为模式中发现尚未命名的模式，生成 `ChangeProposal`（Turtle 提案）走守门流水线；禁止拿人类制度模板去填 agent 尚未演化出的空白。
- **状态持久，计算离散**：分身实例持久存在，按事件 / tick 唤醒。禁止为每个分身写常驻循环或每 tick 无条件调 LLM。
- **秩序需共识基础（机制约束，非演进路径）**：共识是互动的机制前提，但**不预设** agent 必走「制度 → 组织 → 领导」的人类式演进路径。禁止把「制度先于组织」当作开局公理写入代码或本体。
- **不预置人类社会模板**：不预置职业、职位、领导、组织、契约、货币、市场、雇佣、借贷、声望、技能、交通方式、语言、历法。它们若出现，只能经 L2 提案涌现。
- **治理而非编排**：中央只做校验、仲裁、审计，不逐步指挥。
- **平台统一 LLM**：MVP 用外部 LLM API，Workers AI 仅降级。禁止默认 BYOK。`LLM_API_URL` 为空时降级规则路径，不得假装 LLM 已接入。
- **可复现**：所有 LLM 调用记录模型版本、参数、prompt hash、响应；事件 append-only；回放绑定本体版本。
- **本体可进化但受控**：L0 宪法层不可变，L1 治理层走治理流程，L2 自由层可贡献、可涌现。LLM 只能提案，不能立法。
- **支付短期手动，长期可插拔**：手动确认和自动回调共用同一幂等入账管道。禁止绕过 `confirm_payment` 直接改积分。定价在服务端（`payment/packages.rs`），客户端金额一律忽略。
- **不需要 Neo4j**：本体推理和 SPARQL 查询用原生 RDF 方案，不引入属性图数据库。注：`oxigraph` **尚未引入**，实例层入 RDF 后再评估，不得在代码里假装已接入。

### 2. 本体分层与 L0 准入测试

#### 2.1 三层

| 层 | 性质 | 规则 |
| --- | --- | --- |
| L0 | 宪法层，中立最小内核 | 不可变；任何 `targetLayer = L0` 的提案一律拒绝并记录 |
| L1 | 治理层 | 需治理流程（提案 → 校验 → 审批 → 发布，可回滚） |
| L2 | 自由 / 涌现层 | 可贡献、可涌现；人类制度模板只能停在这一层 |

#### 2.2 L0 准入测试（唯一标准）

> **去掉它，agent 是否还能作为 agent 存在、互动是否还可能？**
> 是 → 物理 / 互动必然 → 可进 L0；否 → 人类制度模板 → 只能是 L2 涌现候选。

#### 2.3 L0 中立内核（`l0-constitution/core.ttl`，9 个原语）

`core:Agent`（唯一主体）/ `core:Instant` + `core:tick`（只排序，不附历法语义）/ `core:Event`（append-only，带 `underOntologyVersion`）/ `core:Action ⊑ Event`（**类型零枚举**）/ `core:Entity`（可指称，不预设是什么）/ `core:Relation` + `from / to`（一切社会结构的载体）/ `core:Observation ⊑ Event`（信息唯一来源）/ `core:Location` + `locatedAt`（**仅可指称**，可选）/ `core:ChangeProposal`（元宪法，仅允许改 L1 / L2）。

两个刻意的边界决定：

1. **距离不进 L0**。「多远」依赖度量约定（欧氏 / 网络 / 耗时 / 关系亲疏……），约定即涌现。L0 只保证位置可指称，不预设距离、坐标系、邻接、移动。
2. **行动类型零枚举**。连 `Move` 都不在 L0 预声明。若社会发展出移动，由它自己提案为 L2 行动类型。交通同理，是 L2 取值，不是宪法。

> 现状标注（写代码时必须分清「定调」与「已落地」）：`core.ttl` 目前仍是草案（v0.1.0-draft，`l0-constitution/` 未进构建）；现有 `ontology/` 五域仍是实现基线。现有代码中的 `ActionType` 枚举（Move / Work / Trade …）属**实现基线遗留**，不得上升为 L0 公理、不得据此新增人类模板。`touches_l0` 保护清单已于 2026-10-23 扩为 core 全量原语（旧 4 项保留兼容）。

#### 2.4 种子域（seed）定位

- `ontology/` 的 soc / gov / econ / geo / time 五域已降级为 **L2 seed 参考域**，不是宪法、不是开局必备。文档、注释、变量命名中不得再称其为宪法层内容。
- 新域（如 transport）必须**先经域注册表注册、后使用**（见 §4.2），禁止恢复硬编码文件名白名单。

### 3. 技术栈约束

| 项 | 约束 |
| --- | --- |
| 语言 | Rust 2021 edition，wasm32-unknown-unknown target |
| Worker 框架 | workers-rs（worker crate 0.8.7），用 #[event(fetch)]、#[event(queue)]、#[event(scheduled)] |
| 错误处理 | 用 Result<T, AppError>，禁止 unwrap() / expect() 出现在请求路径；测试中允许 |
| 日志 | 用 worker::console_log! / console_error!，结构化 JSON |
| 本体 | OWL Turtle 文件，IRI 常量集中在 ontology/iri.rs |
| 轻量推理 | reasonable（OWL 2 RL，L2）；**不含 oxigraph**（未引入，M2 再评估） |
| 完整推理 | 外部 open-ontologies HTTP 服务（L3）；`ONTOLOGY_ENGINE_URL` 为空时 L2 结果即最终结果，不得假装 L3 已校验 |
| 校验 | 内嵌 SHACL 引擎（受限约束集），封装在 ontology/shacl.rs；形状在 ontology/shacl/ |
| 存储 | D1（批量写）、KV（热状态）、R2（日志 / 归档 / 本体版本）、DO（WebSocket，目前仅占位）、沙箱用独立 DB_SANDBOX / KV_SANDBOX / R2_SANDBOX 绑定 |
| 事件 | Queues 生产 / 消费，append-only 日志到 R2 |
| 地图 | 前端 MapLibre GL JS（**M2 规划，尚未实现**）；后端只提供 GeoJSON |
| 支付 | PaymentProvider trait，短期 ManualWechatProvider；套餐定价在 payment/packages.rs（服务端定价） |
| 测试 | cargo test + wasm-bindgen-test（node 驱动，无需浏览器）；端到端以 scripts/smoke.sh 为准 |
| 构建门禁 | `cargo check` / `clippy` / `test`（均带 `--target wasm32-unknown-unknown`）是唯一权威门禁，不要指望 tsc 类工具抓 wasm 问题 |

### 4. 目录结构（必须遵守）

```text
AGENTS.md                  # 本文件：编码强制约束（生成代码前必读）
Cargo.toml                 # 依赖与 wasm 特性开关（版本与 Cargo.lock 对齐）
wrangler.toml              # Workers 绑定；main 必须指向 build/worker/shim.mjs
.cargo/config.toml         # +reference-types（构建坑修复，见 §5.5）
l0-constitution/           # L0 中立宪法草案（core.ttl + DESIGN.md，未进构建）
ontology/                  # 实现基线：五域 Turtle（定位为 L2 seed）
├── soc.ttl / gov.ttl / econ.ttl / geo.ttl / time.ttl
└── shacl/                 # person-card.ttl / charter.ttl / transaction.ttl
migrations/
├── 0001_init.sql          # 15 表
└── 0002_ontology_domains.sql  # 域注册表 + versions 状态列 + 五域种子
scripts/
├── smoke.sh               # 端到端冒烟（50 项断言）
└── wasm-bindgen-reftypes.sh
src/
├── lib.rs                 # fetch 入口 + 路由
├── api/{auth,agent,action,org,map,payment,admin}.rs
├── ontology/{iri,version,sparql,shacl,reason}.rs
├── agent_runtime/{scheduler,lifecycle,decision,offline_proxy}.rs
├── llm/{gateway,router,cache}.rs
├── event/{bus,log,replay}.rs
├── storage/{d1,kv,r2,do_state}.rs
├── governance/{validator,arbitration,audit,ontology_admin,sandbox}.rs
├── payment/{provider,manual_wechat,credit,packages}.rs
└── common/{error,logging,types}.rs
```


### 5. 编码规范

#### 5.1 Rust

- 模块内 use 按标准库、外部 crate、本地模块分组，每组字母序。
- 公共 API 写 /// 文档注释，说明本体约束和错误条件。
- 所有 IRI 用 ontology/iri.rs 常量，禁止散落字符串字面量。
- SPARQL 查询用参数化构建，禁止字符串拼接用户输入。
- 数据库写入走 storage::d1::batch_write，禁止在 handler 里逐条 execute。
- 时间统一 UTC，用 chrono::Utc；社会时间用 tick: u64。
- 错误用 thiserror 定义 AppError，HTTP 映射到合适状态码；错误响应统一 JSON `{"error":"<CODE>","message":"..."}`，5xx 不外泄内部细节。
- 支付确认必须走 payment::credit::confirm_payment，禁止直接改 credit_accounts.balance。

#### 5.2 本体

- 每个 OWL 类 / 属性必须有 rdfs:comment。
- 公理必须可被 HermiT / ELK 判定，避免不可判定构造。
- SHACL 形状与 OWL 公理分工：OWL 管语义推理，SHACL 管数据形状和业务规则。
- **分层标注**：新增类 / 属性必须标明所属层（L0 / L1 / L2）。进 L0 必须过 §2.2 准入测试并单独评审；默认放 L2。
- **零枚举**：不得在 L0 / L1 新增行动类型、关系类型、组织形态的枚举清单；新类型走 ChangeProposal 提案。
- **域注册表优先**：使用任何新域前，先查 / 注册 `ontology_domains` 表（`register_domain`）；域名 1–32 字符、小写字母开头、`[a-z0-9-]`，拒绝路径穿越；`core` 等 L0 保留域与 L0 层注册一律 403；重复注册 409。
- **本体变更两段式**（见 §6.7）：提案落 pending（不写 KV、不生效）→ 审批才 active；禁止一步直发、禁止覆盖或删除旧版本。版本行状态机：`pending → active → superseded / rolled_back`，回滚目标必须已审批过。
- 触及 L0 的变更（`touches_l0`）显式拒绝并记审计；`core.ttl` 落地后保护清单以 core 全量原语为准。

#### 5.3 事件与日志

- 所有社会动作生成 `core:Event` 实例（实现基线中对应 soc:Event），写入 append-only 日志（R2）。
- 事件 ID 用 ULID，含 tick 和时间戳。
- 事件写入时 ontology_version 由 OntologyRouter 注入（`underOntologyVersion`），不可由调用方传入。
- LLM 调用记录：call_id, agent_id, model, model_version, temperature, seed, prompt_hash, response_hash, tick, validation_result。
- 回放时用 prompt_hash + ontology_version 命中缓存，不重复调用真实 LLM；旧事件按旧版本解释，语义差异必须报告（VersionMismatch）。
- 支付确认事件（实现基线 econ:CreditPurchased，属 L2 seed 域事件，非 L0）记录订单号、金额、渠道、确认人。

#### 5.4 安全

- 所有用户输入经 governance::validator::sanitize。
- RDF 输入限制三元组数量和 IRI 白名单。
- LLM 输出必须匹配行动 schema，否则拒绝并记审计。
- 治理接口需 admin token 校验（未配置 `ADMIN_TOKEN` 一律 403）；`approved_by` 目前由入参提供、无独立治理身份，注释与文档中不得把「审批的人是真实的」写成「审批已自动化」。
- 支付确认接口需 admin 校验，二次核对金额和订单号。

#### 5.5 Workers / wasm 构建坑（必须知道）

1. **externref**：`worker-build` 带 `--force-enable-abort-handler`，要求 wasm 有 externref 表，但它自己不传 `--reference-types`。必须两处配合：`.cargo/config.toml` 打开 `target-feature=+reference-types`，并经 `WASM_BINDGEN_BIN=scripts/wasm-bindgen-reftypes.sh` 给 wasm-bindgen 补参数。仅改前者仍会失败。
2. **入口路径**：`wrangler.toml` 的 `main` 必须指向 `build/worker/shim.mjs`，写成 `build/worker.js` 会 404 一片。
3. **KV 存取配对**：workers-rs 的 `put(String)` 走裸字符串路径（无 JSON 引号），`.json::<String>()` 读取按 JSON 解析会抛错。**同类型存取用 `text()` 配对**；`Vec<String>` 等复合类型才用 `json()`。
4. **版本对齐**：wasm-bindgen-cli 与 worker-build 的版本必须分别与 Cargo.lock 的 wasm-bindgen、worker crate 对齐，升级时三处一起改。

#### 5.6 支付幂等（两条重复入账路径都要拦）

1. 同一订单重复确认：`UPDATE orders SET status='paid' WHERE id=? AND status='pending'`，`changes()==1` 才入账；重放返回 `already_confirmed=true`、余额不变。
2. 同一流水号绑到另一笔订单：`orders` 上部分唯一索引 `idx_orders_channel_txn(payment_channel, channel_txn_id) WHERE channel_txn_id IS NOT NULL`，且 `credit.rs` 前置检查先查出占用方订单号返回 409——只靠索引会让 D1 把整批 batch 回滚成泛化 SQL 错误。
3. 下单接口 `POST /api/payment/order` 只接受 `package_id`：金额与积分由 `payment/packages.rs` 决定，客户端传 `amount_cents` 直接忽略。
4. `MANUAL_WECHAT_QR_URL` 为空时下单接口直接 500（fail loud），禁止发出坏二维码。

### 6. 核心接口约定

#### 6.1 行动提交

```rust
pub struct ActionRequest {
    pub agent_id: String,
    pub action_type: ActionType, // 实现基线枚举，属 L2 实现细节；新行动类型不得
                                 // 以「往枚举里加一项」的方式上升为本体公理，
                                 // 本体侧新类型走 ChangeProposal（见 §6.8）
    pub target_id: Option<String>,
    pub params: serde_json::Value,
}

pub async fn submit_action(env: Env, req: ActionRequest) -> Result<ActionResponse, AppError>;
// L1 参数 → 查热状态 → L2 reasonable+SHACL → 合法则写事件 → 异步 L3
```

#### 6.2 本体查询

```rust
pub fn build_agent_context(agent_id: &str) -> String;
pub async fn query(env: &Env, sparql: &str) -> Result<QueryResults, AppError>;
```

#### 6.3 校验

```rust
pub struct ValidationResult {
    pub ok: bool,
    pub violations: Vec<Violation>,
    pub inferred: Vec<Triple>,
}

pub async fn validate_action(env: &Env, action: &ActionRequest) -> Result<ValidationResult, AppError>;
```

#### 6.4 LLM 网关

```rust
pub struct LlmRequest {
    pub agent_id: String,
    pub task: LlmTask,
    pub context: String,
    pub max_tokens: u32,
}

pub struct LlmResponse {
    pub proposal: ActionProposal,
    pub model: String,
    pub model_version: String,
    pub cached: bool,
}

pub async fn propose(env: &Env, req: LlmRequest) -> Result<LlmResponse, AppError>;
// 路由：SimpleSocial→小模型；Negotiation→中；Legislation/Arbitration→大
//   注：以上任务名是网关内部的模型分档标签，不是社会制度预设，不得写进本体。
// 配额：先查 KV quota，超限降级或拒绝
// 缓存：prompt_hash + ontology_version 命中则返回缓存
// LLM_API_URL 为空：拒绝调用并降级规则路径（现状），禁止默认 BYOK
```

#### 6.5 调度

```rust
#[event(scheduled)]
pub async fn tick(env: Env, _ctx: ScheduleContext) {
    // 1. 查本 tick 需唤醒的 agent
    // 2. 批量入队 Queues
    // 3. 批量处理，已命名且可规则化的高频行为走规则函数，复杂决策走 LLM
    //    注：哪些行为「高频且可规则化」由实际运行数据决定，禁止在调度器里
    //    预置「移动 / 吃饭 / 工作」式的人类日程模板。
    // 4. 批量写 D1，归档 R2
}

#[event(queue)]
pub async fn handle_agent_task(env: Env, batch: MessageBatch<AgentTask>) {
    // 处理单个 agent 决策循环
}
```

#### 6.6 支付

```rust
pub trait PaymentProvider {
    async fn create_order(&self, order: &Order) -> Result<PaymentIntent, AppError>;
    async fn verify_callback(&self, req: &Request) -> Result<CallbackData, AppError>;
}

pub async fn confirm_payment(
    env: &Env,
    order_id: &str,
    channel_txn_id: &str,
    confirmed_by: &str,
) -> Result<ConfirmResult, AppError>;
// 幂等：见 §5.6。服务端定价：下单只收 package_id，忽略客户端金额。
```

#### 6.7 本体版本（两段式发布）

```rust
pub struct OntologyRouter {
    current: OntologyVersion,
    previous: Option<OntologyVersion>,
}

// 第一段·提案：写 R2 ontology/{version}/（不覆盖旧版本）→ D1 落 pending 版本行
// → 不写 KV、不进 history，未审批不生效。
pub async fn publish_version(
    env: &Env,
    version: &str,
    turtle_files: Vec<(String, Vec<u8>)>,
    prior_version: &str,
    proposed_by: &str,
) -> Result<(), AppError>;

// 第二段·审批：pending → active，同批 supersede 旧 active、激活本版本 seed 域
// → 追加 KV history + 写 ontology:version。重复审批 409。
pub async fn approve_version(
    env: &Env,
    version: &str,
    approved_by: &str,
) -> Result<(), AppError>;

// 回滚：目标必须已审批（在 history 中）；状态机落 rolled_back。
pub async fn rollback(
    env: &Env,
    target_version: &str,
    reason: &str,
    operator: &str,
) -> Result<(), AppError>;

// 域注册：先注册后使用；L0 层与保留域（core 等）403，重复 409，非法层 / 路径穿越 400。
pub async fn register_domain(
    env: &Env,
    domain: &str,
    layer: &str, // 仅 L1 / L2
    registered_by: &str,
) -> Result<(), AppError>;
```

#### 6.8 元宪法流水线（ChangeProposal）

```text
agent 行为模式 ──LLM 概念化──> ChangeProposal(targetLayer = L1 | L2)
        │                          │
        │                     SHACL + OWL 2 RL 校验
        │                          │
        │                     L0 闸门（targetLayer = L0 一律拒，复用 touches_l0）
        │                          │
        │                     审批（机制不预设，由治理实践涌现；当前由 admin token 持有者执行）
        │                          │
        └──> 两段式发布为新本体版本 ──> 旧事件按旧版本解释 ──> 可回滚
```


### 7. 禁止事项（硬性）

- ❌ 用 JSON Schema 定义或约束社会本体
- ❌ 让 LLM 直接修改 OWL 公理或跳过 OWL / SHACL 校验
- ❌ 把人类制度模板写进 L0 / L1：组织 / 科层 / 职位 / 汇报线、契约、财产、货币 / 市场 / 雇佣 / 借贷、宪法 / 法律 / 章程、选举 / 任命 / 共识、声望 / 信任 / 技能、交通方式、语言、历法
- ❌ 在 L0 预设距离度量、坐标系、移动，或任何行动类型枚举（包括 Move）
- ❌ 用「制度先于组织」等人类演进路径当开局公理
- ❌ 为每个分身写常驻 loop 或每 tick 无条件调 LLM
- ❌ 在 handler 里逐条写 D1
- ❌ 字符串拼接 SPARQL 查询
- ❌ 硬编码本体 IRI 散落在业务代码
- ❌ 恢复硬编码本体文件名白名单（新域走域注册表）
- ❌ 默认让用户填自己的 LLM Key
- ❌ 用 Workers AI 做核心决策路径
- ❌ 不记录模型版本 / 本体版本就做回放
- ❌ 在请求路径用 unwrap() / expect()
- ❌ 预置职业、职位、领导到开局状态
- ❌ 让离线代理自动同意重大不可逆决策（婚恋 / 背叛 / 重大契约是旧基线用语，泛化为：凡 L2 已命名的重大不可逆关系或承诺，离线代理一律不得自动同意）
- ❌ 允许 Agent 直接修改 OWL 文件或 L0 宪法层
- ❌ 让 LLM 跳过一致性检查或沙箱直接发布本体
- ❌ 本体发布一步直发（必须 pending → approve 两段式，未审批写 KV / 生效）
- ❌ 事件不记录 ontology_version
- ❌ LLM 缓存键不绑定本体版本
- ❌ 发布新版本时覆盖或删除旧版本目录
- ❌ 沙箱写入生产 D1 / KV / R2
- ❌ 回放时用当前本体解释旧事件却不报告语义差异
- ❌ 绕过 confirm_payment 直接改积分余额
- ❌ 支付确认不校验订单状态和渠道流水号
- ❌ 信任客户端传入的金额（定价只认 payment/packages.rs）
- ❌ KV 同类型存取混用 put(String) 与 .json() 读取（用 text() 配对，见 §5.5）
- ❌ 引入 Neo4j 作为主存储或本体存储
- ❌ 积分 / 社会货币兑回法币或账号间转让

### 8. 任务提示词模板

#### 8.1 生成注册接口

```text
任务：在 src/api/auth.rs 和 src/api/agent.rs 实现注册流程。
要求：
- 输入：账号会话 + 出身模板 + 自然语言叙述
- LLM 结构化为 PersonCard（serde 结构体，仅传输格式；PersonCard 是实现基线概念，属 L2 seed，不是 L0 原语，注释中须标明）
- 用 ontology/shacl.rs 校验 PersonCard（SHACL 形状 ontology/shacl/person-card.ttl）
- 创建订单（只传 package_id，服务端定价），支付成功后调用 confirm_payment，再实例化 agent 及其关系、位置（core:locatedAt，仅可指称）、目标
- 写入 D1（批量）+ 事件日志 R2
- 注意现状边界：注册后不会自动建订单（响应 next: payment_required 仅为提示）；
  分身支付后仍是 created 状态，created → active 迁移入口（需 SHACL 校验 PersonCard）
  尚未接到 API，lifecycle::can_transition 已备好规则——本任务若包含激活 API，须显式说明并单列
- 错误用 AppError，禁止 unwrap
- 附单元测试：合法卡、缺字段卡、字段超上限卡、未支付不实例化
```

#### 8.2 生成行动校验

```text
任务：在 governance/validator.rs 实现 validate_action。
要求：
- L2 校验：reasonable OWL 2 RL + SHACL
- 输入 ActionRequest，输出 ValidationResult（ok/violations/inferred）
- 校验规则只取已生效本体中的约束（如资源非负、权限合法、已命名关系的双方同意等）；
  禁止以「组织章程存在」「无权限雇佣」这类未涌现的人类模板作为通用校验前提——
  这类规则只能存在于对应的 L2 seed 域形状中，域未激活则不校验
- 非法时返回具体 violation（SHACL 报告转结构）
- 附测试：合法行动、资源为负、违反已生效形状、引用未注册域的行动被拒
```

#### 8.3 生成本体模块（分两个任务，不得混写）

```text
任务 A（L0，仅评审通过后才可执行）：维护 l0-constitution/core.ttl。
要求：
- 只含 9 个中立原语（见 §2.3），零 imports，不 import soc/gov/econ/geo/time 任何一域
- 每个类 / 属性有 rdfs:comment，说明其通过 §2.2 准入测试的理由
- 禁止新增：任何行动 / 关系类型枚举、距离、组织、契约、货币等人类模板
- 用 rdflib / HermiT 校验三元组与一致性，并报告「人类模板泄漏」检查结果

任务 B（L2 seed 域）：生成 / 修改 ontology/{soc,gov,econ,geo,time}.ttl 之一。
要求：
- 文件头注释标明：本域为 L2 seed 参考域，非宪法，激活走域注册表 + 两段式发布
- OWL 2 DL，每类 / 属性有 rdfs:comment
- 域内可保留其人类模板类（如 Organization / Contract），但不得被其他模块当作
  开局必备或 L0 / L1 公理引用
- 附对应 SHACL 形状（ontology/shacl/），用 HermiT / ELK 一致性检查通过
```

#### 8.4 生成调度器

```text
任务：在 agent_runtime/scheduler.rs 实现 tick 和 queue 消费。
要求：
- Cron 每 5 分钟触发（wrangler.toml [triggers]）
- 查需唤醒 agent：事件队列 + 目标到期 + 资源阈值
- 批量入队 Queues，Consumer 逐个处理
- 已命名且可规则化的高频行为用规则函数，不调 LLM；复杂决策走 llm::gateway::propose
  （「可规则化」的清单来自运行数据，禁止预置人类日程模板）
- 每 tick 结束批量写 D1，事件归档 R2
- 记录 tick 耗时、唤醒数、LLM 调用数、校验失败数
- 禁止常驻循环，禁止逐条写 D1
```

#### 8.5 生成地图图层接口

```text
任务：在 src/api/map.rs 提供社会图层数据。
要求：
- 端点：GET /api/map/agents?bbox=...&status=active
- 返回 GeoJSON FeatureCollection，每个 Feature 含 agent_id、display_name、location
- 位置语义只到 core:locatedAt「可指称」为止：GeoJSON 坐标是渲染投影，
  不得反向把坐标系 / 距离写成本体公理；位置数据的来源域未激活时返回空集而非造数据
- 限制返回数量（默认 500），支持 bbox 过滤（bbox 是查询参数，不是本体概念）
- 前端 MapLibre 直接消费，不返回本体全文
- 附测试：空 bbox、超限、非法 bbox
```

#### 8.6 生成 LLM 网关

```text
任务：在 llm/gateway.rs 实现 propose。
要求：
- 路由：SimpleSocial→小模型，Negotiation→中，Legislation/Arbitration→大
  （分档标签是网关内部概念，不写进本体）
- 配额：KV quota:{date}:{agent_id}，超限降级到规则或拒绝
- 缓存：KV llm:cache:{prompt_hash}:{ontology_version}:{model_version}:{temp}:{seed}；
  KV 读写按 §5.5 配对规则（裸字符串用 text()）
- 记录：call_id、model、model_version、temperature、seed、prompt_hash、response_hash、tick
- 输出必须是 ActionProposal（serde），解析失败拒绝并记审计
- LLM_API_URL 为空时拒绝调用并降级规则路径；Workers AI 仅作为外部 API 失败时的降级，且仅 SimpleSocial
- 概念化任务（若实现）：输入是行为模式聚合，输出必须是 ChangeProposal 草案，
  targetLayer 只能 L1 / L2，过 §6.8 流水线，禁止直接写本体
- 附测试：配额超限、缓存命中、非法输出、外部 API 超时降级、上游未配置时降级
```

#### 8.7 生成支付模块

```text
任务：在 payment/provider.rs、payment/manual_wechat.rs、payment/credit.rs、payment/packages.rs 实现支付。
要求：
- PaymentProvider trait：create_order、verify_callback
- ManualWechatProvider：返回收款码（MANUAL_WECHAT_QR_URL 为空则下单 500），等待治理员确认
- 下单只收 package_id：金额与积分由 packages.rs 决定，忽略客户端 amount_cents
- confirm_payment：幂等双拦（见 §5.6），影响行数=1 才加积分
- 写 credit_transactions（append-only）和支付确认事件
- 治理接口需 admin 校验，二次核对金额和订单号
- 订单 30 分钟过期由 Cron 清理
- 禁止直接改 credit_accounts.balance
- 附测试：重复确认余额不变、同一流水号绑另一单 409、订单不存在、订单已过期、金额不匹配、未授权确认、客户端伪造金额被忽略
```

#### 8.8 生成本体版本管理与域注册

```text
任务：在 ontology/version.rs 和 governance/ontology_admin.rs 实现本体版本管理。
要求：
- OntologyRouter：current + previous 两个版本
- publish_version（第一段·提案）：写 R2 ontology/{version}/（不覆盖旧版本），
  D1 落 pending 版本行，不写 KV ontology:version、不进 history
- approve_version（第二段·审批）：pending → active，同批 supersede 旧 active，
  激活本版本 seed 域，追加 KV history + 写 ontology:version；重复审批 409
- rollback：校验目标版本已审批，更新 KV，写回滚事件，状态落 rolled_back
- register_domain：新域先注册后使用；L0 层 / 保留域 403、重复 409、
  非法层 / 路径穿越 400；禁止恢复硬编码文件名白名单
- touches_l0：触及 L0 概念清单一律拒绝并记审计（core.ttl 落地后清单扩为 core 全量）
- analyze_impact：临时加载提案本体，SPARQL 查询现有实例，返回受影响实体
- run_sandbox：隔离 D1 / KV / R2（沙箱绑定），跑 N tick，返回 SandboxResult
- 附测试：版本不存在、重复发布、未审批不生效、重复审批、回滚未审批版本被拒、L0 变更被拒、未注册域被拒
```

### 9. 提交前自检清单

- [ ] 没有用 JSON Schema 约束本体
- [ ] 没有把人类制度模板或行动类型枚举写进 L0 / L1；新增概念过了 §2.2 准入测试或明确落在 L2
- [ ] 新域已走域注册表，没有恢复硬编码文件名白名单
- [ ] LLM 输出经 L2 / L3 校验才执行；概念化输出走 ChangeProposal 流水线
- [ ] 没有常驻循环或每 tick 无条件 LLM
- [ ] D1 写入是批量的
- [ ] SPARQL 是参数化构建
- [ ] IRI 来自 ontology/iri.rs
- [ ] 请求路径无 unwrap() / expect()
- [ ] 事件写 append-only 日志
- [ ] LLM 调用记录了模型版本和 hash
- [ ] 没有预置职业 / 职位 / 领导 / 组织 / 货币等人类模板到开局状态
- [ ] 离线代理不自动同意重大不可逆决策
- [ ] 事件写入时 ontology_version 由 OntologyRouter 注入，非调用方传入
- [ ] LLM 缓存键含 ontology_version
- [ ] 本体发布走两段式（pending → approve），不覆盖旧版本，未审批不生效
- [ ] 回放报告 VersionMismatch
- [ ] 沙箱隔离生产存储
- [ ] L0 变更被显式拒绝并记录
- [ ] 支付确认走 confirm_payment，幂等双拦（重复确认 + 流水号复用）
- [ ] 定价只认服务端 packages.rs，客户端金额被忽略
- [ ] KV 存取按 text() / json() 配对规则，没有混用
- [ ] 构建三坑未复发：reference-types 双配合、main 指向 shim.mjs、版本对齐
- [ ] 权威门禁全绿：cargo check / clippy / test（wasm32 target）+ scripts/smoke.sh
- [ ] 积分 / 社会货币不可兑回法币、不可账号间转让
- [ ] 没有引入 Neo4j 作为主存储，也没有假装 oxigraph / L3 / LLM 上游已接入
- [ ] 附了单元测试覆盖边界

### 10. 已知边界（生成代码时不得掩盖，须在注释 / 文档中如实标注）

- LLM 上游未接：概念化执行者缺位，新概念不会自动生成，只能治理员手动提案。
- L3 本体引擎未接、oxigraph 未引入、Durable Objects WebSocket 为占位。
- `core.ttl` 仍是草案未进构建；五域仍是实现基线（L2 seed 定位）。
- 审批由 admin token 持有者执行，无独立治理身份。
- 仅在 miniflare 本地态验证，未在真实 Cloudflare Paid 账号部署；D1 事务与 Queues 线上语义以真实环境为准。
