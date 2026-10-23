//! 地图图层（AGENTS.md §8.5）：
//! - GET /api/map/agents?bbox=minLon,minLat,maxLon,maxLat&status=active&limit=500
//! - GET /api/map/locations?bbox=...&limit=500
//!
//! 返回 GeoJSON FeatureCollection；数据来自 D1（agents + locations，G8）。
//! MVP 不做 SPARQL（本体实例层在 M2 全量入 RDF 后切到 ontology/sparql.rs 的模板）。
//!
//! 位置语义只到 core:locatedAt「可指称」为止：agents.location_id 是指针，
//! locations 的 lon/lat/polygon 只是渲染投影——坐标系与距离不进本体公理。
//! 无位置的分身 geometry 为 null（如实呈现，不造数据）；bbox 过滤下推到 SQL，
//! 给了 bbox 时无位置分身不入集（无从判定其在框内）。

use serde::Deserialize;
use worker::{Response, RouteContext};

use crate::common::error::AppResult;
use crate::common::error::AppError;
use crate::storage::d1;

/// 校验经纬度在 WGS84 范围内（纯函数，admin 建地点与 bbox 共用口径）。
pub fn validate_lon_lat(lon: f64, lat: f64) -> AppResult<()> {
    if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
        return Err(AppError::InvalidInput("lon out of WGS84 range".into()));
    }
    if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return Err(AppError::InvalidInput("lat out of WGS84 range".into()));
    }
    Ok(())
}

/// GeoJSON Point 几何（纯函数，可单测）。
pub fn point_geometry(lon: f64, lat: f64) -> serde_json::Value {
    serde_json::json!({ "type": "Point", "coordinates": [lon, lat] })
}

/// 规范化 Polygon 外环（纯函数，可单测）：入参是 `[[lon,lat],...]` JSON 数组，
/// 每点须在 WGS84 范围内、至少 3 个不同顶点；未闭合时自动补首点闭合。
/// 返回闭合环坐标数组。admin 建地点时校验并以此形态落库。
pub fn normalize_polygon(raw: &serde_json::Value) -> AppResult<Vec<[f64; 2]>> {
    let arr = raw
        .as_array()
        .ok_or_else(|| AppError::InvalidInput("polygon must be an array of [lon,lat] pairs".into()))?;
    let mut ring: Vec<[f64; 2]> = Vec::with_capacity(arr.len() + 1);
    for p in arr {
        let pair = p
            .as_array()
            .ok_or_else(|| AppError::InvalidInput("polygon point must be [lon,lat]".into()))?;
        if pair.len() != 2 {
            return Err(AppError::InvalidInput("polygon point must be [lon,lat]".into()));
        }
        let lon = pair[0]
            .as_f64()
            .ok_or_else(|| AppError::InvalidInput("polygon lon must be a number".into()))?;
        let lat = pair[1]
            .as_f64()
            .ok_or_else(|| AppError::InvalidInput("polygon lat must be a number".into()))?;
        validate_lon_lat(lon, lat)?;
        ring.push([lon, lat]);
    }
    // 去掉与首点重复的尾点后计数不同顶点（闭合点不算新顶点）
    let distinct = {
        let mut pts: Vec<[f64; 2]> = ring.clone();
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        pts.len()
    };
    if distinct < 3 {
        return Err(AppError::InvalidInput("polygon needs at least 3 distinct points".into()));
    }
    if ring.first() != ring.last() {
        let first = ring[0];
        ring.push(first);
    }
    Ok(ring)
}

/// 从落库的 polygon JSON 构造 GeoJSON Polygon 几何（纯函数，可单测）；
/// 存储值损坏时返回 None，读路径降级为 Point 而不是报错。
pub fn polygon_geometry(stored: &str) -> Option<serde_json::Value> {
    let raw: serde_json::Value = serde_json::from_str(stored).ok()?;
    let ring = normalize_polygon(&raw).ok()?;
    Some(serde_json::json!({ "type": "Polygon", "coordinates": [ring] }))
}

/// 地点的 GeoJSON 几何：有合法 polygon 用 Polygon，否则 Point（代表点 lon/lat）。
pub fn location_geometry(loc: &d1::LocationRow) -> serde_json::Value {
    match loc.polygon.as_deref().and_then(polygon_geometry) {
        Some(g) => g,
        None => point_geometry(loc.lon, loc.lat),
    }
}

