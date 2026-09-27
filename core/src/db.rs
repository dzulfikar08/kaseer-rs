//! SQLite persistence. rusqlite (bundled), WAL, synchronous (a local
//! register has no reason to pretend I/O is async). Migrations are
//! numbered via PRAGMA user_version; every step is transactional.

use std::env;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::repos::Repo;
use crate::seed_data::{
    DEFAULT_ADDRESS, DEFAULT_CURRENCY_CODE, DEFAULT_LOYALTY, DEFAULT_RECEIPT_FOOTER,
    DEFAULT_STORE_NAME, DEFAULT_TAX_RATES, SEED_CATEGORIES, SEED_PRODUCTS,
};

pub const DEFAULT_DB_PATH: &str = "./data/kaseer.db";

pub fn resolve_db_path() -> String {
    env::var("KASEER_DB").unwrap_or_else(|_| DEFAULT_DB_PATH.to_string())
}

pub fn open_db(path: &str) -> rusqlite::Result<Connection> {
    if path != ":memory:" {
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// One-call bootstrap: open the database, migrate, seed, return the repo.
pub fn open_repo(path: &str) -> rusqlite::Result<Repo> {
    Ok(Repo::new(path.to_string(), open_db(path)?))
}

const SCHEMA_V1: &str = "
CREATE TABLE settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  store_name TEXT NOT NULL,
  address TEXT NOT NULL DEFAULT '',
  receipt_footer TEXT NOT NULL DEFAULT '',
  currency_code TEXT NOT NULL,
  loyalty_json TEXT NOT NULL
);

CREATE TABLE tax_rates (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  bps INTEGER NOT NULL,
  inclusive INTEGER NOT NULL
);

CREATE TABLE products (
  id TEXT PRIMARY KEY,
  sku TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  category TEXT NOT NULL DEFAULT '',
  price_minor INTEGER NOT NULL,
  cost_minor INTEGER NOT NULL DEFAULT 0,
  tax_rate_id TEXT REFERENCES tax_rates(id),
  track_stock INTEGER NOT NULL DEFAULT 1,
  low_stock_at INTEGER NOT NULL DEFAULT 0,
  archived INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE customers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  phone TEXT NOT NULL DEFAULT '',
  points INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE shifts (
  id TEXT PRIMARY KEY,
  opened_at TEXT NOT NULL,
  opening_float_minor INTEGER NOT NULL,
  currency TEXT NOT NULL,
  closed_at TEXT,
  counted_minor INTEGER
);

CREATE TABLE sales (
  id TEXT PRIMARY KEY,
  number TEXT NOT NULL UNIQUE,
  at TEXT NOT NULL,
  currency TEXT NOT NULL,
  shift_id TEXT REFERENCES shifts(id),
  customer_id TEXT REFERENCES customers(id),
  customer_name_snapshot TEXT,
  gross_minor INTEGER NOT NULL,
  discount_minor INTEGER NOT NULL,
  net_minor INTEGER NOT NULL,
  tax_minor INTEGER NOT NULL,
  total_minor INTEGER NOT NULL,
  change_minor INTEGER NOT NULL,
  points_earned INTEGER NOT NULL DEFAULT 0,
  points_redeemed INTEGER NOT NULL DEFAULT 0,
  tax_by_rate_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_sales_at ON sales(at);
CREATE INDEX idx_sales_shift ON sales(shift_id);

CREATE TABLE sale_lines (
  id TEXT PRIMARY KEY,
  sale_id TEXT NOT NULL REFERENCES sales(id),
  product_id TEXT NOT NULL,
  name TEXT NOT NULL,
  sku TEXT NOT NULL,
  qty REAL NOT NULL,
  unit_minor INTEGER NOT NULL,
  discount_minor INTEGER NOT NULL,
  order_discount_minor INTEGER NOT NULL DEFAULT 0,
  net_minor INTEGER NOT NULL,
  tax_minor INTEGER NOT NULL,
  tax_rate_id TEXT
);
CREATE INDEX idx_lines_sale ON sale_lines(sale_id);

CREATE TABLE sale_tenders (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  sale_id TEXT NOT NULL REFERENCES sales(id),
  method TEXT NOT NULL,
  applied_minor INTEGER NOT NULL,
  given_minor INTEGER
);

CREATE TABLE stock_moves (
  id TEXT PRIMARY KEY,
  product_id TEXT NOT NULL REFERENCES products(id),
  delta INTEGER NOT NULL,
  reason TEXT NOT NULL,
  ref_id TEXT,
  at TEXT NOT NULL,
  note TEXT
);
CREATE INDEX idx_moves_product ON stock_moves(product_id);

CREATE TABLE categories (
  name TEXT PRIMARY KEY,
  sort INTEGER NOT NULL DEFAULT 0
);
";

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        let tx = conn.unchecked_transaction()?;
        conn.execute_batch(SCHEMA_V1)?;
        seed_if_empty(conn)?;
        conn.execute_batch("PRAGMA user_version = 1;")?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = conn.unchecked_transaction()?;
        // Phase 2+: truthful margin needs the cost captured at sale time;
        // configurable analytics periods live in a settings config blob.
        conn.execute_batch(
            "ALTER TABLE sale_lines ADD COLUMN cost_minor INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE settings ADD COLUMN config_json TEXT NOT NULL DEFAULT '{}';",
        )?;
        conn.execute_batch("PRAGMA user_version = 2;")?;
        tx.commit()?;
    }
    Ok(())
}

fn seed_if_empty(conn: &Connection) -> rusqlite::Result<()> {
    let has_settings: Option<i64> = conn
        .query_row("SELECT 1 FROM settings", [], |r| r.get(0))
        .optional()?;
    if has_settings.is_some() {
        return Ok(());
    }

    conn.execute(
        "INSERT INTO settings (id, store_name, address, receipt_footer, currency_code, loyalty_json) VALUES (1, ?, ?, ?, ?, ?)",
        rusqlite::params![
            DEFAULT_STORE_NAME,
            DEFAULT_ADDRESS,
            DEFAULT_RECEIPT_FOOTER,
            DEFAULT_CURRENCY_CODE,
            serde_json::to_string(&DEFAULT_LOYALTY).expect("default loyalty serializes")
        ],
    )?;

    for rate in DEFAULT_TAX_RATES {
        conn.execute(
            "INSERT INTO tax_rates (id, name, bps, inclusive) VALUES (?, ?, ?, ?)",
            rusqlite::params![rate.id, rate.name, rate.bps, rate.inclusive as i64],
        )?;
    }

    for (i, name) in SEED_CATEGORIES.iter().enumerate() {
        conn.execute(
            "INSERT INTO categories (name, sort) VALUES (?, ?)",
            rusqlite::params![name, i as i64],
        )?;
    }

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for p in SEED_PRODUCTS {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO products (id, sku, name, category, price_minor, cost_minor, tax_rate_id, track_stock, low_stock_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                id,
                p.sku,
                p.name,
                p.category,
                p.price_minor,
                p.cost_minor,
                p.tax_rate_id,
                p.track_stock as i64,
                p.low_stock_at
            ],
        )?;
        if p.track_stock && p.stock > 0 {
            conn.execute(
                "INSERT INTO stock_moves (id, product_id, delta, reason, at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id, p.stock, "receive", now],
            )?;
        }
    }
    Ok(())
}
