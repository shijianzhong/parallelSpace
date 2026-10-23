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
| G4 | **四个模块是死代码**：`sandbox::run_sandbox`、`arbitration::file_case`、`event::replay`、`offline_proxy::decide` 均无任何调用方、无路由；AGENTS 模板里承诺的 `analyze_impact` 根本不存在 | 调用图核查（仅 mod.rs re-export） | 逐个接线：sandbox/arbitration/replay 挂 admin 路由；offline_proxy 接入 action 提交路径（离线状态 agent 的不可逆动作 → Defer/Deny）；analyze_impact 新写（提案本体临时装载 + SPARQL 查受影响实例数） | **已落地（2026-10-23，见第九节）：影响分析计数为结构化存储近似口径（完整 SPARQL 待 G14）；Defer 的 PendingDecision 已随 G15 于同日拍板并落地（见第十三/十四节）** |
| G5 | **PersonCard 链路缺失**：注册的 `narrative` 消毒后直接丢弃；agents 表无 card 字段；LLM 结构化 PersonCard 是 TODO(M2) | `api/auth.rs:6,37-44`；`migrations/0001` agents 表 | migration 0003 加 `person_card_rdf TEXT`；注册可选带 card 并先过 SHACL 才落库；激活接口（本轮新增）优先用已存 card。LLM 结构化等 G12 上游定版后接。**已落地（2026-10-23，见第八节）：编号顺延为 migration 0005（0003/0004 已被 redeem/identities 占用）** |
| G9 | **沙箱是假的**：`run_sandbox` 只校验绑定名存在，就无条件返回 `passed: true`，不装载提案本体、不跑 tick | `governance/sandbox.rs:34-55` | 提案 Turtle 装入沙箱 R2 → 用 reasonable 跑一致性 + SHACL 全量形状 → 结果如实返回；ticks 参数在真实仿真接入前从结果里移除，不要假报 |
| G7 | **没有账号认证**：注册只有 email、无口令/会话；`GET /api/agent/:id`、`POST /api/action`、`POST /api/org` 全部无鉴权，任何人可冒充任何 agent_id 提交行动 | `api/auth.rs`、`api/agent.rs`、`api/action.rs` | 会话令牌（注册/登录发 token，KV 存会话）；action/org 校验 token 与 agent 归属；与现有 admin token 体系分开。属安全项，Phase 2 第一位。**登录方案已定版（2026-10-23 用户拍板）**：治理后台走 Cloudflare Access（Google/GitHub + 邮箱白名单，零开发、随 G11 部署配置）；C 端三通道——邮箱验证码首发 + Google/GitHub OAuth；微信登录待开放平台资质排二期。详见文末 G7 细化设计 |
| G8 | **地图是空壳**：Feature 的 `geometry` 恒为 `null`；bbox 只解析不下推过滤；无 locations 数据（agents.location_id 从未被写过）；前端目录不存在，MapLibre/PMTiles 未开始；DO 的 `/ws` 只 accept 后挂起，且 lib.rs 根本没有把请求转发到 DO 的路由，WebSocket 从外部不可达 | `api/map.rs:80-95`、`storage/do_state.rs:31`、`lib.rs` 路由表 | 先补位置数据模型（migration + 激活时写入 location）；map 输出真实 Point 几何并下推 bbox；lib.rs 加 `/api/ws` 转发到 DO；DO 内用 WebSocket hibernation 广播事件；前端单页（MapLibre CDN）放 Phase 3。**已落地（2026-10-23，见第十一节）：migration 编号顺延为 0007；WS 改由事件落盘后统一广播（event::log::append 钩子），覆盖全部事件类型而非仅移动** |

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

## 八、G5 落地记录（2026-10-23）

- **已实现**：migration 0005 给 agents 加 `person_card_rdf TEXT`（文档原写 0003，0003/0004 已被 redeem / identities 占用，顺延）；`POST /api/agent`（需会话）可选携带 `person_card_rdf`，先过 `validate_person_card` SHACL 才落库，违规 → 422 + violations 且不建分身，响应带 `person_card_stored`；不带 card 保持兼容（旧 `/api/auth/register` 不变）。激活接口入参改可选：请求未带（或空白）时用已存 card，两者都无保持原 400 语义；请求带卡仍优先于已存卡。读路径：`GET /api/agent/:id` 与 `GET /api/me` 均带出 `person_card_rdf`。
- **验收**：smoke 新增第 14 节 10 项（坏卡注册 422 且不建分身、无卡兼容、带卡落库+查询带出、付费推进 pending、空体激活用存量卡 → active、无存量卡不带卡激活 400），全量 **88 通过 / 0 失败**；单测 89 不变；clippy 0 warning。
- **遗留**：LLM 结构化生成 PersonCard 仍为 TODO(M2)，等 G12 上游定版后接，本次未假接；`narrative` 本身仍只作消毒校验、不落库（落库的是 card），如需叙述留痕另立条目。

