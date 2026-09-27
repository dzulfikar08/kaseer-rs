//! Kiosk↔cloud identity: pairing-code enrollment, heartbeat, and
//! auto-update. Enrollment is first-boot: with no device identity in the
//! outbox's meta table, the Settings screen collects a 6-digit pairing
//! code (minted by the store owner on the cloud console) and exchanges it
//! at the public `POST /api/kiosk/enroll` for a device token — shown
//! once, stored write-only, never rendered again. From then on the
//! terminal is a first-class kiosk: the drain presents the device token
//! (see `core::sync::drain`), the menu pulls from the cloud (see
//! `menu_sync::menu_source`), and this module keeps the cloud's presence
//! picture fresh (heartbeat every 5 minutes) and the binary current
//! (sha256-verified auto-update at boot and every 6 hours).
//!
//! The update is deliberately boring: never mid-shift (a swap under an
//! open till is deferred to the next boot or poll), never self-restarting
//! — the cashier applies it on the next natural launch, and the Settings
//! banner says so.

use std::io::Read;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{AsyncApp, WindowHandle};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::sync::{self, Outbox};
use sha2::{Digest, Sha256};

/// The production worker; `KASEER_CLOUD_URL` overrides it. Kept editable
/// in Settings so a deployment move never needs a rebuild. Confirmed with
/// the cloud crew on 2026-09-28.
pub const DEFAULT_CLOUD_URL: &str = "https://kaseer-cloud.dzulfikar-at.workers.dev";

/// Heartbeat cadence: the cloud's console freshness picture.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Update poll cadence: at boot plus every 6 hours — a kiosk updates on
/// its own schedule, not mid-shift.
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Release binaries are ~50MB; this ceiling only catches garbage
/// responses, not real updates.
const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;

/// Outbox meta keys for the device identity.
const KEY_DEVICE_TOKEN: &str = "device_token";
const KEY_KIOSK_ID: &str = "kiosk_id";
const KEY_STORE_ID: &str = "kiosk_store_id";
const KEY_KIOSK_NAME: &str = "kiosk_name";
const KEY_LAST_HEARTBEAT: &str = "kiosk_last_heartbeat";
const KEY_UPDATE_PENDING: &str = "update_pending";

/// The enrolled device identity, read back from the outbox meta table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KioskIdentity {
    pub kiosk_id: String,
    pub store_id: String,
    pub device_token: String,
    pub name: String,
}

impl KioskIdentity {
    /// Present only when enrollment stored a full identity.
    pub fn from_outbox(outbox: &Outbox) -> Option<Self> {
        let device_token = outbox.meta_get(KEY_DEVICE_TOKEN)?;
        if device_token.trim().is_empty() {
            return None;
        }
        Some(Self {
            kiosk_id: outbox.meta_get(KEY_KIOSK_ID)?,
            store_id: outbox.meta_get(KEY_STORE_ID)?,
            device_token,
            name: outbox.meta_get(KEY_KIOSK_NAME).unwrap_or_default(),
        })
    }

    fn clear_from(outbox: &Outbox) {
        for key in [
            KEY_DEVICE_TOKEN,
            KEY_KIOSK_ID,
            KEY_STORE_ID,
            KEY_KIOSK_NAME,
            KEY_LAST_HEARTBEAT,
            KEY_UPDATE_PENDING,
        ] {
            let _ = outbox.meta_delete(key);
        }
        // The stored menu revision belongs to the old source; dropping it
        // forces the next poll to re-pull whatever the (new) source serves.
        let _ = outbox.meta_delete("menu_rev");
    }
}

/// Enrollment/update target: `KASEER_CLOUD_URL` or the production default.
pub fn default_enroll_url() -> String {
    std::env::var("KASEER_CLOUD_URL").unwrap_or_else(|_| DEFAULT_CLOUD_URL.to_string())
}

/// Terminal name sent with enrollment: the hostname when readable.
fn device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "kaseer-terminal".to_string())
}

/// Target platform for update manifests, e.g. `x86_64-linux`.
pub fn arch() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

fn now_iso() -> String {
    kaseer_core::chrono::Utc::now()
        .to_rfc3339_opts(kaseer_core::chrono::SecondsFormat::Millis, true)
}

/// The ureq transport for kiosk endpoints: pooled, rustls, hard timeouts
/// so a dead cloud never hangs a background loop.
struct KioskHttp {
    agent: ureq::Agent,
}

impl Default for KioskHttp {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build(),
        }
    }
}

