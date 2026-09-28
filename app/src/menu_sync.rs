//! Menu pull engine: the menu's source of truth is the cloud for
//! enrolled kiosks, the LAN web companion for everyone else (see
//! [`menu_source`]). A background executor task polls
//! `GET /api/kiosk/menu/revision` (cloud, device bearer) or
//! `GET /api/menu/revision` (companion) every 10 or 30 seconds; when the
//! revision moves (or on boot, or on the Settings button) the full menu
//! payload is applied transactionally by [`kaseer_core::menu`] and the
//! source's photo files are downloaded into the ledger's images/
//! directory, best-effort per file. Pull failures are logged to the
//! outbox's meta table (and stderr) — never a toast, and the register
//! keeps selling off its cached catalog.
//!
//! Knowledge convergence: when the Knowledge screen saves, the app also
//! fires `PATCH /api/products/:id/knowledge` at the same source (device
//! bearer against the cloud, bare against the companion), fire-and-forget.
//! The PATCH bumps the source's menu revision, the next poll sees the new
//! revision and re-pulls, and both sides converge on the same copy instead
//! of the next pull clobbering the kiosk's edit. (Training mode is
//! deliberately NOT wired: it edits a throwaway snapshot of the ledger,
//! and its lessons must never reach the real menu.)
//!
//! Config lives beside the cloud-sync settings in the outbox's own SQLite
//! meta table (`companion_url`, `cloud_url`, `device_token`).

use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, BackgroundExecutor};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::menu::{self, MenuApply};
use kaseer_core::repos::KnowledgePatch;
use kaseer_core::sync::{self, Outbox};

/// Placeholder endpoint shown before a store configures its companion.
pub const DEFAULT_COMPANION_URL: &str = "http://127.0.0.1:8788";

/// Poll cadence; the register's UI watch runs on a faster tick, so a pull
/// lands on screen well inside one interval.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Photo responses are trusted only up to this size (web uploads are
/// phone-camera JPEGs; this is a sanity ceiling, not a compression spec).
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

const KEY_COMPANION_URL: &str = "companion_url";
const KEY_REV: &str = "menu_rev";
const KEY_LAST_SYNC: &str = "menu_last_sync_at";
const KEY_ERROR: &str = "menu_last_error";

/// Revision of the last successfully applied pull; the register screen
/// watches this to reload its catalog snapshot.
static APPLIED_REV: AtomicU64 = AtomicU64::new(0);

/// At most one pull at a time: the poll loop and the manual button can
/// otherwise overlap on the same ledger and outbox.
static PULL_LOCK: Mutex<()> = Mutex::new(());

/// Configured companion URL: the Settings field (outbox meta) wins, then
/// `KASEER_COMPANION_URL`, then the loopback default.
pub fn default_companion_url() -> String {
    std::env::var("KASEER_COMPANION_URL").unwrap_or_else(|_| DEFAULT_COMPANION_URL.to_string())
}

fn companion_url(outbox: Option<&Outbox>) -> String {
    outbox
        .and_then(|o| o.meta_get(KEY_COMPANION_URL))
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(default_companion_url)
}

/// Where the menu comes from. Enrollment moves the terminal to the cloud
/// for good — the companion stays available only below that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuSource {
    /// Enrolled: the cloud worker, addressed with the device token.
    Cloud { base: String, token: String },
    /// Not enrolled: the LAN web companion, unauthenticated LAN traffic.
    Companion { base: String },
}

/// Reads the enrollment state off the outbox meta table.
pub fn menu_source(outbox: &Outbox) -> MenuSource {
    match outbox
        .meta_get("device_token")
        .filter(|token| !token.trim().is_empty())
    {
        Some(token) => MenuSource::Cloud {
            base: outbox
                .meta_get("cloud_url")
                .filter(|url| !url.trim().is_empty())
                .map(|url| url.trim_end_matches('/').to_string())
                .unwrap_or_else(crate::kiosk::default_enroll_url),
            token,
        },
        None => MenuSource::Companion {
            base: companion_url(Some(outbox)),
        },
    }
}

/// Poll cadence: enrolled kiosks follow the cloud's 10-second revision
/// contract; the LAN companion keeps its slower 30-second tick.
pub fn poll_interval(outbox: Option<&Outbox>) -> Duration {
    match outbox {
        Some(outbox) if matches!(menu_source(outbox), MenuSource::Cloud { .. }) => {
            Duration::from_secs(10)
        }
        _ => POLL_INTERVAL,
    }
}

