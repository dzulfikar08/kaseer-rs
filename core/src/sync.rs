//! Offline-first cloud sync. The outbox is a device-local SQLite file
//! (`./data/sync-outbox.db`) that never touches the shared ledger's
//! migrations or `user_version`; the drain pushes queued rows to the
//! kaseer-cloud worker over the frozen batch contract (POST
//! /api/sync/batch, Bearer store token, idempotent by row id).
//!
//! Enqueue is polling-primary by design: every drain re-reads the ledger
//! (`sales_between` from the last watermark, plus the full product and
//! customer catalog — which makes the first successful drain the
//! bootstrap push) instead of hooking checkout, so a settled sale is
//! queued no matter which code path wrote it. Rows dedupe on
//! `(kind, ref)`; a changed payload re-activates a done row so catalog
//! edits converge too.
//!
//! Failures degrade softly: an HTTP-level failure backs the next drain
//! off 1s→60s and leaves the queue untouched; per-row errors keep their
//! row queued with an attempts counter and never block the rest of the
//! batch (the cloud counts one bad row in `errors` and accepts the rest).

use std::env;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::receipt::{Sale, SaleLine, SaleTender};
use crate::repos::{Customer, Product, Repo};

/// Device-local outbox path; sibling of the default ledger.
pub const DEFAULT_OUTBOX_PATH: &str = "./data/sync-outbox.db";

/// Placeholder endpoint shown before a store configures its worker URL.
pub const PLACEHOLDER_CLOUD_URL: &str = "https://kaseer-cloud.<subdomain>.workers.dev";

/// Batch size ceiling, matching the cloud's MAX_BATCH_ROWS.
const BATCH_ROWS: usize = 500;

/// Done rows older than this are pruned on drain.
const PRUNE_AFTER_DAYS: i64 = 7;

/// First watermark: `sales_between` string-compares ISO timestamps, so
/// every recorded sale is "since the epoch" on the very first drain.
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

pub fn resolve_outbox_path() -> String {
    env::var("KASEER_OUTBOX").unwrap_or_else(|_| DEFAULT_OUTBOX_PATH.to_string())
}

/// Configured cloud URL (`KASEER_CLOUD_URL`) or the placeholder.
pub fn default_cloud_url() -> String {
    env::var("KASEER_CLOUD_URL").unwrap_or_else(|_| PLACEHOLDER_CLOUD_URL.to_string())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ── Wire shapes (the frozen contract is camelCase) ──────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireSaleLine<'a> {
    product_id: &'a str,
    name: &'a str,
    sku: &'a str,
    qty: f64,
    unit_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_minor: Option<i64>,
    discount_minor: i64,
    net_minor: i64,
    tax_minor: i64,
    tax_rate_id: Option<&'a str>,
}

