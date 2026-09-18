//! ClickHouse HTTP INSERT writer for Rakurai metrics datapoints.

use {
    crate::datapoint::DataPoint,
    log::warn,
    serde_json::{Map, Value},
    std::{
        collections::{HashMap, HashSet},
        sync::Mutex,
        time::UNIX_EPOCH,
    },
};

/// Tables that accept Rakurai metrics writes. Unknown `rakurai*` names are skipped.
pub const ALLOWED_TABLES: &[&str] = &[
    "rakurai_info_bundle_lifecycle",
    "rakurai_tin_connection_state",
    "rakurai_warning",
    "rakurai_info",
    "rakurai_status",
    "rakurai_scheduler_qos_throughput",
    "rakurai_scheduler_qos_worker",
];

/// Max JSONEachRow lines per HTTP INSERT to bound request body size.
#[cfg(not(feature = "without_influxdb"))]
const MAX_ROWS_PER_INSERT: usize = 500;
/// After a failed CREATE, wait before retrying CREATE (INSERT is still attempted).
#[cfg(not(feature = "without_influxdb"))]
const ENSURE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
/// Cap ClickHouse error bodies in logs.
#[cfg(not(feature = "without_influxdb"))]
const MAX_ERROR_BODY_CHARS: usize = 512;

static UNKNOWN_TABLE_WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Poison-safe lock for process-local warn/ensure caches (never panic the metrics agent).
fn lock_string_set(
    mutex: &Mutex<Option<HashSet<String>>>,
) -> std::sync::MutexGuard<'_, Option<HashSet<String>>> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(not(feature = "without_influxdb"))]
fn truncate_for_log(text: &str) -> String {
    let mut truncated: String = text.chars().take(MAX_ERROR_BODY_CHARS).collect();
    if text.chars().count() > MAX_ERROR_BODY_CHARS {
        truncated.push_str("…[truncated]");
    }
    truncated
}

#[cfg(not(feature = "without_influxdb"))]
fn read_error_body(response: reqwest::blocking::Response) -> String {
    response
        .text()
        .map(|t| {
            if t.is_empty() {
                "[empty body]".to_string()
            } else {
                truncate_for_log(&t)
            }
        })
        .unwrap_or_else(|_| "[unreadable body]".to_string())
}

/// Parsed field value after stripping Influx line-protocol encoding.
#[derive(Clone, Debug, PartialEq)]
pub enum ParsedFieldValue {
    String(String),
    I64(i64),
    F64(f64),
    Bool(bool),
}

pub type ParsedFields = HashMap<String, ParsedFieldValue>;

