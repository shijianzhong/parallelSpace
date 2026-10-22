//! 地图图层：GET /api/map/agents?bbox=minLon,minLat,maxLon,maxLat&status=active&limit=500
//! （AGENTS.md §7.5）。
//!
//! 返回 GeoJSON FeatureCollection；数据来自 D1（agents + 位置），
//! MVP 不做 SPARQL（本体实例层在 M2 全量入 RDF 后切到 ontology/sparql.rs 的模板）。

use worker::{Response, RouteContext};

use crate::common::error::AppResult;
use crate::common::error::AppError;
use crate::storage::d1;

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
    if !(-180.0..=180.0).contains(&min_lon)
        || !(-180.0..=180.0).contains(&max_lon)
        || !(-90.0..=90.0).contains(&min_lat)
        || !(-90.0..=90.0).contains(&max_lat)
    {
        return Err(AppError::InvalidInput("bbox out of WGS84 range".into()));
    }
    Ok((min_lon, min_lat, max_lon, max_lat))
}

pub async fn agents_layer(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let url = req.url()?;
    let query: std::collections::HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let limit: u32 = query
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
        .clamp(1, 1000);

    let bbox = match query.get("bbox") {
        Some(raw) => Some(parse_bbox(raw)?),
        None => None,
    };
    let status = query.get("status").cloned();

    let env = ctx.env.clone();
    let db = env.d1("DB")?;

    // MVP：取活跃分身（bbox 过滤在 M2 位置数据入 RDF 后下推）
    let status_bind = match &status {
        Some(s) => worker::wasm_bindgen::JsValue::from_str(s),
        None => worker::wasm_bindgen::JsValue::NULL,
    };
    let rows: Vec<d1::AgentRow> = db
        .prepare(
            "SELECT id, account_id, status, location_id, created_at, ontology_version \
             FROM agents WHERE (?1 IS NULL OR status = ?1) LIMIT ?2",
        )
        .bind(&[status_bind, worker::wasm_bindgen::JsValue::from_f64(limit as f64)])?
        .all()
        .await?
        .results()?;

    let features: Vec<serde_json::Value> = rows
        .iter()
        .map(|a| {
            serde_json::json!({
                "type": "Feature",
                "geometry": null,
                "properties": {
                    "agent_id": a.id,
                    "status": a.status,
                    "location_id": a.location_id,
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
}