impl<'a> From<&'a SaleLine> for WireSaleLine<'a> {
    fn from(l: &'a SaleLine) -> Self {
        Self {
            product_id: &l.product_id,
            name: &l.name,
            sku: &l.sku,
            qty: l.qty,
            unit_minor: l.unit_minor,
            cost_minor: l.cost_minor,
            discount_minor: l.discount_minor,
            net_minor: l.net_minor,
            tax_minor: l.tax_minor,
            tax_rate_id: l.tax_rate_id.as_deref(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireTender<'a> {
    method: &'a crate::payments::TenderMethod,
    applied_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    given_minor: Option<i64>,
}

impl<'a> From<&'a SaleTender> for WireTender<'a> {
    fn from(t: &'a SaleTender) -> Self {
        Self {
            method: &t.method,
            applied_minor: t.applied_minor,
            given_minor: t.given_minor,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireSale<'a> {
    id: &'a str,
    number: &'a str,
    at: &'a str,
    currency: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    shift_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer_name_snapshot: Option<&'a str>,
    lines: Vec<WireSaleLine<'a>>,
    tenders: Vec<WireTender<'a>>,
    gross_minor: i64,
    discount_minor: i64,
    net_minor: i64,
    tax_minor: i64,
    total_minor: i64,
    change_minor: i64,
    points_earned: i64,
    points_redeemed: i64,
    tax_by_rate_id: &'a std::collections::BTreeMap<String, i64>,
}

impl<'a> From<&'a Sale> for WireSale<'a> {
    fn from(s: &'a Sale) -> Self {
        Self {
            id: &s.id,
            number: &s.number,
            at: &s.at,
            currency: &s.currency,
            shift_id: s.shift_id.as_deref(),
            customer_id: s.customer_id.as_deref(),
            customer_name_snapshot: s.customer_name_snapshot.as_deref(),
            lines: s.lines.iter().map(Into::into).collect(),
            tenders: s.tenders.iter().map(Into::into).collect(),
            gross_minor: s.gross_minor,
            discount_minor: s.discount_minor,
            net_minor: s.net_minor,
            tax_minor: s.tax_minor,
            total_minor: s.total_minor,
            change_minor: s.change_minor,
            points_earned: s.points_earned,
            points_redeemed: s.points_redeemed,
            tax_by_rate_id: &s.tax_by_rate_id,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireProduct<'a> {
    id: &'a str,
    sku: &'a str,
    name: &'a str,
    category: &'a str,
    price_minor: i64,
    cost_minor: i64,
    track_stock: bool,
    low_stock_at: i64,
    archived: bool,
    description: &'a str,
    prep_notes: &'a str,
    allergens: &'a str,
    route_kitchen: bool,
}

impl<'a> From<&'a Product> for WireProduct<'a> {
    fn from(p: &'a Product) -> Self {
        Self {
            id: &p.id,
            sku: &p.sku,
            name: &p.name,
            category: &p.category,
            price_minor: p.price_minor,
            cost_minor: p.cost_minor,
            track_stock: p.track_stock,
            low_stock_at: p.low_stock_at,
            archived: p.archived,
            description: &p.description,
            prep_notes: &p.prep_notes,
            allergens: &p.allergens,
            route_kitchen: p.route_kitchen,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireCustomer<'a> {
    id: &'a str,
    name: &'a str,
    phone: &'a str,
    points: i64,
    created_at: &'a str,
}

impl<'a> From<&'a Customer> for WireCustomer<'a> {
    fn from(c: &'a Customer) -> Self {
        Self {
            id: &c.id,
            name: &c.name,
            phone: &c.phone,
            points: c.points,
            created_at: &c.created_at,
        }
    }
}

// ── Cloud ack ───────────────────────────────────────────────────────────

/// One per-row rejection from the cloud; every other row in the batch
/// was accepted or counted as an idempotent duplicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    pub ref_: String,
    pub message: String,
}

/// Parsed 200-response of POST /api/sync/batch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BatchAck {
    pub accepted: u64,
    pub duplicates: u64,
    pub errors: Vec<RowError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AckBody {
    #[serde(default)]
    accepted: u64,
    #[serde(default)]
    duplicates: u64,
    #[serde(default)]
    errors: Vec<AckError>,
}

#[derive(Deserialize)]
struct AckError {
    #[serde(rename = "ref")]
    error_ref: String,
    message: String,
}

/// Parses the batch response; a non-JSON 200 is a protocol error and the
/// chunk stays queued.
pub fn parse_batch_ack(body: &str) -> Result<BatchAck, String> {
    let parsed: AckBody =
        serde_json::from_str(body).map_err(|e| format!("unreadable batch response: {e}"))?;
    Ok(BatchAck {
        accepted: parsed.accepted,
        duplicates: parsed.duplicates,
        errors: parsed
            .errors
            .into_iter()
            .map(|e| RowError {
                ref_: e.error_ref,
                message: e.message,
            })
            .collect(),
    })
}

// ── Outbox ──────────────────────────────────────────────────────────────

/// One queued row: a wire-JSON payload awaiting a batch that acknowledged
/// it (or counted it as a duplicate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRow {
    pub id: i64,
    pub kind: &'static str,
    pub ref_: String,
    pub payload: String,
}

const OUTBOX_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK (kind IN ('sale', 'stockMove', 'customer', 'product')),
  ref TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  done INTEGER NOT NULL DEFAULT 0,
  UNIQUE (kind, ref)
);
CREATE INDEX IF NOT EXISTS idx_outbox_pending ON outbox(done, attempts, id);
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
";

/// The device-local sync queue. Own connection, own file: opening it
/// never migrates or seeds anything else.
pub struct Outbox {
    conn: Connection,
}

/// True for the SQLITE_BUSY family, including the "database is locked"
/// message busy handlers cannot cover (e.g. journal-mode changes while
/// another connection holds a read transaction).
pub fn is_busy_error(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => {
            let message = message.to_lowercase();
            message.contains("locked") || message.contains("busy")
        }
        _ => false,
    }
}