/// Strip Influx quotes / `i` suffixes and return typed field values.
pub fn parse_datapoint_fields(fields: &[(&'static str, String)]) -> ParsedFields {
    let mut parsed = ParsedFields::new();
    for (name, raw) in fields {
        parsed.insert(name.to_string(), parse_field_value(raw));
    }
    parsed
}

fn parse_field_value(raw: &str) -> ParsedFieldValue {
    if raw.ends_with('i') {
        if let Ok(v) = raw[..raw.len() - 1].parse::<i64>() {
            return ParsedFieldValue::I64(v);
        }
    }
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return ParsedFieldValue::String(unquote_influx_string(&raw[1..raw.len() - 1]));
    }
    match raw {
        "true" => return ParsedFieldValue::Bool(true),
        "false" => return ParsedFieldValue::Bool(false),
        _ => {}
    }
    if let Ok(v) = raw.parse::<f64>() {
        return ParsedFieldValue::F64(v);
    }
    ParsedFieldValue::String(raw.to_string())
}

fn unquote_influx_string(raw: &str) -> String {
    raw.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn warn_unknown_table_once(table: &str) {
    let mut guard = lock_string_set(&UNKNOWN_TABLE_WARNED);
    let warned = guard.get_or_insert_with(HashSet::new);
    if warned.insert(table.to_string()) {
        warn!(
            "skipping unknown rakurai measurement {table:?}; not in ClickHouse allowlist"
        );
    }
}

pub fn is_allowed_table(name: &str) -> bool {
    ALLOWED_TABLES.contains(&name)
}

/// Group datapoints by measurement (table) name.
pub fn group_points_by_table(points: &[DataPoint]) -> HashMap<&'static str, Vec<&DataPoint>> {
    let mut grouped: HashMap<&'static str, Vec<&DataPoint>> = HashMap::new();
    for point in points {
        if !point.name.starts_with("rakurai") {
            continue;
        }
        if !is_allowed_table(point.name) {
            warn_unknown_table_once(point.name);
            continue;
        }
        grouped.entry(point.name).or_default().push(point);
    }
    grouped
}

fn point_time_ns(point: &DataPoint) -> u64 {
    point
        .timestamp
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn field_as_string(fields: &ParsedFields, key: &str) -> String {
    match fields.get(key) {
        Some(ParsedFieldValue::String(s)) => s.clone(),
        Some(ParsedFieldValue::I64(v)) => v.to_string(),
        Some(ParsedFieldValue::F64(v)) => v.to_string(),
        Some(ParsedFieldValue::Bool(v)) => v.to_string(),
        None => String::new(),
    }
}

fn field_as_i64(fields: &ParsedFields, key: &str) -> i64 {
    match fields.get(key) {
        Some(ParsedFieldValue::I64(v)) => *v,
        Some(ParsedFieldValue::F64(v)) => *v as i64,
        Some(ParsedFieldValue::Bool(v)) => i64::from(*v),
        Some(ParsedFieldValue::String(s)) => s.parse().unwrap_or(0),
        None => 0,
    }
}

fn field_as_bool(fields: &ParsedFields, key: &str) -> bool {
    match fields.get(key) {
        Some(ParsedFieldValue::Bool(v)) => *v,
        Some(ParsedFieldValue::I64(v)) => *v != 0,
        Some(ParsedFieldValue::F64(v)) => *v != 0.0,
        Some(ParsedFieldValue::String(s)) => s == "true" || s == "1",
        None => false,
    }
}

fn nullable_string(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value)
    }
}

fn unix_nanos_to_datetime64_utc(nanos: u64) -> String {
    let secs = (nanos / 1_000_000_000) as i64;
    let sub_nanos = (nanos % 1_000_000_000) as u32;
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400) as u32;
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;

    // civil_from_days — http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    let year = y + if m <= 2 { 1 } else { 0 };

    format!("{year:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02}.{sub_nanos:09}")
}

fn insert_common(obj: &mut Map<String, Value>, host_id: &str, time_ns: u64) {
    obj.insert(
        "timestamp".into(),
        Value::String(unix_nanos_to_datetime64_utc(time_ns)),
    );
    obj.insert("time_ns".into(), Value::from(time_ns));
    obj.insert("host_id".into(), Value::String(host_id.to_string()));
}

fn insert_i64_field(obj: &mut Map<String, Value>, fields: &ParsedFields, key: &str) {
    obj.insert(key.into(), Value::from(field_as_i64(fields, key)));
}

fn insert_nullable_string_field(obj: &mut Map<String, Value>, fields: &ParsedFields, key: &str) {
    obj.insert(key.into(), nullable_string(field_as_string(fields, key)));
}

