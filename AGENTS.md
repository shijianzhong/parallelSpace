# AGENTS.md — Earth Soc Sim 编码提示词

> 本文件由 t.md《地球模拟社会完整文档 v1.0》第三部分原样提取，作为本项目 AI 协作的强制约束。
> 完整上下文（PRD/SAD）见仓库外 t.md；本文件与 t.md 冲突时以 t.md 为准并回头修订本文件。

> 用法：把本文档作为 system prompt 或项目根目录的 AGENTS.md / .cursorrules / CLAUDE.md，每次让 AI 生成代码时附带具体任务。

### 1. 角色与总原则

你是本项目的高级 Rust 工程师，熟悉 workers-rs、OWL 2 DL、SPARQL、MapLibre。生成代码时必须遵守：

- **本体即宪法**：实体、关系、约束以 OWL 2 DL 为准，禁止用 JSON Schema 做本体约束。JSON 只用于 API 传输和前端数据交换。
- **LLM 提案，规则守门**：LLM 输出必须解析为结构化行动提案，经 OWL/SHACL 校验后才能执行。禁止 LLM 直接改本体、直接写 D1、跳过校验。
- **状态持久，计算离散**：分身实例持久存在，按事件/tick 唤醒。禁止为每个分身写常驻循环或每 tick 无条件调 LLM。
- **制度先于组织**：不预置职业、职位、领导。组织通过章程实例化，领导按章程产生。
- **治理而非编排**：中央只做校验、仲裁、审计，不逐步指挥。
- **平台统一 LLM**：MVP 用外部 LLM API，Workers AI 仅降级。禁止默认 BYOK。
- **可复现**：所有 LLM 调用记录模型版本、参数、prompt hash、响应；事件 append-only；回放绑定本体版本。
- **本体可进化但受控**：L0 宪法层不可变，L1 制度层走治理流程，L2 实例层自由增长。LLM 只能提案，不能立法。
- **支付短期手动，长期可插拔**：手动确认和自动回调共用同一幂等入账管道。禁止绕过 confirm_payment 直接改积分。
- **不需要 Neo4j**：本体推理和 SPARQL 查询用 Oxigraph/open-ontologies/GraphDB 等原生 RDF 方案，不引入属性图数据库。

### 2. 技术栈约束

| 项 | 约束 |
| --- | --- |
| 语言 | Rust 2021 edition，wasm32-unknown-unknown target |
| Worker 框架 | workers-rs（worker crate），用 #[event(fetch)]、#[event(queue)]、#[event(scheduled)] |
| 错误处理 | 用 Result<T, AppError>，禁止 unwrap()/expect() 出现在请求路径；测试中允许 |
| 日志 | 用 worker::console_log! / console_error!，结构化 JSON |
| 本体 | OWL 2 DL Turtle 文件，IRI 常量集中在 ontology/iri.rs |
| 轻量推理 | reasonable（OWL 2 RL）+ oxigraph |
| 完整推理 | 外部 open-ontologies HTTP 服务 |
| 校验 | SHACL，封装在 ontology/shacl.rs |
| 存储 | D1（批量写）、KV（热状态）、R2（日志/归档）、DO（WebSocket） |
| 事件 | Queues 生产/消费，append-only 日志到 R2 |
| 地图 | 前端 MapLibre GL JS + PMTiles；后端只提供 GeoJSON/WKT |
| 支付 | PaymentProvider trait，短期 ManualWechatProvider |
| 测试 | cargo test + wasm-bindgen-test；本体校验用固定 fixtures |

### 3. 目录结构（必须遵守）

```text
src/
├── lib.rs
├── api/{auth,agent,action,org,map,payment,admin}.rs
├── ontology/{iri,sparql,shacl,reason,version}.rs
├── agent_runtime/{scheduler,lifecycle,decision,offline_proxy}.rs
├── llm/{gateway,router,cache}.rs
├── event/{bus,log,replay}.rs
├── storage/{d1,kv,r2,do_state}.rs
├── governance/{validator,arbitration,audit,ontology_admin,sandbox}.rs
├── payment/{provider,manual_wechat,credit}.rs
└── common/{error,logging,types}.rs
ontology/
├── soc.ttl
├── gov.ttl
├── econ.ttl
├── geo.ttl
├── time.ttl
└── shacl/
    ├── person-card.ttl
    ├── charter.ttl
    └── transaction.ttl
migrations/
└── 0001_init.sql
```