/// `PRAGMA journal_mode = WAL` errors immediately (no busy handler) when
/// any other connection is mid-transaction; the mode is sticky, so only
/// set it when it is not already WAL.
pub fn ensure_wal(conn: &Connection) -> rusqlite::Result<()> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        return Ok(());
    }
    let mut attempt = 0;
    loop {
        match conn.execute_batch("PRAGMA journal_mode = WAL;") {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 25 && is_busy_error(&error) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error),
        }
    }
}

/// CREATE TABLE/INDEX statements race between the process's concurrent
/// openers (register UI, drain, menu poll, heartbeat, update check); a
/// short retry turns that collision into a wait instead of a failed open.
fn bootstrap_with_retry(conn: &Connection, statements: &str) -> rusqlite::Result<()> {
    let mut attempt = 0;
    loop {
        match conn.execute_batch(statements) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 25 && is_busy_error(&error) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error),
        }
    }
}

impl Outbox {
    pub fn open(path: &str) -> rusqlite::Result<Outbox> {
        if path != ":memory:" {
            if let Some(dir) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
        }
        let conn = Connection::open(path)?;
        // The outbox has several independent writers (drain, menu poll,
        // heartbeat, enrollment): a 5s busy window turns their rare
        // overlaps into short waits instead of spurious "database is
        // locked" errors.
        conn.busy_timeout(Duration::from_secs(5))?;
        ensure_wal(&conn)?;
        bootstrap_with_retry(&conn, OUTBOX_SCHEMA)?;
        Ok(Outbox { conn })
    }

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Queues a row unless an identical done-or-pending payload is
    /// already recorded. A changed payload re-activates the row (catalog
    /// edits re-push); an unchanged done row stays done (the 7-day prune
    /// eventually re-pushes it and the cloud counts a duplicate).
    fn upsert(&self, kind: &'static str, ref_: &str, payload: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO outbox (kind, ref, payload, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, ref) DO UPDATE SET
               payload = excluded.payload,
               done = 0,
               attempts = 0,
               last_error = NULL
             WHERE outbox.payload IS NOT excluded.payload OR outbox.done = 0",
            params![kind, ref_, payload, now_iso()],
        )?;
        Ok(())
    }

    pub fn enqueue_sale(&self, sale: &Sale) -> rusqlite::Result<()> {
        let payload = serde_json::to_string(&WireSale::from(sale))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        self.upsert("sale", &sale.id, &payload)
    }

    pub fn enqueue_product(&self, product: &Product) -> rusqlite::Result<()> {
        let payload = serde_json::to_string(&WireProduct::from(product))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        self.upsert("product", &product.id, &payload)
    }

    pub fn enqueue_customer(&self, customer: &Customer) -> rusqlite::Result<()> {
        let payload = serde_json::to_string(&WireCustomer::from(customer))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        self.upsert("customer", &customer.id, &payload)
    }

    /// Rows waiting to reach the cloud.
    pub fn pending_count(&self) -> u64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM outbox WHERE done = 0", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n.max(0) as u64)
            .unwrap_or(0)
    }

    /// Next chunk to send, failing rows sinking to the back so a poison
    /// row cannot head-of-line block the queue.
    fn take_chunk(&self, limit: usize) -> rusqlite::Result<Vec<QueuedRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, ref, payload FROM outbox WHERE done = 0 ORDER BY attempts, id LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok(QueuedRow {
                    id: r.get(0)?,
                    kind: kind_const(&r.get::<_, String>(1)?),
                    ref_: r.get(2)?,
                    payload: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn mark_done(&self, ids: &[i64]) -> rusqlite::Result<()> {
        for id in ids {
            self.conn.execute(
                "UPDATE outbox SET done = 1, last_error = NULL WHERE id = ?1",
                [id],
            )?;
        }
        Ok(())
    }

    fn mark_row_failed(&self, id: i64, error: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    /// Drops acknowledged rows older than the prune window.
    fn prune_done(&self) -> rusqlite::Result<usize> {
        let cutoff = (Utc::now() - ChronoDuration::days(PRUNE_AFTER_DAYS))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        self.conn
            .execute("DELETE FROM outbox WHERE done = 1 AND created_at <= ?1", [cutoff])
    }

    fn drain_attempts(&self) -> u64 {
        self.meta_get("drain_attempts")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }
}

fn kind_const(kind: &str) -> &'static str {
    match kind {
        "sale" => "sale",
        "stockMove" => "stockMove",
        "customer" => "customer",
        _ => "product",
    }
}

// ── Drain ───────────────────────────────────────────────────────────────

/// The HTTP boundary of the drain, so the engine stays transport-free
/// (ureq lives in the app; tests capture the body).
pub trait BatchHttp {
    fn post_batch(&self, base_url: &str, token: &str, body: &str) -> Result<String, String>;
}

/// What one drain did, for the Settings status line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DrainOutcome {
    /// Sales newly queued by the watermark poll this drain.
    pub queued_sales: usize,
    pub accepted: u64,
    pub duplicates: u64,
    /// Per-row rejections, still queued for the next drain.
    pub row_errors: Vec<RowError>,
    /// Rows still pending when the drain ended.
    pub remaining: u64,
    /// Set when the transport failed; the whole chunk stays queued.
    pub http_error: Option<String>,
    /// True when skipped inside an active backoff window (a forced,
    /// manual drain never sets this).
    pub backed_off: bool,
}

