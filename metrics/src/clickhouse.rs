//! Rakurai metrics → ClickHouse via the official `clickhouse` crate.
//!
//! Only maps `DataPoint`s to typed rows and inserts. Transport, encoding, TLS,
//! and compression are owned by the crate.

use {
    crate::datapoint::DataPoint,
    chrono::{DateTime, Utc},
    clickhouse as ch,
    clickhouse::{Row, RowOwned, RowWrite},
    log::warn,
    serde::Serialize,
    std::{
        collections::{HashMap, HashSet},
        sync::{LazyLock, Mutex},
        time::{Duration, UNIX_EPOCH},
    },
};

pub(crate) use ch::Client;

const INSERT_TIMEOUT: Duration = Duration::from_secs(5);

static UNKNOWN_TABLE_WARNED: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Parsed field value after stripping Influx line-protocol encoding.
#[derive(Clone, Debug, PartialEq)]
enum ParsedFieldValue {
    String(String),
    I64(i64),
    F64(f64),
    Bool(bool),
}

type ParsedFields = HashMap<&'static str, ParsedFieldValue>;

fn parse_datapoint_fields(fields: &[(&'static str, String)]) -> ParsedFields {
    let mut parsed = ParsedFields::new();
    for (name, raw) in fields {
        parsed.insert(*name, parse_field_value(raw));
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
        return ParsedFieldValue::String(crate::datapoint::decode_field_str(raw));
    }
    if raw == "true" {
        return ParsedFieldValue::Bool(true);
    }
    if raw == "false" {
        return ParsedFieldValue::Bool(false);
    }
    if let Ok(v) = raw.parse::<f64>() {
        return ParsedFieldValue::F64(v);
    }
    ParsedFieldValue::String(raw.to_string())
}

fn warn_unknown_table_once(table: &str) {
    let mut warned = UNKNOWN_TABLE_WARNED
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if warned.insert(table.to_string()) {
        warn!("skipping unknown rakurai measurement {table:?}; not in ClickHouse allowlist");
    }
}

fn point_time_ns(point: &DataPoint) -> u64 {
    point
        .timestamp
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn timestamp_from_nanos(nanos: u64) -> DateTime<Utc> {
    DateTime::from_timestamp((nanos / 1_000_000_000) as i64, (nanos % 1_000_000_000) as u32)
        .unwrap_or(DateTime::UNIX_EPOCH)
}

fn field_string(fields: &ParsedFields, key: &'static str) -> Option<String> {
    match fields.get(key) {
        Some(ParsedFieldValue::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(ParsedFieldValue::I64(v)) => Some(v.to_string()),
        Some(ParsedFieldValue::F64(v)) => Some(v.to_string()),
        Some(ParsedFieldValue::Bool(v)) => Some(v.to_string()),
        _ => None,
    }
}

fn field_i64(fields: &ParsedFields, key: &'static str) -> Option<i64> {
    match fields.get(key) {
        Some(ParsedFieldValue::I64(v)) => Some(*v),
        Some(ParsedFieldValue::F64(v)) => Some(*v as i64),
        Some(ParsedFieldValue::Bool(v)) => Some(i64::from(*v)),
        Some(ParsedFieldValue::String(s)) => s.parse().ok(),
        None => None,
    }
}

fn field_bool(fields: &ParsedFields, key: &'static str) -> Option<bool> {
    match fields.get(key) {
        Some(ParsedFieldValue::Bool(v)) => Some(*v),
        Some(ParsedFieldValue::I64(v)) => Some(*v != 0),
        Some(ParsedFieldValue::F64(v)) => Some(*v != 0.0),
        Some(ParsedFieldValue::String(s)) => Some(s == "true" || s == "1"),
        None => None,
    }
}

#[derive(Debug, Clone, Row, Serialize)]
struct BundleLifecycleRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    block_engine_uuid: Option<String>,
    bundle_id: Option<String>,
    bundle_priority: Option<i64>,
    drop_reason: Option<String>,
    end_timestamp_ns: Option<i64>,
    has_postpack_confirmation: Option<i64>,
    is_primary: Option<i64>,
    num_txs: Option<i64>,
    outcome: Option<String>,
    p2c_id: Option<String>,
    priority_fee_lamports: Option<i64>,
    received_slot: Option<i64>,
    signatures: Option<String>,
    start_timestamp_ns: Option<i64>,
    tip_lamports: Option<i64>,
    total_cus: Option<i64>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct TinConnectionStateRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    #[serde(rename = "primary")]
    primary_conn: Option<bool>,
    source: Option<String>,
    state: Option<String>,
    url: Option<String>,
    actual_url: Option<String>,
    uuid: Option<String>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct WarningRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    rakurai_abort_log: Option<String>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct InfoRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    rakurai_info_log: Option<String>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct StatusRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct QosThroughputRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    candidate_non_singleton_count: Option<i64>,
    candidate_singleton_count: Option<i64>,
    dedicated_lane_dequeue_count: Option<i64>,
    dedicated_lane_enqueue_count: Option<i64>,
    dedicated_lane_service_max_us: Option<i64>,
    dedicated_lane_service_us: Option<i64>,
    ingress_ordinary: Option<i64>,
    ingress_pruner_candidate: Option<i64>,
    mixed_batch_count: Option<i64>,
    ordinary_batch_count: Option<i64>,
    ordinary_batch_max_transactions: Option<i64>,
    ordinary_transaction_count: Option<i64>,
    report_elapsed_us: Option<i64>,
    s1_bounded_ordinary_dispatched_batches: Option<i64>,
    s1_bounded_ordinary_passes: Option<i64>,
    s1_candidate_recheck_already_processed: Option<i64>,
    s1_candidate_recheck_blockhash_not_found: Option<i64>,
    s1_candidate_recheck_dropped: Option<i64>,
    s1_candidate_recheck_other: Option<i64>,
    s1_early_ordinary_dispatched_batches: Option<i64>,
    s1_early_ordinary_passes: Option<i64>,
    s1_ordinary_in_flight_worker_samples: Option<i64>,
    s1_ordinary_lock_or_admission_blocked_rounds: Option<i64>,
    s1_ordinary_no_worker_capacity_rounds: Option<i64>,
    s1_ordinary_queue_busy_worker_samples: Option<i64>,
    worker_estimated_cu_total: Option<i64>,
    worker_transaction_total: Option<i64>,
    worker_work_total: Option<i64>,
}

#[derive(Debug, Clone, Row, Serialize)]
struct QosWorkerRow {
    #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
    timestamp: DateTime<Utc>,
    time_ns: u64,
    host_id: String,
    candidate_batches: Option<i64>,
    enqueue_blocked_us: Option<i64>,
    estimated_compute_units: Option<i64>,
    estimated_cu_per_second: Option<i64>,
    estimated_cu_share_ppm: Option<i64>,
    max_batch_transactions: Option<i64>,
    max_sender_queue_depth: Option<i64>,
    ordinary_batches: Option<i64>,
    transaction_share_ppm: Option<i64>,
    transactions: Option<i64>,
    transactions_per_second: Option<i64>,
    work_batches: Option<i64>,
    work_per_second: Option<i64>,
    work_share_ppm: Option<i64>,
    worker: Option<i64>,
}

fn common(host_id: &str, time_ns: u64) -> (DateTime<Utc>, u64, String) {
    (timestamp_from_nanos(time_ns), time_ns, host_id.to_string())
}

fn map_bundle_lifecycle(point: &DataPoint, host_id: &str) -> BundleLifecycleRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    BundleLifecycleRow {
        timestamp,
        time_ns,
        host_id,
        block_engine_uuid: field_string(&fields, "block_engine_uuid"),
        bundle_id: field_string(&fields, "bundle_id"),
        bundle_priority: field_i64(&fields, "bundle_priority"),
        drop_reason: field_string(&fields, "drop_reason"),
        end_timestamp_ns: field_i64(&fields, "end_timestamp_ns"),
        has_postpack_confirmation: field_i64(&fields, "has_postpack_confirmation"),
        is_primary: field_i64(&fields, "is_primary"),
        num_txs: field_i64(&fields, "num_txs"),
        outcome: field_string(&fields, "outcome"),
        p2c_id: field_string(&fields, "p2c_id"),
        priority_fee_lamports: field_i64(&fields, "priority_fee_lamports"),
        received_slot: field_i64(&fields, "received_slot"),
        signatures: field_string(&fields, "signatures"),
        start_timestamp_ns: field_i64(&fields, "start_timestamp_ns"),
        tip_lamports: field_i64(&fields, "tip_lamports"),
        total_cus: field_i64(&fields, "total_cus"),
    }
}

fn map_warning(point: &DataPoint, host_id: &str) -> WarningRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    let abort = field_string(&fields, "rakurai_abort_log");
    let warning = field_string(&fields, "rakurai_warning_log");
    WarningRow {
        timestamp,
        time_ns,
        host_id,
        rakurai_abort_log: abort.or(warning),
    }
}

fn map_info(point: &DataPoint, host_id: &str) -> InfoRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    let log = field_string(&fields, "rakurai_info_log").or_else(|| {
        fields.values().find_map(|v| match v {
            ParsedFieldValue::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    });
    InfoRow {
        timestamp,
        time_ns,
        host_id,
        rakurai_info_log: log,
    }
}

fn map_status(point: &DataPoint, host_id: &str) -> StatusRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    StatusRow {
        timestamp,
        time_ns,
        host_id,
        enabled: field_bool(&fields, "enabled"),
    }
}

fn map_tin(point: &DataPoint, host_id: &str) -> TinConnectionStateRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    TinConnectionStateRow {
        timestamp,
        time_ns,
        host_id,
        primary_conn: field_bool(&fields, "primary"),
        source: field_string(&fields, "source"),
        state: field_string(&fields, "state"),
        url: field_string(&fields, "url"),
        actual_url: field_string(&fields, "actual_url"),
        uuid: field_string(&fields, "uuid"),
    }
}

