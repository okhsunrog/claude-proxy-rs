use std::collections::HashMap;

use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::client_keys::i64_to_u64;
use crate::db::Connection;
use crate::subscription::timestamp_millis;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeseriesPoint {
    pub timestamp: u64,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeseriesResponse {
    pub period: String,
    pub granularity: String,
    pub points: Vec<TimeseriesPoint>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model: String,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdownResponse {
    pub period: String,
    pub models: Vec<ModelBreakdown>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyBreakdown {
    pub key_id: String,
    pub key_name: Option<String>,
    pub request_count: u64,
    pub cost_microdollars: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyBreakdownResponse {
    pub period: String,
    pub keys: Vec<KeyBreakdown>,
}

pub struct HistoryPeriod {
    label: String,
    cutoff_ms: u64,
    bucket_ms: u64,
    granularity: &'static str,
}

impl HistoryPeriod {
    pub fn parse(period: Option<&str>) -> Self {
        let label = period.unwrap_or("24h");
        let (cutoff_ms, bucket_ms, granularity) = match label {
            "7d" => (7 * 24 * 3600 * 1000, 6 * 3600 * 1000, "6h"),
            "30d" => (30 * 24 * 3600 * 1000, 24 * 3600 * 1000, "day"),
            _ => (24 * 3600 * 1000, 3600 * 1000, "hour"),
        };

        Self {
            label: label.to_string(),
            cutoff_ms,
            bucket_ms,
            granularity,
        }
    }

    pub fn empty_timeseries(&self) -> TimeseriesResponse {
        TimeseriesResponse {
            period: self.label.clone(),
            granularity: self.granularity.to_string(),
            points: Vec::new(),
        }
    }

    pub fn empty_models(&self) -> ModelBreakdownResponse {
        ModelBreakdownResponse {
            period: self.label.clone(),
            models: Vec::new(),
        }
    }

    pub fn empty_keys(&self) -> KeyBreakdownResponse {
        KeyBreakdownResponse {
            period: self.label.clone(),
            keys: Vec::new(),
        }
    }
}

pub async fn timeseries(
    conn: &Connection,
    period: &HistoryPeriod,
    key_id: Option<&str>,
) -> Result<TimeseriesResponse, sqlx::Error> {
    let now = timestamp_millis();
    let cutoff = now.saturating_sub(period.cutoff_ms);

    let rows = sqlx::query!(
        "SELECT (created_at / $1) * $1 AS \"bucket!\", \
         COUNT(*) AS \"request_count!\", \
         COALESCE(SUM(cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
         COALESCE(SUM(input_tokens), 0)::BIGINT AS \"input_tokens!\", \
         COALESCE(SUM(output_tokens), 0)::BIGINT AS \"output_tokens!\", \
         COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", \
         COALESCE(SUM(cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
         FROM request_log WHERE created_at >= $2 AND ($3::TEXT IS NULL OR key_id = $3) \
         GROUP BY 1 ORDER BY 1",
        period.bucket_ms as i64,
        cutoff as i64,
        key_id,
    )
    .fetch_all(conn)
    .await?;

    let mut data_map = HashMap::new();
    for row in rows {
        let ts = i64_to_u64(row.bucket);
        data_map.insert(
            ts,
            TimeseriesPoint {
                timestamp: ts,
                request_count: i64_to_u64(row.request_count),
                cost_microdollars: i64_to_u64(row.cost_microdollars),
                input_tokens: i64_to_u64(row.input_tokens),
                output_tokens: i64_to_u64(row.output_tokens),
                cache_read_tokens: i64_to_u64(row.cache_read_tokens),
                cache_write_tokens: i64_to_u64(row.cache_write_tokens),
            },
        );
    }

    let bucket_start = (cutoff / period.bucket_ms) * period.bucket_ms;
    let bucket_end = (now / period.bucket_ms) * period.bucket_ms;
    let mut points = Vec::new();
    let mut ts = bucket_start;
    while ts <= bucket_end {
        points.push(data_map.remove(&ts).unwrap_or(TimeseriesPoint {
            timestamp: ts,
            request_count: 0,
            cost_microdollars: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }));
        ts += period.bucket_ms;
    }

    Ok(TimeseriesResponse {
        period: period.label.clone(),
        granularity: period.granularity.to_string(),
        points,
    })
}

pub async fn by_model(
    conn: &Connection,
    period: &HistoryPeriod,
    key_id: Option<&str>,
) -> Result<ModelBreakdownResponse, sqlx::Error> {
    let cutoff = timestamp_millis().saturating_sub(period.cutoff_ms);

    let rows = sqlx::query!(
        "SELECT model, COUNT(*) AS \"request_count!\", \
         COALESCE(SUM(cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
         COALESCE(SUM(input_tokens), 0)::BIGINT AS \"input_tokens!\", \
         COALESCE(SUM(output_tokens), 0)::BIGINT AS \"output_tokens!\", \
         COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", \
         COALESCE(SUM(cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
         FROM request_log WHERE created_at >= $1 AND ($2::TEXT IS NULL OR key_id = $2) \
         GROUP BY model ORDER BY SUM(cost_microdollars) DESC",
        cutoff as i64,
        key_id,
    )
    .fetch_all(conn)
    .await?;

    let models = rows
        .into_iter()
        .map(|row| ModelBreakdown {
            model: row.model,
            request_count: i64_to_u64(row.request_count),
            cost_microdollars: i64_to_u64(row.cost_microdollars),
            input_tokens: i64_to_u64(row.input_tokens),
            output_tokens: i64_to_u64(row.output_tokens),
            cache_read_tokens: i64_to_u64(row.cache_read_tokens),
            cache_write_tokens: i64_to_u64(row.cache_write_tokens),
        })
        .collect();

    Ok(ModelBreakdownResponse {
        period: period.label.clone(),
        models,
    })
}

pub async fn by_key(
    conn: &Connection,
    period: &HistoryPeriod,
) -> Result<KeyBreakdownResponse, sqlx::Error> {
    let cutoff = timestamp_millis().saturating_sub(period.cutoff_ms);

    let rows = sqlx::query!(
        "SELECT r.key_id, k.name AS \"key_name?\", COUNT(*) AS \"request_count!\", \
         COALESCE(SUM(r.cost_microdollars), 0)::BIGINT AS \"cost_microdollars!\", \
         COALESCE(SUM(r.input_tokens), 0)::BIGINT AS \"input_tokens!\", \
         COALESCE(SUM(r.output_tokens), 0)::BIGINT AS \"output_tokens!\", \
         COALESCE(SUM(r.cache_read_tokens), 0)::BIGINT AS \"cache_read_tokens!\", \
         COALESCE(SUM(r.cache_write_tokens), 0)::BIGINT AS \"cache_write_tokens!\" \
         FROM request_log r LEFT JOIN client_keys k ON r.key_id = k.id \
         WHERE r.created_at >= $1 \
         GROUP BY r.key_id, k.name ORDER BY SUM(r.cost_microdollars) DESC",
        cutoff as i64,
    )
    .fetch_all(conn)
    .await?;

    let keys = rows
        .into_iter()
        .map(|row| KeyBreakdown {
            key_id: row.key_id,
            key_name: row.key_name,
            request_count: i64_to_u64(row.request_count),
            cost_microdollars: i64_to_u64(row.cost_microdollars),
            input_tokens: i64_to_u64(row.input_tokens),
            output_tokens: i64_to_u64(row.output_tokens),
            cache_read_tokens: i64_to_u64(row.cache_read_tokens),
            cache_write_tokens: i64_to_u64(row.cache_write_tokens),
        })
        .collect();

    Ok(KeyBreakdownResponse {
        period: period.label.clone(),
        keys,
    })
}