impl DrainOutcome {
    /// One-line status for the UI.
    pub fn summary(&self) -> String {
        if self.backed_off {
            return format!("Backing off — {} pending, retry later", self.remaining);
        }
        if let Some(error) = &self.http_error {
            return format!("Cloud unreachable — {error}");
        }
        let mut text = format!(
            "{} accepted, {} duplicates",
            self.accepted, self.duplicates
        );
        if !self.row_errors.is_empty() {
            text.push_str(&format!(" — {} row(s) rejected", self.row_errors.len()));
        }
        if self.remaining > 0 {
            text.push_str(&format!(", {} still pending", self.remaining));
        }
        text
    }
}

/// Exponential 1s→60s backoff for the nth consecutive transport failure.
fn backoff_secs(attempts: u64) -> i64 {
    2i64
        .saturating_pow(attempts.saturating_sub(1).min(62) as u32)
        .min(60)
}

/// Assembles one batch body from queued rows. Empty groups still appear:
/// the cloud's row cap counts every array.
fn chunk_body(chunk: &[QueuedRow]) -> Result<String, String> {
    let (mut sales, mut moves, mut customers, mut products) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for row in chunk {
        let value: serde_json::Value = serde_json::from_str(&row.payload)
            .map_err(|e| format!("corrupt outbox payload {}: {e}", row.ref_))?;
        match row.kind {
            "sale" => sales.push(value),
            "stockMove" => moves.push(value),
            "customer" => customers.push(value),
            _ => products.push(value),
        }
    }
    serde_json::to_string(&json!({
        "sales": sales,
        "stockMoves": moves,
        "customers": customers,
        "products": products,
    }))
    .map_err(|e| e.to_string())
}

