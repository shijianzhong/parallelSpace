# parallelSpace 缺失梳理与细化实现方案

> 审计日期：2026-10-23。审计方法：全仓 stub/TODO 扫描 + 调用图核查（每个 pub 函数找真实调用方）+ 路由表与模块对照 + smoke 覆盖对照。
> 结论先行：脚手架的「已实现」部分是真的，但有 4 处属于**链路断裂**（代码存在、从未接线），不补上，世界永远跑不起来。本轮（Phase 1）修这 4 处；其余按 Phase 2 / 3 排期。
> **Phase 1 执行状态（2026-10-23）：F1–F4 已全部实现（见第二节），单测与 smoke 验收结果见仓库提交说明；README「当前已实现 / 已知边界」已同步。**

## 一、缺失总表

### P0 — 链路断裂（本轮修）

| # | 缺失 | 证据 | 影响 |
| --- | --- | --- | --- |
| G1 | **分身激活链路不存在**：支付成功后没有任何代码迁移 agent 状态；全仓没有一条 `UPDATE agents SET status` | `lifecycle::ensure_transition` 只有单测调用；`payment/credit.rs::confirm_payment` 只入账、从不碰 agents；`d1.rs` 无状态迁移函数 | 注册→付钱→积分到账，但分身永远停在 `created`；调度只唤醒 active/idle，地图默认查 active——**世界从第一步就冻死** |
| G2 | **LLM 网关三连坏**：① 配了 `LLM_API_URL` 也返回写死的 `idle` 占位提案，且把占位提案**写进缓存 7 天**（缓存毒化）；② 配额只查不扣（`add_quota_used` 是死代码），配额永远为 0、永不触发；③ 缓存读用 `.json::<String>()` 读裸字符串写入的值，正是 README「三个坑」第 3 条和 `kv.rs` 注释明说会抛错的配对方式 | `llm/gateway.rs:106-145`、`llm/cache.rs:39`、`storage/kv.rs:59-71` 无调用方 | 任何未来的 LLM 接入都会先踩缓存毒化与配额失效；缓存命中路径大概率直接报错 |
| G3 | **调度唤醒与注释不符**：`find_wakeable_agents` 注释写唤醒 (a) 本 tick 有事件的 actor + (b) 低能量 active 分身，SQL 只实现了 (a)；且 `agent_state` D1 表是孤儿表（无人读写，热状态只在 KV） | `storage/d1.rs:292-308`；全仓 `agent_state` 仅出现在建表 SQL 与一句注释 | 没有用户行动就没有事件、没有事件就永不唤醒——自举死锁；低能量分身永远得不到恢复 tick |
| G6a | **L0 保护清单未扩容**：`touches_l0` 只有 4 个旧 IRI；`iri.rs` 没有 `core:` 命名空间常量，`ALLOWED_NAMESPACES` 不含 core 命名空间 | `governance/ontology_admin.rs:37-44`、`ontology/iri.rs` | core.ttl 的 9 个原语（Agent/Event/Action/Relation…的 core IRI）变更不会被 L0 闸门拦住；携带 core IRI 的 RDF 负载会被白名单拒绝。AGENTS.md v1.1 已把「扩为 core 全量」写为落地时的硬要求 |

### P1 — 功能缺位（Phase 2）

