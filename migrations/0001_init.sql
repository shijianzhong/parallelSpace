-- 账号与风控
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    email TEXT UNIQUE NOT NULL,
    created_at TEXT NOT NULL,
    risk_score REAL NOT NULL DEFAULT 0
);

-- 分身主记录
CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    status TEXT NOT NULL,
    location_id TEXT,
    created_at TEXT NOT NULL,
    ontology_version TEXT NOT NULL
);
CREATE INDEX idx_agents_account ON agents(account_id);
CREATE INDEX idx_agents_status ON agents(status);

-- 分身热状态
CREATE TABLE agent_state (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id),
    money REAL NOT NULL DEFAULT 0,
    food REAL NOT NULL DEFAULT 0,
    energy REAL NOT NULL DEFAULT 100,
    reputation REAL NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

-- 关系
CREATE TABLE relationships (
    from_id TEXT NOT NULL REFERENCES agents(id),
    to_id TEXT NOT NULL REFERENCES agents(id),
    type TEXT NOT NULL,
    trust REAL NOT NULL DEFAULT 0.5,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (from_id, to_id, type)
);

-- 事件索引
CREATE TABLE events_index (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    actor_id TEXT,
    target_id TEXT,
    tick INTEGER NOT NULL,
    ontology_version TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_events_tick ON events_index(tick);
CREATE INDEX idx_events_actor ON events_index(actor_id);
CREATE INDEX idx_events_type ON events_index(type);

-- 契约
CREATE TABLE contracts (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    parties TEXT NOT NULL,  -- JSON 数组
    status TEXT NOT NULL,
    terms_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 组织
CREATE TABLE organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    charter_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 职位
CREATE TABLE positions (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id),
    title TEXT NOT NULL,
    holder_id TEXT REFERENCES agents(id),
    permissions TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 配额
CREATE TABLE quotas (
    agent_id TEXT NOT NULL,
    date TEXT NOT NULL,
    llm_tokens_used INTEGER NOT NULL DEFAULT 0,
    actions_used INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, date)
);

-- LLM 调用日志
CREATE TABLE llm_calls (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    model TEXT NOT NULL,
    model_version TEXT NOT NULL,
    temperature_millis INTEGER NOT NULL,
    seed INTEGER NOT NULL,
    prompt_hash TEXT NOT NULL,
    response_hash TEXT NOT NULL,
    tick INTEGER NOT NULL,
    ontology_version TEXT NOT NULL,
    validation_result TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_llm_prompt ON llm_calls(prompt_hash, ontology_version);

-- 支付订单
CREATE TABLE orders (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    amount_cents INTEGER NOT NULL,
    credits INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payment_channel TEXT NOT NULL,
    channel_txn_id TEXT,
    created_at TEXT NOT NULL,
    paid_at TEXT,
    confirmed_by TEXT,
    note TEXT
);
CREATE INDEX idx_orders_status ON orders(status);
CREATE INDEX idx_orders_account ON orders(account_id);
-- 手工支付由治理员手填渠道流水号：同一渠道流水号只能落在一笔订单上。
-- 少了这条约束，把同一个微信流水号填到另一笔待付订单就能二次入账。
CREATE UNIQUE INDEX idx_orders_channel_txn ON orders(payment_channel, channel_txn_id)
    WHERE channel_txn_id IS NOT NULL;

-- 积分账户
CREATE TABLE credit_accounts (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id),
    balance INTEGER NOT NULL DEFAULT 0,
    total_purchased INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

-- 积分流水（append-only）
CREATE TABLE credit_transactions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    delta INTEGER NOT NULL,
    type TEXT NOT NULL,
    order_id TEXT,
    event_id TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_credit_txn_account ON credit_transactions(account_id);

-- 本体提案索引
CREATE TABLE ontology_proposals (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    proposer TEXT NOT NULL,
    target_version TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 本体版本索引
CREATE TABLE ontology_versions (
    version TEXT PRIMARY KEY,
    published_at TEXT NOT NULL,
    prior_version TEXT,
    status TEXT NOT NULL
);