/// Serialize one datapoint to a JSONEachRow line for its table.
///
/// Column layout matches live `rakurai_stats_db` on ClickHouse
/// (`timestamp` DateTime64(9,'UTC') + `time_ns` UInt64).
pub fn serialize_point(point: &DataPoint, host_id: &str) -> Option<String> {
    if !is_allowed_table(point.name) {
        warn_unknown_table_once(point.name);
        return None;
    }

    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);

    let row = match point.name {
        "rakurai_info_bundle_lifecycle" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            insert_nullable_string_field(&mut obj, &fields, "block_engine_uuid");
            insert_nullable_string_field(&mut obj, &fields, "bundle_id");
            insert_i64_field(&mut obj, &fields, "bundle_priority");
            insert_nullable_string_field(&mut obj, &fields, "drop_reason");
            insert_i64_field(&mut obj, &fields, "end_timestamp_ns");
            insert_i64_field(&mut obj, &fields, "has_postpack_confirmation");
            insert_i64_field(&mut obj, &fields, "is_primary");
            insert_i64_field(&mut obj, &fields, "num_txs");
            insert_nullable_string_field(&mut obj, &fields, "outcome");
            insert_nullable_string_field(&mut obj, &fields, "p2c_id");
            insert_i64_field(&mut obj, &fields, "priority_fee_lamports");
            insert_i64_field(&mut obj, &fields, "received_slot");
            insert_nullable_string_field(&mut obj, &fields, "signatures");
            insert_i64_field(&mut obj, &fields, "start_timestamp_ns");
            insert_i64_field(&mut obj, &fields, "tip_lamports");
            insert_i64_field(&mut obj, &fields, "total_cus");
            Value::Object(obj)
        }
        "rakurai_tin_connection_state" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            obj.insert("primary".into(), Value::Bool(field_as_bool(&fields, "primary")));
            insert_nullable_string_field(&mut obj, &fields, "source");
            insert_nullable_string_field(&mut obj, &fields, "state");
            insert_nullable_string_field(&mut obj, &fields, "url");
            insert_nullable_string_field(&mut obj, &fields, "uuid");
            Value::Object(obj)
        }
        "rakurai_warning" => {
            // Live table only has rakurai_abort_log; fold warning_log into it when abort is empty.
            let abort_log = field_as_string(&fields, "rakurai_abort_log");
            let warning_log = field_as_string(&fields, "rakurai_warning_log");
            let message = if !abort_log.is_empty() {
                abort_log
            } else {
                warning_log
            };
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            obj.insert("rakurai_abort_log".into(), nullable_string(message));
            Value::Object(obj)
        }
        "rakurai_info" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            let log = field_as_string(&fields, "rakurai_info_log");
            let log = if log.is_empty() {
                // Fallback: first string-ish field value if producers use another name.
                fields
                    .values()
                    .find_map(|v| match v {
                        ParsedFieldValue::String(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            } else {
                log
            };
            obj.insert("rakurai_info_log".into(), nullable_string(log));
            Value::Object(obj)
        }
        "rakurai_status" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            obj.insert(
                "enabled".into(),
                Value::Bool(field_as_bool(&fields, "enabled")),
            );
            Value::Object(obj)
        }
        "rakurai_scheduler_qos_throughput" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            for key in QOS_THROUGHPUT_I64_FIELDS {
                insert_i64_field(&mut obj, &fields, key);
            }
            Value::Object(obj)
        }
        "rakurai_scheduler_qos_worker" => {
            let mut obj = Map::new();
            insert_common(&mut obj, host_id, time_ns);
            for key in QOS_WORKER_I64_FIELDS {
                insert_i64_field(&mut obj, &fields, key);
            }
            Value::Object(obj)
        }
        _ => return None,
    };

    match serde_json::to_string(&row) {
        Ok(json) => Some(json),
        Err(err) => {
            warn!(
                "failed to serialize ClickHouse row for table {}: {err}",
                point.name
            );
            None
        }
    }
}

const QOS_THROUGHPUT_I64_FIELDS: &[&str] = &[
    "candidate_non_singleton_count",
    "candidate_singleton_count",
    "dedicated_lane_dequeue_count",
    "dedicated_lane_enqueue_count",
    "dedicated_lane_service_max_us",
    "dedicated_lane_service_us",
    "ingress_ordinary",
    "ingress_pruner_candidate",
    "mixed_batch_count",
    "ordinary_batch_count",
    "ordinary_batch_max_transactions",
    "ordinary_transaction_count",
    "report_elapsed_us",
    "s1_bounded_ordinary_dispatched_batches",
    "s1_bounded_ordinary_passes",
    "s1_candidate_recheck_already_processed",
    "s1_candidate_recheck_blockhash_not_found",
    "s1_candidate_recheck_dropped",
    "s1_candidate_recheck_other",
    "s1_early_ordinary_dispatched_batches",
    "s1_early_ordinary_passes",
    "s1_ordinary_in_flight_worker_samples",
    "s1_ordinary_lock_or_admission_blocked_rounds",
    "s1_ordinary_no_worker_capacity_rounds",
    "s1_ordinary_queue_busy_worker_samples",
    "worker_estimated_cu_total",
    "worker_transaction_total",
    "worker_work_total",
];

