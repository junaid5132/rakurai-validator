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

/// Declares a ClickHouse row struct plus `Row::from_point(&DataPoint, host_id)`.
/// Field syntax: `[#[rename = "col"]] name: kind = "source_key"` (`kind` is
/// `str` | `i64` | `bool`). `timestamp`/`time_ns`/`host_id` are added for you.
macro_rules! rakurai_row {
    ($row:ident { $($(#[rename = $rename:literal])? $field:ident : $kind:ident = $key:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Row, Serialize)]
        struct $row {
            #[serde(with = "clickhouse::serde::chrono::datetime64::nanos")]
            timestamp: DateTime<Utc>,
            time_ns: u64,
            host_id: String,
            $($(#[serde(rename = $rename)])? $field: rakurai_row!(@ty $kind)),+
        }

        impl $row {
            fn from_point(point: &DataPoint, host_id: &str) -> Self {
                let fields = parse_datapoint_fields(&point.fields);
                let time_ns = point_time_ns(point);
                let (timestamp, time_ns, host_id) = common(host_id, time_ns);
                Self {
                    timestamp,
                    time_ns,
                    host_id,
                    $($field: rakurai_row!(@get $kind, &fields, $key)),+
                }
            }
        }
    };
    (@ty str) => { Option<String> };
    (@ty i64) => { Option<i64> };
    (@ty bool) => { Option<bool> };
    (@get str, $fields:expr, $key:literal) => { field_string($fields, $key) };
    (@get i64, $fields:expr, $key:literal) => { field_i64($fields, $key) };
    (@get bool, $fields:expr, $key:literal) => { field_bool($fields, $key) };
}

fn common(host_id: &str, time_ns: u64) -> (DateTime<Utc>, u64, String) {
    (timestamp_from_nanos(time_ns), time_ns, host_id.to_string())
}

rakurai_row!(BundleLifecycleRow {
    block_engine_uuid: str = "block_engine_uuid",
    bundle_id: str = "bundle_id",
    bundle_priority: i64 = "bundle_priority",
    drop_reason: str = "drop_reason",
    end_timestamp_ns: i64 = "end_timestamp_ns",
    has_postpack_confirmation: i64 = "has_postpack_confirmation",
    is_primary: i64 = "is_primary",
    num_txs: i64 = "num_txs",
    outcome: str = "outcome",
    p2c_id: str = "p2c_id",
    priority_fee_lamports: i64 = "priority_fee_lamports",
    received_slot: i64 = "received_slot",
    signatures: str = "signatures",
    start_timestamp_ns: i64 = "start_timestamp_ns",
    tip_lamports: i64 = "tip_lamports",
    total_cus: i64 = "total_cus",
});

rakurai_row!(TinConnectionStateRow {
    #[rename = "primary"]
    primary_conn: bool = "primary",
    source: str = "source",
    state: str = "state",
    url: str = "url",
    actual_url: str = "actual_url",
    uuid: str = "uuid",
});

rakurai_row!(WarningRow {
    rakurai_abort_log: str = "rakurai_abort_log",
});

rakurai_row!(InfoRow {
    rakurai_info_log: str = "rakurai_info_log",
});

rakurai_row!(StatusRow {
    enabled: bool = "enabled",
});

rakurai_row!(QosThroughputRow {
    candidate_non_singleton_count: i64 = "candidate_non_singleton_count",
    candidate_singleton_count: i64 = "candidate_singleton_count",
    dedicated_lane_dequeue_count: i64 = "dedicated_lane_dequeue_count",
    dedicated_lane_enqueue_count: i64 = "dedicated_lane_enqueue_count",
    dedicated_lane_service_max_us: i64 = "dedicated_lane_service_max_us",
    dedicated_lane_service_us: i64 = "dedicated_lane_service_us",
    ingress_ordinary: i64 = "ingress_ordinary",
    ingress_pruner_candidate: i64 = "ingress_pruner_candidate",
    mixed_batch_count: i64 = "mixed_batch_count",
    ordinary_batch_count: i64 = "ordinary_batch_count",
    ordinary_batch_max_transactions: i64 = "ordinary_batch_max_transactions",
    ordinary_transaction_count: i64 = "ordinary_transaction_count",
    report_elapsed_us: i64 = "report_elapsed_us",
    s1_bounded_ordinary_dispatched_batches: i64 = "s1_bounded_ordinary_dispatched_batches",
    s1_bounded_ordinary_passes: i64 = "s1_bounded_ordinary_passes",
    s1_candidate_recheck_already_processed: i64 = "s1_candidate_recheck_already_processed",
    s1_candidate_recheck_blockhash_not_found: i64 = "s1_candidate_recheck_blockhash_not_found",
    s1_candidate_recheck_dropped: i64 = "s1_candidate_recheck_dropped",
    s1_candidate_recheck_other: i64 = "s1_candidate_recheck_other",
    s1_early_ordinary_dispatched_batches: i64 = "s1_early_ordinary_dispatched_batches",
    s1_early_ordinary_passes: i64 = "s1_early_ordinary_passes",
    s1_ordinary_in_flight_worker_samples: i64 = "s1_ordinary_in_flight_worker_samples",
    s1_ordinary_lock_or_admission_blocked_rounds: i64 = "s1_ordinary_lock_or_admission_blocked_rounds",
    s1_ordinary_no_worker_capacity_rounds: i64 = "s1_ordinary_no_worker_capacity_rounds",
    s1_ordinary_queue_busy_worker_samples: i64 = "s1_ordinary_queue_busy_worker_samples",
    worker_estimated_cu_total: i64 = "worker_estimated_cu_total",
    worker_transaction_total: i64 = "worker_transaction_total",
    worker_work_total: i64 = "worker_work_total",
});

rakurai_row!(QosWorkerRow {
    candidate_batches: i64 = "candidate_batches",
    enqueue_blocked_us: i64 = "enqueue_blocked_us",
    estimated_compute_units: i64 = "estimated_compute_units",
    estimated_cu_per_second: i64 = "estimated_cu_per_second",
    estimated_cu_share_ppm: i64 = "estimated_cu_share_ppm",
    max_batch_transactions: i64 = "max_batch_transactions",
    max_sender_queue_depth: i64 = "max_sender_queue_depth",
    ordinary_batches: i64 = "ordinary_batches",
    transaction_share_ppm: i64 = "transaction_share_ppm",
    transactions: i64 = "transactions",
    transactions_per_second: i64 = "transactions_per_second",
    work_batches: i64 = "work_batches",
    work_per_second: i64 = "work_per_second",
    work_share_ppm: i64 = "work_share_ppm",
    worker: i64 = "worker",
});

rakurai_row!(P2cUpdateCountRow {
    p2c_tpu_enabled: bool = "p2c_tpu_enabled",
    scheduler_count: i64 = "scheduler_count",
    slot: i64 = "slot",
    total_count: i64 = "total_count",
    tpu_count: i64 = "tpu_count",
    uuid: str = "uuid",
});

rakurai_row!(P2cTotalUpdateCountRow {
    scheduler_count: i64 = "scheduler_count",
    slot: i64 = "slot",
    total_count: i64 = "total_count",
    tpu_count: i64 = "tpu_count",
});

// `rakurai_warning`/`rakurai_info` datapoints used two possible field names for
// their message depending on the caller; fold both onto the one row column.
fn map_warning(point: &DataPoint, host_id: &str) -> WarningRow {
    let mut row = WarningRow::from_point(point, host_id);
    if row.rakurai_abort_log.is_none() {
        let fields = parse_datapoint_fields(&point.fields);
        row.rakurai_abort_log = field_string(&fields, "rakurai_warning_log");
    }
    row
}

fn map_info(point: &DataPoint, host_id: &str) -> InfoRow {
    let mut row = InfoRow::from_point(point, host_id);
    if row.rakurai_info_log.is_none() {
        let fields = parse_datapoint_fields(&point.fields);
        row.rakurai_info_log = fields.values().find_map(|v| match v {
            ParsedFieldValue::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        });
    }
    row
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
    let mut p2c_update = Vec::new();
    let mut p2c_total_update = Vec::new();

    for point in points {
        match point.name {
            "rakurai_info_bundle_lifecycle" => {
                bundle.push(BundleLifecycleRow::from_point(point, host_id))
            }
            "rakurai_tin_connection_state" => {
                tin.push(TinConnectionStateRow::from_point(point, host_id))
            }
            "rakurai_warning" => warning.push(map_warning(point, host_id)),
            "rakurai_info" => info.push(map_info(point, host_id)),
            "rakurai_status" => status.push(StatusRow::from_point(point, host_id)),
            "rakurai_scheduler_qos_throughput" => {
                qos_tp.push(QosThroughputRow::from_point(point, host_id))
            }
            "rakurai_scheduler_qos_worker" => {
                qos_worker.push(QosWorkerRow::from_point(point, host_id))
            }
            "rakurai_p2c_update_count" => {
                p2c_update.push(P2cUpdateCountRow::from_point(point, host_id))
            }
            "rakurai_p2c_total_update_count" => {
                p2c_total_update.push(P2cTotalUpdateCountRow::from_point(point, host_id))
            }
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
    insert_rows(client, "rakurai_p2c_update_count", &p2c_update).await;
    insert_rows(client, "rakurai_p2c_total_update_count", &p2c_total_update).await;
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
        "rakurai_p2c_update_count",
        "rakurai_p2c_total_update_count",
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
    fn test_row_from_point_maps_typed_fields() {
        let mut point = DataPoint::new("rakurai_info_bundle_lifecycle");
        point.add_field_str("bundle_id", "abc");
        point.add_field_i64("num_txs", 3);
        point.add_field_i64("is_primary", 1);
        point.add_field_str("outcome", "landed");
        point.add_field_str("drop_reason", "");

        let row = BundleLifecycleRow::from_point(&point, "host-1");
        assert_eq!(row.host_id, "host-1");
        assert_eq!(row.bundle_id.as_deref(), Some("abc"));
        assert_eq!(row.num_txs, Some(3));
        assert_eq!(row.is_primary, Some(1));
        assert_eq!(row.outcome.as_deref(), Some("landed"));
        assert!(row.drop_reason.is_none()); // empty string -> no value, not Some("")
        assert_eq!(row.time_ns, point_time_ns(&point));

        let mut tin = DataPoint::new("rakurai_tin_connection_state");
        tin.add_field_str("uuid", "u-1");
        tin.add_field_bool("primary", true); // renamed column: primary_conn <- "primary"
        let tin_row = TinConnectionStateRow::from_point(&tin, "host-1");
        assert_eq!(tin_row.uuid.as_deref(), Some("u-1"));
        assert_eq!(tin_row.primary_conn, Some(true));

        let mut p2c = DataPoint::new("rakurai_p2c_update_count");
        p2c.add_field_i64("total_count", 5);
        p2c.add_field_bool("p2c_tpu_enabled", true);
        let p2c_row = P2cUpdateCountRow::from_point(&p2c, "host-1");
        assert_eq!(p2c_row.total_count, Some(5));
        assert_eq!(p2c_row.p2c_tpu_enabled, Some(true));
    }

    #[test]
    fn test_map_warning_and_info_fall_back_to_legacy_field_name() {
        let mut point = DataPoint::new("rakurai_warning");
        point.add_field_str("rakurai_warning_log", "warn only");
        let row = map_warning(&point, "host-1");
        assert_eq!(row.rakurai_abort_log.as_deref(), Some("warn only"));

        let mut point = DataPoint::new("rakurai_info");
        point.add_field_str("some_other_field", "fallback message");
        let row = map_info(&point, "host-1");
        assert_eq!(row.rakurai_info_log.as_deref(), Some("fallback message"));
    }

    #[test]
    fn test_unknown_table_not_allowed() {
        assert!(!ALLOWED_TABLES.contains(&"rakurai_write_grant_probe"));
        assert!(ALLOWED_TABLES.contains(&"rakurai_warning"));
    }
}
