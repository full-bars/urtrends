use actix_web::{web, HttpResponse};
use serde_json::json;
use chrono::Duration;
use std::collections::HashMap;

use crate::{AppState, models::*, db, regions};

pub async fn api_live_total(data: web::Data<AppState>) -> actix_web::Result<HttpResponse> {
    let pool = &data.pool;
    
    #[derive(sqlx::FromRow)]
    struct Record {
        timestamp: String,
        total: Option<i32>,
    }
    
    let result = sqlx::query_as::<_, Record>(
        "SELECT timestamp, SUM(provider_count) as total FROM provider_counts GROUP BY timestamp ORDER BY timestamp DESC LIMIT 1"
    )
    .fetch_optional(pool)
    .await.map_err(actix_web::error::ErrorInternalServerError)?
    .unwrap_or(Record { timestamp: String::new(), total: Some(0) });

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "timestamp": result.timestamp,
        "total": result.total.unwrap_or(0)
    })))
}

pub async fn api_summary(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;

    // Check cache (30s TTL — data only changes every 15min via poller)
    {
        let cache = state.summary_cache.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.at.elapsed().as_secs() < 30 {
                return HttpResponse::Ok().json(&cached.data);
            }
        }
    }

    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let hour_ago = format_time_offset(&latest, -60);
    let day_ago = format_time_offset(&latest, -1440);
    let week_ago = format_time_offset(&latest, -10080);
    let two_week_ago = format_time_offset(&latest, -20160);
    let month_ago = format_time_offset(&latest, -43200);

    // Run all independent queries in parallel
    let (
        current_total_r,
        hour_ago_total_r,
        day_ago_total_r,
        week_ago_total_r,
        two_week_ago_total_r,
        top_10_r,
        hour_range_r,
        day_range_r,
        week_range_r,
        two_week_range_r,
        month_range_r,
        ath_atl_r,
    ) = tokio::join!(
        db::get_total_at_timestamp(pool, &latest),
        db::get_total_at_timestamp(pool, &hour_ago),
        db::get_total_at_timestamp(pool, &day_ago),
        db::get_total_at_timestamp(pool, &week_ago),
        db::get_total_at_timestamp(pool, &two_week_ago),
        db::get_top_countries(pool, &latest, 10),
        db::get_network_range(pool, &hour_ago, &latest),
        db::get_network_range(pool, &day_ago, &latest),
        db::get_network_range(pool, &week_ago, &latest),
        db::get_network_range(pool, &two_week_ago, &latest),
        db::get_network_range(pool, &month_ago, &latest),
        db::get_ath_atl(pool),
    );

    let current_total = current_total_r.unwrap_or(None).unwrap_or(0);
    let hour_ago_total = hour_ago_total_r.unwrap_or(None).unwrap_or(current_total);
    let day_ago_total = day_ago_total_r.unwrap_or(None).unwrap_or(current_total);
    let week_ago_total = week_ago_total_r.unwrap_or(None).unwrap_or(current_total);
    let two_week_ago_total = two_week_ago_total_r.unwrap_or(None).unwrap_or(current_total);

    let hour_delta = current_total - hour_ago_total;
    let day_delta = current_total - day_ago_total;
    let week_delta = current_total - week_ago_total;
    let two_week_delta = current_total - two_week_ago_total;

    let top_10 = top_10_r.unwrap_or_default();

    let (hour_high, hour_low) = hour_range_r.unwrap_or((0, 0));
    let (day_high, day_low) = day_range_r.unwrap_or((0, 0));
    let (week_high, week_low) = week_range_r.unwrap_or((0, 0));
    let (two_week_high, two_week_low) = two_week_range_r.unwrap_or((0, 0));
    let (month_high, month_low) = month_range_r.unwrap_or((0, 0));
    let ((ath_value, ath_ts), (atl_value, atl_ts)) = ath_atl_r.unwrap_or(((current_total, latest.clone()), (current_total, latest.clone())));

    let response = SummaryResponse {
        timestamp: latest,
        total: current_total,
        hour_delta,
        day_delta,
        week_delta,
        two_week_delta,
        top_10,
        hour_range: (hour_low, hour_high),
        day_range: (day_low, day_high),
        week_range: (week_low, week_high),
        two_week_range: (two_week_low, two_week_high),
        month_range: (month_low, month_high),
        ath: AthAtl { value: ath_value, timestamp: ath_ts },
        atl: AthAtl { value: atl_value, timestamp: atl_ts },
    };

    // Cache the response
    {
        let mut cache = state.summary_cache.lock().unwrap();
        *cache = Some(CachedResponse {
            data: response.clone(),
            at: std::time::Instant::now(),
        });
    }

    HttpResponse::Ok().json(response)
}