pub fn applied_revision() -> u64 {
    APPLIED_REV.load(Ordering::Acquire)
}

/// The menu endpoints: one GET for JSON/bytes, one PATCH for knowledge
/// pushes. `token` is the device bearer against the cloud, `None` against
/// the LAN companion. A trait so the poll flow is testable without either.
pub trait MenuHttp {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String>;
    fn patch_json(&self, url: &str, token: Option<&str>, body: &str) -> Result<(), String>;
}

/// The ureq transport: pooled, rustls, hard timeouts so a dead source
/// never hangs a poll thread.
pub struct UreqMenuHttp {
    agent: ureq::Agent,
}

impl Default for UreqMenuHttp {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .build(),
        }
    }
}

/// Adds the device bearer when a token is present.
fn authorized(request: ureq::Request, token: Option<&str>) -> ureq::Request {
    match token {
        Some(token) => request.set("Authorization", &format!("Bearer {token}")),
        None => request,
    }
}

impl MenuHttp for UreqMenuHttp {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        let response = authorized(self.agent.get(url), token)
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(status, response) => {
                    let detail = response.into_string().unwrap_or_default();
                    format!("HTTP {status}: {detail}")
                }
                error => error.to_string(),
            })?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_IMAGE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }

    fn patch_json(&self, url: &str, token: Option<&str>, body: &str) -> Result<(), String> {
        match authorized(self.agent.patch(url), token).send_string(body) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(status, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(format!("HTTP {status}: {detail}"))
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// One poll cycle against explicit paths (the live wrappers below supply
/// the real ones). Returns a short human summary for the Settings line.
fn run_poll(
    http: &dyn MenuHttp,
    ledger_path: &str,
    outbox_path: &str,
    forced: bool,
) -> Result<String, String> {
    let _guard = PULL_LOCK
        .try_lock()
        .map_err(|_| "A menu pull is already running".to_string())?;
    let outbox = Outbox::open(outbox_path).map_err(|e| format!("Outbox unavailable: {e}"))?;
    let (base, token) = match menu_source(&outbox) {
        MenuSource::Cloud { base, token } => (base, Some(token)),
        MenuSource::Companion { base } => (base.trim_end_matches('/').to_string(), None),
    };

    let result = (|| -> Result<String, String> {
        let revision_bytes = http.get(&format!("{base}/api/menu/revision"), token.as_deref())?;
        let wire: serde_json::Value = serde_json::from_slice(&revision_bytes)
            .map_err(|e| format!("invalid revision payload: {e}"))?;
        let rev = wire
            .get("rev")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let last: u64 = outbox
            .meta_get(KEY_REV)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if !forced && rev == last {
            return Ok(format!("Menu already current (rev {rev})"));
        }

        let menu_bytes = http.get(&format!("{base}/api/menu"), token.as_deref())?;
        let payload = menu::parse_menu(&menu_bytes)?;
        let ledger = open_repo(ledger_path).map_err(|e| format!("Ledger unavailable: {e}"))?;
        let apply: MenuApply = menu::apply_menu(&ledger, &payload).map_err(|e| e.to_string())?;
        let fetched = fetch_images(
            http,
            &base,
            token.as_deref(),
            ledger.db_path(),
            &apply.image_files,
        );

        // The applied revision goes out even when some photos failed:
        // prices/knowledge are live, and the next pull retries the files
        // (they stay flagged while missing locally).
        APPLIED_REV.store(apply.rev, Ordering::Release);
        outbox
            .meta_set(KEY_REV, &apply.rev.to_string())
            .map_err(|e| e.to_string())?;
        outbox
            .meta_set(KEY_LAST_SYNC, &now_iso())
            .map_err(|e| e.to_string())?;
        outbox.meta_delete(KEY_ERROR).map_err(|e| e.to_string())?;
        Ok(match fetched {
            0 => format!("Menu rev {} — {} products", apply.rev, apply.products),
            n => format!(
                "Menu rev {} — {} products, {n} photos",
                apply.rev, apply.products
            ),
        })
    })();

    match result {
        Ok(summary) => Ok(summary),
        Err(error) => {
            // Logged, not toasted: a cashier sells happily on the cached
            // menu while the pull keeps trying in the background.
            eprintln!("[menu_sync] pull failed: {error}");
            let _ = outbox.meta_set(KEY_ERROR, &error);
            Err(error)
        }
    }
}

/// Best-effort photo download; returns how many files landed. A failure
/// keeps the product's `image_path` (the card falls back to text until a
/// later pull lands the file). The cloud serves images behind the device
/// token via query param (a plain `<img src>`-style URL); the LAN
/// companion serves them bare.
fn fetch_images(
    http: &dyn MenuHttp,
    base: &str,
    token: Option<&str>,
    db_path: &str,
    files: &[String],
) -> usize {
    let mut fetched = 0;
    for file in files {
        let Some(target) = menu::local_image_path(db_path, &format!("images/{file}")) else {
            continue;
        };
        let url = match token {
            Some(token) => format!("{base}/images/{file}?token={token}"),
            None => format!("{base}/images/{file}"),
        };
        match http.get(&url, None) {
            Ok(bytes) if !bytes.is_empty() => {
                // Write-then-rename: a half-written photo must never be
                // the one the register renders.
                let tmp = target.with_extension("part");
                let parent = target
                    .parent()
                    .map(|dir| dir.to_path_buf())
                    .unwrap_or_default();
                if std::fs::create_dir_all(parent).is_ok()
                    && std::fs::write(&tmp, &bytes).is_ok()
                    && std::fs::rename(&tmp, &target).is_ok()
                {
                    fetched += 1;
                } else {
                    eprintln!("[menu_sync] could not store photo {file}");
                }
            }
            Ok(_) => eprintln!("[menu_sync] empty photo response for {file}"),
            Err(error) => eprintln!("[menu_sync] photo download failed for {file}: {error}"),
        }
    }
    fetched
}

fn now_iso() -> String {
    // The app owns no chrono dep; core re-exports it for exactly this.
    kaseer_core::chrono::Utc::now()
        .to_rfc3339_opts(kaseer_core::chrono::SecondsFormat::Millis, true)
}

/// Live poll cycle: real paths, real transport, revision-gated.
pub fn poll_once() {
    let _ = run_poll(
        &UreqMenuHttp::default(),
        &resolve_db_path(),
        &sync::resolve_outbox_path(),
        false,
    );
}

/// Manual pull from Settings: ignores the revision gate and returns a
/// summary for the status line.
pub fn pull_now() -> String {
    match run_poll(
        &UreqMenuHttp::default(),
        &resolve_db_path(),
        &sync::resolve_outbox_path(),
        true,
    ) {
        Ok(summary) => summary,
        Err(error) => format!("Menu pull failed: {error}"),
    }
}

/// The forever loop: poll at boot, then every [`poll_interval`] — 10s
/// enrolled, 30s on the companion. Runs on the background executor for
/// the process's lifetime; the interval re-reads enrollment each tick so
/// enrolling takes effect without a restart.
pub fn spawn_menu_poll(bg: BackgroundExecutor) {
    let timer_bg = bg.clone();
    bg.spawn(async move {
        loop {
            poll_once();
            let outbox = Outbox::open(&sync::resolve_outbox_path()).ok();
            timer_bg.timer(poll_interval(outbox.as_ref())).await;
        }
    })
    .detach();
}

/// Fire-and-forget knowledge push after a local save (see the module
/// comment): the menu source merges the same patch, bumps its menu
/// revision, and the next pull converges instead of clobbering.
pub fn spawn_knowledge_push(cx: &mut App, product_id: String, patch: KnowledgePatch) {
    cx.background_spawn(async move {
        let outbox = Outbox::open(&sync::resolve_outbox_path()).ok();
        let (base, token) = match outbox.as_ref().map(menu_source) {
            Some(MenuSource::Cloud { base, token }) => (base, Some(token)),
            Some(MenuSource::Companion { base }) => (base.trim_end_matches('/').to_string(), None),
            None => (companion_url(None).trim_end_matches('/').to_string(), None),
        };
        // Omitted keys keep their value on the source, so None entries
        // are left out of the body entirely.
        let mut body = serde_json::Map::new();
        if let Some(value) = patch.description {
            body.insert("description".into(), serde_json::Value::String(value));
        }
        if let Some(value) = patch.prep_notes {
            body.insert("prepNotes".into(), serde_json::Value::String(value));
        }
        if let Some(value) = patch.allergens {
            body.insert("allergens".into(), serde_json::Value::String(value));
        }
        if let Some(value) = patch.route_kitchen {
            body.insert("routeKitchen".into(), serde_json::Value::Bool(value));
        }
        let url = format!("{base}/api/products/{product_id}/knowledge");
        if let Err(error) = UreqMenuHttp::default().patch_json(
            &url,
            token.as_deref(),
            &serde_json::Value::Object(body).to_string(),
        ) {
            eprintln!("[menu_sync] knowledge push failed for {product_id}: {error}");
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    /// The companion contract on a wire: revision-gated full menu with one
    /// photo, counting knowledge PATCHes so convergence can be asserted.
    struct FakeCompanion {
        rev: AtomicU64,
        product_id: String,
        image_file: String,
        image_bytes: Vec<u8>,
        pulls: AtomicU64,
        patches: parking_lot::Mutex<Vec<(String, String)>>,
    }

    impl FakeCompanion {
        /// Serves one product addressed by `product_id` — the shared-ledger
        /// model, where the companion serves the very rows the kiosk reads.
        /// Both endpoints always report the same, current revision.
        fn new(rev: u64, product_id: &str, image_file: &str) -> Self {
            Self {
                rev: AtomicU64::new(rev),
                product_id: product_id.to_string(),
                image_file: image_file.to_string(),
                image_bytes: b"jpeg-bytes".to_vec(),
                pulls: AtomicU64::new(0),
                patches: parking_lot::Mutex::new(Vec::new()),
            }
        }

        fn menu_bytes(&self) -> Vec<u8> {
            serde_json::json!({
                "v": 1, "rev": self.rev.load(Ordering::Acquire),
                "taxRates": [{ "id": "ppn11", "name": "PPN 11%", "bps": 1100, "inclusive": true }],
                "categories": ["Coffee"],
                "products": [{
                    "id": self.product_id, "sku": "8991002100028", "name": "Americano",
                    "category": "Coffee", "priceMinor": 16000, "costMinor": 5000,
                    "taxRateId": "ppn11", "trackStock": false, "lowStockAt": 0,
                    "archived": false, "description": "", "prepNotes": "Keep it hot.",
                    "allergens": "none", "routeKitchen": false,
                    "imagePath": format!("images/{}", self.image_file)
                }]
            })
            .to_string()
            .into_bytes()
        }
    }

    impl MenuHttp for FakeCompanion {
        fn get(&self, url: &str, _token: Option<&str>) -> Result<Vec<u8>, String> {
            if url.ends_with("/api/menu/revision") {
                return Ok(
                    serde_json::json!({ "v": 1, "rev": self.rev.load(Ordering::Acquire) })
                        .to_string()
                        .into_bytes(),
                );
            }
            if url.ends_with("/api/menu") {
                self.pulls.fetch_add(1, Ordering::AcqRel);
                return Ok(self.menu_bytes());
            }
            if url.contains("/images/") {
                return Ok(self.image_bytes.clone());
            }
            Err(format!("unexpected GET {url}"))
        }

        fn patch_json(&self, url: &str, _token: Option<&str>, body: &str) -> Result<(), String> {
            self.patches
                .lock()
                .push((url.to_string(), body.to_string()));
            Ok(())
        }
    }

    /// The enrolled kiosk's cloud: records every request URL + token so
    /// the cloud source can be asserted end to end (revision + menu carry
    /// the device bearer; images carry the token query param).
    struct FakeCloud {
        rev: u64,
        product_id: String,
        requests: parking_lot::Mutex<Vec<(String, String)>>,
    }

    impl FakeCloud {
        fn new(rev: u64, product_id: &str) -> Self {
            Self {
                rev,
                product_id: product_id.to_string(),
                requests: parking_lot::Mutex::new(Vec::new()),
            }
        }
    }

    impl MenuHttp for FakeCloud {
        fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
            self.requests
                .lock()
                .push((url.to_string(), token.unwrap_or_default().to_string()));
            if url.ends_with("/api/menu/revision") {
                return Ok(serde_json::json!({ "v": 1, "rev": self.rev })
                    .to_string()
                    .into_bytes());
            }
            if url.ends_with("/api/menu") {
                return Ok(serde_json::json!({
                    "v": 1, "rev": self.rev,
                    "taxRates": [],
                    "categories": ["Coffee"],
                    "products": [{
                        "id": self.product_id, "sku": "8991002100028", "name": "Americano",
                        "category": "Coffee", "priceMinor": 17000, "costMinor": 5000,
                        "trackStock": false, "lowStockAt": 0, "archived": false,
                        "description": "", "prepNotes": "", "allergens": "", "routeKitchen": false,
                        "imagePath": "images/cloud.jpg"
                    }]
                })
                .to_string()
                .into_bytes());
            }
            if url.contains("/images/") {
                if !url.contains("token=dev-tok") {
                    return Err("image request missing device token".to_string());
                }
                return Ok(b"jpeg-bytes".to_vec());
            }
            Err(format!("unexpected GET {url}"))
        }

        fn patch_json(&self, url: &str, token: Option<&str>, body: &str) -> Result<(), String> {
            self.requests.lock().push((
                url.to_string(),
                format!("patch:{}:{}", token.unwrap_or_default(), body),
            ));
            Ok(())
        }
    }

    fn temp_paths(dir: &tempfile::TempDir) -> (String, String) {
        (
            dir.path().join("ledger.db").to_str().unwrap().to_string(),
            dir.path().join("outbox.db").to_str().unwrap().to_string(),
        )
    }

    /// PULL_LOCK is process-global, so parallel test threads would trip
    /// each other's try_lock; serialize the tests instead.
    static SERIAL: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// The seeded americano's id — the payload must address the very row
    /// the kiosk already has (companion and kiosk share one ledger file).
    fn seeded_americano_id(ledger_path: &str) -> String {
        open_repo(ledger_path)
            .unwrap()
            .product_by_sku("8991002100028")
            .unwrap()
            .unwrap()
            .id
    }

    #[test]
    fn first_pull_applies_and_downloads_then_revision_gates() {
        let _serial = SERIAL.lock();
        let dir = tempfile::tempdir().unwrap();
        let (ledger, outbox_path) = temp_paths(&dir);
        let americano_id = seeded_americano_id(&ledger);
        let companion = FakeCompanion::new(4, &americano_id, "americano-01.jpg");

        // First poll: nothing stored yet, so the full menu applies.
        let summary = run_poll(&companion, &ledger, &outbox_path, false).unwrap();
        assert_eq!(summary, "Menu rev 4 — 1 products, 1 photos");
        assert_eq!(companion.pulls.load(Ordering::Acquire), 1);
        assert_eq!(applied_revision(), 4);

        // The photo landed next to the ledger; the price and knowledge
        // converged on the local row.
        let repo = open_repo(&ledger).unwrap();
        let stored = repo.product_by_sku("8991002100028").unwrap().unwrap();
        assert_eq!(stored.price_minor, 16000);
        assert_eq!(stored.prep_notes, "Keep it hot.");
        let photo = menu::local_image_path(repo.db_path(), &stored.image_path).unwrap();
        assert_eq!(std::fs::read(&photo).unwrap(), b"jpeg-bytes");

        // Second poll: same revision, no pull, no error recorded.
        let summary = run_poll(&companion, &ledger, &outbox_path, false).unwrap();
        assert_eq!(summary, "Menu already current (rev 4)");
        assert_eq!(companion.pulls.load(Ordering::Acquire), 1);

        let outbox = Outbox::open(&outbox_path).unwrap();
        assert!(outbox.meta_get("menu_last_error").is_none());
        assert!(outbox.meta_get("menu_last_sync_at").is_some());
    }

    #[test]
    fn moved_revision_repulls_and_missing_photos_retry() {
        let _serial = SERIAL.lock();
        let dir = tempfile::tempdir().unwrap();
        let (ledger, outbox_path) = temp_paths(&dir);
        let americano_id = seeded_americano_id(&ledger);
        let companion = FakeCompanion::new(4, &americano_id, "americano-01.jpg");
        run_poll(&companion, &ledger, &outbox_path, false).unwrap();

        // Delete the local photo: a same-revision forced pull still
        // refetches it (that is what the Settings button does).
        let repo = open_repo(&ledger).unwrap();
        let photo = menu::local_image_path(repo.db_path(), "images/americano-01.jpg").unwrap();
        std::fs::remove_file(&photo).unwrap();
        drop(repo);
        let summary = run_poll(&companion, &ledger, &outbox_path, true).unwrap();
        assert_eq!(summary, "Menu rev 4 — 1 products, 1 photos");
        let photo = menu::local_image_path(&ledger, "images/americano-01.jpg").unwrap();
        assert!(photo.exists());

        // A moved revision repulls even from an unchanged payload.
        companion.rev.store(5, Ordering::Release);
        let summary = run_poll(&companion, &ledger, &outbox_path, false).unwrap();
        assert_eq!(summary, "Menu rev 5 — 1 products");
        assert_eq!(companion.pulls.load(Ordering::Acquire), 3);
    }

    #[test]
    fn pull_failures_land_in_the_meta_log_not_the_ui() {
        let _serial = SERIAL.lock();
        let dir = tempfile::tempdir().unwrap();
        let (ledger, outbox_path) = temp_paths(&dir);

        struct Dead;
        impl MenuHttp for Dead {
            fn get(&self, _: &str, _: Option<&str>) -> Result<Vec<u8>, String> {
                Err("connection refused".into())
            }
            fn patch_json(&self, _: &str, _: Option<&str>, _: &str) -> Result<(), String> {
                unreachable!()
            }
        }

        let error = run_poll(&Dead, &ledger, &outbox_path, false).unwrap_err();
        assert!(error.contains("connection refused"));
        let outbox = Outbox::open(&outbox_path).unwrap();
        assert_eq!(
            outbox.meta_get("menu_last_error").as_deref(),
            Some(error.as_str())
        );
        assert_eq!(
            outbox.meta_get("menu_rev"),
            None,
            "a dead pull applies no revision"
        );
    }

    #[test]
    fn enrolled_kiosk_pulls_from_the_cloud_with_the_device_token() {
        let _serial = SERIAL.lock();
        let dir = tempfile::tempdir().unwrap();
        let (ledger, outbox_path) = temp_paths(&dir);
        let americano_id = seeded_americano_id(&ledger);
        let outbox = Outbox::open(&outbox_path).unwrap();
        outbox.meta_set("device_token", "dev-tok").unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();

        let cloud = FakeCloud::new(7, &americano_id);
        let summary = run_poll(&cloud, &ledger, &outbox_path, false).unwrap();
        assert_eq!(summary, "Menu rev 7 — 1 products, 1 photos");

        let requests = cloud.requests.lock().clone();
        assert!(
            requests
                .iter()
                .all(|(url, _)| url.starts_with("https://cloud.test/"))
        );
        let menu_token = requests
            .iter()
            .find(|(url, _)| url.ends_with("/api/menu"))
            .expect("menu was pulled");
        assert_eq!(
            menu_token.1, "dev-tok",
            "menu GET carries the device bearer"
        );
        let image_url = requests
            .iter()
            .map(|(url, _)| url.as_str())
            .find(|url| url.contains("/images/"))
            .expect("photo was fetched");
        assert!(image_url.ends_with("/images/cloud.jpg?token=dev-tok"));

        // The cloud's price landed on the shared row.
        let repo = open_repo(&ledger).unwrap();
        let stored = repo.product_by_sku("8991002100028").unwrap().unwrap();
        assert_eq!(stored.price_minor, 17000);

        // Enrolled cadence is the cloud's 10-second contract.
        assert_eq!(poll_interval(Some(&outbox)), Duration::from_secs(10));
        outbox.meta_delete("device_token").unwrap();
        assert_eq!(poll_interval(Some(&outbox)), POLL_INTERVAL);
    }

    #[test]
    fn knowledge_push_targets_the_enrolled_cloud() {
        let _serial = SERIAL.lock();
        let dir = tempfile::tempdir().unwrap();
        let (ledger, outbox_path) = temp_paths(&dir);
        let americano_id = seeded_americano_id(&ledger);
        let outbox = Outbox::open(&outbox_path).unwrap();
        outbox.meta_set("device_token", "dev-tok").unwrap();
        outbox.meta_set("cloud_url", "https://cloud.test").unwrap();

        let cloud = FakeCloud::new(1, &americano_id);
        run_poll(&cloud, &ledger, &outbox_path, true).unwrap();
        let before = cloud.requests.lock().len();

        // Same merge semantics as the companion path, new address: the
        // device bearer travels with the PATCH.
        let url = format!("https://cloud.test/api/products/{americano_id}/knowledge");
        cloud
            .patch_json(&url, Some("dev-tok"), r#"{"prepNotes":"Extra hot."}"#)
            .unwrap();
        let requests = cloud.requests.lock().clone();
        assert!(requests.len() > before);
        assert!(
            requests
                .iter()
                .any(|(u, t)| u == &url && t == "patch:dev-tok:{\"prepNotes\":\"Extra hot.\"}")
        );
    }
}