## 九、G4 落地记录（2026-10-23）

- **已实现**：
  - 三个死代码模块挂 admin 路由（沿用 `check_admin`）：`POST /api/admin/sandbox/run` → `sandbox::run_sandbox`；`POST /api/admin/arbitration/cases` → `arbitration::file_case`；`GET /api/admin/replay?from_tick=&to_tick=` → `event::replay`（`ReplayReport` / `EventRecordLite` 补 Serialize 派生，签名未动）。
  - `analyze_impact` 新写：`GET /api/admin/ontology/versions/:version/impact` —— 从 R2 取回该版本提案 Turtle → `Owl2RlReasoner` 临时装载（内嵌基线本体 + 提案一轮 OWL 2 RL）报一致性 → `proposal_concepts` 提取提案定义/修改的概念（作主语出现的具名 IRI）→ 按类统计现存实例。**近似点如实标注**：实例层未全量入 RDF、无 SPARQL 执行引擎（G14 未引入 oxigraph），计数走结构化存储——Agent（含 core:Agent）/ Organization / Position 查对应表，其余概念查 `events_index` 事件类型计数；未被索引的概念计 0（当前无可计数实例，非语义无影响）；仅作宾语被引用的概念不在清单内。报告 `notes` 字段同步披露此口径。
  - `offline_proxy::decide` 接入 `POST /api/action`：位置在 L2 SHACL 校验通过后、落事件前（不改变既有 violations 拒绝语义）。所有者在线（请求会话账号 == 分身归属账号）时代理规则不介入；不在场时 Allow（Move/Work/Learn/Chat）放行，DeferToOwner（Trade/Ally/Employ/Vote/Legislate）与 Deny（Breach）一律拒绝提交 + 写审计（`action.proxy_deferred` / `action.proxy_denied`）+ 响应 `accepted:false` 与机器可读 `proxy_decision`。**边界**：不建 PendingDecision 表、不建决策收件箱/审批接口（G15 未经用户拍板写入）——Defer 本次语义是「拒绝并留痕」而非「排队待批」；`contract_class_deferred`（marriage/major_contract/loan）仍无 HTTP 调用面（现无接口接收契约类别参数，org 创建只有 charter_id），保持库级函数待 G15 一并设计。
  - 顺带补 G7 遗留 org 创始人绑定：migration 0006 给 organizations 加 `founder_account_id`（历史行 NULL，不回填）；`POST /api/org` 带会话时创始人 = 会话账号、入参声称不一致 → 403；无会话按过渡模式（`REQUIRE_AUTH=true` → 401，否则取入参、可空）；响应与 OrganizationCreated 事件负载均带 `founder_account_id`。
- **验收**：smoke 新增第 15 节 19 项（沙箱 passed/ticks/无令牌 401、仲裁立案/原被告相同 400、回放空范围/倒置 400、影响分析版本回显/概念含 TransportMode/与基线一致/不存在版本 400、离线 trade Defer/breach Deny/work 放行/本人在线 trade 接受、org 创始人会话优先/入参/冒名 403），全量 **107 通过 / 0 失败**；单测 **91 passed**（+2：`reason::parse_turtle`、`ontology_admin::proposal_concepts`）；cargo check（wasm32 --all-targets）通过、clippy 0 warning。本地跑 smoke 前需先 `wrangler d1 migrations apply --local` 应用 0006（本地环境步骤，非代码事项）。
- **遗留**：`run_sandbox` 本体仍是 MVP 占位（隔离校验 + 无条件 passed），真实 N tick 仿真是 G9；仲裁只有立案，裁决流程（举证/陪审）仍在 M2；影响分析的完整 SPARQL 口径待 G14。

## 十、G9 落地记录（2026-10-23）

