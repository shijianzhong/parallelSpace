-- G4 接线顺带补 G7 遗留：组织创始人绑定（归属校验的落点）。
-- founder_account_id 记录建组织的账号（会话账号优先，见 api/org.rs）；
-- 历史组织无创始人记录 → NULL（未知，不回填伪造）。
ALTER TABLE organizations ADD COLUMN founder_account_id TEXT REFERENCES accounts(id);
