-- parallelSpace 本体治理 v2：域注册表 + 两段式发布（pending → approve）
--
-- 存在论约定：L0 中立宪法不可注册、不可改；soc/gov/econ/geo/time 按约定
-- 降级为 L2 种子域——首次经审批的发布即激活该域（随种子激活）。

-- 域注册表：域是本体文件的承载单位（{domain}.ttl），替代原 api/admin.rs 的
-- 硬编码文件名白名单（那个白名单把新域锁死在外，本体无法随社会演化扩展）。
CREATE TABLE IF NOT EXISTS ontology_domains (
    domain TEXT PRIMARY KEY,
    layer TEXT NOT NULL,                     -- 'L1' 治理层 / 'L2' 自由涌现层（L0 不可注册）
    status TEXT NOT NULL DEFAULT 'seed',     -- 'seed' 种子 / 'active' 已激活 / 'retired' 退役
    description TEXT,
    registered_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    activated_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_domains_status ON ontology_domains(status);

-- 版本 ↔ 域：审批激活时按域把 seed 提升为 active
CREATE TABLE IF NOT EXISTS ontology_version_domains (
    version TEXT NOT NULL,
    domain TEXT NOT NULL,
    PRIMARY KEY (version, domain)
);

-- 两段式发布：publish 落 pending（proposed_by 记录提案者，approved_by 为空），
-- approve 才置 active 并写 KV ontology:version（未审批的版本不生效、不进历史）。
ALTER TABLE ontology_versions ADD COLUMN proposed_by TEXT;
ALTER TABLE ontology_versions ADD COLUMN approved_by TEXT;

-- 种子数据：原有五域降级为 L2 seed（system 注册，等待首次审批激活）。
-- 这是 2026-10-22 存在论纠偏的落地——它们从宪法层降级为「可能路径之一」。
INSERT OR IGNORE INTO ontology_domains (domain, layer, status, description, registered_by, created_at)
VALUES
    ('soc',  'L2', 'seed', '社会域：组织/科层/契约/声望等人类制度模板，降级为 L2 涌现候选', 'system', '2026-10-22T00:00:00Z'),
    ('gov',  'L2', 'seed', '治理域：宪法/法律/章程/选举等制度模板，降级为 L2 涌现候选', 'system', '2026-10-22T00:00:00Z'),
    ('econ', 'L2', 'seed', '经济域：货币/市场/雇佣/借贷等模板，降级为 L2 涌现候选', 'system', '2026-10-22T00:00:00Z'),
    ('geo',  'L2', 'seed', '地理域：位置可指称等中立空间原语的候选承载', 'system', '2026-10-22T00:00:00Z'),
    ('time', 'L2', 'seed', '时间域：离散 tick 排序原语的候选承载', 'system', '2026-10-22T00:00:00Z');