- **已实现**：`governance::sandbox::run_sandbox` 从 MVP 占位（隔离校验 + 无条件 passed）升级为真沙箱，`proposal_id` 语义与 analyze_impact 对齐 = 已发布的本体版本（典型用法：审批前对 pending 版本验证）；判定类失败不走 HTTP Err，而是如实落在 `passed=false` + `notes` 明细。四路判定（任一失败即 failed）：
  - **真装载**：版本行 + 域清单查生产 D1（只读）→ R2 取回提案 Turtle（与 analyze_impact 共用新抽出的 `ontology_admin::load_version_turtle`）→ `Owl2RlReasoner::build`（内嵌基线 + 提案一轮 OWL 2 RL）装入内存临时推理环境，不碰线上 active 本体；解析/装载失败即 failed、`ticks_run=0`。
  - **一致性**：装载后出现 owl:Nothing 实例即不一致 → failed，且跳过仿真（自相矛盾的本体下推进无语义）。
  - **SHACL 全量形状**：提案图过全部内嵌形状（person-card / charter / transaction），违规逐条进 notes（上限 10 条 + 总数），计数落 `shacl_violations` 字段。
  - **真仿真**：世界快照 = 生产 D1 `list_simulatable_agents`（active/idle，上限 500，与唤醒心跳集同口径）+ 各自生产 KV 热状态（决策循环的真实状态源，未命中用默认值）+ organizations/positions 背景计数；先查初始不变量，再逐 tick 用与线上**同一**纯函数推进——为此把 `decision::run_decision_cycle` 的规则抽成 `decision::rule_step`，线上与沙箱共用，杜绝两套规则。每 tick 后复查不变量：四项资源（money/food/energy/reputation）有限且非负、能量 ≤ 100；打破即 failed 并注明 tick 与 agent。仿真全程内存推进，不落正式事件流、不写任何存储；对生产侧只有只读访问（硬禁约束的是写入）。ticks 上限 1000（`MAX_SANDBOX_TICKS`），超限 400 不静默截断。版本不存在 → 400（OntologyVersionNotFound，与 analyze_impact 同口径）。
  - **返回结构**：保留 `proposal_id` / `ticks_run` / `passed` / `notes`，新增 `consistent` / `shacl_violations` / `agents_simulated`（向后兼容扩展）。
- **验收**：smoke 第 15 节沙箱用例由 3 项扩展为 11 项（好提案 V1 passed/consistent/shacl_violations=0/ticks_run=3、坏提案 owl:Nothing 落 pending 后 passed=false/consistent=false/ticks_run=0、不存在版本 400、ticks 超上限 400、无令牌 401），全量 **115 通过 / 0 失败**；单测 **97 passed**（+6：好提案 passed 真仿真、坏提案不一致 failed、坏 Turtle 装载 failed、SHACL 违规 failed、非法快照不变量 failed、rule_step 与线上语义对齐）；cargo check（wasm32 --all-targets）通过、clippy 0 warning。
- **近似点与遗留**：仿真只覆盖现有规则决策循环（能量推进）——LLM 决策路径未接上游（G12），不在仿真范围；organizations/positions 无 tick 规则，只作快照背景计数不参与推进。SHACL 形状以实例类为 target，TBox 型提案通常无命中，提案携带实例数据时才被真实校验；完整实例层校验待 G14（oxigraph 评估）。沙箱结果不持久化（同步返回），如需留痕审计另立条目。G9 文档原口径「装入沙箱 R2」未采用：提案已在生产 R2 的版本目录中，装载走只读取回 + 内存推理环境，隔离靠「生产侧零写入 + *_SANDBOX 绑定守卫」达成，语义等价且少一份副本同步问题——如后续需要沙箱侧留痕再启用 R2_SANDBOX 落盘。

## 十一、G8 落地记录（2026-10-23）