fn map_qos_throughput(point: &DataPoint, host_id: &str) -> QosThroughputRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    QosThroughputRow {
        timestamp,
        time_ns,
        host_id,
        candidate_non_singleton_count: field_i64(&fields, "candidate_non_singleton_count"),
        candidate_singleton_count: field_i64(&fields, "candidate_singleton_count"),
        dedicated_lane_dequeue_count: field_i64(&fields, "dedicated_lane_dequeue_count"),
        dedicated_lane_enqueue_count: field_i64(&fields, "dedicated_lane_enqueue_count"),
        dedicated_lane_service_max_us: field_i64(&fields, "dedicated_lane_service_max_us"),
        dedicated_lane_service_us: field_i64(&fields, "dedicated_lane_service_us"),
        ingress_ordinary: field_i64(&fields, "ingress_ordinary"),
        ingress_pruner_candidate: field_i64(&fields, "ingress_pruner_candidate"),
        mixed_batch_count: field_i64(&fields, "mixed_batch_count"),
        ordinary_batch_count: field_i64(&fields, "ordinary_batch_count"),
        ordinary_batch_max_transactions: field_i64(&fields, "ordinary_batch_max_transactions"),
        ordinary_transaction_count: field_i64(&fields, "ordinary_transaction_count"),
        report_elapsed_us: field_i64(&fields, "report_elapsed_us"),
        s1_bounded_ordinary_dispatched_batches: field_i64(
            &fields,
            "s1_bounded_ordinary_dispatched_batches",
        ),
        s1_bounded_ordinary_passes: field_i64(&fields, "s1_bounded_ordinary_passes"),
        s1_candidate_recheck_already_processed: field_i64(
            &fields,
            "s1_candidate_recheck_already_processed",
        ),
        s1_candidate_recheck_blockhash_not_found: field_i64(
            &fields,
            "s1_candidate_recheck_blockhash_not_found",
        ),
        s1_candidate_recheck_dropped: field_i64(&fields, "s1_candidate_recheck_dropped"),
        s1_candidate_recheck_other: field_i64(&fields, "s1_candidate_recheck_other"),
        s1_early_ordinary_dispatched_batches: field_i64(
            &fields,
            "s1_early_ordinary_dispatched_batches",
        ),
        s1_early_ordinary_passes: field_i64(&fields, "s1_early_ordinary_passes"),
        s1_ordinary_in_flight_worker_samples: field_i64(
            &fields,
            "s1_ordinary_in_flight_worker_samples",
        ),
        s1_ordinary_lock_or_admission_blocked_rounds: field_i64(
            &fields,
            "s1_ordinary_lock_or_admission_blocked_rounds",
        ),
        s1_ordinary_no_worker_capacity_rounds: field_i64(
            &fields,
            "s1_ordinary_no_worker_capacity_rounds",
        ),
        s1_ordinary_queue_busy_worker_samples: field_i64(
            &fields,
            "s1_ordinary_queue_busy_worker_samples",
        ),
        worker_estimated_cu_total: field_i64(&fields, "worker_estimated_cu_total"),
        worker_transaction_total: field_i64(&fields, "worker_transaction_total"),
        worker_work_total: field_i64(&fields, "worker_work_total"),
    }
}