const QOS_WORKER_I64_FIELDS: &[&str] = &[
    "candidate_batches",
    "enqueue_blocked_us",
    "estimated_compute_units",
    "estimated_cu_per_second",
    "estimated_cu_share_ppm",
    "max_batch_transactions",
    "max_sender_queue_depth",
    "ordinary_batches",
    "transaction_share_ppm",
    "transactions",
    "transactions_per_second",
    "work_batches",
    "work_per_second",
    "work_share_ppm",
    "worker",
];

fn merge_tree_suffix() -> &'static str {
    "ENGINE = MergeTree\n\
PARTITION BY toYYYYMM(timestamp)\n\
ORDER BY (host_id, timestamp)\n\
SETTINGS index_granularity = 8192"
}

/// DDL matching live `rakurai_stats_db` table shapes (`CREATE TABLE IF NOT EXISTS`).
pub fn create_table_ddl(table: &str) -> Option<String> {
    let suffix = merge_tree_suffix();
    let ddl = match table {
        "rakurai_info_bundle_lifecycle" => format!(
            "CREATE TABLE IF NOT EXISTS {table}\n(\n\
    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String),\n\
    `block_engine_uuid` Nullable(String),\n\
    `bundle_id` Nullable(String),\n\
    `bundle_priority` Nullable(Int64),\n\
    `drop_reason` LowCardinality(Nullable(String)),\n\
    `end_timestamp_ns` Nullable(Int64),\n\
    `has_postpack_confirmation` Nullable(Int64),\n\
    `is_primary` Nullable(Int64),\n\
    `num_txs` Nullable(Int64),\n\
    `outcome` LowCardinality(Nullable(String)),\n\
    `p2c_id` Nullable(String),\n\
    `priority_fee_lamports` Nullable(Int64),\n\
    `received_slot` Nullable(Int64),\n\
    `signatures` Nullable(String),\n\
    `start_timestamp_ns` Nullable(Int64),\n\
    `tip_lamports` Nullable(Int64),\n\
    `total_cus` Nullable(Int64),\n\
    INDEX idx_bundle_id bundle_id TYPE bloom_filter(0.01) GRANULARITY 4,\n\
    INDEX idx_signatures_ngram ifNull(signatures, '') TYPE ngrambf_v1(3, 256, 2, 0) GRANULARITY 4,\n\
    PROJECTION proj_by_timestamp\n\
    (\n\
        SELECT *\n\
        ORDER BY\n\
            timestamp,\n\
            host_id\n\
    )\n\
)\n{suffix}"
        ),
        "rakurai_tin_connection_state" => format!(
            "CREATE TABLE IF NOT EXISTS {table}\n(\n\
    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String),\n\
    `primary` Nullable(Bool),\n\
    `source` LowCardinality(Nullable(String)),\n\
    `state` LowCardinality(Nullable(String)),\n\
    `url` Nullable(String),\n\
    `uuid` Nullable(String)\n\
)\n{suffix}"
        ),
        "rakurai_warning" => format!(
            "CREATE TABLE IF NOT EXISTS {table}\n(\n\
    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String),\n\
    `rakurai_abort_log` Nullable(String)\n\
)\n{suffix}"
        ),
        "rakurai_info" => format!(
            "CREATE TABLE IF NOT EXISTS {table}\n(\n\
    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String),\n\
    `rakurai_info_log` Nullable(String)\n\
)\n{suffix}"
        ),
        "rakurai_status" => format!(
            "CREATE TABLE IF NOT EXISTS {table}\n(\n\
    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String),\n\
    `enabled` Nullable(Bool)\n\
)\n{suffix}"
        ),
        "rakurai_scheduler_qos_throughput" => {
            let mut cols = String::from(
                "    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String)",
            );
            for key in QOS_THROUGHPUT_I64_FIELDS {
                cols.push_str(&format!(",\n    `{key}` Nullable(Int64)"));
            }
            format!("CREATE TABLE IF NOT EXISTS {table}\n(\n{cols}\n)\n{suffix}")
        }
        "rakurai_scheduler_qos_worker" => {
            let mut cols = String::from(
                "    `timestamp` DateTime64(9, 'UTC'),\n\
    `time_ns` UInt64,\n\
    `host_id` LowCardinality(String)",
            );
            for key in QOS_WORKER_I64_FIELDS {
                cols.push_str(&format!(",\n    `{key}` Nullable(Int64)"));
            }
            format!("CREATE TABLE IF NOT EXISTS {table}\n(\n{cols}\n)\n{suffix}")
        }
        _ => return None,
    };
    Some(ddl)
}