- **已实现**：
  - **位置数据模型**：migration 0007 新建 `locations` 表（id/name/lon/lat/polygon/created_at）+ `agents(location_id)` 索引。本体口径：地点是 core:Location（基线 soc:Location）的结构化落点，`agents.location_id` 即 core:locatedAt 指针（可选属性，无公理要求分身必须有位置）；lon/lat/polygon 只是渲染投影（AGENTS §8.5），不进本体公理。地点只能经治理通道创建（`POST /api/admin/locations`，check_admin + 审计；id 冲突 409、坐标出 WGS84/polygon 不合法 400、polygon 落库前规范化为闭合环）——**系统不预置任何地点数据**，无地点时地图返回空集而非造数据（§8.5 口径）。
  - **位置写入两条路径**：① 激活 `POST /api/agent/:id/activate` 新增可选 `location_id`——迁移前校验地点存在（未知 → 400），状态迁移成功后经 `d1::set_agent_location` 落点并写入 AgentActivated 事件负载；② Move 动作（`POST /api/action`）目的地取 `params.location_id`（优先）或 `target_id`——给了目的地但地点不存在 → 落事件前 400；事件落盘后更新位置指针并在响应带 `location_id`，不带目的地只记事件不改位置。非 Move 动作即使携带 location_id 也不改位置。
  - **地图几何**：`GET /api/map/agents` 改 LEFT JOIN locations 输出真实 Point 几何（无位置分身 geometry 如实为 null）+ properties 补 location_name；bbox 过滤下推 SQL（无位置行在给 bbox 时自然不入集）。新增 `GET /api/map/locations` 返回地点 FeatureCollection：有合法 polygon 输出 Polygon 几何，否则 Point（代表点）；其 bbox 按代表点下推（Polygon 与框的精确相交判定不在 MVP）。agents 现有 properties 未含 display_name——分身无此字段（PersonCard 内有，但地图层不解析 RDF），不造字段。
  - **WebSocket 广播**：lib.rs 新增 `GET /api/ws` 转发到单例 hub DO（`WORLD_HUB_NAME = "world"`，与 per-agent 状态 stub 共用 AgentStateDO 类）；DO 用 hibernation（accept_web_socket）收连接并回 `{"kind":"connected"}`，入站只处理 ping→pong（连接即全量订阅，不设主题协议）。广播触发点收敛在 `event::log::append`：事件写入 R2 + D1 索引**之后**，把事件信封 `{"kind":"event","event":{...}}` 经 `do_state::notify_world` POST 到 hub 的 /broadcast 端点、hub 推给全部在线连接——因此行动、激活、决策等**全部已落事件流的事件**都会广播，而非仅移动。广播是 best-effort 分发通道：失败只记 `ws_broadcast_failed` 日志，不回滚、不阻塞事件主流程；无连接时为 no-op；事件唯一事实源仍是 append-only 事件流，WS 不做第二事实源。
- **验收**：cargo check（wasm32 --all-targets）通过、clippy 0 warning；单测 **105 passed**（+8：map 几何 Point/Polygon 规范化与降级、bbox 既有 2 项保留、Move 目的地提取优先级与非 Move 隔离、DO ws 路径匹配）；smoke 新增第 16 节 17 项（建地点 401/回显/Polygon 几何/重复 409/非法坐标 400、地点图层含新地点、激活未知地点 400/带落点激活/响应与落库 location_id、地图 Point 几何与坐标一致、bbox 外排除、Move 接受+响应+落库更新、Move 未知地点 400、WS 订阅经 Node 全局 WebSocket 实连收到 Move 事件广播），全量 **132 通过 / 0 失败**（本地 wrangler 实跑，跑前 `wrangler d1 migrations apply --local` 已应用 0007）。
- **近似点与遗留**：hub 是单个 DO 实例、无订阅鉴权（与地图图层同为公开只读面，订阅者不能借此写世界），规模化分片与鉴权待前端阶段设计；Polygon 几何只支持单个外环（无内环/洞），复杂区域形状待真实地图数据出现时再扩；位置只有「当前值」投影，历史轨迹靠事件流回放（events_index + R2 事件体含 params.location_id），不另建轨迹表；前端 MapLibre 单页仍在 Phase 3，本次未做；广播不做离线补发（断线期间事件靠回放接口补齐）。

## 十二、前端入口落地记录（试运行 MVP，2026-10-23）

