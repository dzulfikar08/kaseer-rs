//! Repository layer over rusqlite. Synchronous by design: a local register
//! never has a reason to pretend I/O is async. All money crosses in minor
//! units. `insert_sale` is the write-through point that also books stock
//! movements and loyalty points in one transaction.

use std::collections::HashMap;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::cart::AddableProduct;
use crate::inventory::StockReason;
use crate::loyalty::LoyaltyConfig;
use crate::payments::TenderMethod;
use crate::receipt::{Sale, SaleLine, SaleTender};
use crate::shift::{SettledSale, Shift, TenderLine};
use crate::tax::TaxRate;

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("a shift is already open")]
    ShiftAlreadyOpen,
    #[error("unknown tender method: {0}")]
    BadTenderMethod(String),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = RepoError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreConfig {
    /// Customer is 'lost' after this many days without a purchase (Phase 2 definition).
    /// The companion stores this file camelCase (`lostAfterDays`, plus its
    /// own `menuRev` — ignored here); the alias keeps old kiosk-written
    /// snake_case ledgers readable.
    #[serde(default = "default_lost_after_days", alias = "lost_after_days")]
    pub lost_after_days: i64,
}

fn default_lost_after_days() -> i64 {
    30
}

impl Default for StoreConfig {
    fn default() -> Self {
        StoreConfig { lost_after_days: 30 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub store_name: String,
    pub address: String,
    pub receipt_footer: String,
    pub currency_code: String,
    pub loyalty: LoyaltyConfig,
    pub config: StoreConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: String,
    pub sku: String,
    pub name: String,
    pub category: String,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub tax_rate_id: Option<String>,
    pub track_stock: bool,
    pub low_stock_at: i64,
    pub archived: bool,
    /// Knowledge slice (schema v3, shared with the web companion).
    pub description: String,
    pub prep_notes: String,
    pub allergens: String,
    pub route_kitchen: bool,
    /// Repo-relative photo path (`images/<file>`), written by the menu
    /// pull; the file itself lands in the ledger's images/ directory
    /// (schema v4, shared with the web companion).
    pub image_path: String,
}

/// Editable slice of [`Product`] knowledge; `None` keys keep their value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgePatch {
    pub description: Option<String>,
    pub prep_notes: Option<String>,
    pub allergens: Option<String>,
    pub route_kitchen: Option<bool>,
}

impl Product {
    /// The cart-facing slice of a product.
    pub fn addable(&self) -> AddableProduct {
        AddableProduct {
            id: self.id.clone(),
            name: self.name.clone(),
            sku: self.sku.clone(),
            price_minor: self.price_minor,
            tax_rate_id: self.tax_rate_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    pub id: String,
    pub name: String,
    pub phone: String,
    pub points: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTicket {
    pub id: String,
    pub label: String,
    pub cart_json: String,
    pub held_at: String,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub struct Repo {
    db_path: String,
    conn: Connection,
}

impl Repo {
    pub fn new(db_path: String, conn: Connection) -> Repo {
        Repo { db_path, conn }
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ── Settings ──────────────────────────────────────────────────────────

    pub fn settings(&self) -> Result<Settings> {
        let row = self
            .conn
            .query_row(
                "SELECT store_name, address, receipt_footer, currency_code, loyalty_json, config_json FROM settings WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        let (store_name, address, receipt_footer, currency_code, loyalty_json, config_json) =
            row.ok_or_else(|| RepoError::NotFound("settings row missing; migrate() must run first".to_string()))?;
        let loyalty: LoyaltyConfig = serde_json::from_str(&loyalty_json)?;
        let config: StoreConfig = serde_json::from_str(&config_json).unwrap_or_default();
        Ok(Settings { store_name, address, receipt_footer, currency_code, loyalty, config })
    }

    pub fn update_settings(&self, next: &Settings) -> Result<()> {
        self.conn.execute(
            "UPDATE settings SET store_name = ?, address = ?, receipt_footer = ?, currency_code = ?, loyalty_json = ?, config_json = ? WHERE id = 1",
            params![
                next.store_name,
                next.address,
                next.receipt_footer,
                next.currency_code,
                serde_json::to_string(&next.loyalty)?,
                serde_json::to_string(&next.config)?
            ],
        )?;
        Ok(())
    }

    pub fn tax_rates(&self) -> Result<Vec<TaxRate>> {
        let mut stmt = self.conn.prepare("SELECT id, name, bps, inclusive FROM tax_rates ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(TaxRate {
                id: r.get(0)?,
                name: r.get(1)?,
                bps: r.get(2)?,
                inclusive: r.get::<_, i64>(3)? == 1,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn upsert_tax_rate(&self, rate: &TaxRate) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tax_rates (id, name, bps, inclusive) VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, bps = excluded.bps, inclusive = excluded.inclusive",
            params![rate.id, rate.name, rate.bps, rate.inclusive as i64],
        )?;
        Ok(())
    }

    // ── Catalog ───────────────────────────────────────────────────────────

    const PRODUCT_COLS: &'static str = "id, sku, name, category, price_minor, cost_minor, tax_rate_id, track_stock, low_stock_at, archived, description, prep_notes, allergens, route_kitchen, image_path";

    pub fn products(&self, include_archived: bool, query: Option<&str>) -> Result<Vec<Product>> {
        let query = query.map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
        let sql = format!("SELECT {} FROM products ORDER BY category, name", Self::PRODUCT_COLS);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_product)?;
        let mapped = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(mapped
            .into_iter()
            .filter(|p| {
                if !include_archived && p.archived {
                    return false;
                }
                match &query {
                    None => true,
                    Some(q) => format!("{} {}", p.name, p.sku).to_lowercase().contains(q),
                }
            })
            .collect())
    }

    pub fn product_by_sku(&self, sku: &str) -> Result<Option<Product>> {
        let sql = format!("SELECT {} FROM products WHERE sku = ?1", Self::PRODUCT_COLS);
        Ok(self.conn.query_row(&sql, [sku], row_to_product).optional()?)
    }

    pub fn product_by_id(&self, id: &str) -> Result<Option<Product>> {
        let sql = format!("SELECT {} FROM products WHERE id = ?1", Self::PRODUCT_COLS);
        Ok(self.conn.query_row(&sql, [id], row_to_product).optional()?)
    }

    pub fn upsert_product(&self, p: &Product) -> Result<Product> {
        self.conn.execute(
            "INSERT INTO products (id, sku, name, category, price_minor, cost_minor, tax_rate_id, track_stock, low_stock_at, archived,
               description, prep_notes, allergens, route_kitchen, image_path)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET sku = excluded.sku, name = excluded.name, category = excluded.category,
               price_minor = excluded.price_minor, cost_minor = excluded.cost_minor, tax_rate_id = excluded.tax_rate_id,
               track_stock = excluded.track_stock, low_stock_at = excluded.low_stock_at, archived = excluded.archived,
               description = excluded.description, prep_notes = excluded.prep_notes, allergens = excluded.allergens,
               route_kitchen = excluded.route_kitchen, image_path = excluded.image_path",
            params![p.id, p.sku, p.name, p.category, p.price_minor, p.cost_minor, p.tax_rate_id, p.track_stock as i64, p.low_stock_at, p.archived as i64, p.description, p.prep_notes, p.allergens, p.route_kitchen as i64, p.image_path],
        )?;
        Ok(p.clone())
    }

    /// Patch the knowledge slice of one product; unknown ids raise.
    pub fn update_knowledge(&self, product_id: &str, patch: &KnowledgePatch) -> Result<Product> {
        let Some(current) = self.product_by_id(product_id)? else {
            return Err(RepoError::NotFound(format!("product {product_id} not found")));
        };
        let next = KnowledgePatch {
            description: patch.description.clone().or_else(|| Some(current.description.clone())),
            prep_notes: patch.prep_notes.clone().or_else(|| Some(current.prep_notes.clone())),
            allergens: patch.allergens.clone().or_else(|| Some(current.allergens.clone())),
            route_kitchen: patch.route_kitchen.or(Some(current.route_kitchen)),
        };
        self.conn.execute(
            "UPDATE products SET description = ?, prep_notes = ?, allergens = ?, route_kitchen = ? WHERE id = ?",
            params![
                next.description.clone().unwrap_or_default(),
                next.prep_notes.clone().unwrap_or_default(),
                next.allergens.clone().unwrap_or_default(),
                next.route_kitchen.unwrap_or(false) as i64,
                product_id
            ],
        )?;
        Ok(Product {
            description: next.description.unwrap_or_default(),
            prep_notes: next.prep_notes.unwrap_or_default(),
            allergens: next.allergens.unwrap_or_default(),
            route_kitchen: next.route_kitchen.unwrap_or(false),
            ..current
        })
    }

    pub fn categories(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT name FROM categories ORDER BY sort, name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_category(&self, name: &str) -> Result<()> {
        let next: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(sort), -1) + 1 AS next FROM categories", [], |r| r.get(0))?;
        self.conn
            .execute("INSERT OR IGNORE INTO categories (name, sort) VALUES (?, ?)", params![name, next])?;
        Ok(())
    }

    // ── Customers ─────────────────────────────────────────────────────────

    pub fn customers(&self, query: Option<&str>) -> Result<Vec<Customer>> {
        let mut stmt = self.conn.prepare("SELECT id, name, phone, points, created_at FROM customers ORDER BY name")?;
        let rows = stmt.query_map([], row_to_customer)?;
        let mapped = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let q = query.map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
        Ok(match q {
            None => mapped,
            Some(q) => mapped
                .into_iter()
                .filter(|c| format!("{} {}", c.name, c.phone).to_lowercase().contains(&q))
                .collect(),
        })
    }

    pub fn customer_by_id(&self, id: &str) -> Result<Option<Customer>> {
        Ok(self
            .conn
            .query_row("SELECT id, name, phone, points, created_at FROM customers WHERE id = ?1", [id], row_to_customer)
            .optional()?)
    }

    pub fn upsert_customer(&self, c: &Customer) -> Result<Customer> {
        self.conn.execute(
            "INSERT INTO customers (id, name, phone, points, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, phone = excluded.phone, points = excluded.points",
            params![c.id, c.name, c.phone, c.points, c.created_at],
        )?;
        Ok(c.clone())
    }

    pub fn adjust_customer_points(&self, id: &str, delta: i64) -> Result<()> {
        self.conn
            .execute("UPDATE customers SET points = points + ? WHERE id = ?", params![delta, id])?;
        Ok(())
    }

    // ── Shifts ────────────────────────────────────────────────────────────

    pub fn active_shift(&self) -> Result<Option<Shift>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, opened_at, opening_float_minor, currency, closed_at, counted_minor FROM shifts WHERE closed_at IS NULL ORDER BY opened_at DESC LIMIT 1",
                [],
                row_to_shift,
            )
            .optional()?)
    }

    pub fn open_shift(&self, opening_float_minor: i64, currency: &str) -> Result<Shift> {
        if self.active_shift()?.is_some() {
            return Err(RepoError::ShiftAlreadyOpen);
        }
        let shift = Shift {
            id: Uuid::new_v4().to_string(),
            opened_at: now_iso(),
            opening_float_minor,
            currency: currency.to_string(),
            closed_at: None,
            counted_minor: None,
        };
        self.conn.execute(
            "INSERT INTO shifts (id, opened_at, opening_float_minor, currency) VALUES (?, ?, ?, ?)",
            params![shift.id, shift.opened_at, shift.opening_float_minor, shift.currency],
        )?;
        Ok(shift)
    }

    pub fn close_shift(&self, id: &str, counted_minor: i64) -> Result<Shift> {
        let closed_at = now_iso();
        self.conn.execute(
            "UPDATE shifts SET closed_at = ?, counted_minor = ? WHERE id = ?",
            params![closed_at, counted_minor, id],
        )?;
        Ok(self.conn.query_row(
            "SELECT id, opened_at, opening_float_minor, currency, closed_at, counted_minor FROM shifts WHERE id = ?1",
            [id],
            row_to_shift,
        )?)
    }

    pub fn shift_by_id(&self, id: &str) -> Result<Option<Shift>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, opened_at, opening_float_minor, currency, closed_at, counted_minor FROM shifts WHERE id = ?1",
                [id],
                row_to_shift,
            )
            .optional()?)
    }

    /// Settled sales belonging to a shift, reduced for drawer/report math.
    /// The sale's change lands on its first cash tender (drawer math in
    /// cash_expected_minor expects exactly that).
    pub fn shift_sales(&self, shift_id: &str) -> Result<Vec<SettledSale>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, gross_minor, discount_minor, tax_minor, total_minor, change_minor FROM sales WHERE shift_id = ? ORDER BY at",
        )?;
        let rows = stmt.query_map([shift_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, gross_minor, discount_minor, tax_minor, total_minor, change_minor) = row?;
            let mut change_assigned = false;
            let tenders: Vec<TenderLine> = self
                .tenders_for(&id)?
                .into_iter()
                .map(|(method, applied_minor, _given)| {
                    if method == TenderMethod::Cash && !change_assigned {
                        change_assigned = true;
                        TenderLine { method, applied_minor, change_minor }
                    } else {
                        TenderLine { method, applied_minor, change_minor: 0 }
                    }
                })
                .collect();
            out.push(SettledSale { gross_minor, discount_minor, tax_minor, total_minor, tenders });
        }
        Ok(out)
    }

    // ── Stock ─────────────────────────────────────────────────────────────

    pub fn stock_on_hand(&self, product_id: &str) -> Result<i64> {
        let on_hand: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(delta), 0) AS on_hand FROM stock_moves WHERE product_id = ?",
            [product_id],
            |r| r.get(0),
        )?;
        Ok(on_hand)
    }

    pub fn adjust_stock(
        &self,
        product_id: &str,
        delta: i64,
        reason: StockReason,
        note: Option<&str>,
        ref_id: Option<&str>,
    ) -> Result<()> {
        insert_stock_move(&self.conn, product_id, delta, reason, ref_id, &now_iso(), note)
    }

    // ── Sales ─────────────────────────────────────────────────────────────

    fn tenders_for(&self, sale_id: &str) -> Result<Vec<(TenderMethod, i64, Option<i64>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT method, applied_minor, given_minor FROM sale_tenders WHERE sale_id = ? ORDER BY id")?;
        let rows = stmt.query_map([sale_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (method, applied, given) = row?;
            let method =
                TenderMethod::parse(&method).ok_or_else(|| RepoError::BadTenderMethod(method.clone()))?;
            Ok((method, applied, given))
        })
        .collect()
    }

    /// Rehydrated tenders for a stored sale.
    fn sale_tenders(&self, sale_id: &str) -> Result<Vec<SaleTender>> {
        Ok(self
            .tenders_for(sale_id)?
            .into_iter()
            .map(|(method, applied_minor, given_minor)| SaleTender { method, applied_minor, given_minor })
            .collect())
    }

    /// Sale lines for a stored sale. `with_cost` controls whether the
    /// captured cost surfaces (fresh reads) or stays omitted (legacy shape).
    fn sale_lines(&self, sale_id: &str, with_cost: bool) -> Result<Vec<SaleLine>> {
        let mut stmt = self.conn.prepare(
            "SELECT product_id, name, sku, qty, unit_minor, discount_minor, order_discount_minor, net_minor, tax_minor, tax_rate_id, cost_minor
             FROM sale_lines WHERE sale_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([sale_id], |r| {
            let cost_minor: i64 = r.get(10)?;
            Ok(SaleLine {
                product_id: r.get(0)?,
                name: r.get(1)?,
                sku: r.get(2)?,
                qty: r.get(3)?,
                unit_minor: r.get(4)?,
                cost_minor: if with_cost { Some(cost_minor) } else { None },
                discount_minor: r.get::<_, i64>(5)? + r.get::<_, i64>(6)?,
                net_minor: r.get(7)?,
                tax_minor: r.get(8)?,
                tax_rate_id: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn sale_fields(row: &rusqlite::Row) -> rusqlite::Result<SaleFields> {
        Ok(SaleFields {
            number: row.get("number")?,
            at: row.get("at")?,
            currency: row.get("currency")?,
            shift_id: row.get("shift_id")?,
            customer_id: row.get("customer_id")?,
            customer_name_snapshot: row.get("customer_name_snapshot")?,
            gross_minor: row.get("gross_minor")?,
            discount_minor: row.get("discount_minor")?,
            net_minor: row.get("net_minor")?,
            tax_minor: row.get("tax_minor")?,
            total_minor: row.get("total_minor")?,
            change_minor: row.get("change_minor")?,
            points_earned: row.get("points_earned")?,
            points_redeemed: row.get("points_redeemed")?,
            tax_by_rate_json: row.get("tax_by_rate_json")?,
        })
    }

    pub fn next_sale_number(&self) -> Result<(String, i64)> {
        let day = Utc::now().format("%Y%m%d").to_string();
        let pattern = format!("KSR-{}-%", day);
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) AS n FROM sales WHERE number LIKE ?",
            [pattern],
            |r| r.get(0),
        )?;
        let seq = n + 1;
        Ok((format!("KSR-{}-{:04}", day, seq), seq))
    }

    /// Persist a settled sale atomically: the sale, its lines and tenders,
    /// stock movements for tracked products, and loyalty point changes.
    pub fn insert_sale(
        &self,
        sale: &Sale,
        tracked_qty_by_product_id: &HashMap<String, f64>,
        points_delta: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO sales (id, number, at, currency, shift_id, customer_id, customer_name_snapshot,
               gross_minor, discount_minor, net_minor, tax_minor, total_minor, change_minor,
               points_earned, points_redeemed, tax_by_rate_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                sale.id,
                sale.number,
                sale.at,
                sale.currency,
                sale.shift_id,
                sale.customer_id,
                sale.customer_name_snapshot,
                sale.gross_minor,
                sale.discount_minor,
                sale.net_minor,
                sale.tax_minor,
                sale.total_minor,
                sale.change_minor,
                sale.points_earned,
                sale.points_redeemed,
                serde_json::to_string(&sale.tax_by_rate_id)?
            ],
        )?;

        for line in &sale.lines {
            let cost: Option<i64> = tx
                .query_row("SELECT cost_minor FROM products WHERE id = ?1", [&line.product_id], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO sale_lines (id, sale_id, product_id, name, sku, qty, unit_minor, cost_minor, discount_minor, order_discount_minor, net_minor, tax_minor, tax_rate_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    Uuid::new_v4().to_string(),
                    sale.id,
                    line.product_id,
                    line.name,
                    line.sku,
                    line.qty,
                    line.unit_minor,
                    cost.unwrap_or(0),
                    line.discount_minor,
                    0,
                    line.net_minor,
                    line.tax_minor,
                    line.tax_rate_id
                ],
            )?;
        }

        for t in &sale.tenders {
            tx.execute(
                "INSERT INTO sale_tenders (sale_id, method, applied_minor, given_minor) VALUES (?, ?, ?, ?)",
                params![sale.id, t.method.as_str(), t.applied_minor, t.given_minor],
            )?;
        }

        for (product_id, qty) in tracked_qty_by_product_id {
            let whole = qty.round() as i64;
            if whole > 0 {
                insert_stock_move(
                    &tx,
                    product_id,
                    -whole,
                    StockReason::Sale,
                    Some(&sale.id),
                    &now_iso(),
                    Some(&sale.number),
                )?;
            }
        }

        if let Some(customer_id) = &sale.customer_id {
            if points_delta != 0 {
                tx.execute(
                    "UPDATE customers SET points = points + ? WHERE id = ?",
                    params![points_delta, customer_id],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// All settled sales for one customer, oldest first.
    pub fn sales_by_customer(&self, customer_id: &str) -> Result<Vec<Sale>> {
        let mut stmt = self.conn.prepare("SELECT id FROM sales WHERE customer_id = ? ORDER BY at")?;
        let ids = stmt
            .query_map([customer_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids
            .into_iter()
            .filter_map(|id| self.sale_by_id(&id).ok().flatten())
            .collect())
    }

    pub fn sale_by_id(&self, id: &str) -> Result<Option<Sale>> {
        let row = self
            .conn
            .query_row("SELECT * FROM sales WHERE id = ?1", [id], Self::sale_fields)
            .optional()?;
        let Some(f) = row else { return Ok(None) };
        Ok(Some(f.into_sale(id, self.sale_lines(id, false)?, self.sale_tenders(id)?)))
    }

    /// Full sales (lines + tenders rehydrated) in an ISO range, inclusive.
    pub fn sales_between(&self, from_iso: &str, to_iso: &str) -> Result<Vec<Sale>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sales WHERE at >= ? AND at <= ? ORDER BY at")?;
        let ids = stmt
            .query_map(params![from_iso, to_iso], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = vec![];
        for id in ids {
            let f = self
                .conn
                .query_row("SELECT * FROM sales WHERE id = ?1", [&id], Self::sale_fields)?;
            out.push(f.into_sale(&id, self.sale_lines(&id, true)?, self.sale_tenders(&id)?));
        }
        Ok(out)
    }

    // ── Held tickets ──────────────────────────────────────────────────────

    pub fn hold_ticket(&self, label: &str, cart_json: &str) -> Result<HeldTicket> {
        let ticket = HeldTicket {
            id: Uuid::new_v4().to_string(),
            label: label.to_string(),
            cart_json: cart_json.to_string(),
            held_at: now_iso(),
        };
        // Table created lazily so v1 schema stays focused on settled sales.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS held_tickets (
               id TEXT PRIMARY KEY, label TEXT NOT NULL, cart_json TEXT NOT NULL, held_at TEXT NOT NULL
             );",
        )?;
        self.conn.execute(
            "INSERT INTO held_tickets (id, label, cart_json, held_at) VALUES (?, ?, ?, ?)",
            params![ticket.id, ticket.label, ticket.cart_json, ticket.held_at],
        )?;
        Ok(ticket)
    }

    pub fn held_tickets(&self) -> Result<Vec<HeldTicket>> {
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT id, label, cart_json, held_at FROM held_tickets ORDER BY held_at")
        else {
            // Table not created yet: an empty list, like the TS catch.
            return Ok(vec![]);
        };
        let rows = stmt.query_map([], |r| {
            Ok(HeldTicket {
                id: r.get(0)?,
                label: r.get(1)?,
                cart_json: r.get(2)?,
                held_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_held_ticket(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM held_tickets WHERE id = ?", [id])?;
        Ok(())
    }
}

// ── Row mappers ─────────────────────────────────────────────────────────

struct SaleFields {
    number: String,
    at: String,
    currency: String,
    shift_id: Option<String>,
    customer_id: Option<String>,
    customer_name_snapshot: Option<String>,
    gross_minor: i64,
    discount_minor: i64,
    net_minor: i64,
    tax_minor: i64,
    total_minor: i64,
    change_minor: i64,
    points_earned: i64,
    points_redeemed: i64,
    tax_by_rate_json: String,
}

impl SaleFields {
    fn into_sale(self, id: &str, lines: Vec<SaleLine>, tenders: Vec<SaleTender>) -> Sale {
        Sale {
            id: id.to_string(),
            number: self.number,
            at: self.at,
            currency: self.currency,
            shift_id: self.shift_id,
            customer_id: self.customer_id,
            customer_name_snapshot: self.customer_name_snapshot,
            lines,
            tenders,
            gross_minor: self.gross_minor,
            discount_minor: self.discount_minor,
            net_minor: self.net_minor,
            tax_minor: self.tax_minor,
            total_minor: self.total_minor,
            change_minor: self.change_minor,
            points_earned: self.points_earned,
            points_redeemed: self.points_redeemed,
            tax_by_rate_id: serde_json::from_str(&self.tax_by_rate_json).unwrap_or_default(),
        }
    }
}

fn insert_stock_move(
    conn: &Connection,
    product_id: &str,
    delta: i64,
    reason: StockReason,
    ref_id: Option<&str>,
    at: &str,
    note: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO stock_moves (id, product_id, delta, reason, ref_id, at, note) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![Uuid::new_v4().to_string(), product_id, delta, reason.as_str(), ref_id, at, note],
    )?;
    Ok(())
}

fn row_to_product(row: &rusqlite::Row) -> rusqlite::Result<Product> {
    Ok(Product {
        id: row.get(0)?,
        sku: row.get(1)?,
        name: row.get(2)?,
        category: row.get(3)?,
        price_minor: row.get(4)?,
        cost_minor: row.get(5)?,
        tax_rate_id: row.get(6)?,
        track_stock: row.get::<_, i64>(7)? == 1,
        low_stock_at: row.get(8)?,
        archived: row.get::<_, i64>(9)? == 1,
        description: row.get(10)?,
        prep_notes: row.get(11)?,
        allergens: row.get(12)?,
        route_kitchen: row.get::<_, i64>(13)? == 1,
        image_path: row.get(14)?,
    })
}

fn row_to_customer(row: &rusqlite::Row) -> rusqlite::Result<Customer> {
    Ok(Customer {
        id: row.get(0)?,
        name: row.get(1)?,
        phone: row.get(2)?,
        points: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn row_to_shift(row: &rusqlite::Row) -> rusqlite::Result<Shift> {
    Ok(Shift {
        id: row.get(0)?,
        opened_at: row.get(1)?,
        opening_float_minor: row.get(2)?,
        currency: row.get(3)?,
        closed_at: row.get(4)?,
        counted_minor: row.get(5)?,
    })
}