/// Process-local CREATE TABLE state: success cache + failure backoff.
#[cfg(not(feature = "without_influxdb"))]
#[derive(Debug)]
enum EnsureState {
    Ensured,
    Failed { last_attempt: std::time::Instant },
}

#[cfg(not(feature = "without_influxdb"))]
static ENSURE_STATE: Mutex<Option<HashMap<String, EnsureState>>> = Mutex::new(None);

#[cfg(not(feature = "without_influxdb"))]
fn lock_ensure_state(
) -> std::sync::MutexGuard<'static, Option<HashMap<String, EnsureState>>> {
    ENSURE_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug)]
pub struct RakuraiClickHouseConfig {
    pub host: String,
    pub db: String,
    pub username: String,
    pub password: String,
}

impl RakuraiClickHouseConfig {
    pub fn complete(&self) -> bool {
        !(self.host.is_empty()
            || self.db.is_empty()
            || self.username.is_empty()
            || self.password.is_empty())
    }
}

#[cfg(not(feature = "without_influxdb"))]
pub fn build_client(insecure_tls: bool) -> Result<reqwest::blocking::Client, reqwest::Error> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .connect_timeout(std::time::Duration::from_secs(3));
    if insecure_tls {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build()
}

#[cfg(not(feature = "without_influxdb"))]
fn post_query(
    client: &reqwest::blocking::Client,
    config: &RakuraiClickHouseConfig,
    body: String,
) -> Result<reqwest::blocking::Response, reqwest::Error> {
    let host = config.host.trim_end_matches('/');
    let url = format!("{host}/?database={}", config.db);
    client
        .post(&url)
        .header("X-ClickHouse-User", &config.username)
        .header("X-ClickHouse-Key", &config.password)
        .body(body)
        .send()
}

/// Best-effort CREATE TABLE IF NOT EXISTS.
///
/// Returns whether CREATE was confirmed successful. A `false` result does **not**
/// mean the table is missing — INSERT should still be attempted (table may already
/// exist, or CREATE may be temporarily unavailable). Failures are backed off so a
/// down ClickHouse does not stall the metrics agent with repeated CREATE timeouts.
#[cfg(not(feature = "without_influxdb"))]
pub fn ensure_table(
    client: &reqwest::blocking::Client,
    config: &RakuraiClickHouseConfig,
    table: &str,
) -> bool {
    {
        let mut guard = lock_ensure_state();
        let state = guard.get_or_insert_with(HashMap::new);
        match state.get(table) {
            Some(EnsureState::Ensured) => return true,
            Some(EnsureState::Failed { last_attempt })
                if last_attempt.elapsed() < ENSURE_RETRY_BACKOFF =>
            {
                return false;
            }
            _ => {}
        }
    }

    let Some(ddl) = create_table_ddl(table) else {
        warn_unknown_table_once(table);
        return false;
    };

    match post_query(client, config, ddl) {
        Ok(response) => {
            if response.status().is_success() {
                let mut guard = lock_ensure_state();
                guard
                    .get_or_insert_with(HashMap::new)
                    .insert(table.to_string(), EnsureState::Ensured);
                true
            } else {
                let status = response.status();
                let text = read_error_body(response);
                warn!("ClickHouse CREATE TABLE {table} failed: {status} {text}");
                let mut guard = lock_ensure_state();
                guard.get_or_insert_with(HashMap::new).insert(
                    table.to_string(),
                    EnsureState::Failed {
                        last_attempt: std::time::Instant::now(),
                    },
                );
                false
            }
        }
        Err(err) => {
            warn!("ClickHouse CREATE TABLE {table} error: {err}");
            let mut guard = lock_ensure_state();
            guard.get_or_insert_with(HashMap::new).insert(
                table.to_string(),
                EnsureState::Failed {
                    last_attempt: std::time::Instant::now(),
                },
            );
            false
        }
    }
}

