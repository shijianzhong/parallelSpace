-- 0003：积分兑换申请（AGENTS §5.7）。
-- 流程：申请即冻结扣减（credit_transactions type='redeem_hold'）→ admin 审批
-- → 人工打款后 settle（status='paid'）/ 驳回自动释放（type='redeem_release'）。
CREATE TABLE redeem_requests (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    credits INTEGER NOT NULL,
    amount_cents INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'rejected')),
    payout_ref TEXT,
    decided_by TEXT,
    decided_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_redeem_account ON redeem_requests(account_id, status);
