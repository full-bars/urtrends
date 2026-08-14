use sqlx::SqlitePool;
use chrono::Duration;
use anyhow::Result;
use crate::models::*;

pub async fn get_latest_timestamp(pool: &SqlitePool) -> Result<String> {
    let result = sqlx::query_scalar::<_, String>(
        "SELECT MAX(timestamp) FROM provider_counts"
    )
    .fetch_one(pool)
    .await?;

    Ok(result)
}

pub async fn get_total_at_timestamp(pool: &SqlitePool, timestamp: &str) -> Result<Option<i32>> {
    let result = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT SUM(provider_count) FROM provider_counts
         WHERE timestamp = (SELECT timestamp FROM provider_counts WHERE timestamp <= ? ORDER BY timestamp DESC LIMIT 1)"
    )
    .bind(timestamp)
    .fetch_optional(pool)
    .await?
    .flatten();

    Ok(result)
}

pub async fn get_top_countries(pool: &SqlitePool, timestamp: &str, limit: i64) -> Result<Vec<CountryCount>> {
    let rows = sqlx::query_as::<_, CountryCount>(
        "SELECT country_code, country_name, provider_count FROM provider_counts
         WHERE timestamp = ? ORDER BY provider_count DESC LIMIT ?"
    )
    .bind(timestamp)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_network_totals(pool: &SqlitePool, limit: Option<i64>) -> Result<Vec<NetworkTotal>> {
    let rows = if let Some(limit) = limit {
        sqlx::query_as::<_, NetworkTotal>(
            "SELECT timestamp, SUM(provider_count) as total, NULL as ma FROM provider_counts
             GROUP BY timestamp ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, NetworkTotal>(
            "SELECT timestamp, SUM(provider_count) as total, NULL as ma FROM provider_counts
             GROUP BY timestamp ORDER BY timestamp DESC",
        )
        .fetch_all(pool)
        .await?
    };

    let mut result = rows;
    result.reverse();

    // Calculate 24-hour moving average
    let window = 24;
    for i in 0..result.len() {
        let start = if i >= window { i - window + 1 } else { 0 };
        let sum: i32 = result[start..=i].iter().map(|r| r.total).sum();
        let count = (i - start + 1) as i32;
        result[i].ma = Some((sum as f64 / count as f64).round() as i32);
    }

    Ok(result)
}

pub async fn get_nearest_timestamp(pool: &SqlitePool, timestamp: &str) -> Result<String> {
    let result = sqlx::query_scalar::<_, String>(
        "SELECT MAX(timestamp) FROM provider_counts WHERE timestamp <= ?"
    )
    .bind(timestamp)
    .fetch_one(pool)
    .await?;

    Ok(result)
}

pub async fn get_countries_at_timestamp(pool: &SqlitePool, timestamp: &str) -> Result<Vec<ProviderCount>> {
    let rows = sqlx::query_as::<_, ProviderCount>(
        "SELECT timestamp, country_code, country_name, provider_count FROM provider_counts
         WHERE timestamp = ? ORDER BY provider_count DESC"
    )
    .bind(timestamp)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_countries_before(pool: &SqlitePool, timestamp: &str) -> Result<Vec<ProviderCount>> {
    let nearest = get_nearest_timestamp(pool, timestamp).await?;
    get_countries_at_timestamp(pool, &nearest).await
}

pub async fn get_last_complete_snapshot(pool: &SqlitePool, min_countries: i32) -> Result<Vec<ProviderCount>> {
    let rows = sqlx::query_as::<_, ProviderCount>(
        "SELECT p.timestamp, p.country_code, p.country_name, p.provider_count
         FROM provider_counts p
         WHERE p.timestamp = (
             SELECT p2.timestamp FROM provider_counts p2
             GROUP BY p2.timestamp
             HAVING COUNT(*) >= ?
             ORDER BY p2.timestamp DESC LIMIT 1
         )"
    )
    .bind(min_countries)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_country_at_time(
    pool: &SqlitePool,
    country_code: &str,
    timestamp: &str,
) -> Result<Option<i32>> {
    let result = sqlx::query_scalar::<_, i32>(
        "SELECT provider_count FROM provider_counts WHERE country_code = ? AND timestamp <= ?
         ORDER BY timestamp DESC LIMIT 1"
    )
    .bind(country_code)
    .bind(timestamp)
    .fetch_optional(pool)
    .await?;

    Ok(result)
}

pub async fn get_movers(
    pool: &SqlitePool,
    since_timestamp: &str,
    current_timestamp: &str,
) -> Result<(Vec<Mover>, Vec<Mover>)> {
    let current = get_countries_at_timestamp(pool, current_timestamp).await?;

    let mut gainers = Vec::new();
    let mut losers = Vec::new();

    for cc in current.iter() {
        let past_count = get_country_at_time(pool, &cc.country_code, since_timestamp).await?;
        let delta = cc.provider_count - past_count.unwrap_or(0);

        let mover = Mover {
            country_code: cc.country_code.clone(),
            country_name: cc.country_name.clone(),
            provider_count: cc.provider_count,
            delta,
        };

        if delta > 0 {
            gainers.push(mover);
        } else if delta < 0 {
            losers.push(mover);
        }
    }

    gainers.sort_by(|a, b| b.delta.cmp(&a.delta));
    losers.sort_by(|a, b| a.delta.cmp(&b.delta));

    Ok((gainers.into_iter().take(10).collect(), losers.into_iter().take(10).collect()))
}

pub async fn get_country_history(pool: &SqlitePool, country_code: &str, limit: i64) -> Result<Vec<ProviderCount>> {
    let rows = sqlx::query_as::<_, ProviderCount>(
        "SELECT timestamp, country_code, country_name, provider_count FROM provider_counts
         WHERE country_code = ? ORDER BY timestamp DESC LIMIT ?"
    )
    .bind(country_code)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut result = rows;
    result.reverse();
    Ok(result)
}

pub async fn get_network_range(
    pool: &SqlitePool,
    since_timestamp: &str,
    until_timestamp: &str,
) -> Result<(i32, i32)> {
    #[derive(sqlx::FromRow)]
    struct Range {
        high: Option<i32>,
        low: Option<i32>,
    }

    let result = sqlx::query_as::<_, Range>(
        "WITH totals AS (
            SELECT SUM(provider_count) as total, timestamp
            FROM provider_counts
            WHERE timestamp >= ? AND timestamp <= ?
            GROUP BY timestamp
        )
        SELECT MAX(total) as high, MIN(total) as low
        FROM totals"
    )
    .bind(since_timestamp)
    .bind(until_timestamp)
    .fetch_one(pool)
    .await?;

    Ok((result.high.unwrap_or(0), result.low.unwrap_or(0)))
}

pub async fn get_ath_atl(pool: &SqlitePool) -> Result<((i32, String), (i32, String))> {
    #[derive(sqlx::FromRow)]
    struct Record {
        total: Option<i32>,
        timestamp: Option<String>,
    }

    async fn get_one(pool: &SqlitePool, atype: &str, order: &str) -> Result<(i32, String)> {
        // Try persisted table first (table may not exist yet on old DBs)
        let persisted = sqlx::query_as::<_, Record>(
            "SELECT value as total, timestamp FROM ath_atl WHERE metric_type = ?"
        )
        .bind(atype)
        .fetch_optional(pool)
        .await;

        if let Ok(Some(r)) = persisted {
            if let (Some(v), Some(ts)) = (r.total, r.timestamp) {
                return Ok((v, ts));
            }
        }

        // Fallback: compute from provider_counts
        let q = format!(
            "WITH totals AS (
                SELECT timestamp, SUM(provider_count) as total
                FROM provider_counts GROUP BY timestamp
            )
            SELECT timestamp, total FROM totals ORDER BY total {} LIMIT 1",
            order
        );
        sqlx::query_as::<_, Record>(&q)
            .fetch_optional(pool)
            .await?
            .and_then(|r| Some((r.total?, r.timestamp?)))
            .ok_or_else(|| anyhow::anyhow!("no data"))
    }

    let ath = get_one(pool, "ath", "DESC").await.unwrap_or((0, String::new()));
    let atl = get_one(pool, "atl", "ASC").await.unwrap_or((0, String::new()));

    Ok((ath, atl))
}

pub async fn get_anomalies(
    pool: &SqlitePool,
    current_timestamp: &str,
    hour_ago: &str,
    threshold: f64,
) -> Result<(Vec<Anomaly>, Vec<Anomaly>)> {
    let current = get_countries_at_timestamp(pool, current_timestamp).await?;

    let mut gains = Vec::new();
    let mut losses = Vec::new();

    for cc in current.iter() {
        let past_count = get_country_at_time(pool, &cc.country_code, hour_ago).await?;

        if let Some(past) = past_count {
            if past > 0 {
                let percent_change = ((cc.provider_count - past) as f64) / (past as f64);

                if percent_change.abs() > threshold {
                    let delta = cc.provider_count - past;
                    let anomaly = Anomaly {
                        country_code: cc.country_code.clone(),
                        country_name: cc.country_name.clone(),
                        provider_count: cc.provider_count,
                        delta,
                        percent_change: percent_change * 100.0,
                    };

                    if percent_change > 0.0 {
                        gains.push(anomaly);
                    } else {
                        losses.push(anomaly);
                    }
                }
            }
        }
    }

    gains.sort_by(|a, b| b.percent_change.abs().partial_cmp(&a.percent_change.abs()).unwrap());
    losses.sort_by(|a, b| b.percent_change.abs().partial_cmp(&a.percent_change.abs()).unwrap());

    Ok((gains, losses))
}