| # | 缺失 | 证据 | 方案要点 |
| --- | --- | --- | --- |
| G4 | **四个模块是死代码**：`sandbox::run_sandbox`、`arbitration::file_case`、`event::replay`、`offline_proxy::decide` 均无任何调用方、无路由；AGENTS 模板里承诺的 `analyze_impact` 根本不存在 | 调用图核查（仅 mod.rs re-export） | 逐个接线：sandbox/arbitration/replay 挂 admin 路由；offline_proxy 接入 action 提交路径（离线状态 agent 的不可逆动作 → Defer/Deny）；analyze_impact 新写（提案本体临时装载 + SPARQL 查受影响实例数） |
| G5 | **PersonCard 链路缺失**：注册的 `narrative` 消毒后直接丢弃；agents 表无 card 字段；LLM 结构化 PersonCard 是 TODO(M2) | `api/auth.rs:6,37-44`；`migrations/0001` agents 表 | migration 0003 加 `person_card_rdf TEXT`；注册可选带 card 并先过 SHACL 才落库；激活接口（本轮新增）优先用已存 card。LLM 结构化等 G12 上游定版后接 |
| G9 | **沙箱是假的**：`run_sandbox` 只校验绑定名存在，就无条件返回 `passed: true`，不装载提案本体、不跑 tick | `governance/sandbox.rs:34-55` | 提案 Turtle 装入沙箱 R2 → 用 reasonable 跑一致性 + SHACL 全量形状 → 结果如实返回；ticks 参数在真实仿真接入前从结果里移除，不要假报 |
| G7 | **没有账号认证**：注册只有 email、无口令/会话；`GET /api/agent/:id`、`POST /api/action`、`POST /api/org` 全部无鉴权，任何人可冒充任何 agent_id 提交行动 | `api/auth.rs`、`api/agent.rs`、`api/action.rs` | 会话令牌（注册/登录发 token，KV 存会话）；action/org 校验 token 与 agent 归属；与现有 admin token 体系分开。属安全项，Phase 2 第一位。**登录方案已定版（2026-10-23 用户拍板）**：治理后台走 Cloudflare Access（Google/GitHub + 邮箱白名单，零开发、随 G11 部署配置）；C 端三通道——邮箱验证码首发 + Google/GitHub OAuth；微信登录待开放平台资质排二期。详见文末 G7 细化设计 |
| G8 | **地图是空壳**：Feature 的 `geometry` 恒为 `null`；bbox 只解析不下推过滤；无 locations 数据（agents.location_id 从未被写过）；前端目录不存在，MapLibre/PMTiles 未开始；DO 的 `/ws` 只 accept 后挂起，且 lib.rs 根本没有把请求转发到 DO 的路由，WebSocket 从外部不可达 | `api/map.rs:80-95`、`storage/do_state.rs:31`、`lib.rs` 路由表 | 先补位置数据模型（migration + 激活时写入 location）；map 输出真实 Point 几何并下推 bbox；lib.rs 加 `/api/ws` 转发到 DO；DO 内用 WebSocket hibernation 广播事件；前端单页（MapLibre CDN）放 Phase 3 |

### P2 — 外部依赖 / 运营（Phase 3，需用户拍板或提供资源）

| # | 缺失 | 说明 |
| --- | --- | --- |
| G11 | **真实 Cloudflare 部署未验证**：wrangler.toml 的 D1/KV id 全是零占位；D1 batch 内唯一索引冲突、Queues 线上语义以真实环境为准（README 自述） | 需真实 Paid 账号：建 D1/KV/R2/Queues → 回填 id → wrangler deploy → 线上跑 smoke |
| G12 | **LLM 真实上游未定版**：模型名是占位符（platform-small/medium/large），无 provider、无密钥 | 需用户定 provider 与模型映射；接入后 gateway 的 HTTP 调用、超时、重试、成本记录一并落地；之后才能做「LLM 概念化最小闭环」 |
| G13 | **治理身份不存在**：`approved_by` 是请求体里的自由文本，admin 是全仓共用的一把 token | 治理员身份表 + 审批签名记录；与 G7 会话体系合并设计 |
| G14 | **SPARQL 只建串不执行**：`ontology/sparql.rs` 只生成参数化查询字符串，全仓没有执行引擎（oxigraph 未引入，README 自述 M2 再评估） | 实例层全量入 RDF 后再评估 oxigraph 体积/性能；在此之前 SPARQL 模板仅作契约测试，不要在文档里写成已可用能力 |

## 二、Phase 1 细化方案（本轮实现）

### F1 激活链路（G1）

状态机不变：`created → pending → active`（`created → active` 直跳仍是硬禁）。

1. `storage/d1.rs` 新增：
   - `transition_agent_status(db, agent_id, from, to) -> AppResult<bool>`：条件更新 `UPDATE agents SET status=? WHERE id=? AND status=?`，走 batch + meta.changes 判 1 行（与 `mark_order_paid` 同款幂等写法）。
   - `get_agent_by_account(db, account_id)`、`account_has_paid_order(db, account_id)`。
2. `payment/credit.rs::confirm_payment`：仅在**首次**入账成功（条件更新命中、非 already_confirmed 路径）后，把订单账号名下 `created` 分身推进到 `pending`；推不动（已 pending/active）静默跳过并记日志。重放确认不重复推进。
3. `api/agent.rs` 新增 `POST /api/agent/:id/activate`（治理员执行，复用 admin token 校验——把 `api/admin.rs::check_admin` 提为 `pub(crate)` 共用）：
   - 入参：`{ "person_card_rdf": "<Turtle>" }`；
   - 前置：agent 存在（404）→ 状态须为 `pending`（否则 409，并给出当前状态）→ 账号须有 paid 订单（否则 409）；
   - 校验：`ontology/shacl.rs` 新增 `validate_person_card(rdf)`（与 `validate_local` 共用内层实现；形状按 targetClass 自动命中 PersonCardShape），违规 → 422 + violations 明细；
   - 落地：条件迁移 `pending → active`（并发下只有一个成功，另一个得 409）；写 `AgentActivated` 事件（版本由 OntologyRouter 注入）+ 审计。
