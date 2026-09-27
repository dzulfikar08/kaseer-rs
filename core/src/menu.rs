//! Kiosk menu pull (schema v4): the web companion is the source of truth
//! for the register catalog. The kiosk polls `/api/menu/revision` and, when
//! the revision moves, re-pulls `/api/menu` and transactionally upserts
//! every product (price, knowledge, archive flag, photo path), the tax
//! rates, and the category list. Photo files themselves are downloaded
//! separately into the ledger's images/ directory — best-effort, one file
//! at a time; a failed download keeps the `image_path` but leaves no local
//! file, so the register card falls back to its text-only look and the
//! file is retried on the next pull.
//!
//! Knowledge convergence loop: when the kiosk's Knowledge screen saves,
//! the app fires a `PATCH /api/products/:id/knowledge` at the companion
//! (see `app/src/menu_sync.rs`). The PATCH bumps the companion's menu
//! revision, the next poll re-pulls, and both sides converge on the same
//! copy — the pull can never silently clobber a kiosk-side edit that
//! reached the companion.
//!
//! The wire shapes here mirror the companion's row mappers exactly
//! (`kaseer-web/src/data/repos.ts`): camelCase keys, `taxRateId` nullable,
//! products include archived rows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::Deserialize;

use crate::repos::Repo;

/// One `/api/menu` payload.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuPayload {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub tax_rates: Vec<MenuTaxRate>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub products: Vec<MenuProduct>,
}

/// Companion tax rate on the wire (`{"id","name","bps","inclusive"}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MenuTaxRate {
    pub id: String,
    pub name: String,
    pub bps: i64,
    #[serde(default)]
    pub inclusive: bool,
}

/// Companion product on the wire — the camelCase of `repos::Product`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuProduct {
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
    pub description: String,
    pub prep_notes: String,
    pub allergens: String,
    pub route_kitchen: bool,
    pub image_path: String,
}

/// What one apply changed, plus the photo files that need downloading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuApply {
    pub rev: u64,
    pub products: usize,
    /// Sanitized file names (the `images/<file>` basenames) to fetch.
    pub image_files: Vec<String>,
}

/// Parse and shape-check one `/api/menu` body. Unknown JSON keys are
/// ignored (forward-compatible with the frozen contract's additions).
pub fn parse_menu(bytes: &[u8]) -> Result<MenuPayload, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid menu payload: {e}"))
}

/// The `<file>` half of a repo-relative `images/<file>` path, rejected
/// unless it is the same flat `[A-Za-z0-9._-]+` name the companion
/// sanitizes uploads to — this string is joined onto the images dir, so
/// traversal-shaped values must never survive.
pub fn image_file_of(image_path: &str) -> Option<String> {
    let file = image_path.strip_prefix("images/")?;
    if file.is_empty()
        || file == "."
        || file == ".."
        || file.contains(['/', '\\'])
        || file.contains("..")
        || !file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(file.to_string())
}

/// Where photos live for a ledger: `<db dir>/images`, matching the web's
/// `imagesDirFor` (`resolve(dirname(dbPath), 'images')`). Paths without a
/// directory component resolve relative to the working directory.
pub fn images_dir(db_path: &str) -> PathBuf {
    match Path::new(db_path).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("images"),
        _ => PathBuf::from("data/images"),
    }
}

/// The local file one product's `image_path` resolves to, or `None` when
/// the path is empty or fails the flat-name check.
pub fn local_image_path(db_path: &str, image_path: &str) -> Option<PathBuf> {
    images_dir(db_path).join(image_file_of(image_path)?).into()
}

/// Transactionally converge the ledger on one menu payload: upsert every
/// tax rate, then every product (including archived), then replace the
/// category list (companion order wins; categories carry no references).
/// Products absent from the payload are left alone — the companion owns
/// catalog edits, deletion goes through its archive flag.
///
/// Returns which photo files changed (or are simply missing locally) so
/// the caller can fetch them; the ledger commit lands first, so a crash
/// between commit and downloads only costs a re-download.
pub fn apply_menu(repo: &Repo, payload: &MenuPayload) -> crate::repos::Result<MenuApply> {
    let conn = repo.conn();

    // Previous photo paths, so the caller can fetch exactly what changed.
    let old_paths: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, image_path FROM products")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };

    let tx = conn.unchecked_transaction()?;
    // The transaction lives on the same connection, so every repo statement
    // below joins it — commit (or drop) covers the whole pull.
    for rate in &payload.tax_rates {
        repo.upsert_tax_rate(&crate::tax::TaxRate {
            id: rate.id.clone(),
            name: rate.name.clone(),
            bps: rate.bps,
            inclusive: rate.inclusive,
        })?;
    }
    for p in &payload.products {
        repo.upsert_product(&crate::repos::Product {
            id: p.id.clone(),
            sku: p.sku.clone(),
            name: p.name.clone(),
            category: p.category.clone(),
            price_minor: p.price_minor,
            cost_minor: p.cost_minor,
            tax_rate_id: p.tax_rate_id.clone(),
            track_stock: p.track_stock,
            low_stock_at: p.low_stock_at,
            archived: p.archived,
            description: p.description.clone(),
            prep_notes: p.prep_notes.clone(),
            allergens: p.allergens.clone(),
            route_kitchen: p.route_kitchen,
            image_path: p.image_path.clone(),
        })?;
    }
    tx.execute("DELETE FROM categories", [])?;
    for (ix, name) in payload.categories.iter().enumerate() {
        tx.execute(
            "INSERT OR IGNORE INTO categories (name, sort) VALUES (?, ?)",
            params![name, ix as i64],
        )?;
    }
    tx.commit()?;

    // Files to fetch: anything whose path just moved, plus anything whose
    // local copy is missing (heals failed downloads on the next pull).
    let db_path = repo.db_path();
    let mut image_files = Vec::new();
    for p in &payload.products {
        let Some(file) = image_file_of(&p.image_path) else { continue };
        let changed = old_paths.get(&p.id).map(String::as_str).unwrap_or("") != p.image_path;
        let missing = !local_image_path(db_path, &p.image_path).is_some_and(|path| path.exists());
        if (changed || missing) && !image_files.contains(&file) {
            image_files.push(file);
        }
    }

    Ok(MenuApply { rev: payload.rev, products: payload.products.len(), image_files })
}