- **背景**：Phase 2 全部完成后唯独零前端，用户拍板「先把前端入口做了」。策划草案两个悬项按默认执行（交付时明示、可一句话翻转）：**首页形态 = 分身卡优先**（地图作二级页签）；**世界层不对访客开放**（地图/时间线数据均需登录，登录页本身公开）。
- **技术路线**：零构建原生 HTML/CSS/JS 单页应用，文件在仓库根 `web/`（index.html / app.css / app.js），由新模块 `src/api/web.rs` 以 `include_str!` 编译期内嵌、Worker 路由直出（`/`、`/login`、`/create`、`/home`、`/timeline`、`/funds`、`/map`、`/app.css`、`/app.js`）。未选 wrangler `[assets]` 绑定：当前构建链以 worker-build 的 `build/worker/shim.mjs` 为入口，assets 需额外 run_worker_first 配置且本地/线上一致性需另行验证；内嵌路线零配置、wrangler dev 与线上行为一致、smoke 可直接断言，代价是改前端需重新构建。不引前端框架、不引 MapLibre（AGENTS §3：M2 规划），地图用 SVG 直接画 `/api/map` 的 GeoJSON 坐标。
- **页面与接口对接**（写操作全部走既有接口，无假数据）：
  - 登录页：邮箱 OTP（`POST /api/auth/email/start` / `verify`，会话靠 `ps_session` Cookie、fetch 带 credentials；dev 日志模式下回显码仅在服务端 ALLOW_DEV_OTP_ECHO 时出现）+ Google/GitHub OAuth 按钮（跳既有 `/api/auth/oauth/:provider/start`，回调带 Cookie 跳回 `/`）。
  - 创建分身：表单调 `POST /api/agent`（叙述 + 可选 PersonCard RDF，附示例卡）；422 时把 violations（path/message）逐条显示在表单上。
  - 首页（分身卡优先）：`GET /api/me` 渲染分身卡（状态、PersonCard 是否已存、积分、热状态）；位置经 `GET /api/agent/:id` 的 location_id + 地图地点表取名。pending 显示激活入口：调既有 `POST /api/agent/:id/activate`（admin 通道，页面要求输入治理员令牌；默认空体 = 优先已存卡，可临时覆盖卡与出生落点），结果/422 如实回显。
  - 生活时间线：见下方新增只读接口。事件按类型给中文标题（行动类型、激活等），payload 可展开；离线代理审计（action.proxy_deferred / proxy_denied）单独样式呈现并附中文说明（被拦下的行动未执行）。**不做决策收件箱页面**（G15 不存在），被拦动作只在时间线呈现。
  - 资金页：余额（/api/me）+ 流水（新接口）+ 自助转让 `POST /api/me/transfer` + 自助兑换申请 `POST /api/me/redeem`，结果如实回显（新余额 / 申请单号 / 折合金额 / 冻结后余额）。
  - 地图页（二级页签）：`GET /api/map/locations` + `/api/map/agents` 渲染 Polygon/Point 与分身位置（本人分身高亮）；连 `GET /api/ws`，收到 event 广播后防抖刷新图层并在页面信息流提示；无地点时如实空状态。未登录访问受保护页面：壳可打开但启动即查 /api/me，401 跳 /login（数据接口本身一律会话校验）。
- **为时间线/流水补的两个只读会话接口**（如实标注：此前不存在用户侧读口——流水只有写账本函数、事件回放只有 admin 全局接口且不含审计）：`GET /api/me/timeline`（需会话；events_index 按 actor 查本人分身事件 + 回 R2 取事件体；离线代理审计不在事件流内，故按 `audit/{date}/` 前缀回扫近 14 天、扫描上限 500 个对象，命中本分身的 proxy_deferred/proxy_denied 并入，合并按时间倒序、limit 夹紧 1–100）与 `GET /api/me/transactions`（需会话；credit_transactions 按账号倒序）。另补 `POST /api/auth/logout`（删 KV 会话 + 过期 Cookie）供退出按钮。写路径未新增任何接口。
- **验收**：cargo check（wasm32 --all-targets）通过、clippy 0 warning、单测 105 passed（与基线持平，本轮无新增 Rust 单测，覆盖走 smoke + 实跑）；本地 wrangler dev 实跑核验：全部页面/静态资源 200 且 content-type 正确，OTP 全流程以纯 Cookie 走通（start 回显 dev 码 → verify 下发 Cookie → /api/me 200 → 建分身 → 流水/时间线 200 → logout 后 401），坏卡 422 violations 形状与前端渲染预期一致，Node 直连 /api/ws 收到 connected；smoke 新增第 17 节 14 项（首页/登录/地图壳 200、app.js/app.css 200、无会话流水/时间线 401、流水余额与三类 type、时间线含 Defer 审计与已落事件、退出后 401），全量 **146 通过 / 0 失败**。
- **未覆盖处**：OAuth 未用真实 client id/secret 端到端验证（本地未配密钥，按钮跳既有起点路由为止）；浏览器端交互未做真人点击验证（页面行为以 API/WS 协议级实跑 + JS 语法检查为准）；激活入口需治理员令牌（激活本就是治理动作，普通用户在 pending 只见状态说明）；WS 断线重连为页面级简单重试，广播不补发（靠时间线/回放补齐）；地图 SVG 只支持单外环 Polygon 的等经纬投影，MapLibre 仍在 M2。