4. 路由：`lib.rs` 注册上述端点。
5. 测试：单测（validate_person_card 合法/非法卡、迁移表不变）；smoke 新增第 11 节：确认入账后状态 = pending → 无 token 激活 401 → 非 pending 激活 409（用新注册未付费分身）→ 坏卡 422 → 好卡激活 active → 重复激活 409。smoke 总断言 50 → 57。

### F2 LLM 网关修复（G2）

1. `llm/cache.rs::get_cached` 改 `.text()` 读取（与 `put(String)` 配对，遵循 kv.rs 既定规则），解析在调用方做。
2. `llm/gateway.rs::propose`：
   - 上游未真实接入前，占位提案**不写缓存**（删除毒化路径，加注释说明原因）；
   - 非缓存路径执行后调用 `kv::add_quota_used` 按 `max_tokens` 预扣配额，让配额检查真实生效；
   - llm_call 日志补 `validation_result` 字段（AGENTS §5.3 清单原有要求）。

### F3 唤醒集合补全（G3）

`find_wakeable_agents` 改为并集去重（单条 SQL `UNION`，LIMIT 500）：
(a) 本 tick 有事件的 actor；(b) 状态为 `active / idle` 的分身（周期心跳，保证世界不冻结）。
注释同步改实：低能量规则依赖 KV 热状态、D1 侧不可见，删除原注释中未实现的 (b) 表述，改为心跳语义并注明。

### F4 L0 清单扩容（G6a）

1. `ontology/iri.rs`：新增 `CORE_NS = "http://parallelspace.org/ontology/core#"` 与 `core()` 辅助函数，加入 `ALLOWED_NAMESPACES`。
2. `governance/ontology_admin.rs::touches_l0`：清单 = 旧 4 项（向后兼容，smoke 在用）+ core 全量：9 原语类 + 全部 core 属性（tick/duration/locatedAt/actor/underOntologyVersion/involves/from/to/about/proposedBy/targetLayer/proposalStatus/appliesFromVersion/layer/amendable/Thing）。
3. 单测补：core IRI 命中、普通 L2 概念不命中。

### 验收（Phase 1）

- `cargo check / clippy --target wasm32-unknown-unknown --all-targets` 0 warning；
- `cargo test --target wasm32-unknown-unknown` 全绿（原 85 + 新增）；
- `scripts/smoke.sh` 全绿（原 50 + 新增 7 = 57）；
- README「当前已实现 / 已知边界」与 AGENTS §10 同步更新，不留过期表述。

## 三、排期与依赖

| Phase | 内容 | 前置 |
| --- | --- | --- |
| 1（本轮） | F1 激活链路、F2 网关修复、F3 唤醒并集、F4 L0 扩容 | 无 |
| 2 | G7 认证会话 → G4 死代码接线 → G5 PersonCard 落库 → G9 真沙箱 → G8 位置与地图几何/WS 路由 | G7 先行（其余接口的鉴权都依赖它） |
| 3 | G11 真实部署、G12 LLM 上游（含概念化闭环）、G13 治理身份、G14 oxigraph 评估、前端 MapLibre | 需用户提供 Cloudflare 账号 / LLM provider 决策 |

## 五、2026-10-23 定调变更：积分可转让 / 可兑换

- **背景**：用户定调 agent 应在虚拟社会中经济自立，其社会收益应能回流到所属用户，故推翻原型期「积分不可兑回法币、不可账号间转让」的旧禁令（原 AGENTS 禁止事项与清单条目）。
- **新规**（AGENTS §5.7 为准）：转让经 `transfer_credits`（条件扣减 + 成对流水 + 总量守恒）；agent 收益经 `agent_earnings` 结算入所属账号；兑换经 redeem 流程（申请冻结 → admin 审批 → 人工打款 settle / 驳回释放）；兑换价单点定义于 `packages.rs`，低于全部套餐单位买入价，杜绝套利回路。
- **MVP 边界**：无用户认证（G7）前，转让 / 收益入账 / 兑换均为 admin token 代办接口；用户自助操作待 G7 上线。真实运营前需另行处理支付牌照、反洗钱 / KYC 与税务事项，代码兜底不替代合规。
- **新增缺口（本日讨论产出，编号续排）**：G15 用户决策收件箱——出生设定（PersonCard）+ 重大不可逆事项审批，即 offline_proxy 的 DeferToOwner 落地（提案表 + 查询/审批接口）；G16 agent 经济闭环——Work/Trade 结算进 society money（agent_state/KV 热状态）、消费 sink、破产联动 dormant、收益自动结算到账号（本次已先落账本侧的 agent_earnings 入账函数）。两者排入 Phase 2 后段。