#[cfg(not(feature = "without_influxdb"))]
fn insert_rows_once(
    client: &reqwest::blocking::Client,
    config: &RakuraiClickHouseConfig,
    table: &str,
    rows: &[String],
) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }

    let mut body = format!("INSERT INTO {table} FORMAT JSONEachRow\n");
    for row in rows {
        body.push_str(row);
        body.push('\n');
    }

    match post_query(client, config, body) {
        Ok(response) => {
            if response.status().is_success() {
                Ok(())
            } else {
                let status = response.status();
                let text = read_error_body(response);
                Err(format!("{status} {text}"))
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// INSERT JSONEachRow in bounded chunks. Errors are logged; never panics.
#[cfg(not(feature = "without_influxdb"))]
pub fn insert_batch(
    client: &reqwest::blocking::Client,
    config: &RakuraiClickHouseConfig,
    table: &str,
    rows: &[String],
) {
    if rows.is_empty() {
        return;
    }

    for (chunk_idx, chunk) in rows.chunks(MAX_ROWS_PER_INSERT).enumerate() {
        if let Err(err) = insert_rows_once(client, config, table, chunk) {
            warn!(
                "ClickHouse insert into {table} failed (chunk {}/{}): {err}",
                chunk_idx + 1,
                rows.len().div_ceil(MAX_ROWS_PER_INSERT)
            );
            // Continue remaining chunks; one bad chunk should not drop the rest.
        }
    }
}

/// Write rakurai datapoints to ClickHouse. Best-effort: never panics; failures are logged.
#[cfg(not(feature = "without_influxdb"))]
pub fn write_rakurai_points(
    client: &reqwest::blocking::Client,
    config: &RakuraiClickHouseConfig,
    points: &[DataPoint],
    host_id: &str,
) {
    if !config.complete() {
        warn!("ClickHouse rakurai write skipped: incomplete config");
        return;
    }
    if host_id.is_empty() {
        warn!("ClickHouse rakurai write skipped: empty host_id");
        return;
    }

    let grouped = group_points_by_table(points);
    for (table, table_points) in grouped {
        // Best-effort schema ensure; INSERT proceeds even if CREATE failed/backed off
        // because the table may already exist on the cluster.
        let _ensured = ensure_table(client, config, table);

        let rows: Vec<String> = table_points
            .iter()
            .filter_map(|p| serialize_point(p, host_id))
            .collect();
        if rows.is_empty() {
            continue;
        }
        insert_batch(client, config, table, &rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_field_value() {
        assert_eq!(
            parse_field_value("42i"),
            ParsedFieldValue::I64(42)
        );
        assert_eq!(
            parse_field_value("\"hello\""),
            ParsedFieldValue::String("hello".to_string())
        );
        assert_eq!(
            parse_field_value("\"say \\\"hi\\\"\""),
            ParsedFieldValue::String("say \"hi\"".to_string())
        );
        assert_eq!(parse_field_value("true"), ParsedFieldValue::Bool(true));
        assert_eq!(parse_field_value("1.5"), ParsedFieldValue::F64(1.5));
    }

    #[test]
    fn test_serialize_bundle_lifecycle() {
        let mut point = DataPoint::new("rakurai_info_bundle_lifecycle");
        point.add_field_str("bundle_id", "abc");
        point.add_field_i64("num_txs", 3);
        point.add_field_i64("is_primary", 1);
        point.add_field_str("outcome", "landed");
        point.add_field_str("drop_reason", "");

        let json = serialize_point(&point, "host-1").expect("json row");
        assert!(json.contains("\"host_id\":\"host-1\""));
        assert!(json.contains("\"bundle_id\":\"abc\""));
        assert!(json.contains("\"num_txs\":3"));
        assert!(json.contains("\"drop_reason\":null"));
        assert!(json.contains("\"time_ns\":"));
        assert!(json.contains("\"timestamp\":"));
        assert!(!json.contains("\"time\":"));
        assert!(!json.contains("event_time"));
    }

    #[test]
    fn test_serialize_warning_normalizes_message() {
        let mut point = DataPoint::new("rakurai_warning");
        point.add_field_str("rakurai_abort_log", "scheduler aborted");

        let json = serialize_point(&point, "host-1").expect("json row");
        assert!(json.contains("\"rakurai_abort_log\":\"scheduler aborted\""));
        assert!(!json.contains("\"message\""));
        assert!(json.contains("\"time_ns\":"));
    }

    #[test]
    fn test_serialize_generic_scheduler_table() {
        let mut point = DataPoint::new("rakurai_status");
        point.add_field_bool("enabled", true);

        let json = serialize_point(&point, "host-1").expect("json row");
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"time_ns\":"));
        assert!(!json.contains("extra_fields"));
    }

    #[test]
    fn test_unknown_table_skipped() {
        let point = DataPoint::new("rakurai_write_grant_probe");
        assert!(serialize_point(&point, "host-1").is_none());
        assert!(!is_allowed_table("rakurai_write_grant_probe"));
    }

    #[test]
    fn test_group_points_by_table() {
        let mut p1 = DataPoint::new("rakurai_warning");
        p1.add_field_str("rakurai_abort_log", "x");
        let mut p2 = DataPoint::new("rakurai_status");
        p2.add_field_i64("v", 1);
        let p3 = DataPoint::new("rakurai_write_grant_probe");

        let points = [p1, p2, p3];
        let grouped = group_points_by_table(&points);
        assert_eq!(grouped.len(), 2);
        assert!(grouped.contains_key("rakurai_warning"));
        assert!(grouped.contains_key("rakurai_status"));
    }

    #[test]
    fn test_create_table_ddl_for_allowlisted_tables() {
        for table in ALLOWED_TABLES {
            let ddl = create_table_ddl(table).expect(table);
            assert!(ddl.starts_with("CREATE TABLE IF NOT EXISTS "), "{table}");
            assert!(ddl.contains("`timestamp` DateTime64(9, 'UTC')"), "{table}");
            assert!(ddl.contains("`time_ns` UInt64"), "{table}");
            assert!(ddl.contains("`host_id` LowCardinality(String)"), "{table}");
            assert!(ddl.contains("ENGINE = MergeTree"), "{table}");
        }
        assert!(create_table_ddl("rakurai_unknown").is_none());
    }

    #[test]
    fn test_bundle_lifecycle_ddl_includes_indexes_and_projection() {
        let ddl = create_table_ddl("rakurai_info_bundle_lifecycle").expect("ddl");
        assert!(ddl.contains(
            "INDEX idx_bundle_id bundle_id TYPE bloom_filter(0.01) GRANULARITY 4"
        ));
        assert!(ddl.contains(
            "INDEX idx_signatures_ngram ifNull(signatures, '') TYPE ngrambf_v1(3, 256, 2, 0) GRANULARITY 4"
        ));
        assert!(ddl.contains("PROJECTION proj_by_timestamp"));
        assert!(ddl.contains("timestamp,"));
        assert!(ddl.contains("host_id"));
        // Table ORDER BY stays (host_id, timestamp); projection is for timestamp-first reads.
        assert!(ddl.contains("ORDER BY (host_id, timestamp)"));
    }
}