## 十三、G15 设计与定版：用户决策收件箱（2026-10-23 用户拍板）

**拍板记录**：2026-10-23 用户明确「G15 开工，挂起 72 小时自动驳回」。本章为定版设计，落地记录见第十四节。

**问题**：G4 接线后，离线代理的 Defer 语义是「拒绝 + 审计留痕」（`action.proxy_deferred`）——动作丢了，所有者上线也无从知晓、无法补批。G15 把 Defer 升级为「挂起待批」：不可逆动作生成待决项落库，所有者在决策收件箱里批准/驳回，批了才真正执行。

**数据模型**（migration 0008 `pending_decisions`）：id（ULID）/ agent_id / account_id（归属者）/ action_type / payload（完整 ActionRequest JSON，批准重放用）/ status（pending / approved / rejected / expired）/ created_at / expires_at（created_at + 72h）/ decided_at / decided_by / event_id（批准落盘的事件）/ reject_reason / audit_id（创建审计关联）。索引：(account_id, status)、(agent_id, status)、(status, expires_at)。

**闸门语义变更**：`POST /api/action` 的 proxy_gate 判定 DeferToOwner（Trade / Ally / Employ / Vote / Legislate，以 `offline_proxy::decide` 分类为准；契约类 marriage / major_contract / loan 仍无 HTTP 调用面，维持库级 `contract_class_deferred`）且分身存在时，不再直接拒绝：落 PendingDecision + 审计 `decision.pending_created`，响应保持 `accepted:false` + `proxy_decision:"defer_to_owner"` 并新增 `pending_decision_id`。Deny（Breach）保持直接拒绝（审计 `action.proxy_denied` 不变）；Allow 与所有者本人提交路径不变。分身不存在的极端情形无归属者可挂起，维持旧「拒绝 + `action.proxy_deferred` 审计」兜底。

**72 小时自动驳回（expired）**：惰性过期为主——三处触发：① `GET /api/me/decisions` 读取收件箱时扫本账号；② approve / reject 尝试时先判该项；③ 调度 tick 顺带全局 sweep（沿用订单过期清理的同位钩子，不新造调度器）。过期 = 状态置 `expired`（语义等同自动驳回）+ 审计 `decision.expired`。批准一个已过期项 → 410 明确错误，不执行。

**收件箱接口**（用户侧，需会话 + 归属校验，沿用 G7 模式；出账类同理，分身归属账号以外一律 403/不可见）：
- `GET /api/me/decisions`：列本人分身的待决与历史（含已过期），待决在前、时间倒序。
- `POST /api/me/decisions/:id/approve`：**以所有者身份把存储的动作重新走正常 action 入口**（`validate_action` SHACL 校验 + 事件落盘路径，与在线提交同一函数），禁止凭载荷绕过校验直落事件；校验不过 → 该项置 `rejected` + 回显 violations（422）；执行成功 → 置 `approved` 并记录 event_id + 审计 `decision.approved`。
- `POST /api/me/decisions/:id/reject`：置 `rejected` + 审计 `decision.rejected`。
- 重复决策 → 明确错误：已人工裁决（approved / rejected）再操作 → 409；已自动驳回（expired）再操作 → 410（与「批准已过期项」同一错误口径）。他人待决项 → 403（列表天然按账号隔离）。

**前端**：`web/` 新增「决策收件箱」页签（时间线之后）：待决列表（分身、动作类型与关键参数中文摘要、发起时间、到期时间/剩余时限、批准/驳回按钮）+ 历史区（已批/已驳/已过期自动驳回，状态中文标注），操作结果如实回显。

**红线（同步写入 AGENTS.md §5.9）**：① 批准必须重走 action 入口校验，不得绕过；② 72h 到期自动驳回，批准已过期项必须拒绝；③ 收件箱只批/驳，不提供修改待决动作载荷的入口（append-only 历史与载荷均不可改，所有者可改的只有目标/代理策略/充值/退休申请等既有宪法口径）。

## 十四、G15 落地记录（2026-10-23）

按第十三节定版实现并验收：

