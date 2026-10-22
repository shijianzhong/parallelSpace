# Earth Soc Sim · 地球社会模拟

> OWL 本体驱动的多智能体社会模拟 —— 基于真实地球地理数据，分身进入由本体约束的共享社会语义空间，自主行动、交易、结盟、立法、演化；中央只做**校验、仲裁、审计**。

[English](./README.en.md) · 编码约束见 [`AGENTS.md`](./AGENTS.md)

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
4. **制度先于组织先于领导** —— 没有宪章（Charter）就没有组织，没有组织就没有领导。
5. **本体分层进化** —— L0 不可变（宪法层）/ L1 治理（需治理流程）/ L2 自由（玩家可贡献）。

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
earth-soc-sim/
├── AGENTS.md              # 编码强制约束（必读，t.md 第三部分原样提取）
├── Cargo.toml             # 依赖与 wasm 特性开关
├── wrangler.toml          # Workers 绑定与 D1/KV/R2/Queues/DO 配置
├── .cargo/config.toml     # +reference-types（构建坑修复，见下）
├── ontology/              # OWL 2 DL Turtle：soc / gov / econ / geo / time
├── shacl/                 # L2 形状：person-card / charter / transaction
├── migrations/            # D1 初始化 SQL（0001_init.sql，15 表）
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

npx wrangler d1 migrations apply earth-soc-sim-db       --local
npx wrangler d1 migrations apply earth-soc-sim-sandbox-db --local
npx wrangler dev --local --port 8787
```

### 5. 端到端冒烟（另开终端，26 项断言）

```bash
bash scripts/smoke.sh
```

---

## 必须知道的两个构建坑

1. **`externref table required for catch wrappers`**
   `worker-build` 会带 `--force-enable-abort-handler`，这要求 wasm 里有 externref 表；
   而它自己**不传** `--reference-types`。解法是两处配合：`.cargo/config.toml` 打开
   `target-feature=+reference-types`，并用 `scripts/wasm-bindgen-reftypes.sh`（经
   `WASM_BINDGEN_BIN` 注入）给 wasm-bindgen 补该参数。仅改前者仍会失败。

2. **`wrangler.toml` 的 `main` 必须指向 `build/worker/shim.mjs`**，不是 `build/worker.js`。
   写错会 404 一片。

---

## 验证矩阵

> 以下均为本次实跑结果，非声明。

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 编译（dev） | `cargo check --target wasm32-unknown-unknown` | 0 error / 0 warning |
| 编译（release） | `cargo build --release --target wasm32-unknown-unknown` | Finished |
| Clippy（含测试目标） | `cargo clippy --target wasm32-unknown-unknown --all-targets` | 0 warning |
| 单元测试 | `cargo test --target wasm32-unknown-unknown` | **80 passed / 0 failed** |
| 迁移 SQL | `sqlite3 x.db < migrations/0001_init.sql` | 15 表，`foreign_key_check` / `integrity_check` ok |
| Workers 打包 | `worker-build --release` | wasm + shim.mjs 产出成功 |
| 本地运行时 | `wrangler dev --local` | 10 个绑定全部就绪，`/health` 200 |
| 端到端冒烟 | `bash scripts/smoke.sh` | **26 通过 / 0 失败** |

冒烟覆盖：注册 → 下单（服务端定价、忽略客户端金额、无账号 404）→ admin 令牌 401 →
金额二次核对 409 → 入账 500 积分 → 幂等重放（余额不变）→ 同一流水号复用 409 →
SHACL 拒非法 status / 接受合法负载 → 查询 200/404 → 地图 FeatureCollection。

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
- 支付：`POST /api/payment/order` 下单（服务端定价，客户端仅选套餐）+ 幂等入账 `confirm_payment`
- Cron tick + Queue 消费、KV 热状态/配额/缓存、R2 事件日志
- 注册 / 查询 / 行动 / 组织 / 地图 / 下单 / 支付确认 / 本体发布回滚 API
- 错误响应统一 JSON `{"error":"<CODE>","message":"..."}`，5xx 不外泄内部细节
- 48 个 Rust 文件、80 个单测、26 项端到端断言

---

## 已知边界与 Roadmap

**MVP 有意留白：**

- LLM 上游未接（`LLM_API_URL` 空时降级规则路径）
- L3 本体引擎未接（`ONTOLOGY_ENGINE_URL` 空时 L2 结果即最终结果）
- oxigraph 未引入（M2 实例层入 RDF 后再评估）
- Durable Objects WebSocket 广播占位
- 注册后**不会自动建订单**，需前端再调 `/api/payment/order`；注册响应里的
  `next: payment_required` 只是提示
- 分身支付后仍是 `created` 状态，`created → active` 的迁移入口（需 SHACL 校验 PersonCard）
  尚未接到 API，`lifecycle::can_transition` 已备好规则
- 冒烟跑在 miniflare 本地态，**未在真实 Cloudflare 账号（Workers Paid）上部署验证**；
  D1 事务与 Queues 的线上语义（尤其是 batch 内唯一索引冲突的表现）以真实环境为准

**Roadmap：**

- [ ] 接入真实 Cloudflare Workers Paid 部署并跑通 D1/Queues 线上语义
- [ ] `created → active` 分身激活 API（SHACL 校验 PersonCard）
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
- `t.md`（仓库外）—— PRD + SAD + Coding Prompt 完整文档 v1.0
- [`ontology/`](./ontology/) —— OWL 2 DL Turtle + SHACL 形状
- [`migrations/`](./migrations/) —— D1 初始化 SQL（15 表，含 FK 与完整性约束）

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
