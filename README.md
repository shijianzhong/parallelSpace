# parallelSpace · 平行空间

> OWL 本体驱动的多智能体社会模拟 —— **agent 是唯一主体**，随时间自主行动、交互、演化；衍生出什么完全开放、未知，**不对标人类当前社会**。中央只做**校验、仲裁、审计**。

[English](./README.en.md) · 编码约束见 [`AGENTS.md`](./AGENTS.md) · 设计定调见 [`l0-constitution/DESIGN.md`](./l0-constitution/DESIGN.md)

---

## 目录

- [理念：本体即宪法](#理念本体即宪法)
- [架构](#架构)
- [技术栈](#技术栈)
- [目录结构](#目录结构)
- [快速开始](#快速开始)
- [必须知道的两个构建坑](#必须知道的两个构建坑)
- [验证矩阵](#验证矩阵)
- [支付幂等设计](#支付幂等设计)
- [当前已实现](#当前已实现)
- [已知边界与 Roadmap](#已知边界与-roadmap)
- [环境变量与绑定](#环境变量与绑定)
- [文档索引](#文档索引)
- [贡献](#贡献)
- [许可证](#许可证)

---

## 理念：本体即宪法

本项目从第一天起就把「规则」当作一等公民。任何状态变更都要先过本体校验，LLM 只负责提议，确定性规则引擎负责守门。详见 [`AGENTS.md`](./AGENTS.md)（编码强制约束，**每次 AI 生成代码前必读**）。

**五条不可违反的原则：**

1. **本体即宪法** —— OWL 管约束，JSON 只传输。状态变更先过本体校验。
2. **LLM 提案、规则守门** —— LLM 可提议行动，但落库前必须经确定性规则引擎审批。
3. **状态持久、计算离散** —— 世界状态落在 D1/KV/R2，tick 式推进，计算与存储分离。
4. **秩序需共识基础（机制约束，非演进路径）** —— 共识是互动的机制前提，但**不预设** agent 必走「制度 → 组织 → 领导」的人类式演进路径。
5. **本体分层进化** —— L0 不可变（宪法层）/ L1 治理（需治理流程）/ L2 自由（可贡献、可涌现）。

**存在论约定（2026-10 定调，最高优先级）：**

- parallelSpace 是**平行空间**：agent 为唯一主体，衍生出什么**完全开放、未知**；
- **不对标、不预设人类当前社会**——人类的一切（货币/公司/法律/交通/雇佣…）只是特定约束下的局部最优解，**不是蓝本**；
- 该系统**可能比当前社会发展得更快、更先进，或走向完全不同的结构**；架构要能容纳尚未命名/尚未想象的社会形态；
- 因此 L0 宪法层保持**中立与最小化**，人类制度模板一律从 L0/L1 **降级为 L2 涌现候选**，且仅是「可能路径之一」。

>  [`l0-constitution/core.ttl`](./l0-constitution/core.ttl) 是这一定调的落地草案（v0.1.0-draft，独立目录、未进构建）：9 个中立原语（Agent / Instant·tick / Event / Action·零枚举 / Entity / Relation / Observation / Location·仅可指称 / ChangeProposal），刻意**不含**距离、移动、交通、组织、契约、货币——连 `Move` 都不预声明，若社会发展出移动，由它自己提案。

---

## 架构

```
            ┌──────────── 客户端（MapLibre GL JS, M2）──────────┐
            │  世界地图 / 分身卡 / 行动面板 / 组织视图           │
            └──────────────────────┬───────────────────────────┘
                                   │ HTTPS
            ┌──────────────────────▼───────────────────────────┐
            │  Cloudflare Worker（Rust + workers-rs 0.8.7）       │
            │  ├─ API 薄层   auth / agent / action / org / map    │
            │  │            / payment / admin                     │
            │  ├─ Ontology    iri / version / sparql / shacl       │
            │  │            / reason（OWL 2 RL L2 推理）           │
            │  ├─ Governance  validator / arbitration / audit     │
            │  │            / ontology_admin / sandbox             │
            │  ├─ Agent Runtime  scheduler / lifecycle / decision  │
            │  │            / offline_proxy                        │
            │  ├─ Payment     provider / manual_wechat / credit    │
            │  │            / packages（服务端定价）               │
            │  ├─ Event       bus / log / replay                   │
            │  └─ LLM         gateway / router / cache             │
            └───┬──────────────┬──────────────┬──────────┬───────┘
                │ D1           │ KV / R2       │ Queues   │ DO
                │ (状态/关系)  │ (热态/配额/   │ (tick    │ (会话/
                │              │  事件日志)    │  驱动)   │  广播)
```

---

## 技术栈

- **语言 / 运行时**：Rust 1.94 + [workers-rs](https://github.com/cloudflare/workers-rs) 0.8.7，编译目标 `wasm32-unknown-unknown`
- **平台**：Cloudflare Workers（Paid，约 $5/月）—— D1 / KV / R2 / Queues / Durable Objects / Cron
- **推理**：[reasonable](https://github.com/owl-cs/reasonable)（OWL 2 RL，L2）+ 可选外部 open-ontologies（OWL 2 DL，L3）
- **校验**：内嵌 SHACL 引擎（L2 形状：person-card / charter / transaction）
- **前端（M2 规划）**：MapLibre GL JS
- **依赖版本（2026-10 对齐）**：worker 0.8.7 / reasonable 0.4.4 / oxrdf 0.3.5 / rio_turtle 0.7.1 / ulid 3.0.0 / chrono 0.4.45 / thiserror 2.0.21 / wasm-bindgen 0.2.129

---

## 目录结构

```
parallelspace/
├── AGENTS.md              # 编码强制约束（必读，t.md 第三部分原样提取）
├── Cargo.toml             # 依赖与 wasm 特性开关
├── wrangler.toml          # Workers 绑定与 D1/KV/R2/Queues/DO 配置
├── .cargo/config.toml     # +reference-types（构建坑修复，见下）
├── ontology/              # OWL 2 DL Turtle：soc / gov / econ / geo / time
├── shacl/                 # L2 形状：person-card / charter / transaction
├── l0-constitution/       # L0 中立宪法草案（core.ttl + DESIGN.md，未进构建）
├── migrations/            # D1 SQL：0001_init（15 表）/ 0002_ontology_domains
├── scripts/               # smoke.sh（端到端）/ wasm-bindgen-reftypes.sh
└── src/
    ├── lib.rs             # fetch 入口 + 路由
    ├── api/               # HTTP handlers（薄层）
    ├── ontology/          # iri / version / sparql / shacl / reason
    ├── agent_runtime/     # scheduler / lifecycle / decision / offline_proxy
    ├── llm/               # gateway / router / cache
    ├── event/             # bus / log / replay
    ├── storage/           # d1 / kv / r2 / do_state
    ├── governance/        # validator / arbitration / audit / ontology_admin / sandbox
    ├── payment/           # provider / manual_wechat / credit / packages
    └── common/            # error / logging / types
```

---

## 快速开始

### 1. 工具链（一次性）

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # 与 Cargo.lock 的 wasm-bindgen 对齐
cargo install worker-build   --version 0.8.7   --locked     # 与 worker crate 同版本
npm install wrangler
```

### 2. 编译 / 测试（唯一权威门禁）

> 不要指望 tsc 类工具抓 wasm 问题；下面三步才是权威。

```bash
cargo check    --target wasm32-unknown-unknown
cargo clippy   --target wasm32-unknown-unknown --all-targets
cargo test     --target wasm32-unknown-unknown        # node 驱动，无需浏览器
```

### 3. 打包成 Workers bundle

```bash
WASM_BINDGEN_BIN=scripts/wasm-bindgen-reftypes.sh worker-build --release
#   → build/index_bg.wasm + build/index.js + build/worker/shim.mjs
```

### 4. 本地运行（D1/KV/R2/Queues/DO 全用 miniflare 本地态）

```bash
cat > .dev.vars <<'EOF'
ADMIN_TOKEN=dev-admin-token
MANUAL_WECHAT_QR_URL=https://example.com/qr.png
EOF

npx wrangler d1 migrations apply parallelspace-db           --local
npx wrangler d1 migrations apply parallelspace-sandbox-db   --local
npx wrangler dev --local --port 8787
```

### 5. 端到端冒烟（另开终端，57 项断言）

```bash
bash scripts/smoke.sh
```

冒烟分 10 节：注册 → 下单 → 支付入账（幂等）→ SHACL → 查询 → 地图 →
**本体治理（域注册表 + pending → approve 两段式发布 + 回滚状态机）**。
脚本可重复运行（治理段用运行期唯一域名/版本号，不依赖干净库）。

---

## 必须知道的三个坑

1. **`externref table required for catch wrappers`**
   `worker-build` 会带 `--force-enable-abort-handler`，这要求 wasm 里有 externref 表；
   而它自己**不传** `--reference-types`。解法是两处配合：`.cargo/config.toml` 打开
   `target-feature=+reference-types`，并用 `scripts/wasm-bindgen-reftypes.sh`（经
   `WASM_BINDGEN_BIN` 注入）给 wasm-bindgen 补该参数。仅改前者仍会失败。

2. **`wrangler.toml` 的 `main` 必须指向 `build/worker/shim.mjs`**，不是 `build/worker.js`。
   写错会 404 一片。

3. **workers-rs 的 KV 存取不对称**：`put(String)` 走「裸字符串」存储路径（无 JSON 引号），
   而 `.json::<String>()` 读取时按 JSON 解析——裸版本号会直接抛错。**同类型存取也要用
   `text()` 配对**；`Vec<String>` 等复合类型才用 `json()`。这个潜伏 bug 曾被新冒烟首次踩响。

---

## 验证矩阵

> 以下均为本次实跑结果，非声明。

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 编译（dev） | `cargo check --target wasm32-unknown-unknown` | 0 error / 0 warning |
| 编译（release） | `cargo build --release --target wasm32-unknown-unknown` | Finished |
| Clippy（含测试目标） | `cargo clippy --target wasm32-unknown-unknown --all-targets` | 0 warning |
| 单元测试 | `cargo test --target wasm32-unknown-unknown` | **89 passed / 0 failed** |
| 迁移 SQL | `sqlite3 x.db < migrations/0001_init.sql` | 15 表，`foreign_key_check` / `integrity_check` ok |
| 迁移 SQL（0002） | `wrangler d1 migrations apply <db> --local` | 域注册表 + versions 状态列 + 五域种子就位 |
| Workers 打包 | `worker-build --release` | wasm + shim.mjs 产出成功 |
| 本地运行时 | `wrangler dev --local` | 10 个绑定全部就绪，`/health` 200 |
| 端到端冒烟 | `bash scripts/smoke.sh` | **78 通过 / 0 失败** |

冒烟覆盖：注册 → 下单（服务端定价、忽略客户端金额、无账号 404）→ admin 令牌 401 →
金额二次核对 409 → 入账 500 积分 → 幂等重放（余额不变）→ 同一流水号复用 409 →
SHACL 拒非法 status / 接受合法负载 → 查询 200/404 → 地图 FeatureCollection →
分身激活（pending / 401 无令牌 / 422 坏卡 / active / 409 重复与未付费）→
域注册（seed / 409 重复 / L0 保留域 403 / 非法层 400 / 路径穿越 400）→
提案落 pending（未审批不写 KV、不生效）→ 审批激活（版本 + seed 域联动）→
重复审批 409 → 回滚状态机（rolled_back / 目标须已审批）。

测试走 `wasm-bindgen-test-runner` 的 **node 驱动**，不需要浏览器 / chromedriver。

---

## 支付幂等设计

手工微信支付由治理员手填渠道流水号，存在两条重复入账路径，均已拦截：

1. **同一订单重复确认** —— `UPDATE orders SET status='paid' WHERE id=? AND status='pending'`，
   `changes()==1` 才入账；冒烟实测重放第二次余额不变、`already_confirmed=true`。
2. **同一流水号绑到另一笔订单** —— `orders` 上加部分唯一索引
   `idx_orders_channel_txn(payment_channel, channel_txn_id) WHERE channel_txn_id IS NOT NULL`，
   `credit.rs` 前置检查先查出占用方订单号并返回 409
   （只靠索引的话 D1 会把整批 batch 回滚成泛化 SQL 错误，排查成本高）。

下单接口 `POST /api/payment/order` 只接受 `package_id`：金额与积分由
`payment/packages.rs` 决定，客户端传 `amount_cents` 会被直接忽略（冒烟已验）。

---

## 当前已实现

- 本体资产落盘 + 3 处公理修正 + `geo.ttl` 前缀补全
- SHACL L2 引擎：内嵌形状解析、受限约束集、`rdfs:subClassOf` 传递闭包（含环状继承终止）
- reasonable OWL 2 RL 封装（推理 / 一致性）
- **本体治理两段式发布**：`POST /api/admin/ontology/publish` 落 **pending**（不写 KV、不生效）→
  `POST /api/admin/ontology/versions/{v}/approve` 审批后才 active 并切换 KV；回滚仅限已审批目标，
  版本行状态机 `pending → active → superseded / rolled_back`
- **域注册表**（`ontology_domains` 表）：替代原硬编码文件名白名单，新域（如 transport）先注册后使用；
  防目录穿越；`core` 等 L0 保留域与 L0 层注册一律 403
- **种子降级**：soc/gov/econ/geo/time 五域注册为 **L2 seed**（存在论纠偏的落地：人类制度模板从宪法层降级为涌现候选）
- 支付：`POST /api/payment/order` 下单（服务端定价，客户端仅选套餐）+ 幂等入账 `confirm_payment`
- **分身激活链路**（2026-10-23 补齐，见 `docs/GAPS_AND_PLAN.md` G1）：支付首次确认推进
  `created → pending`；`POST /api/agent/{id}/activate`（admin 执行）带 PersonCard 过 SHACL
  后 `pending → active`，条件迁移防并发，激活凭 paid 订单可自愈补走 pending
- **L0 保护清单扩容**：`touches_l0` 已含 core.ttl 全量原语（旧 4 项保留兼容）；`iri.rs` 新增
  `core:` 命名空间并入 IRI 白名单
- **LLM 网关修复**：缓存读改 `text()` 配对（原 `.json::<String>()` 违反 KV 配对规则）；
  占位提案不再写缓存（防 7 天毒化）；配额改为真实预扣（`add_quota_used` 接入）
- Cron tick（唤醒集合 = 本 tick 事件 actor ∪ active/idle 心跳）+ Queue 消费、KV 热状态/配额/缓存、R2 事件日志
- 注册 / 查询 / 激活 / 行动 / 组织 / 地图 / 下单 / 支付确认 / 本体域注册 / 提案 / 审批 / 回滚 API
- 积分转让 / agent 收益入账 / 兑换申请与审批（2026-10-23 定调：积分可转让、可兑换，admin 代办，规则见 AGENTS §5.7）
- 用户认证（G7）：邮箱验证码 + Google/GitHub OAuth + KV 会话、登录后建分身、/api/me、自助转让/兑换、行动归属校验（AGENTS §5.8）
- 错误响应统一 JSON `{"error":"<CODE>","message":"..."}`，5xx 不外泄内部细节
- 50 个 Rust 源文件、89 个单测、78 项端到端断言
- L0 中立宪法草案 `l0-constitution/core.ttl`（rdflib 实测：140 三元组、9 原语、零 imports、零人类模板泄漏）

---

## 已知边界与 Roadmap

**MVP 有意留白：**

- LLM 上游未接（`LLM_API_URL` 空时降级规则路径）——「概念化」执行者缺位，交通/货币等新概念**不会自动生成**，只能治理员手动提案
- L3 本体引擎未接（`ONTOLOGY_ENGINE_URL` 空时 L2 结果即最终结果）
- oxigraph 未引入（M2 实例层入 RDF 后再评估）
- Durable Objects WebSocket 广播占位
- 注册后**不会自动建订单**，需前端再调 `/api/payment/order`；注册响应里的
  `next: payment_required` 只是提示
- L0 中立宪法仍是**草案**（`l0-constitution/` 未进构建）；`core.ttl` 尚未替换现有五域
  （`touches_l0` 保护清单已先行扩为 core 全量，见「当前已实现」）
- 审批目前仍由 admin token 持有者扮演（`approved_by` 为入参但无独立治理身份），两段式的**人**是真实的、**自动化**不是
- 冒烟跑在 miniflare 本地态，**未在真实 Cloudflare 账号（Workers Paid）上部署验证**；
  D1 事务与 Queues 的线上语义（尤其是 batch 内唯一索引冲突的表现）以真实环境为准

**Roadmap：**

- [ ] `l0-constitution/core.ttl` 正式落地为 L0（soc/gov/econ 退场为 L2 参考域）
- [ ] 接入真实 Cloudflare Workers Paid 部署并跑通 D1/Queues 线上语义
- [ ] LLM 概念化最小闭环：从 agent 行为模式发现未命名模式 → 生成 Turtle 提案 → 过守门流水线
- [ ] LLM 网关与 L3 本体引擎对接
- [ ] MapLibre 前端（M2）与 WebSocket 实时广播
- [ ] oxigraph 实例层评估

---

## 环境变量与绑定

详见 [`wrangler.toml`](./wrangler.toml)。

- **secret**：`ADMIN_TOKEN`（治理接口，未配置一律 403）
- **var**：`ONTOLOGY_VERSION`、`ONTOLOGY_ENGINE_URL`、`LLM_API_URL`、
  `MANUAL_WECHAT_QR_URL`（空时下单接口 500，fail loud 而不是发坏二维码）
- 本地开发把这些写进 `.dev.vars`（已在 `.gitignore`）

---

## 文档索引

- [`AGENTS.md`](./AGENTS.md) —— 编码强制约束（每次 AI 生成代码前必读）
- [`l0-constitution/DESIGN.md`](./l0-constitution/DESIGN.md) —— L0 中立宪法草案：准入测试、刻意排除清单、元宪法流水线
- `t.md`（仓库外）—— PRD + SAD + Coding Prompt 完整文档 v1.0
- [`ontology/`](./ontology/) —— OWL 2 DL Turtle + SHACL 形状（注意：soc/gov/econ 已定位为 L2 seed，非宪法）
- [`migrations/`](./migrations/) —— D1 SQL（16 表 + 域注册表，含 FK 与完整性约束）

---

## 贡献

1. Fork 并新建分支（`feat/...`、`fix/...`）。
2. **先读 [`AGENTS.md`](./AGENTS.md)**，所有新增代码必须过 `cargo check` / `clippy` / `test` 三道门禁。
3. 提交信息用中文或英文均可，但需说明「做了什么 / 没动的部分 / 根因」。
4. 发起 Pull Request，CI 以三道门禁 + `scripts/smoke.sh` 为通过标准。

---

## 许可证

本项目默认采用 **MIT License**（宽松开源，允许商用与修改，需保留版权声明）。
如需其他许可证（如 AGPL 以契合「本体即宪法」的开放治理理念），请提出 Issue 讨论。

Copyright © 2026 shijianzhong