- **表结构**：migration 0008 `pending_decisions`（id / agent_id / account_id / action_type / payload（完整 ActionRequest JSON）/ status（pending/approved/rejected/expired）/ created_at / expires_at / decided_at / decided_by / event_id / reject_reason / audit_id）；索引 (account_id, status, created_at)、(agent_id, status)、(status, expires_at)。
- **闸门语义变化**：`proxy_gate` 的 Defer 分支由「拒绝 + `action.proxy_deferred` 审计」改为「落 PendingDecision + `decision.pending_created` 审计」，响应保持 `accepted:false` + `proxy_decision:"defer_to_owner"` 并新增 `pending_decision_id`；既有 smoke 第 15 节断言（defer_to_owner / accepted=false）不变、只加一条 pending_decision_id 断言。Deny 审计 `action.proxy_denied` 不变；Allow 与所有者本人提交路径不变。分身不存在时无归属者可挂起，保留旧拒绝路径 + `action.proxy_deferred` 兜底（该审计名因此仍在代码与时间线匹配清单中，仅兜底/历史用途）。时间线审计回扫同步纳入 `decision.pending_created / approved / rejected / expired`，前端时间线为四种决策审计各有中文呈现。
- **落盘段共用**：`api/action.rs` 把「校验后执行」段抽为 `execute_validated_action`（Move 目的地校验 → 事件 append → 位置指针 → `action.submit` 审计），在线提交与批准重放共用同一函数——批准不绕校验是结构性的，不靠审批代码自觉。
- **批准流程**：条件认领（`UPDATE ... WHERE status='pending'` 命中 1 行才继续）防重复/并发双批；认领后重走 `validate_action`，不过 → `reject_claimed_decision` 置 rejected + 422 回显 violations + `decision.rejected` 审计（reason=validation_failed_on_approve）；落盘段输入类拒绝（Move 目的地已不存在等 InvalidInput）同级置 rejected；其他基础设施错误回滚认领回 pending 可重试（批准只在事件落盘后才算数）。成功 → 回填 event_id + `decision.approved` 审计。
- **过期机制落点**：惰性过期三处——`GET /api/me/decisions` 读取前扫本账号、approve/reject 入口先 classify（已到期先置 expired 再回 410）、`scheduler::run_tick` 在订单过期清理旁顺带全局 sweep（tick_metrics 新增 decisions_expired；沿用既有 tick，未新造调度器）。每次置 expired 写 `decision.expired` 审计，reject_reason 落「超过 72 小时未决策，系统自动驳回」。重复决策口径：已人工裁决再操作 409，已自动驳回（expired）再操作 410。
- **接口**：`GET /api/me/decisions`（待决在前、时间倒序，上限 100；载荷只下发摘要，params 去 rdf 并以 has_rdf 标记）、`POST /api/me/decisions/:id/approve`、`POST /api/me/decisions/:id/reject`；均需会话，单项操作先归属校验（不存在 404、他人 403），前端页签 `/decisions`（时间线之后）。
- **单测**：+6（到期时刻 = 创建 +72h 含跨月、过期边界含等号、classify 三态、载荷往返保真含 rdf、无 target 往返、坏载荷报错不 panic）+ error 映射新增 DECISION_EXPIRED 断言。DB 流程按全仓惯例由 smoke 覆盖（纯函数层无 D1 绑定）。
- **验收**：cargo check（wasm32 --all-targets）0 error、clippy 0 warning、cargo test **111 passed**（基线 105）；本地 wrangler smoke **161/161**（基线 146：第 15 节 +1 挂起断言、第 17 节时间线断言改 decision.pending_created、第 18 节新增 14 条覆盖挂起可见/他人 403/批准落 event_id/重复批准 409/驳回/重复驳回 409/D1 改 expires_at 后读收件箱变 expired/批准已过期 410）。本地 D1（主库 + 沙箱库）已 apply 0008。
- **近似与遗留**：① 契约类（marriage / major_contract / loan）仍只有 `offline_proxy::contract_class_deferred` 库级分类、无 HTTP 创建入口，故未经收件箱——与 G4 现状一致，本轮未新造入口；② 收件箱列表上限 100 条、无分页（试运行量级）；③ tick sweep 每轮上限 200 条；④ scheduler 在 kill switch 开启时整 tick 跳过，过期收敛靠读取/审批两处惰性过期兜底（语义不受影响）；⑤ 前端时间线对 G15 前的 `action.proxy_deferred` 历史审计保留旧呈现分支。