/// Fixed budget set for ?max_points=. Floors to the largest bucket <= v, so
/// the public parameter can only mint a tiny number of distinct cache keys.
const MAX_POINTS_BUCKETS: [i64; 6] = [500, 1000, 1500, 2000, 3000, 5000];

fn quantize_max_points(v: i64) -> i64 {
    let mut q = MAX_POINTS_BUCKETS[0];
    for b in MAX_POINTS_BUCKETS.iter() {
        if v >= *b {
            q = *b;
        }
    }
    q
}

pub async fn api_network_total(state: web::Data<AppState>, query: web::Query<HashMap<String, String>>) -> HttpResponse {
    let pool = &state.pool;

    let raw = query.get("range").map(|s| s.as_str()).unwrap_or("30");
    // Normalize: unknown values fall back to the 30-day arm AND the canonical
    // "30" cache key, so arbitrary ?range= values cannot mint unbounded keys.
    let (key, limit) = match raw {
        "7" => ("7", Some(672)), // 7 days x 96 snapshots/day
        "all" => ("all", None),  // no limit = full history
        _ => ("30", Some(2880)), // 30 days x 96 snapshots/day
    };

    // Point budget. Quantize so ?max_points= cannot mint unbounded cache
    // keys either, mirroring the canonical-range fix above.
    let raw_mp = query
        .get("max_points")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(2000)
        .clamp(100, 1_000_000);
    let max_points = quantize_max_points(raw_mp);
    let cache_key = format!("{}|{}", key, max_points);

    {
        let cache = state.network_total_cache.lock().unwrap();
        if let Some(ref cached) = cache.get(&cache_key) {
            if cached.at.elapsed().as_secs() < 30 {
                return HttpResponse::Ok().json(&cached.data);
            }
        }
    }

    match db::get_network_totals(pool, limit, Some(max_points)).await {
        Ok(data) => {
            let mut cache = state.network_total_cache.lock().unwrap();
            cache.insert(cache_key, CachedResponse {
                data: data.clone(),
                at: std::time::Instant::now(),
            });
            HttpResponse::Ok().json(data)
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

pub async fn api_regions(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;
    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let day_ago = format_time_offset(&latest, -1440);
    let current_countries = match db::get_countries_at_timestamp(pool, &latest).await {
        Ok(countries) => countries,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let day_ago_ts = match db::get_nearest_timestamp(pool, &day_ago).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };
    let day_ago_countries = match db::get_countries_at_timestamp(pool, &day_ago_ts).await {
        Ok(countries) => countries,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };
    let past_map: std::collections::HashMap<String, i32> = day_ago_countries
        .into_iter()
        .map(|c| (c.country_code, c.provider_count))
        .collect();

    let mut region_totals: HashMap<&'static str, (i32, i32)> = HashMap::new();

    for cc in current_countries.iter() {
        let region = regions::get_region(&cc.country_code);
        let entry = region_totals.entry(region).or_insert((0, 0));
        entry.0 += cc.provider_count;
        if let Some(past) = past_map.get(&cc.country_code) {
            entry.1 += cc.provider_count - past;
        }
    }

    let mut result: Vec<Region> = region_totals
        .into_iter()
        .map(|(region, (total, delta))| Region {
            region: region.to_string(),
            total,
            delta_24h: delta,
        })
        .collect();

    result.sort_by(|a, b| b.total.cmp(&a.total));
    HttpResponse::Ok().json(result)
}

pub async fn api_at_risk(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;
    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let day_ago = format_time_offset(&latest, -1440);
    let current_countries = match db::get_countries_at_timestamp(pool, &latest).await {
        Ok(countries) => countries,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let day_ago_countries = db::get_countries_at_timestamp(pool, &day_ago).await.unwrap_or_default();
    let past_map: HashMap<String, i32> = day_ago_countries.into_iter()
        .map(|c| (c.country_code, c.provider_count))
        .collect();

    let mut disappeared = Vec::new();
    let mut near_zero = Vec::new();

    for cc in current_countries.iter() {
        let past = past_map.get(&cc.country_code).copied();
        if cc.provider_count == 0 {
            if past.is_some_and(|p| p > 0) {
                disappeared.push(CountryCount {
                    country_code: cc.country_code.clone(),
                    country_name: cc.country_name.clone(),
                    provider_count: cc.provider_count,
                });
            }
        } else if cc.provider_count >= 1 && cc.provider_count <= 5 {
            if past.is_some_and(|p| cc.provider_count < p) {
                near_zero.push(CountryCount {
                    country_code: cc.country_code.clone(),
                    country_name: cc.country_name.clone(),
                    provider_count: cc.provider_count,
                });
            }
        }
    }

    HttpResponse::Ok().json(json!({
        "disappeared": disappeared,
        "near_zero": near_zero
    }))
}

pub async fn api_anomalies(state: web::Data<AppState>, query: web::Query<HashMap<String, String>>) -> HttpResponse {
    let pool = &state.pool;
    let threshold_pct: f64 = query
        .get("threshold")
        .and_then(|s| s.parse().ok())
        .unwrap_or(15.0);
    let threshold = threshold_pct / 100.0;

    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let hour_ago = format_time_offset(&latest, -60);

    match db::get_anomalies(pool, &latest, &hour_ago, threshold).await {
        Ok((gains, losses)) => {
            let combined: Vec<_> = gains
                .into_iter()
                .chain(losses)
                .collect();
            
            let response = combined.iter().map(|a| json!({
                "country_code": a.country_code,
                "country_name": a.country_name,
                "provider_count": a.provider_count,
                "delta": a.delta,
                "pct_change": a.percent_change
            })).collect::<Vec<_>>();

            HttpResponse::Ok().json(json!({"anomalies": response, "threshold": threshold_pct}))
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

pub async fn api_movers(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;
    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let hour_ago = format_time_offset(&latest, -60);
    let day_ago = format_time_offset(&latest, -1440);
    let week_ago = format_time_offset(&latest, -10080);

    let (hour_gainers, hour_losers) = db::get_movers(pool, &hour_ago, &latest).await.unwrap_or((vec![], vec![]));
    let (day_gainers, day_losers) = db::get_movers(pool, &day_ago, &latest).await.unwrap_or((vec![], vec![]));
    let (week_gainers, week_losers) = db::get_movers(pool, &week_ago, &latest).await.unwrap_or((vec![], vec![]));

    let response = json!({
        "1h": {"gainers": hour_gainers, "losers": hour_losers},
        "24h": {"gainers": day_gainers, "losers": day_losers},
        "7d": {"gainers": week_gainers, "losers": week_losers}
    });

    HttpResponse::Ok().json(response)
}

pub async fn api_movers_detailed(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;
    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let windows = vec![
        ("15m", 15),
        ("1h", 60),
        ("2h", 120),
        ("3h", 180),
        ("6h", 360),
        ("12h", 720),
        ("24h", 1440),
        ("2d", 2880),
        ("3d", 4320),
        ("4d", 5760),
        ("5d", 7200),
        ("6d", 8640),
        ("7d", 10080),
        ("14d", 20160),
        ("30d", 43200),
    ];

    let mut current_countries = match db::get_countries_at_timestamp(pool, &latest).await {
        Ok(c) => c,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    // If the API returned a degraded snapshot, merge in missing countries
    // from the last complete snapshot so the movers tables stay populated
    if current_countries.len() < 80 {
        if let Ok(fallback) = db::get_last_complete_snapshot(pool, 80).await {
            let current_codes: std::collections::HashSet<String> = current_countries.iter()
                .map(|c| c.country_code.clone())
                .collect();
            for fc in fallback {
                if !current_codes.contains(&fc.country_code) {
                    current_countries.push(fc);
                }
            }
        }
    }

    let mut past_data: HashMap<String, HashMap<String, i32>> = HashMap::new();
    for (window_name, minutes) in windows.iter() {
        let past_time = format_time_offset(&latest, -(*minutes as i32));
        if let Ok(ts) = db::get_nearest_timestamp(pool, &past_time).await {
            if let Ok(countries) = db::get_countries_at_timestamp(pool, &ts).await {
                let map: HashMap<String, i32> = countries.into_iter()
                    .map(|c| (c.country_code, c.provider_count))
                    .collect();
                past_data.insert(window_name.to_string(), map);
            }
        }
    }

    let mut country_data: HashMap<String, serde_json::Value> = HashMap::new();

    for cc in current_countries.iter() {
        let mut deltas = HashMap::new();

        for (window_name, _) in windows.iter() {
            let delta_val = match past_data.get(*window_name).and_then(|m| m.get(&cc.country_code)) {
                Some(past_count) => json!(cc.provider_count - past_count),
                None => json!(null),
            };
            deltas.insert(window_name.to_string(), delta_val);
        }

        country_data.insert(
            cc.country_code.clone(),
            json!({
                "code": cc.country_code,
                "name": cc.country_name,
                "current": cc.provider_count,
                "deltas": deltas
            }),
        );
    }

    // Get top 50 gainers (largest positive delta, sorted by country code for stability)
    let mut gainers_vec: Vec<_> = country_data.values().cloned().collect();
    gainers_vec.sort_by(|a, b| {
        let delta_a = a.get("deltas").and_then(|d| d.get("24h")).and_then(|v| v.as_i64()).unwrap_or(0);
        let delta_b = b.get("deltas").and_then(|d| d.get("24h")).and_then(|v| v.as_i64()).unwrap_or(0);
        match delta_b.cmp(&delta_a) {
            std::cmp::Ordering::Equal => {
                let code_a = a.get("code").and_then(|v| v.as_str()).unwrap_or("");
                let code_b = b.get("code").and_then(|v| v.as_str()).unwrap_or("");
                code_a.cmp(code_b)
            }
            other => other,
        }
    });
    let gainers: Vec<_> = gainers_vec.into_iter().take(50).collect();

    // Get top 50 losers (smallest negative delta, sorted by country code for stability)
    let mut losers_vec: Vec<_> = country_data.values().cloned().collect();
    losers_vec.sort_by(|a, b| {
        let delta_a = a.get("deltas").and_then(|d| d.get("24h")).and_then(|v| v.as_i64()).unwrap_or(0);
        let delta_b = b.get("deltas").and_then(|d| d.get("24h")).and_then(|v| v.as_i64()).unwrap_or(0);
        match delta_a.cmp(&delta_b) {
            std::cmp::Ordering::Equal => {
                let code_a = a.get("code").and_then(|v| v.as_str()).unwrap_or("");
                let code_b = b.get("code").and_then(|v| v.as_str()).unwrap_or("");
                code_a.cmp(code_b)
            }
            other => other,
        }
    });
    let losers: Vec<_> = losers_vec.into_iter().take(50).collect();

    HttpResponse::Ok().json(json!({"gainers": gainers, "losers": losers}))
}

pub async fn api_top_countries(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;

    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let countries = match db::get_countries_at_timestamp(pool, &latest).await {
        Ok(c) => c,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let mut sorted: Vec<_> = countries
        .into_iter()
        .map(|c| json!({"code": c.country_code, "name": c.country_name, "current": c.provider_count}))
        .collect();

    sorted.sort_by(|a, b| {
        let count_a = a.get("current").and_then(|v| v.as_i64()).unwrap_or(0);
        let count_b = b.get("current").and_then(|v| v.as_i64()).unwrap_or(0);
        count_b.cmp(&count_a)
    });

    let top_5: Vec<_> = sorted.into_iter().take(5).collect();
    HttpResponse::Ok().json(top_5)
}

pub async fn api_country_stats(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    let pool = &state.pool;
    let code = path.into_inner();

    let data = match db::get_country_history(pool, &code, 96).await {
        Ok(d) => d,
        Err(_) => return HttpResponse::Ok().json(json!({"volatility": "N/A", "churn_rate": 0})),
    };

    if data.len() < 2 {
        return HttpResponse::Ok().json(json!({"volatility": "N/A", "churn_rate": 0}));
    }

    let changes: Vec<i32> = (0..data.len() - 1)
        .map(|i| (data[i + 1].provider_count - data[i].provider_count).abs())
        .collect();

    let avg_change = if changes.is_empty() {
        0.0
    } else {
        changes.iter().sum::<i32>() as f64 / changes.len() as f64
    };

    let volatility = if avg_change > 100.0 {
        "high"
    } else if avg_change > 50.0 {
        "medium"
    } else {
        "low"
    };

    HttpResponse::Ok().json(json!({
        "volatility": volatility,
        "churn_rate": (avg_change * 10.0).round() / 10.0
    }))
}

pub async fn api_country(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    let pool = &state.pool;
    let code = path.into_inner();

    match db::get_country_history(pool, &code, 720).await {
        Ok(data) => {
            let response: Vec<_> = data
                .iter()
                .map(|d| json!({"timestamp": d.timestamp, "count": d.provider_count}))
                .collect();
            HttpResponse::Ok().json(response)
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

pub async fn api_growth_projection(state: web::Data<AppState>) -> HttpResponse {
    let pool = &state.pool;
    let latest = match db::get_latest_timestamp(pool).await {
        Ok(ts) => ts,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };

    let current = db::get_total_at_timestamp(pool, &latest).await.unwrap_or(None).unwrap_or(0);
    let past_1d = db::get_total_at_timestamp(pool, &format_time_offset(&latest, -1440)).await.unwrap_or(None);
    let past_3d = db::get_total_at_timestamp(pool, &format_time_offset(&latest, -4320)).await.unwrap_or(None);
    let past_7d = db::get_total_at_timestamp(pool, &format_time_offset(&latest, -10080)).await.unwrap_or(None);

    let mut model = "weighted-exponential";
    let (projected_7d, projected_14d, projected_30d, projected_90d, growth_rate, daily_growth) = if current == 0 || past_3d.is_none() || past_7d.is_none() || past_3d == Some(0) || past_7d == Some(0) {
        // Fallback to linear 1-day logic
        let p1 = past_1d.unwrap_or(current);
        let dg = current - p1;
        let gr = if p1 > 0 { ((dg as f64) / (p1 as f64)) * 100.0 } else { 0.0 };
        model = "linear-1d-fallback";
        (
            (current as i32 + (dg * 7)) as i32,
            (current as i32 + (dg * 14)) as i32,
            (current as i32 + (dg * 30)) as i32,
            (current as i32 + (dg * 90)) as i32,
            gr,
            dg
        )
    } else {
        let c = current as f64;
        let p3 = past_3d.unwrap() as f64;
        let p7 = past_7d.unwrap() as f64;
        
        // Calculate daily compounded growth rates for 3d and 7d horizons
        let r3 = (c / p3).powf(1.0/3.0) - 1.0;
        let r7 = (c / p7).powf(1.0/7.0) - 1.0;
        
        // Weighting: 70% to stable 7-day trend, 30% to recent 3-day momentum
        let mut r_weighted = (0.7 * r7) + (0.3 * r3);
        
        // Cap r_weighted to +/- 5% daily to prevent astronomical projections from anomalies
        r_weighted = r_weighted.max(-0.05).min(0.05);
        
        // Exponential Projection: current * (1 + r)^n
        let p7 = (c * (1.0 + r_weighted).powf(7.0)) as i32;
        let p14 = (c * (1.0 + r_weighted).powf(14.0)) as i32;
        let p30 = (c * (1.0 + r_weighted).powf(30.0)) as i32;
        let p90 = (c * (1.0 + r_weighted).powf(90.0)) as i32;
        
        let dg = current - past_1d.unwrap_or(current);
        (p7, p14, p30, p90, r_weighted * 100.0, dg)
    };

    HttpResponse::Ok().json(json!({
        "current": current,
        "daily_growth": daily_growth,
        "growth_rate": growth_rate.max(-100.0).min(100.0),
        "projected_7d": projected_7d.max(0),
        "projected_14d": projected_14d.max(0),
        "projected_30d": projected_30d.max(0),
        "projected_90d": projected_90d.max(0),
        "model": model
    }))
}

pub async fn api_churn(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    let pool = &state.pool;
    let code = path.into_inner();

    match db::get_country_history(pool, &code, 24).await {
        Ok(data) => {
            let response: Vec<_> = data.iter().map(|d| json!({"timestamp": d.timestamp, "count": d.provider_count})).collect();
            if response.len() < 2 {
                return HttpResponse::Ok().json(json!({"churn_rate": 0, "volatility": "N/A", "data": response}));
            }
            let changes: Vec<i32> = (0..response.len()-1).map(|i| (response[i+1]["count"].as_i64().unwrap_or(0) - response[i]["count"].as_i64().unwrap_or(0)).abs() as i32).collect();
            let avg_change = if changes.is_empty() { 0.0 } else { changes.iter().sum::<i32>() as f64 / changes.len() as f64 };
            let volatility = if avg_change > 100.0 { "high" } else if avg_change > 50.0 { "medium" } else { "low" };
            HttpResponse::Ok().json(json!({"churn_rate": avg_change, "volatility": volatility, "data": response}))
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

pub async fn api_comparison(state: web::Data<AppState>, path: web::Path<(String, String)>) -> HttpResponse {
    let pool = &state.pool;
    let (code1, code2) = path.into_inner();

    let data1 = match db::get_country_history(pool, &code1, 720).await {
        Ok(d) => d.iter().map(|d| json!({"timestamp": d.timestamp, "code": code1.to_uppercase(), "count": d.provider_count})).collect::<Vec<_>>(),
        Err(_) => vec![],
    };
    let data2 = match db::get_country_history(pool, &code2, 720).await {
        Ok(d) => d.iter().map(|d| json!({"timestamp": d.timestamp, "code": code2.to_uppercase(), "count": d.provider_count})).collect::<Vec<_>>(),
        Err(_) => vec![],
    };

    HttpResponse::Ok().json(json!({"data1": data1, "data2": data2}))
}

// Helper function - offset is in minutes
fn format_time_offset(timestamp: &str, offset_minutes: i32) -> String {
    // Try RFC3339 first, then fall back to naive datetime format
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        let offset_dt = dt + Duration::minutes(offset_minutes as i64);
        offset_dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S") {
        let offset_ndt = ndt + Duration::minutes(offset_minutes as i64);
        offset_ndt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        timestamp.to_string()
    }
}