/// One drain: poll the ledger for anything unsynced, then push queued
/// rows in batches. `forced` (the manual button) ignores the backoff
/// window; the automatic drains (app start, shift close) respect it.
pub fn drain(
    ledger: &Repo,
    outbox: &Outbox,
    http: &dyn BatchHttp,
    forced: bool,
) -> Result<DrainOutcome, String> {
    // Credentials: an enrolled kiosk presents its device token to the same
    // /api/sync/batch endpoint; the pre-enrollment store token keeps
    // working for terminals that sync without enrolling.
    let token = match outbox.meta_get("device_token").filter(|t| !t.trim().is_empty()) {
        Some(device_token) => device_token,
        None => outbox
            .meta_get("store_token")
            .filter(|t| !t.trim().is_empty())
            .ok_or("No store token configured")?,
    };
    let base_url = outbox
        .meta_get("cloud_url")
        .filter(|u| !u.trim().is_empty())
        .ok_or("No cloud URL configured")?;

    let now = now_iso();
    if !forced
        && outbox
            .meta_get("next_attempt_at")
            .is_some_and(|next| now < next)
    {
        let outcome = DrainOutcome {
            backed_off: true,
            remaining: outbox.pending_count(),
            ..Default::default()
        };
        return Ok(outcome);
    }

    // Enqueue pass (polling-primary): sales since the watermark, then the
    // full catalog so the first drain bootstraps master data.
    let watermark = outbox.meta_get("last_synced_at").unwrap_or_else(|| EPOCH.to_string());
    let mut outcome = DrainOutcome::default();
    for sale in ledger
        .sales_between(&watermark, &now)
        .map_err(|e| e.to_string())?
    {
        outbox.enqueue_sale(&sale).map_err(|e| e.to_string())?;
        outcome.queued_sales += 1;
    }
    for product in ledger.products(true, None).map_err(|e| e.to_string())? {
        outbox.enqueue_product(&product).map_err(|e| e.to_string())?;
    }
    for customer in ledger.customers(None).map_err(|e| e.to_string())? {
        outbox.enqueue_customer(&customer).map_err(|e| e.to_string())?;
    }

    // Push pass: batches of up to 500 rows until the queue is empty, the
    // transport fails, or a chunk reports row errors (those retry next
    // drain; pushing onward would re-send the identical chunk forever).
    loop {
        let chunk = outbox.take_chunk(BATCH_ROWS).map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            break;
        }
        let body = chunk_body(&chunk)?;
        match http.post_batch(&base_url, &token, &body) {
            Err(message) => {
                let attempts = outbox.drain_attempts() + 1;
                let next_at = (Utc::now() + ChronoDuration::seconds(backoff_secs(attempts)))
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                let _ = outbox.meta_set("drain_attempts", &attempts.to_string());
                let _ = outbox.meta_set("next_attempt_at", &next_at);
                let _ = outbox.meta_set("last_error", &message);
                outcome.http_error = Some(message);
                break;
            }
            Ok(response) => {
                let ack = parse_batch_ack(&response)?;
                outcome.accepted += ack.accepted;
                outcome.duplicates += ack.duplicates;
                let rejected: std::collections::HashMap<&str, &str> = ack
                    .errors
                    .iter()
                    .map(|e| (e.ref_.as_str(), e.message.as_str()))
                    .collect();
                outcome.row_errors.extend(ack.errors.iter().map(|e| RowError {
                    ref_: e.ref_.clone(),
                    message: e.message.clone(),
                }));
                let mut done_ids = Vec::with_capacity(chunk.len());
                for row in &chunk {
                    match rejected.get(row.ref_.as_str()) {
                        Some(message) => {
                            let _ = outbox.mark_row_failed(row.id, message);
                        }
                        None => done_ids.push(row.id),
                    }
                }
                outbox.mark_done(&done_ids).map_err(|e| e.to_string())?;
                if !rejected.is_empty() {
                    break;
                }
            }
        }
    }

    // The watermark advances on any fully-responded drain: errored rows
    // stay queued in the outbox and retry from there, so re-polling them
    // via the watermark would only rewrite identical payloads.
    if outcome.http_error.is_none() {
        let _ = outbox.meta_set("last_synced_at", &now);
        let _ = outbox.meta_set("drain_attempts", "0");
        let _ = outbox.meta_delete("next_attempt_at");
        let _ = outbox.meta_set("last_sync_at", &now_iso());
        if outcome.row_errors.is_empty() {
            let _ = outbox.meta_delete("last_error");
        } else {
            let joined = outcome
                .row_errors
                .iter()
                .map(|e| format!("{}: {}", e.ref_, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            let _ = outbox.meta_set("last_error", &joined);
        }
    }
    let _ = outbox.prune_done();
    outcome.remaining = outbox.pending_count();
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_repo;
    use crate::payments::TenderMethod;
    use crate::receipt::{Sale, SaleLine};

    /// Records request bodies, tokens, and replays scripted responses.
    struct FakeHttp {
        responses: std::cell::RefCell<std::collections::VecDeque<Result<String, String>>>,
        bodies: std::cell::RefCell<Vec<String>>,
        tokens: std::cell::RefCell<Vec<String>>,
    }

    impl FakeHttp {
        fn ok(accepted: u64, duplicates: u64, errors: Vec<RowError>) -> Self {
            let errors: Vec<serde_json::Value> = errors
                .iter()
                .map(|e| json!({"ref": e.ref_, "message": e.message}))
                .collect();
            Self {
                responses: std::cell::RefCell::new(std::collections::VecDeque::from(vec![Ok(
                    format!(
                        r#"{{"accepted":{accepted},"duplicates":{duplicates},"errors":{}}}"#,
                        serde_json::to_string(&errors).unwrap()
                    ),
                )])),
                bodies: std::cell::RefCell::new(Vec::new()),
                tokens: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                responses: std::cell::RefCell::new(std::collections::VecDeque::from(vec![Err(
                    message.to_string(),
                )])),
                bodies: std::cell::RefCell::new(Vec::new()),
                tokens: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl BatchHttp for FakeHttp {
        fn post_batch(&self, _base: &str, token: &str, body: &str) -> Result<String, String> {
            self.bodies.borrow_mut().push(body.to_string());
            self.tokens.borrow_mut().push(token.to_string());
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("no scripted response left")
        }
    }

    fn repo(path: &str) -> Repo {
        open_repo(path).unwrap()
    }

    fn sample_sale(id: &str, total: i64) -> Sale {
        Sale {
            id: id.to_string(),
            number: format!("KSR-TEST-{id}"),
            // Inside any drain window: sales_between bounds on real now.
            at: (Utc::now() - ChronoDuration::seconds(60))
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            currency: "IDR".to_string(),
            shift_id: None,
            customer_id: None,
            customer_name_snapshot: None,
            lines: vec![SaleLine {
                product_id: "p1".to_string(),
                name: "Kopi".to_string(),
                sku: "KOPI".to_string(),
                qty: 2.0,
                unit_minor: total,
                cost_minor: None,
                discount_minor: 0,
                net_minor: total * 2,
                tax_minor: 0,
                tax_rate_id: None,
            }],
            tenders: vec![crate::receipt::SaleTender {
                method: TenderMethod::Cash,
                applied_minor: total * 2,
                given_minor: Some(total * 3),
            }],
            gross_minor: total * 2,
            discount_minor: 0,
            net_minor: total * 2,
            tax_minor: 0,
            total_minor: total * 2,
            change_minor: total,
            points_earned: 0,
            points_redeemed: 0,
            tax_by_rate_id: Default::default(),
        }
    }

    #[test]
    fn wire_payload_uses_the_frozen_camel_case_contract() {
        let sale = sample_sale("s1", 1_000);
        let payload = serde_json::to_value(WireSale::from(&sale)).unwrap();
        assert_eq!(payload["id"], "s1");
        assert_eq!(payload["grossMinor"], 2_000);
        assert!(payload["taxByRateId"].as_object().unwrap().is_empty());
        assert!(payload.get("shiftId").is_none());
        assert!(payload.get("customerId").is_none());
        let line = &payload["lines"][0];
        assert_eq!(line["productId"], "p1");
        assert_eq!(line["unitMinor"], 1_000);
        assert!(line.get("costMinor").is_none());
        let tender = &payload["tenders"][0];
        assert_eq!(tender["method"], "cash");
        assert_eq!(tender["appliedMinor"], 2_000);
        assert_eq!(tender["givenMinor"], 3_000);
    }

    #[test]
    fn drain_pushes_sales_and_bootstraps_catalog_once() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("ledger.db");
        let ledger = repo(ledger_path.to_str().unwrap());
        ledger
            .upsert_product(&crate::repos::Product {
                id: "p1".into(),
                sku: "KOPI".into(),
                name: "Kopi".into(),
                category: "Drinks".into(),
                price_minor: 5_000,
                cost_minor: 2_000,
                tax_rate_id: None,
                track_stock: true,
                low_stock_at: 3,
                archived: false,
                description: String::new(),
                prep_notes: String::new(),
                allergens: String::new(),
                route_kitchen: false,
                image_path: String::new(),
            })
            .unwrap();
        ledger
            .upsert_customer(&crate::repos::Customer {
                id: "c1".into(),
                name: "Sari".into(),
                phone: "0812".into(),
                points: 5,
                created_at: "2026-09-27T09:00:00.000Z".into(),
            })
            .unwrap();
        ledger.insert_sale(&sample_sale("s1", 1_000), &Default::default(), 0).unwrap();

        // open_repo seeds a demo catalog; the drain bootstrap-pushes all
        // of it, so expected row counts are 1 sale + the whole catalog.
        let catalog_rows =
            ledger.products(true, None).unwrap().len() + ledger.customers(None).unwrap().len();
        assert!(catalog_rows > 2, "seed catalog should be non-trivial");

        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();
        outbox.meta_set("store_token", "tok").unwrap();

        let http = FakeHttp::ok((1 + catalog_rows) as u64, 0, vec![]);
        let outcome = drain(&ledger, &outbox, &http, false).unwrap();
        assert_eq!(outcome.queued_sales, 1);
        assert_eq!(outcome.accepted as usize, 1 + catalog_rows);
        assert_eq!(outcome.remaining, 0);
        assert_eq!(outbox.pending_count(), 0);

        let body = http.bodies.borrow()[0].clone();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["sales"][0]["id"], "s1");
        let product_ids: Vec<&str> = parsed["products"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert!(product_ids.contains(&"p1"));
        let customer_ids: Vec<&str> = parsed["customers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(customer_ids.contains(&"c1"));
        assert!(outbox.meta_get("last_synced_at").is_some());

        // A second drain re-pushes nothing new: the catalog rows are done,
        // the watermark covers the sale.
        let http2 = FakeHttp::ok(0, 0, vec![]);
        let outcome = drain(&ledger, &outbox, &http2, false).unwrap();
        assert_eq!(outcome.queued_sales, 0);
        assert_eq!(outcome.accepted, 0);
    }

    #[test]
    fn enrolled_kiosk_drains_with_the_device_token() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = repo(dir.path().join("ledger.db").to_str().unwrap());
        ledger.insert_sale(&sample_sale("s1", 1_000), &Default::default(), 0).unwrap();
        let product = crate::repos::Product {
            id: "p1".into(),
            sku: "KOPI".into(),
            name: "Kopi".into(),
            category: "Drinks".into(),
            price_minor: 5_000,
            cost_minor: 2_000,
            tax_rate_id: None,
            track_stock: true,
            low_stock_at: 3,
            archived: false,
            description: String::new(),
            prep_notes: String::new(),
            allergens: String::new(),
            route_kitchen: false,
            image_path: String::new(),
        };
        ledger.upsert_product(&product).unwrap();
        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();

        // Without enrollment the store token keeps working.
        outbox.meta_set("store_token", "store-tok").unwrap();
        let catalog_rows = ledger.customers(None).unwrap().len() as u64 + 1;
        let http = FakeHttp::ok(catalog_rows, 0, vec![]);
        drain(&ledger, &outbox, &http, false).unwrap();
        assert!(http.tokens.borrow().iter().all(|t| t == "store-tok"));

        // Enrollment swaps the credential on the same endpoint: the next
        // drain (forced here, so the backoff window never interferes)
        // presents the device token for the re-activated catalog edit.
        outbox.meta_set("device_token", "device-tok").unwrap();
        let mut edited = product.clone();
        edited.price_minor = 6_000;
        ledger.upsert_product(&edited).unwrap();
        let http2 = FakeHttp::ok(1, 0, vec![]);
        drain(&ledger, &outbox, &http2, true).unwrap();
        assert!(http2.tokens.borrow().iter().all(|t| t == "device-tok"));
    }

    #[test]
    fn transport_failure_leaves_rows_queued_and_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = repo(dir.path().join("ledger.db").to_str().unwrap());
        ledger.insert_sale(&sample_sale("s1", 1_000), &Default::default(), 0).unwrap();
        let catalog_rows =
            ledger.products(true, None).unwrap().len() + ledger.customers(None).unwrap().len();
        let all_rows = 1 + catalog_rows;
        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();
        outbox.meta_set("store_token", "tok").unwrap();

        let http = FakeHttp::failing("connection refused");
        let outcome = drain(&ledger, &outbox, &http, false).unwrap();
        assert_eq!(outcome.http_error.as_deref(), Some("connection refused"));
        assert_eq!(outbox.pending_count(), all_rows as u64);
        assert!(outbox.meta_get("last_synced_at").is_none());
        assert!(outbox.meta_get("next_attempt_at").is_some());
        assert_eq!(outbox.drain_attempts(), 1);

        // Inside the backoff window an automatic drain skips, a forced one
        // goes through.
        let outcome = drain(&ledger, &outbox, &FakeHttp::ok(all_rows as u64, 0, vec![]), false)
            .unwrap();
        assert!(outcome.backed_off);
        assert_eq!(outbox.pending_count(), all_rows as u64);
        let outcome =
            drain(&ledger, &outbox, &FakeHttp::ok(all_rows as u64, 0, vec![]), true).unwrap();
        assert!(!outcome.backed_off);
        assert_eq!(outcome.accepted as usize, all_rows);
        assert_eq!(outbox.pending_count(), 0);
        assert!(outbox.meta_get("next_attempt_at").is_none());
    }

    #[test]
    fn replays_dedupe_and_row_errors_stay_queued() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = repo(dir.path().join("ledger.db").to_str().unwrap());
        ledger.insert_sale(&sample_sale("s1", 1_000), &Default::default(), 0).unwrap();
        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();
        outbox.meta_set("store_token", "tok").unwrap();

        // Same sale enqueued twice (crash between ack and watermark) is one row.
        outbox.enqueue_sale(&sample_sale("s1", 1_000)).unwrap();
        outbox.enqueue_sale(&sample_sale("s1", 1_000)).unwrap();
        assert_eq!(outbox.pending_count(), 1);

        let http = FakeHttp::ok(
            0,
            0,
            vec![RowError {
                ref_: "s1".into(),
                message: "NOT NULL constraint failed".into(),
            }],
        );
        let outcome = drain(&ledger, &outbox, &http, false).unwrap();
        assert_eq!(outcome.row_errors.len(), 1);
        assert_eq!(outbox.pending_count(), 1);
        assert_eq!(outbox.meta_get("last_error").unwrap(), "s1: NOT NULL constraint failed");

        // Next drain retries just the failed row and clears the error.
        let outcome = drain(&ledger, &outbox, &FakeHttp::ok(1, 0, vec![]), false).unwrap();
        assert_eq!(outcome.accepted, 1);
        assert_eq!(outbox.pending_count(), 0);
        assert!(outbox.meta_get("last_error").is_none());
    }

    #[test]
    fn catalog_edits_reactivate_done_rows() {
        let dir = tempfile::tempdir().unwrap();
        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        let product = crate::repos::Product {
            id: "p1".into(),
            sku: "KOPI".into(),
            name: "Kopi".into(),
            category: "Drinks".into(),
            price_minor: 5_000,
            cost_minor: 2_000,
            tax_rate_id: None,
            track_stock: true,
            low_stock_at: 3,
            archived: false,
            description: String::new(),
            prep_notes: String::new(),
            allergens: String::new(),
            route_kitchen: false,
            image_path: String::new(),
        };
        outbox.enqueue_product(&product).unwrap();
        // Mark it sent.
        outbox.mark_done(&[1]).unwrap();
        assert_eq!(outbox.pending_count(), 0);

        // Unchanged re-enqueue stays done.
        outbox.enqueue_product(&product).unwrap();
        assert_eq!(outbox.pending_count(), 0);

        // A price edit re-queues with the new payload.
        let mut edited = product.clone();
        edited.price_minor = 6_000;
        outbox.enqueue_product(&edited).unwrap();
        assert_eq!(outbox.pending_count(), 1);
    }

    #[test]
    fn prune_drops_done_rows_past_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let outbox = Outbox::open(dir.path().join("outbox.db").to_str().unwrap()).unwrap();
        outbox
            .conn
            .execute(
                "INSERT INTO outbox (kind, ref, payload, created_at, done) VALUES
                 ('sale', 'old', '{}', '2026-01-01T00:00:00.000Z', 1),
                 ('sale', 'fresh', '{}', '2026-09-27T00:00:00.000Z', 1),
                 ('sale', 'pending', '{}', '2026-01-01T00:00:00.000Z', 0)",
                [],
            )
            .unwrap();
        outbox.prune_done().unwrap();
        let remaining: i64 = outbox
            .conn
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 2);
    }

    #[test]
    fn backoff_grows_to_the_sixty_second_ceiling() {
        assert_eq!(backoff_secs(1), 1);
        assert_eq!(backoff_secs(2), 2);
        assert_eq!(backoff_secs(3), 4);
        assert_eq!(backoff_secs(6), 32);
        assert_eq!(backoff_secs(7), 60);
        assert_eq!(backoff_secs(50), 60);
    }

    #[test]
    fn unreadable_ack_is_a_protocol_error() {
        assert!(parse_batch_ack("<html>502</html>").is_err());
        let ack = parse_batch_ack(
            r#"{"accepted":2,"duplicates":1,"errors":[{"ref":"s9","message":"bad"}]}"#,
        )
        .unwrap();
        assert_eq!(ack.accepted, 2);
        assert_eq!(ack.duplicates, 1);
        assert_eq!(ack.errors[0].ref_, "s9");
    }
}
