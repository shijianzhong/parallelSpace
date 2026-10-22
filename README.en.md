# Earth Soc Sim

> An OWL-ontology-driven multi-agent society simulation. Built on real-world geographic data, avatars enter a shared social semantic space constrained by an ontology, acting, trading, allying, legislating, and evolving autonomously; the center only **validates, arbitrates, and audits**.

[中文](./README.md) · Encoding constraints in [`AGENTS.md`](./AGENTS.md)

---

## Table of Contents

- [Philosophy: The Ontology Is the Constitution](#philosophy-the-ontology-is-the-constitution)
- [Architecture](#architecture)
- [Tech Stack](#tech-stack)
- [Directory Layout](#directory-layout)
- [Quick Start](#quick-start)
- [Two Build Pitfalls You Must Know](#two-build-pitfalls-you-must-know)
- [Verification Matrix](#verification-matrix)
- [Payment Idempotency Design](#payment-idempotency-design)
- [What's Implemented](#whats-implemented)
- [Known Boundaries & Roadmap](#known-boundaries--roadmap)
- [Environment Variables & Bindings](#environment-variables--bindings)
- [Documentation Index](#documentation-index)
- [Contributing](#contributing)
- [License](#license)

---

## Philosophy: The Ontology Is the Constitution

From day one, rules are first-class citizens. Every state change must pass ontology validation; the LLM only *proposes*, and a deterministic rule engine *gates*. See [`AGENTS.md`](./AGENTS.md) (mandatory encoding constraints — **read before any AI generates code**).

**Five non-negotiable principles:**

1. **The ontology is the constitution** — OWL governs constraints; JSON only transports. State changes validate against the ontology first.
2. **LLM proposes, rules gate** — The LLM may propose actions, but they must clear a deterministic rule engine before persisting.
3. **State persists, computation is discrete** — World state lives in D1/KV/R2; tick-based advancement; compute and storage are separated.
4. **Institution before organization before leader** — No Charter, no organization; no organization, no leader.
5. **Ontology evolves in layers** — L0 immutable (constitutional) / L1 governed (needs process) / L2 free (player-contributed).

---

## Architecture

```
            ┌──────────── Client (MapLibre GL JS, M2)────────────┐
            │  World map / Avatar card / Action panel / Org view │
            └──────────────────────┬─────────────────────────────┘
                                   │ HTTPS
            ┌──────────────────────▼─────────────────────────────┐
            │  Cloudflare Worker (Rust + workers-rs 0.8.7)         │
            │  ├─ API thin layer  auth / agent / action / org /   │
            │  │                 map / payment / admin             │
            │  ├─ Ontology        iri / version / sparql / shacl   │
            │  │                 / reason (OWL 2 RL L2)            │
            │  ├─ Governance      validator / arbitration / audit │
            │  │                 / ontology_admin / sandbox        │
            │  ├─ Agent Runtime   scheduler / lifecycle / decision│
            │  │                 / offline_proxy                   │
            │  ├─ Payment         provider / manual_wechat /       │
            │  │                 credit / packages (server pricing)│
            │  ├─ Event           bus / log / replay               │
            │  └─ LLM             gateway / router / cache         │
            └───┬────────────────┬──────────────┬──────────┬──────┘
                │ D1             │ KV / R2       │ Queues   │ DO
                │ (state/rels)   │ (hot/quota/   │ (tick    │ (session/
                │                │  event log)   │  drive)  │  broadcast)
```

---

## Tech Stack

- **Language / Runtime**: Rust 1.94 + [workers-rs](https://github.com/cloudflare/workers-rs) 0.8.7, target `wasm32-unknown-unknown`
- **Platform**: Cloudflare Workers (Paid, ~$5/mo) — D1 / KV / R2 / Queues / Durable Objects / Cron
- **Reasoning**: [reasonable](https://github.com/owl-cs/reasonable) (OWL 2 RL, L2) + optional external open-ontologies (OWL 2 DL, L3)
- **Validation**: embedded SHACL engine (L2 shapes: person-card / charter / transaction)
- **Frontend (M2, planned)**: MapLibre GL JS
- **Pinned deps (2026-10)**: worker 0.8.7 / reasonable 0.4.4 / oxrdf 0.3.5 / rio_turtle 0.7.1 / ulid 3.0.0 / chrono 0.4.45 / thiserror 2.0.21 / wasm-bindgen 0.2.129

---

## Directory Layout

```
earth-soc-sim/
├── AGENTS.md              # Mandatory encoding constraints (read first; verbatim §3 of t.md)
├── Cargo.toml             # Deps and wasm feature flags
├── wrangler.toml          # Workers bindings & D1/KV/R2/Queues/DO config
├── .cargo/config.toml     # +reference-types (build-pitfall fix, see below)
├── ontology/              # OWL 2 DL Turtle: soc / gov / econ / geo / time
├── shacl/                 # L2 shapes: person-card / charter / transaction
├── migrations/            # D1 init SQL (0001_init.sql, 15 tables)
├── scripts/               # smoke.sh (e2e) / wasm-bindgen-reftypes.sh
└── src/
    ├── lib.rs             # fetch entry + routing
    ├── api/               # HTTP handlers (thin layer)
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

## Quick Start

### 1. Toolchain (one-time)

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # match wasm-bindgen in Cargo.lock
cargo install worker-build   --version 0.8.7   --locked     # match worker crate version
npm install wrangler
```

### 2. Build / Test (the only authoritative gate)

> Don't rely on tsc-like tooling to catch wasm issues; the three steps below are authoritative.

```bash
cargo check    --target wasm32-unknown-unknown
cargo clippy   --target wasm32-unknown-unknown --all-targets
cargo test     --target wasm32-unknown-unknown        # node-driven, no browser needed
```

### 3. Bundle into a Workers artifact

```bash
WASM_BINDGEN_BIN=scripts/wasm-bindgen-reftypes.sh worker-build --release
#   → build/index_bg.wasm + build/index.js + build/worker/shim.mjs
```

### 4. Run locally (D1/KV/R2/Queues/DO all on miniflare local state)

```bash
cat > .dev.vars <<'EOF'
ADMIN_TOKEN=dev-admin-token
MANUAL_WECHAT_QR_URL=https://example.com/qr.png
EOF

npx wrangler d1 migrations apply earth-soc-sim-db         --local
npx wrangler d1 migrations apply earth-soc-sim-sandbox-db --local
npx wrangler dev --local --port 8787
```

### 5. End-to-end smoke test (new terminal, 26 assertions)

```bash
bash scripts/smoke.sh
```

---

## Two Build Pitfalls You Must Know

1. **`externref table required for catch wrappers`**
   `worker-build` passes `--force-enable-abort-handler`, which requires an externref table in
   the wasm; but it does **not** pass `--reference-types` itself. The fix is two-fold: enable
   `target-feature=+reference-types` in `.cargo/config.toml`, and inject
   `scripts/wasm-bindgen-reftypes.sh` via `WASM_BINDGEN_BIN` to add that flag to wasm-bindgen.
   Enabling only the former still fails.

2. **`wrangler.toml`'s `main` must point to `build/worker/shim.mjs`**, not `build/worker.js`.
   Wrong path → a wall of 404s.

---

## Verification Matrix

> All results below were actually run, not declared.

| Check | Command | Result |
| --- | --- | --- |
| Build (dev) | `cargo check --target wasm32-unknown-unknown` | 0 error / 0 warning |
| Build (release) | `cargo build --release --target wasm32-unknown-unknown` | Finished |
| Clippy (incl. tests) | `cargo clippy --target wasm32-unknown-unknown --all-targets` | 0 warning |
| Unit tests | `cargo test --target wasm32-unknown-unknown` | **80 passed / 0 failed** |
| Migration SQL | `sqlite3 x.db < migrations/0001_init.sql` | 15 tables, `foreign_key_check` / `integrity_check` ok |
| Workers bundle | `worker-build --release` | wasm + shim.mjs produced |
| Local runtime | `wrangler dev --local` | 10 bindings ready, `/health` 200 |
| E2E smoke | `bash scripts/smoke.sh` | **26 passed / 0 failed** |

Smoke coverage: register → order (server pricing, client amount ignored, 404 if no account) →
admin token 401 → amount re-check 409 → credit 500 points → idempotent replay (balance unchanged)
→ same txn id reused 409 → SHACL rejects bad status / accepts valid payload → query 200/404 →
map FeatureCollection.

Tests run under `wasm-bindgen-test-runner`'s **node driver** — no browser / chromedriver needed.

---

## Payment Idempotency Design

Manual WeChat pay has the operator hand-fill a channel transaction id, creating two double-credit
paths; both are blocked:

1. **Re-confirming the same order** — `UPDATE orders SET status='paid' WHERE id=? AND status='pending'`;
   credit only happens when `changes()==1`. Smoke confirmed the second replay leaves balance
   unchanged and returns `already_confirmed=true`.
2. **Reusing a txn id on a different order** — a partial unique index
   `idx_orders_channel_txn(payment_channel, channel_txn_id) WHERE channel_txn_id IS NOT NULL` is added;
   `credit.rs` pre-checks and returns 409 with the occupying order id
   (relying on the index alone makes D1 roll back the whole batch into a generic SQL error — costly to debug).

The order endpoint `POST /api/payment/order` accepts only `package_id`: amount and points are
decided by `payment/packages.rs`; any client-supplied `amount_cents` is ignored (verified by smoke).

---

## What's Implemented

- Ontology assets on disk + 3 axiom fixes + `geo.ttl` prefix completion
- SHACL L2 engine: embedded shape parsing, restricted constraint set, `rdfs:subClassOf` transitive closure (with cycle termination)
- reasonable OWL 2 RL wrapper (inference / consistency)
- Payment: `POST /api/payment/order` (server pricing, client picks package only) + idempotent `confirm_payment`
- Cron tick + Queue consumption, KV hot state/quota/cache, R2 event log
- Register / query / action / org / map / order / payment-confirm / ontology-publish-rollback APIs
- Unified JSON errors `{"error":"<CODE>","message":"..."}`; 5xx leaks no internals
- 48 Rust files, 80 unit tests, 26 e2e assertions

---

## Known Boundaries & Roadmap

**Intentional MVP gaps:**

- LLM upstream not wired (`LLM_API_URL` empty → rule-path fallback)
- L3 ontology engine not wired (`ONTOLOGY_ENGINE_URL` empty → L2 result is final)
- oxigraph not introduced (re-evaluate after M2 instance layer enters RDF)
- Durable Objects WebSocket broadcast is a stub
- Register does **not** auto-create an order; the client must call `/api/payment/order` again;
  the `next: payment_required` in the register response is only a hint
- After payment an avatar stays `created`; the `created → active` transition entry (needs SHACL
  PersonCard validation) is not yet wired to an API, though `lifecycle::can_transition` is ready
- Smoke ran on miniflare local state; **not yet deployed/verified on a real Cloudflare account
  (Workers Paid)**; D1 transactions and Queues online semantics (esp. in-batch unique-index
  conflict behavior) should be confirmed in the real environment

**Roadmap:**

- [ ] Deploy on real Cloudflare Workers Paid and validate D1/Queues online semantics
- [ ] `created → active` avatar activation API (SHACL PersonCard validation)
- [ ] LLM gateway and L3 ontology engine integration
- [ ] MapLibre frontend (M2) with WebSocket real-time broadcast
- [ ] oxigraph instance-layer evaluation

---

## Environment Variables & Bindings

See [`wrangler.toml`](./wrangler.toml).

- **secret**: `ADMIN_TOKEN` (governance endpoints; 403 if unset)
- **var**: `ONTOLOGY_VERSION`, `ONTOLOGY_ENGINE_URL`, `LLM_API_URL`,
  `MANUAL_WECHAT_QR_URL` (empty → order endpoint 500, fail loud instead of a bad QR)
- For local dev, put these in `.dev.vars` (already git-ignored)

---

## Documentation Index

- [`AGENTS.md`](./AGENTS.md) — mandatory encoding constraints (read before any AI writes code)
- `t.md` (outside this repo) — full PRD + SAD + Coding Prompt, v1.0
- [`ontology/`](./ontology/) — OWL 2 DL Turtle + SHACL shapes
- [`migrations/`](./migrations/) — D1 init SQL (15 tables, with FKs and integrity constraints)

---

## Contributing

1. Fork and create a branch (`feat/...`, `fix/...`).
2. **Read [`AGENTS.md`](./AGENTS.md) first**; all new code must pass `cargo check` / `clippy` / `test`.
3. Commit messages may be Chinese or English, but must state *what changed / what was left alone / root cause*.
4. Open a Pull Request; CI passes on the three gates plus `scripts/smoke.sh`.

---

## License

This project defaults to the **MIT License** (permissive; commercial use and modification allowed,
copyright notice required). To adopt another license (e.g. AGPL, fitting the "ontology is the
constitution" open-governance ethos), please open an Issue to discuss.

Copyright © 2026 shijianzhong