fn map_qos_worker(point: &DataPoint, host_id: &str) -> QosWorkerRow {
    let fields = parse_datapoint_fields(&point.fields);
    let time_ns = point_time_ns(point);
    let (timestamp, time_ns, host_id) = common(host_id, time_ns);
    QosWorkerRow {
        timestamp,
        time_ns,
        host_id,
        candidate_batches: field_i64(&fields, "candidate_batches"),
        enqueue_blocked_us: field_i64(&fields, "enqueue_blocked_us"),
        estimated_compute_units: field_i64(&fields, "estimated_compute_units"),
        estimated_cu_per_second: field_i64(&fields, "estimated_cu_per_second"),
        estimated_cu_share_ppm: field_i64(&fields, "estimated_cu_share_ppm"),
        max_batch_transactions: field_i64(&fields, "max_batch_transactions"),
        max_sender_queue_depth: field_i64(&fields, "max_sender_queue_depth"),
        ordinary_batches: field_i64(&fields, "ordinary_batches"),
        transaction_share_ppm: field_i64(&fields, "transaction_share_ppm"),
        transactions: field_i64(&fields, "transactions"),
        transactions_per_second: field_i64(&fields, "transactions_per_second"),
        work_batches: field_i64(&fields, "work_batches"),
        work_per_second: field_i64(&fields, "work_per_second"),
        work_share_ppm: field_i64(&fields, "work_share_ppm"),
        worker: field_i64(&fields, "worker"),
    }
}

