-- 0004：用户登录身份（G7）。一个账号可绑定多个登录方式（email / google / github），
-- 同邮箱自动关联同一账号。会话本身存 KV（session:{token}），不入 D1。
CREATE TABLE user_identities (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    provider TEXT NOT NULL CHECK (provider IN ('email', 'google', 'github')),
    provider_subject TEXT NOT NULL,
    email TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (provider, provider_subject)
);

CREATE INDEX idx_identity_account ON user_identities(account_id);