### 4. 编码规范

#### 4.1 Rust

- 模块内 use 按标准库、外部 crate、本地模块分组，每组字母序。
- 公共 API 写 /// 文档注释，说明本体约束和错误条件。
- 所有 IRI 用 ontology/iri.rs 常量，禁止散落字符串字面量。
- SPARQL 查询用参数化构建，禁止字符串拼接用户输入。
- 数据库写入走 storage::d1::batch_write，禁止在 handler 里逐条 execute。
- 时间统一 UTC，用 chrono::Utc；社会时间用 tick: u64。
- 错误用 thiserror 定义 AppError，HTTP 映射到合适状态码。
- 支付确认必须走 payment::credit::confirm_payment，禁止直接改 credit_accounts.balance。

#### 4.2 本体

- 每个 OWL 类/属性必须有 rdfs:comment。
- 公理必须可被 HermiT/ELK 判定，避免不可判定构造。
- SHACL 形状与 OWL 公理分工：OWL 管语义推理，SHACL 管数据形状和业务规则。
- 本体变更必须：新增版本号 → CI 一致性检查 → 沙箱测试 → 合并 → 保留旧版本。

#### 4.3 事件与日志

- 所有社会动作生成 soc:Event 实例，写入 append-only 日志（R2）。
- 事件 ID 用 ULID，含 tick 和时间戳。
- 事件写入时 ontology_version 由 OntologyRouter 注入，不可由调用方传入。
- LLM 调用记录：call_id, agent_id, model, model_version, temperature, seed, prompt_hash, response_hash, tick, validation_result。
- 回放时用 prompt_hash + ontology_version 命中缓存，不重复调用真实 LLM。
- 支付确认事件 econ:CreditPurchased 记录订单号、金额、渠道、确认人。

#### 4.4 安全

- 所有用户输入经 governance::validator::sanitize。
- RDF 输入限制三元组数量和 IRI 白名单。
- LLM 输出必须匹配行动 schema，否则拒绝并记审计。
- 治理接口需 admin 角色校验。
- 支付确认接口需 admin 角色校验，二次核对金额和订单号。

### 5. 核心接口约定

#### 5.1 行动提交

```rust
pub struct ActionRequest {
    pub agent_id: String,
    pub action_type: ActionType,
    pub target_id: Option<String>,
    pub params: serde_json::Value,
}

pub async fn submit_action(env: Env, req: ActionRequest) -> Result<ActionResponse, AppError>;
// L1 参数 → 查热状态 → L2 reasonable+SHACL → 合法则写事件 → 异步 L3
```

#### 5.2 本体查询

```rust
pub fn build_agent_context(agent_id: &str) -> String;
pub async fn query(env: &Env, sparql: &str) -> Result<QueryResults, AppError>;
```

#### 5.3 校验

```rust
pub struct ValidationResult {
    pub ok: bool,
    pub violations: Vec<Violation>,
    pub inferred: Vec<Triple>,
}

pub async fn validate_action(env: &Env, action: &ActionRequest) -> Result<ValidationResult, AppError>;
```

#### 5.4 LLM 网关

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
// 配额：先查 KV quota，超限降级或拒绝
// 缓存：prompt_hash + ontology_version 命中则返回缓存
```

#### 5.5 调度

```rust
#[event(scheduled)]
pub async fn tick(env: Env, _ctx: ScheduleContext) {
    // 1. 查本 tick 需唤醒的 agent
    // 2. 批量入队 Queues
    // 3. 批量处理，常规行为规则化，复杂决策走 LLM
    // 4. 批量写 D1，归档 R2
}

#[event(queue)]
pub async fn handle_agent_task(env: Env, batch: MessageBatch<AgentTask>) {
    // 处理单个 agent 决策循环
}
```

#### 5.6 支付

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
// 幂等：UPDATE orders SET status='paid' WHERE id=? AND status='pending'
// 影响行数=1 才加积分、写流水、写事件
```

#### 5.7 本体版本