impl KioskHttp {
    fn send(
        &self,
        request: ureq::Request,
        token: Option<&str>,
        body: Option<String>,
    ) -> Result<Vec<u8>, String> {
        let request = match token {
            Some(token) => request.set("Authorization", &format!("Bearer {token}")),
            None => request,
        };
        let response = match body {
            Some(body) => request.send_string(&body),
            None => request.call(),
        };
        match response {
            Ok(response) => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_UPDATE_BYTES)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                Ok(bytes)
            }
            Err(ureq::Error::Status(status, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(format!("HTTP {status}: {detail}"))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        self.send(self.agent.get(url), token, None)
    }

    fn post_json(&self, url: &str, token: Option<&str>, body: String) -> Result<Vec<u8>, String> {
        self.send(self.agent.post(url), token, Some(body))
    }
}

/// Enrolls this terminal against `base` with a pairing code and stores
/// the identity in the outbox meta table. The cloud URL becomes the sync
/// base (the drain presents the device token from now on).
pub fn enroll(base_url: &str, pairing_code: &str) -> Result<KioskIdentity, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Cloud URL is empty".to_string());
    }
    let outbox = Outbox::open(&sync::resolve_outbox_path()).map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "pairingCode": pairing_code.trim(),
        "deviceName": device_name(),
        "arch": arch(),
    })
    .to_string();
    let bytes = KioskHttp::default().post_json(&format!("{base}/api/kiosk/enroll"), None, body)?;
    let wire: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid enroll response: {e}"))?;
    let identity = KioskIdentity {
        kiosk_id: wire
            .get("kioskId")
            .and_then(serde_json::Value::as_str)
            .ok_or("enroll response missing kioskId")?
            .to_string(),
        store_id: wire
            .get("storeId")
            .and_then(serde_json::Value::as_str)
            .ok_or("enroll response missing storeId")?
            .to_string(),
        device_token: wire
            .get("deviceToken")
            .and_then(serde_json::Value::as_str)
            .ok_or("enroll response missing deviceToken")?
            .to_string(),
        name: device_name(),
    };
    let stored = outbox.meta_set(KEY_DEVICE_TOKEN, &identity.device_token)
        .and_then(|_| outbox.meta_set(KEY_KIOSK_ID, &identity.kiosk_id))
        .and_then(|_| outbox.meta_set(KEY_STORE_ID, &identity.store_id))
        .and_then(|_| outbox.meta_set(KEY_KIOSK_NAME, &identity.name))
        .and_then(|_| outbox.meta_set("cloud_url", base))
        .and_then(|_| outbox.meta_delete("menu_rev"));
    stored.map_err(|e| format!("could not store identity: {e}"))?;
    Ok(identity)
}

/// Clears the device identity; the terminal falls back to store-token
/// sync and LAN-companion menus. The cloud-side kiosk record (and its
/// ability to mint a new pairing code) is untouched.
pub fn revoke() -> Result<(), String> {
    let outbox = Outbox::open(&sync::resolve_outbox_path()).map_err(|e| e.to_string())?;
    KioskIdentity::clear_from(&outbox);
    Ok(())
}

/// One heartbeat: tells the cloud this kiosk lives (and its version), and
/// stamps the Settings line. Best-effort by design — a failed beat is
/// retried on the next tick and surfaces nowhere but the timestamp.
pub fn heartbeat() -> Result<(), String> {
    let outbox = Outbox::open(&sync::resolve_outbox_path()).map_err(|e| e.to_string())?;
    let identity = KioskIdentity::from_outbox(&outbox).ok_or("Not enrolled")?;
    let base = cloud_base(&outbox);
    let body = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "arch": arch(),
    })
    .to_string();
    KioskHttp::default().post_json(
        &format!("{base}/api/kiosk/heartbeat"),
        Some(&identity.device_token),
        body,
    )?;
    outbox
        .meta_set(KEY_LAST_HEARTBEAT, &now_iso())
        .map_err(|e| e.to_string())
}

/// The kiosk endpoints' base: the enrolled cloud URL (the drain and the
/// menu share it), falling back to the enrollment default.
fn cloud_base(outbox: &Outbox) -> String {
    outbox
        .meta_get("cloud_url")
        .filter(|u| !u.trim().is_empty())
        .map(|u| u.trim_end_matches('/').to_string())
        .unwrap_or_else(default_enroll_url)
}

// ── Auto-update ─────────────────────────────────────────────────────────

/// `(major, minor, patch)` for the semver-ish versions the release
/// manifest carries. Pre-release/build suffixes are ignored: a kiosk
/// tracks the stable channel.
fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_semver(candidate), parse_semver(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Where the downloaded release waits and where the running binary lives.
struct UpdateTargets {
    /// `./data/kaseer.new` — the contract's staging file, next to the
    /// ledger's data dir.
    staging: PathBuf,
    /// The running executable (`std::env::current_exe`).
    current: PathBuf,
}

fn update_targets() -> Result<UpdateTargets, String> {
    let current = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let data_dir = Path::new(&resolve_db_path())
        .parent()
        .map(|dir| dir.to_path_buf())
        .unwrap_or_default();
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("data dir: {e}"))?;
    Ok(UpdateTargets {
        staging: data_dir.join("kaseer.new"),
        current,
    })
}