/// 解析 bbox 查询参数 "minLon,minLat,maxLon,maxLat"（纯函数，可单测）。
pub fn parse_bbox(raw: &str) -> AppResult<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = raw
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<std::result::Result<Vec<f64>, _>>()
        .map_err(|_| AppError::InvalidInput("bbox must be minLon,minLat,maxLon,maxLat".into()))?;
    if parts.len() != 4 {
        return Err(AppError::InvalidInput("bbox needs 4 numbers".into()));
    }
    let (min_lon, min_lat, max_lon, max_lat) = (parts[0], parts[1], parts[2], parts[3]);
    if min_lon > max_lon || min_lat > max_lat {
        return Err(AppError::InvalidInput("bbox min must be <= max".into()));
    }
    validate_lon_lat(min_lon, min_lat)?;
    validate_lon_lat(max_lon, max_lat)?;
    Ok((min_lon, min_lat, max_lon, max_lat))
}

fn query_params(req: &worker::Request) -> AppResult<std::collections::HashMap<String, String>> {
    let url = req.url()?;
    Ok(url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect())
}

fn parse_limit(query: &std::collections::HashMap<String, String>) -> u32 {
    query
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
        .clamp(1, 1000)
}

/// bbox 的 4 个绑定值：无 bbox 时全 NULL（SQL 侧 `?2 IS NULL OR ...` 短路）。
fn bbox_binds(bbox: &Option<(f64, f64, f64, f64)>) -> [worker::wasm_bindgen::JsValue; 4] {
    use worker::wasm_bindgen::JsValue;
    match bbox {
        Some((a, b, c, d)) => [
            JsValue::from_f64(*a),
            JsValue::from_f64(*b),
            JsValue::from_f64(*c),
            JsValue::from_f64(*d),
        ],
        None => [JsValue::NULL, JsValue::NULL, JsValue::NULL, JsValue::NULL],
    }
}

#[derive(Debug, Deserialize)]
struct MapAgentRow {
    id: String,
    status: String,
    location_id: Option<String>,
    location_name: Option<String>,
    lon: Option<f64>,
    lat: Option<f64>,
}

pub async fn agents_layer(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let query = query_params(&req)?;
    let limit = parse_limit(&query);
    let bbox = match query.get("bbox") {
        Some(raw) => Some(parse_bbox(raw)?),
        None => None,
    };
    let status = query.get("status").cloned();

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    // 位置经 LEFT JOIN locations 取渲染投影；bbox 在 SQL 侧下推过滤
    // （lon/lat 为 NULL 的行在给了 bbox 时自然不入集）。
    let status_bind = match &status {
        Some(s) => worker::wasm_bindgen::JsValue::from_str(s),
        None => worker::wasm_bindgen::JsValue::NULL,
    };
    let bb = bbox_binds(&bbox);
    let rows: Vec<MapAgentRow> = db
        .prepare(
            "SELECT a.id AS id, a.status AS status, a.location_id AS location_id, \
                    l.name AS location_name, l.lon AS lon, l.lat AS lat \
             FROM agents a LEFT JOIN locations l ON l.id = a.location_id \
             WHERE (?1 IS NULL OR a.status = ?1) \
               AND (?2 IS NULL OR (l.lon >= ?2 AND l.lon <= ?4 AND l.lat >= ?3 AND l.lat <= ?5)) \
             LIMIT ?6",
        )
        .bind(&[
            status_bind,
            bb[0].clone(),
            bb[1].clone(),
            bb[2].clone(),
            bb[3].clone(),
            worker::wasm_bindgen::JsValue::from_f64(limit as f64),
        ])?
        .all()
        .await?
        .results()?;

    let features: Vec<serde_json::Value> = rows
        .iter()
        .map(|a| {
            let geometry = match (a.lon, a.lat) {
                (Some(lon), Some(lat)) => point_geometry(lon, lat),
                _ => serde_json::Value::Null,
            };
            serde_json::json!({
                "type": "Feature",
                "geometry": geometry,
                "properties": {
                    "agent_id": a.id,
                    "status": a.status,
                    "location_id": a.location_id,
                    "location_name": a.location_name,
                }
            })
        })
        .collect();

    Ok(Response::from_json(&serde_json::json!({
        "type": "FeatureCollection",
        "bbox": bbox.map(|(a, b, c, d)| vec![a, b, c, d]),
        "count": features.len(),
        "features": features,
    }))?)
}

