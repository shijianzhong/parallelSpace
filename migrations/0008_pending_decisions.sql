-- 0008：用户决策收件箱（G15，docs/GAPS_AND_PLAN.md 第十三节）。
-- 离线代理 Defer 的落地形态：所有者不在场时，分身的不可逆动作不直接
-- 拒绝，而是生成待决项挂起待批；所有者上线后批准/驳回，批准才重新走
-- action 入口校验落事件。挂起 72 小时未决策自动驳回（status='expired'，
-- 惰性过期：读取收件箱 / 审批尝试 / 调度 tick 三处触发）。
-- 载荷 payload 是完整 ActionRequest JSON（批准重放用）；只批/驳，
-- 不提供修改载荷的入口（append-only 口径）。
CREATE TABLE pending_decisions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    account_id TEXT NOT NULL REFERENCES accounts(id),
    action_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'rejected', 'expired')),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    decided_at TEXT,
    decided_by TEXT,
    event_id TEXT,
    reject_reason TEXT,
    audit_id TEXT
);

CREATE INDEX idx_pending_decisions_account ON pending_decisions(account_id, status, created_at);
CREATE INDEX idx_pending_decisions_agent ON pending_decisions(agent_id, status);
CREATE INDEX idx_pending_decisions_expiry ON pending_decisions(status, expires_at);