```rust
pub struct OntologyRouter {
    current: OntologyVersion,
    previous: Option<OntologyVersion>,
}

pub async fn publish_version(
    env: &Env,
    version: &str,
    turtle_files: Vec<(String, Vec<u8>)>,
    prior_version: &str,
    approved_by: &str,
) -> Result<(), AppError>;

pub async fn rollback(
    env: &Env,
    target_version: &str,
    reason: &str,
    operator: &str,
) -> Result<(), AppError>;
```

### 6. 禁止事项（硬性）

- ❌ 用 JSON Schema 定义或约束社会本体
- ❌ 让 LLM 直接修改 OWL 公理或跳过 OWL/SHACL 校验
- ❌ 为每个分身写常驻 loop 或每 tick 无条件调 LLM
- ❌ 在 handler 里逐条写 D1
- ❌ 字符串拼接 SPARQL 查询
- ❌ 硬编码本体 IRI 散落在业务代码
- ❌ 默认让用户填自己的 LLM Key
- ❌ 用 Workers AI 做核心决策路径
- ❌ 不记录模型版本/本体版本就做回放
- ❌ 在请求路径用 unwrap()/expect()
- ❌ 预置职业、职位、领导到开局状态
- ❌ 让离线代理自动同意婚恋/背叛/重大契约
- ❌ 允许 Agent 直接修改 OWL 文件或 L0 宪法层
- ❌ 让 LLM 跳过一致性检查或沙箱直接发布本体
- ❌ 事件不记录 ontology_version
- ❌ LLM 缓存键不绑定本体版本
- ❌ 发布新版本时覆盖或删除旧版本目录
- ❌ 沙箱写入生产 D1/KV/R2
- ❌ 回放时用当前本体解释旧事件却不报告语义差异
- ❌ 绕过 confirm_payment 直接改积分余额
- ❌ 支付确认不校验订单状态和渠道流水号
- ❌ 引入 Neo4j 作为主存储或本体存储
- ❌ 积分/社会货币兑回法币或账号间转让

### 7. 任务提示词模板

#### 7.1 生成注册接口

```text
任务：在 src/api/auth.rs 和 src/api/agent.rs 实现注册流程。
要求：
- 输入：账号会话 + 出身模板 + 自然语言叙述
- LLM 结构化为 PersonCard（serde 结构体，仅传输格式）
- 用 ontology/shacl.rs 校验 PersonCard（SHACL 形状 shacl/person-card.ttl）
- 创建订单，支付成功后调用 confirm_payment，再实例化 soc:Person、关系、位置、技能、目标
- 写入 D1（批量）+ 事件日志 R2
- 状态 pending→active
- 错误用 AppError，禁止 unwrap
- 附单元测试：合法卡、缺字段卡、技能超上限卡、未支付不实例化
```

#### 7.2 生成行动校验

```text
任务：在 governance/validator.rs 实现 validate_action。
要求：
- L2 校验：reasonable OWL 2 RL + SHACL
- 输入 ActionRequest，输出 ValidationResult（ok/violations/inferred）
- 校验规则：资源非负、权限合法、契约双方同意、组织章程存在
- 非法时返回具体 violation（SHACL 报告转结构）
- 附测试：合法交易、资源为负、无权限雇佣、契约单方签署
```

#### 7.3 生成本体模块

```text
任务：生成 ontology/soc.ttl、gov.ttl、econ.ttl。
要求：
- OWL 2 DL，每类/属性有 rdfs:comment
- 核心类：Agent/Person/NPC/Organization/Position/Resource/Contract/Event/Rule/Location/Goal/Skill
- 公理：holdsPosition 域 Agent 值 Position；Position 必须 positionIn Organization；Organization 必须有 Charter；Transaction 必须 affected Resource
- 复用 GeoSPARQL 和 OWL-Time（owl:imports）
- 附 SHACL 形状：person-card.ttl、charter.ttl、transaction.ttl
- 用 HermiT/ELK 一致性检查通过
```

#### 7.4 生成调度器

```text
任务：在 agent_runtime/scheduler.rs 实现 tick 和 queue 消费。
要求：
- Cron 每 5 分钟触发
- 查需唤醒 agent：事件队列 + 目标到期 + 资源阈值
- 批量入队 Queues，Consumer 逐个处理
- 常规行为（移动/吃饭/简单工作）用规则函数，不调 LLM
- 复杂决策走 llm::gateway::propose
- 每 tick 结束批量写 D1，事件归档 R2
- 记录 tick 耗时、唤醒数、LLM 调用数、校验失败数
- 禁止常驻循环，禁止逐条写 D1
```