/// 地点图层：全部可指称地点及其几何（Point 或 Polygon）。
pub async fn locations_layer(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let query = query_params(&req)?;
    let limit = parse_limit(&query);
    let bbox = match query.get("bbox") {
        Some(raw) => Some(parse_bbox(raw)?),
        None => None,
    };

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    // bbox 按代表点（lon/lat）下推过滤；Polygon 是否与框相交的精确判定不在 MVP。
    let bb = bbox_binds(&bbox);
    let rows: Vec<d1::LocationRow> = db
        .prepare(
            "SELECT id, name, lon, lat, polygon, created_at FROM locations \
             WHERE (?1 IS NULL OR (lon >= ?1 AND lon <= ?3 AND lat >= ?2 AND lat <= ?4)) \
             ORDER BY id LIMIT ?5",
        )
        .bind(&[
            bb[0].clone(),
            bb[1].clone(),
            bb[2].clone(),
            bb[3].clone(),
            worker::wasm_bindgen::JsValue::from_f64(limit as f64),
        ])?
        .all()
        .await?
        .results()?;

    let features: Vec<serde_json::Value> = rows
        .iter()
        .map(|l| {
            serde_json::json!({
                "type": "Feature",
                "geometry": location_geometry(l),
                "properties": {
                    "location_id": l.id,
                    "name": l.name,
                    "lon": l.lon,
                    "lat": l.lat,
                }
            })
        })
        .collect();

    Ok(Response::from_json(&serde_json::json!({
        "type": "FeatureCollection",
        "bbox": bbox.map(|(a, b, c, d)| vec![a, b, c, d]),
        "count": features.len(),
        "features": features,
    }))?)
}

#[cfg(test)]
mod tests {

    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn parse_bbox_accepts_valid() {
        assert_eq!(parse_bbox("116.0,39.0,117.0,40.0").unwrap(), (116.0, 39.0, 117.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn parse_bbox_rejects_malformed() {
        assert!(parse_bbox("116,39").is_err());
        assert!(parse_bbox("a,b,c,d").is_err());
        assert!(parse_bbox("117,39,116,40").is_err()); // min > max
        assert!(parse_bbox("999,39,1000,40").is_err()); // 超出 WGS84
        assert!(parse_bbox("116,39,117,40,extra").is_err());
    }

    #[wasm_bindgen_test]
    fn point_geometry_is_geojson() {
        let g = point_geometry(116.4, 39.9);
        assert_eq!(g["type"], "Point");
        assert_eq!(g["coordinates"], serde_json::json!([116.4, 39.9]));
    }

    #[wasm_bindgen_test]
    fn normalize_polygon_closes_ring() {
        let raw = serde_json::json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        let ring = normalize_polygon(&raw).unwrap();
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.first(), ring.last());
        // 已闭合的环不重复补点
        let closed = serde_json::json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]);
        assert_eq!(normalize_polygon(&closed).unwrap().len(), 4);
    }

    #[wasm_bindgen_test]
    fn normalize_polygon_rejects_bad_input() {
        assert!(normalize_polygon(&serde_json::json!([[0.0, 0.0], [1.0, 1.0]])).is_err()); // 顶点不足
        assert!(normalize_polygon(&serde_json::json!([[999.0, 0.0], [1.0, 0.0], [1.0, 1.0]])).is_err()); // 出界
        assert!(normalize_polygon(&serde_json::json!([["a", 0.0], [1.0, 0.0], [1.0, 1.0]])).is_err()); // 非数字
        assert!(normalize_polygon(&serde_json::json!("nope")).is_err()); // 非数组
    }

    #[wasm_bindgen_test]
    fn location_geometry_prefers_polygon_then_point() {
        let with_poly = d1::LocationRow {
            id: "l1".into(),
            name: "n".into(),
            lon: 10.0,
            lat: 20.0,
            polygon: Some("[[0,0],[1,0],[1,1],[0,0]]".into()),
            created_at: "t".into(),
        };
        let g = location_geometry(&with_poly);
        assert_eq!(g["type"], "Polygon");
        // 存储损坏 → 降级 Point，不报错
        let bad = d1::LocationRow {
            polygon: Some("{oops".into()),
            ..d1::LocationRow {
                id: "l2".into(),
                name: "n".into(),
                lon: 10.0,
                lat: 20.0,
                polygon: None,
                created_at: "t".into(),
            }
        };
        let g = location_geometry(&bad);
        assert_eq!(g["type"], "Point");
        assert_eq!(g["coordinates"], serde_json::json!([10.0, 20.0]));
    }
}