/// One update cycle: check the manifest, download and verify a newer
/// release, atomically swap it over the running binary. Returns the
/// installed version, or `None` when up to date / deferred (shift open).
/// Never restarts the process — the swap lands on disk only.
pub fn check_for_update() -> Result<Option<String>, String> {
    let outbox = Outbox::open(&sync::resolve_outbox_path()).map_err(|e| e.to_string())?;
    let identity = KioskIdentity::from_outbox(&outbox).ok_or("Not enrolled")?;
    let base = cloud_base(&outbox);

    // Shift guard: the running binary owns the open till; defer.
    let ledger = open_repo(&resolve_db_path()).map_err(|e| e.to_string())?;
    if ledger.active_shift().map_err(|e| e.to_string())?.is_some() {
        return Ok(None);
    }
    drop(ledger);

    let http = KioskHttp::default();
    let bytes = match http.get(
        &format!("{base}/api/kiosk/latest?arch={}", arch()),
        Some(&identity.device_token),
    ) {
        Ok(bytes) => bytes,
        // "No release for this arch yet" is the healthy steady state.
        Err(error) if error.starts_with("HTTP 404") => {
            let _ = outbox.meta_delete(KEY_UPDATE_PENDING);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let wire: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid update manifest: {e}"))?;
    let version = wire
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or("update manifest missing version")?
        .to_string();
    let url = wire
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or("update manifest missing url")?;
    let expected_sha = wire
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or("update manifest missing sha256")?
        .to_lowercase();

    let current = env!("CARGO_PKG_VERSION");
    if !is_newer(&version, current) {
        // Up to date (or the cloud regressed): a stale pending banner
        // from a previous install is now provably applied or moot.
        let _ = outbox.meta_delete(KEY_UPDATE_PENDING);
        return Ok(None);
    }

    let release = http.get(url, Some(&identity.device_token))?;
    if release.is_empty() {
        return Err("update download is empty".to_string());
    }
    let actual_sha: String = Sha256::digest(&release)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual_sha != expected_sha {
        return Err(format!(
            "update sha256 mismatch: expected {expected_sha}, got {actual_sha}"
        ));
    }

    install(&release)?;
    outbox
        .meta_set(KEY_UPDATE_PENDING, &version)
        .map_err(|e| e.to_string())?;
    Ok(Some(version))
}

/// Stages the verified bytes in `./data/kaseer.new`, then swaps them over
/// the running binary: a copy beside the target (same filesystem, so the
/// final rename is atomic and never EXDEVs from the data dir), +x, rename.
fn install(release: &[u8]) -> Result<(), String> {
    let targets = update_targets()?;
    std::fs::write(&targets.staging, release)
        .map_err(|e| format!("staging write: {e}"))?;
    let beside = targets
        .current
        .parent()
        .ok_or("binary has no parent dir")?
        .join("kaseer.new");
    let install = std::fs::copy(&targets.staging, &beside)
        .map(|_| ())
        .and_then(|_| {
            std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755))
        })
        .and_then(|_| std::fs::rename(&beside, &targets.current));
    let _ = std::fs::remove_file(&targets.staging);
    install.map_err(|e| format!("binary swap: {e}"))
}

/// Update checks run on the background executor; the toast is the one
/// bit of UI they own (via the main window handle).
fn toast_update(window: &WindowHandle<Root>, cx: &mut AsyncApp, version: &str) {
    let message = format!("Update {version} installed — restart to apply");
    let _ = window.update(cx, |_, window, cx| {
        window.push_notification(Notification::success(message), cx);
    });
}

fn run_update_check(window: &WindowHandle<Root>, cx: &mut AsyncApp) {
    match check_for_update() {
        Ok(Some(version)) => toast_update(window, cx, &version),
        Ok(None) => {}
        Err(error) => eprintln!("[kiosk] update check failed: {error}"),
    }
}

/// The forever loop, awaited by main for the process's lifetime: a boot
/// heartbeat and boot update check, then heartbeats every
/// [`HEARTBEAT_INTERVAL`] and update checks every [`UPDATE_INTERVAL`].
/// Everything inside skips itself while the terminal is not enrolled.
pub async fn kiosk_loop(cx: &mut AsyncApp, window: WindowHandle<Root>) {
    let bg = cx.background_executor().clone();
    let mut next_heartbeat = std::time::Instant::now();
    let mut next_update = next_heartbeat;
    loop {
        if std::time::Instant::now() >= next_heartbeat {
            if let Err(error) = heartbeat() {
                eprintln!("[kiosk] heartbeat failed: {error}");
            }
            next_heartbeat += HEARTBEAT_INTERVAL;
        }
        if std::time::Instant::now() >= next_update {
            run_update_check(&window, cx);
            next_update += UPDATE_INTERVAL;
        }
        let wait = next_heartbeat
            .min(next_update)
            .saturating_duration_since(std::time::Instant::now());
        bg.timer(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compares_stably_and_ignores_suffixes() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("1.10.0", "1.9.9"), "numeric, not lexicographic");
        assert!(is_newer("0.1.1-beta", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("banana", "0.1.0"), "unparsable never updates");
        assert!(!is_newer("0.1", "0.1.0"), "incomplete semver never updates");
    }

    #[test]
    fn arch_matches_the_contract_shape() {
        // The manifest keys releases by this string; the contract's
        // example is x86_64-linux.
        assert!(arch().ends_with("-linux"));
        assert_eq!(arch(), format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS));
    }
}
