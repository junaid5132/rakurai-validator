-- Schema hardening for rakurai_info_bundle_lifecycle (rakurai_stats_db).
--
-- Live production already has these objects. This file is for:
--   1) Existing envs that need ALTER + MATERIALIZE
--   2) Documentation of what new CREATE TABLE templates must include
--
-- The metrics crate (official `clickhouse` client) only INSERTs rows; schema
-- DDL is managed out-of-band. INSERT path is unchanged: same columns;
-- ClickHouse maintains indexes/projection.
--
-- We intentionally keep ORDER BY (host_id, timestamp). Timestamp-first dashboard
-- reads use PROJECTION proj_by_timestamp instead of a full table rebuild.

-- ---------------------------------------------------------------------------
-- Existing environments (idempotent-ish: skip if object already exists)
-- ---------------------------------------------------------------------------

ALTER TABLE rakurai_info_bundle_lifecycle
    ADD INDEX IF NOT EXISTS idx_bundle_id bundle_id TYPE bloom_filter(0.01) GRANULARITY 4;

ALTER TABLE rakurai_info_bundle_lifecycle
    ADD INDEX IF NOT EXISTS idx_signatures_ngram ifNull(signatures, '') TYPE ngrambf_v1(3, 256, 2, 0) GRANULARITY 4;

ALTER TABLE rakurai_info_bundle_lifecycle
    ADD PROJECTION IF NOT EXISTS proj_by_timestamp
    (
        SELECT *
        ORDER BY (timestamp, host_id)
    );

-- Backfill existing parts (run once per env after ADD INDEX / ADD PROJECTION).
ALTER TABLE rakurai_info_bundle_lifecycle MATERIALIZE INDEX idx_bundle_id;
ALTER TABLE rakurai_info_bundle_lifecycle MATERIALIZE INDEX idx_signatures_ngram;
ALTER TABLE rakurai_info_bundle_lifecycle MATERIALIZE PROJECTION proj_by_timestamp;

-- ---------------------------------------------------------------------------
-- Optional follow-up (NOT applied live yet): better ngram indexing
-- ---------------------------------------------------------------------------
-- Per ClickHouse best practice (avoid Nullable when empty string is enough):
--   ALTER ... MODIFY COLUMN signatures String DEFAULT '';
-- then replace idx_signatures_ngram to index `signatures` directly.
-- That is a schema migration only; producer already emits '' for empty values
-- (serialized as null today via Nullable). Coordinate cutover before changing
-- the CREATE TABLE template away from Nullable(String).