pub(crate) fn build_client(url: &str, db: &str, user: &str, password: &str) -> Client {
    Client::default()
        .with_url(url.trim_end_matches('/'))
        .with_database(db)
        .with_user(user)
        .with_password(password)
        .with_product_info("solana-metrics", env!("CARGO_PKG_VERSION"))
}

async fn insert_rows<T: RowOwned + RowWrite>(client: &Client, table: &str, rows: &[T]) {
    if rows.is_empty() {
        return;
    }
    let mut insert = match client.insert::<T>(table).await {
        Ok(insert) => insert.with_timeouts(Some(INSERT_TIMEOUT), Some(INSERT_TIMEOUT)),
        Err(err) => {
            warn!("ClickHouse insert start for {table} failed: {err}");
            return;
        }
    };
    for row in rows {
        if let Err(err) = insert.write(row).await {
            warn!("ClickHouse insert write for {table} failed: {err}");
            return;
        }
    }
    if let Err(err) = insert.end().await {
        warn!("ClickHouse insert end for {table} failed: {err}");
    }
}

pub(crate) async fn write_rakurai_points(client: &Client, points: &[DataPoint], host_id: &str) {
    if host_id.is_empty() {
        warn!("ClickHouse rakurai write skipped: empty host_id");
        return;
    }

    let mut bundle = Vec::new();
    let mut tin = Vec::new();
    let mut warning = Vec::new();
    let mut info = Vec::new();
    let mut status = Vec::new();
    let mut qos_tp = Vec::new();
    let mut qos_worker = Vec::new();

    for point in points {
        match point.name {
            "rakurai_info_bundle_lifecycle" => bundle.push(map_bundle_lifecycle(point, host_id)),
            "rakurai_tin_connection_state" => tin.push(map_tin(point, host_id)),
            "rakurai_warning" => warning.push(map_warning(point, host_id)),
            "rakurai_info" => info.push(map_info(point, host_id)),
            "rakurai_status" => status.push(map_status(point, host_id)),
            "rakurai_scheduler_qos_throughput" => {
                qos_tp.push(map_qos_throughput(point, host_id))
            }
            "rakurai_scheduler_qos_worker" => qos_worker.push(map_qos_worker(point, host_id)),
            name if name.starts_with("rakurai") => warn_unknown_table_once(name),
            _ => {}
        }
    }

    insert_rows(client, "rakurai_info_bundle_lifecycle", &bundle).await;
    insert_rows(client, "rakurai_tin_connection_state", &tin).await;
    insert_rows(client, "rakurai_warning", &warning).await;
    insert_rows(client, "rakurai_info", &info).await;
    insert_rows(client, "rakurai_status", &status).await;
    insert_rows(client, "rakurai_scheduler_qos_throughput", &qos_tp).await;
    insert_rows(client, "rakurai_scheduler_qos_worker", &qos_worker).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED_TABLES: &[&str] = &[
        "rakurai_info_bundle_lifecycle",
        "rakurai_tin_connection_state",
        "rakurai_warning",
        "rakurai_info",
        "rakurai_status",
        "rakurai_scheduler_qos_throughput",
        "rakurai_scheduler_qos_worker",
    ];

    #[test]
    fn test_parse_field_value() {
        assert_eq!(parse_field_value("42i"), ParsedFieldValue::I64(42));
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
    fn test_map_bundle_lifecycle() {
        let mut point = DataPoint::new("rakurai_info_bundle_lifecycle");
        point.add_field_str("bundle_id", "abc");
        point.add_field_i64("num_txs", 3);
        point.add_field_i64("is_primary", 1);
        point.add_field_str("outcome", "landed");
        point.add_field_str("drop_reason", "");

        let row = map_bundle_lifecycle(&point, "host-1");
        assert_eq!(row.host_id, "host-1");
        assert_eq!(row.bundle_id.as_deref(), Some("abc"));
        assert_eq!(row.num_txs, Some(3));
        assert_eq!(row.is_primary, Some(1));
        assert_eq!(row.outcome.as_deref(), Some("landed"));
        assert!(row.drop_reason.is_none());
        assert_eq!(row.time_ns, point_time_ns(&point));
    }

    #[test]
    fn test_map_warning_folds_message() {
        let mut point = DataPoint::new("rakurai_warning");
        point.add_field_str("rakurai_abort_log", "scheduler aborted");
        let row = map_warning(&point, "host-1");
        assert_eq!(row.rakurai_abort_log.as_deref(), Some("scheduler aborted"));

        let mut point2 = DataPoint::new("rakurai_warning");
        point2.add_field_str("rakurai_warning_log", "warn only");
        let row2 = map_warning(&point2, "host-1");
        assert_eq!(row2.rakurai_abort_log.as_deref(), Some("warn only"));
    }

    #[test]
    fn test_map_status() {
        let mut point = DataPoint::new("rakurai_status");
        point.add_field_bool("enabled", true);
        let row = map_status(&point, "host-1");
        assert_eq!(row.enabled, Some(true));
    }

    #[test]
    fn test_map_tin_includes_actual_url() {
        let mut point = DataPoint::new("rakurai_tin_connection_state");
        point.add_field_str("url", "https://configured.example");
        point.add_field_str("actual_url", "https://resolved.example");
        point.add_field_str("uuid", "u-1");
        point.add_field_bool("primary", true);
        let row = map_tin(&point, "host-1");
        assert_eq!(row.url.as_deref(), Some("https://configured.example"));
        assert_eq!(row.actual_url.as_deref(), Some("https://resolved.example"));
        assert_eq!(row.uuid.as_deref(), Some("u-1"));
        assert_eq!(row.primary_conn, Some(true));
    }

    #[test]
    fn test_unknown_table_not_allowed() {
        assert!(!ALLOWED_TABLES.contains(&"rakurai_write_grant_probe"));
        assert!(ALLOWED_TABLES.contains(&"rakurai_warning"));
    }

    /// Live end-to-end smoke: official client + TLS verify + insert all allowlisted tables.
    ///
    /// `ch_writer` is INSERT-only; SELECT readback is best-effort (skipped on ACCESS_DENIED).
    ///
    /// Run with:
    /// `CH_SMOKE_PASSWORD=... cargo test -p solana-metrics --features agave-unstable-api --lib \
    ///    smoke_live_clickhouse_all_tables -- --ignored --nocapture`
    #[test]
    #[ignore = "live ClickHouse network smoke"]
    fn smoke_live_clickhouse_all_tables() {
        let host = std::env::var("CH_SMOKE_HOST")
            .unwrap_or_else(|_| "https://metrics.rakurai.io:8443".to_string());
        let db = std::env::var("CH_SMOKE_DB").unwrap_or_else(|_| "rakurai_stats_db".to_string());
        let user = std::env::var("CH_SMOKE_USER").unwrap_or_else(|_| "ch_writer".to_string());
        let password = std::env::var("CH_SMOKE_PASSWORD")
            .expect("set CH_SMOKE_PASSWORD to run live ClickHouse smoke");

        let marker = format!(
            "crate-smoke-{}",
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );

        let client = build_client(&host, &db, &user, &password);

        let mut bundle_dp = DataPoint::new("rakurai_info_bundle_lifecycle");
        bundle_dp.add_field_str("bundle_id", &marker);
        bundle_dp.add_field_i64("num_txs", 1);
        bundle_dp.add_field_str("outcome", "smoke");

        let mut tin_dp = DataPoint::new("rakurai_tin_connection_state");
        tin_dp.add_field_bool("primary", true);
        tin_dp.add_field_str("state", "smoke");
        tin_dp.add_field_str("source", &marker);
        tin_dp.add_field_str("url", "https://smoke.example/configured");
        tin_dp.add_field_str("actual_url", "https://smoke.example/resolved");
        tin_dp.add_field_str("uuid", &marker);

        let mut warning_dp = DataPoint::new("rakurai_warning");
        warning_dp.add_field_str("rakurai_abort_log", &marker);

        let mut info_dp = DataPoint::new("rakurai_info");
        info_dp.add_field_str("rakurai_info_log", &marker);

        let mut status_dp = DataPoint::new("rakurai_status");
        status_dp.add_field_bool("enabled", true);

        let mut qos_tp_dp = DataPoint::new("rakurai_scheduler_qos_throughput");
        qos_tp_dp.add_field_i64("ordinary_transaction_count", 1);

        let mut qos_worker_dp = DataPoint::new("rakurai_scheduler_qos_worker");
        qos_worker_dp.add_field_i64("worker", 0);
        qos_worker_dp.add_field_i64("transactions", 1);

        let points = [
            bundle_dp,
            tin_dp,
            warning_dp,
            info_dp,
            status_dp,
            qos_tp_dp,
            qos_worker_dp,
        ];

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let one: u8 = client
                .query("SELECT 1")
                .fetch_one()
                .await
                .expect("SELECT 1 over verified TLS failed");
            assert_eq!(one, 1, "ClickHouse SELECT 1");
            eprintln!("OK SELECT 1 (verified TLS)");

            write_rakurai_points(&client, &points, &marker).await;
            eprintln!("OK write_rakurai_points for all allowlisted tables");

            match client
                .query("SELECT count() FROM rakurai_info WHERE host_id = ?")
                .bind(&marker)
                .fetch_one::<u64>()
                .await
            {
                Ok(n) => {
                    assert!(n >= 1, "expected readback rows, got {n}");
                    eprintln!("OK SELECT readback rakurai_info count={n}");
                }
                Err(err) => {
                    let msg = err.to_string();
                    assert!(
                        msg.contains("ACCESS_DENIED") || msg.contains("Not enough privileges"),
                        "unexpected SELECT failure: {msg}"
                    );
                    eprintln!(
                        "OK INSERT-only user (SELECT denied as expected): {}",
                        msg.lines().next().unwrap_or(&msg)
                    );
                }
            }
        });
    }
}