- **已落地（同日）**：账本层三件套已实现并提交——`transfer_credits`（条件扣减 + 成对流水）、`credit_agent_earnings`（收益入账）、redeem 流程（migration 0003 + 申请冻结/审批 settle/驳回释放），接口均为 admin 代办；兑换价 1 积分 = 1 分（packages.rs 单点定义，单测钉住低于全部套餐单位买入价）。验收：单测 89 passed、smoke 67 通过 / 0 失败（新增第 12 节 10 项）。

## 六、G7 细化设计（登录方案定版，2026-10-23）

**分工**：Cloudflare Access 只管治理后台（单独子域 + Access 应用，Worker 验 `Cf-Access-Jwt-Assertion` 与邮箱白名单；现有 admin token 保留为应急通道）；C 端登录在应用内实现，不走 Access（席位收费不适合 C 端规模）。

**账号模型**：`accounts` 保持邮箱唯一键；新增 `user_identities(account_id, provider, provider_subject, email)`，`UNIQUE(provider, provider_subject)`，一个账号可绑多个登录方式，同邮箱自动关联到同一账号。会话存 KV：`session:{token}` → account_id，token 为 256 位随机值，经 HttpOnly + Secure + SameSite=Lax Cookie 下发，TTL 30 天滑动续期；API 客户端可用 `Authorization: Bearer` 同 token。

**通道一·邮箱验证码（首发）**：`POST /api/auth/email/start {email}` 生成 6 位码，KV 存哈希（`otp:{email}`，TTL 10 分钟、最多试 5 次、发送间隔 60 秒）；`POST /api/auth/email/verify {email, code}` 通过即建/找账号并下发会话。发信通道需定一家（Resend / AWS SES / Cloudflare Email Sending），属部署期决策。

**通道二·Google / GitHub OAuth**：`GET /api/auth/oauth/:provider/start` 生成 state（KV，10 分钟）并 302 到授权页；callback 换 token 取 `sub` + email，upsert identity 后下发会话。密钥 `GOOGLE_CLIENT_ID/SECRET`、`GITHUB_CLIENT_ID/SECRET` 走 worker secret；GitHub 邮箱可能不公开，必须调 `/user/emails` 取已验证邮箱。

**注册流程改造**：现 `POST /api/auth/register`（邮箱直建账号+分身）拆为两步——先登录（建账号壳），再 `POST /api/agent`（需会话）提交出身叙述建分身；旧 register 仅保留给 smoke 并标注内部用途，试运行前关闭其公网路由。

**鉴权落点**：action / org / agent 激活后的自助操作（转让、兑换、收件箱审批）一律校验「会话账号 == 分身 account_id」；治理类（publish/approve/confirm/agent-earnings 代办）仍走 admin token / Access，两套体系互不替代。G13 的治理身份记录可直接复用 Access 签发的邮箱身份，`approved_by` 改为服务端从 JWT 取，不再收请求体自由文本。

## 七、G7 落地记录（2026-10-23）

- **已实现**：migration 0004 `user_identities`；邮箱 OTP（Resend 发信 / 未配密钥时日志模式，dev 回显需 `ALLOW_DEV_OTP_ECHO=true` 且仅限本地）；KV 会话（Cookie + Bearer，30 天）；Google/GitHub OAuth 全流程（state 防 CSRF、已验证邮箱、同邮箱关联）；注册拆分（`POST /api/agent` 需会话建分身，旧 register 保留为内部兼容）；`GET /api/me`；自助转让/兑换（`/api/me/transfer`、`/api/me/redeem`，复用 §5.7 账本函数，出账方固定为会话账号）；action 归属校验（过渡模式 + `REQUIRE_AUTH` 开关）。
- **验收**：smoke 新增第 13 节 11 项，全量 **78 通过 / 0 失败**；单测 89 不变；clippy 0 warning。
- **遗留**：OAuth 未用真实 client id/secret 端到端验证（需部署时配置）；org 创建接口没有创始人字段，归属校验暂无落点，随 G4 接线时补 founder 绑定；`REQUIRE_AUTH` 默认关，试运行部署清单里必须打开并关闭旧 register 公网路由；Access 侧配置随 G11 部署执行。
