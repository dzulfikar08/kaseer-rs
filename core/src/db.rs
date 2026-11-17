//! SQLite persistence. rusqlite (bundled), WAL, synchronous (a local
//! register has no reason to pretend I/O is async). Migrations are
//! numbered via PRAGMA user_version; every step is transactional.

use std::env;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::repos::Repo;
use crate::seed_data::{
    DEFAULT_ADDRESS, DEFAULT_CURRENCY_CODE, DEFAULT_LOYALTY, DEFAULT_RECEIPT_FOOTER,
    DEFAULT_STORE_NAME, DEFAULT_TAX_RATES, SEED_CATEGORIES, SEED_PRODUCTS,
};

/// Where the ledger, outbox, photos and training sandbox live:
/// `KASEER_DATA_DIR`; else `./data` when a ledger already exists there
/// (terminals installed before 0.3 and `cargo run` from the repo); else
/// the per-user data folder (`~/.local/share/kaseer`,
/// `~/Library/Application Support/Kaseer`, `%APPDATA%\Kaseer`).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = env::var_os("KASEER_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let legacy = Path::new("./data");
    if legacy.join("kaseer.db").exists() {
        return legacy.to_path_buf();
    }
    user_data_dir().unwrap_or_else(|| legacy.to_path_buf())
}

fn user_data_dir() -> Option<PathBuf> {
    let var = |name: &str| env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").map(|dir| dir.join("Kaseer"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Application Support/Kaseer"))
    } else {
        var("XDG_DATA_HOME")
            .or_else(|| var("HOME").map(|home| home.join(".local/share")))
            .map(|dir| dir.join("kaseer"))
    }
}

pub fn resolve_db_path() -> String {
    env::var("KASEER_DB")
        .unwrap_or_else(|_| data_dir().join("kaseer.db").to_string_lossy().into_owned())
}

pub fn open_db(path: &str) -> rusqlite::Result<Connection> {
    if path != ":memory:" {
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let conn = Connection::open(path)?;
    // The ledger serves the register, the drain's polling read, the menu
    // apply, and the update loop's shift check concurrently: brief locks
    // must wait, not error.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    crate::sync::ensure_wal(&conn)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    // Migrate is user_version-staged, hence idempotent; a BUSY from a
    // concurrent opener is resolved by a short retry, not a failed open.
    let mut attempt = 0;
    loop {
        match migrate(&conn) {
            Ok(()) => break,
            Err(error) if attempt < 25 && crate::sync::is_busy_error(&error) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(error) => return Err(error),
        }
    }
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
    if version < 3 {
        let tx = conn.unchecked_transaction()?;
        // Phase 3 (web companion): product knowledge for the register
        // popover and inventory editor, plus kitchen tickets derived when
        // a sale settles with kitchen-routed lines. Kept verbatim against
        // kaseer-web's migrate(): both apps open the same ledger file.
        conn.execute_batch(
            "ALTER TABLE products ADD COLUMN description TEXT NOT NULL DEFAULT '';
             ALTER TABLE products ADD COLUMN prep_notes TEXT NOT NULL DEFAULT '';
             ALTER TABLE products ADD COLUMN allergens TEXT NOT NULL DEFAULT '';
             ALTER TABLE products ADD COLUMN route_kitchen INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE kitchen_tickets (
               id TEXT PRIMARY KEY,
               sale_id TEXT NOT NULL REFERENCES sales(id),
               sale_number TEXT NOT NULL,
               created_at TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'queued'
             );
             CREATE INDEX idx_kitchen_tickets_created ON kitchen_tickets(created_at);
             CREATE TABLE kitchen_ticket_items (
               id TEXT PRIMARY KEY,
               ticket_id TEXT NOT NULL REFERENCES kitchen_tickets(id),
               product_id TEXT NOT NULL,
               name TEXT NOT NULL,
               qty REAL NOT NULL,
               state TEXT NOT NULL DEFAULT 'queued',
               bumped INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX idx_kitchen_items_ticket ON kitchen_ticket_items(ticket_id);",
        )?;
        seed_knowledge(conn)?;
        conn.execute_batch("PRAGMA user_version = 3;")?;
        tx.commit()?;
    }
    if version < 4 {
        let tx = conn.unchecked_transaction()?;
        // Phase 4 (menu pull): register card photos. The ALTER is ported
        // verbatim from kaseer-web's v4: both apps open the same ledger
        // file. The web's step also backfills stock photos from its own
        // data dir; the terminal skips that on purpose: photos arrive over
        // the wire via the menu pull, which downloads the companion's
        // image files into the ledger's images/ directory.
        conn.execute_batch("ALTER TABLE products ADD COLUMN image_path TEXT NOT NULL DEFAULT '';")?;
        conn.execute_batch("PRAGMA user_version = 4;")?;
        tx.commit()?;
    }
    Ok(())
}

/// Demo knowledge for prepared seed products, matched by stable SKU.
/// Row-for-row parity with kaseer-web's `seedKnowledge`: whichever app
/// seeds first, both read the same copy.
fn seed_knowledge(conn: &Connection) -> rusqlite::Result<()> {
    const KNOWLEDGE_SEED: [(&str, &str, &str, &str, i64); 7] = [
        (
            "8991002100011",
            "Espresso with palm-sugar syrup and fresh milk over ice.",
            "Pull 2 shots over ice, add palm syrup, top with milk, stir twice.",
            "milk",
            1,
        ),
        (
            "8991002100028",
            "Double shot of espresso lengthened with hot water. Served black.",
            "Pull 2 shots, add hot water to the 8oz line. Serve immediately.",
            "none",
            1,
        ),
        (
            "8991002101018",
            "Fried rice with egg, chicken, and prawn crackers.",
            "Egg first in the wok, then rice on high heat for 3 min.",
            "egg, soy, crustacean (krupuk)",
            1,
        ),
        (
            "8991002101025",
            "Stir-fried noodles with vegetables and sweet soy.",
            "Wok on high heat 3 min; keep noodles al dente.",
            "gluten, egg, soy",
            1,
        ),
        (
            "8991002101032",
            "Smashed fried chicken thigh over rice with sambal.",
            "Fry thigh 6 min, smash with sambal, plate over warm rice.",
            "gluten, poultry",
            1,
        ),
        (
            "8991002101049",
            "Toasted bread with chocolate sprinkles and condensed milk.",
            "Toast 2 min per side; add chocolate while hot so it melts.",
            "gluten, milk, soy",
            1,
        ),
        (
            "8991002102017",
            "Hand-cut fries, salted.",
            "Fry 3.5 min at 175C, drain, salt, serve hot.",
            "none",
            1,
        ),
    ];
    for (sku, description, prep, allergens, routed) in KNOWLEDGE_SEED {
        conn.execute(
            "UPDATE products SET description = ?, prep_notes = ?, allergens = ?, route_kitchen = ? WHERE sku = ?",
            rusqlite::params![description, prep, allergens, routed, sku],
        )?;
    }
    Ok(())
}

/// Copy `source` into a fresh `target` file. The training sandbox starts
/// from a copy of the live ledger; `VACUUM INTO` reads through WAL and
/// writes a consistent snapshot, so the copy stays valid even while the
/// register holds the source open. `target` must not exist: callers
/// remove stale copies (and their `-wal`/`-shm` sidecars) first.
pub fn snapshot_db(source: &str, target: &str) -> rusqlite::Result<()> {
    let src = Connection::open(source)?;
    src.execute("VACUUM INTO ?1", [target])?;
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
