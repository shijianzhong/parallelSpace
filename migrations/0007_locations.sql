-- G8 位置数据模型：可指称地点表。
-- 本体口径：地点是 core:Location（基线 soc:Location）的结构化落点，
-- agents.location_id 即 core:locatedAt 指针（可选——L0 无公理要求 Thing 必须有位置）。
-- lon/lat/polygon 只是渲染投影（AGENTS.md §8.5），供地图图层输出 GeoJSON；
-- 坐标系与距离不进本体公理。地点由治理员经 admin 接口创建，不预置种子数据
-- （来源数据未就绪时地图返回空集而非造数据）。
CREATE TABLE locations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    lon REAL NOT NULL,
    lat REAL NOT NULL,
    polygon TEXT,               -- GeoJSON Polygon 外环 JSON：[[lon,lat],...]（闭合环），可空
    created_at TEXT NOT NULL
);
CREATE INDEX idx_agents_location ON agents(location_id);