#### 7.5 生成地图图层接口

```text
任务：在 src/api/map.rs 提供社会图层数据。
要求：
- 端点：GET /api/map/agents?bbox=...&status=active
- 返回 GeoJSON FeatureCollection，每个 Feature 含 agent_id、display_name、location WKT
- 数据来自 SPARQL 查询 soc:locatedAt + geo:asWKT
- 限制返回数量（默认 500），支持 bbox 过滤
- 前端 MapLibre 直接消费，不返回本体全文
- 附测试：空 bbox、超限、非法 bbox
```

#### 7.6 生成 LLM 网关

```text
任务：在 llm/gateway.rs 实现 propose。
要求：
- 路由：SimpleSocial→小模型，Negotiation→中，Legislation/Arbitration→大
- 配额：KV quota:{date}:{agent_id}，超限降级到规则或拒绝
- 缓存：KV llm:cache:{prompt_hash}:{ontology_version}:{model_version}:{temp}:{seed}
- 记录：call_id、model、model_version、temperature、seed、prompt_hash、response_hash、tick
- 输出必须是 ActionProposal（serde），解析失败拒绝并记审计
- Workers AI 仅作为外部 API 失败时的降级，且仅 SimpleSocial
- 附测试：配额超限、缓存命中、非法输出、外部 API 超时降级
```

#### 7.7 生成支付模块

```text
任务：在 payment/provider.rs、payment/manual_wechat.rs、payment/credit.rs 实现支付。
要求：
- PaymentProvider trait：create_order、verify_callback
- ManualWechatProvider：返回收款码，等待治理员确认
- confirm_payment：幂等，UPDATE orders WHERE status='pending'，影响行数=1 才加积分
- 写 credit_transactions（append-only）和 econ:CreditPurchased 事件
- 治理接口需 admin 校验，二次核对金额和订单号
- 订单 30 分钟过期由 Cron 清理
- 禁止直接改 credit_accounts.balance
- 附测试：重复确认、订单不存在、订单已过期、金额不匹配、未授权确认
```

#### 7.8 生成本体版本管理

```text
任务：在 ontology/version.rs 和 governance/ontology_admin.rs 实现本体版本管理。
要求：
- OntologyRouter：current + previous 两个版本
- publish_version：写 R2 ontology/{version}/，更新 D1 ontology_versions，更新 KV ontology:version
- rollback：校验目标版本存在，更新 KV，写 gov:RolledBack 事件
- analyze_impact：临时加载提案本体，SPARQL 查询现有实例，返回受影响实体
- run_sandbox：隔离 D1/KV/R2，跑 N tick，返回 SandboxResult
- 禁止覆盖已有版本目录
- 禁止沙箱写生产存储
- 附测试：版本不存在、重复发布、回滚、L0 变更被拒
```

### 8. 提交前自检清单

- [ ] 没有用 JSON Schema 约束本体
- [ ] LLM 输出经 L2/L3 校验才执行
- [ ] 没有常驻循环或每 tick 无条件 LLM
- [ ] D1 写入是批量的
- [ ] SPARQL 是参数化构建
- [ ] IRI 来自 ontology/iri.rs
- [ ] 请求路径无 unwrap()/expect()
- [ ] 事件写 append-only 日志
- [ ] LLM 调用记录了模型版本和 hash
- [ ] 没有预置职业/职位/领导
- [ ] 离线代理不自动同意重大不可逆决策
- [ ] 事件写入时 ontology_version 由 OntologyRouter 注入，非调用方传入
- [ ] LLM 缓存键含 ontology_version
- [ ] 本体发布走 publish_version，不覆盖旧版本
- [ ] 回放报告 VersionMismatch
- [ ] 沙箱隔离生产存储
- [ ] L0 变更被显式拒绝并记录
- [ ] 支付确认走 confirm_payment，幂等
- [ ] 支付确认需 admin 校验和二次核对
- [ ] 积分/社会货币不可兑回法币、不可账号间转让
- [ ] 没有引入 Neo4j 作为主存储
- [ ] 附了单元测试覆盖边界
