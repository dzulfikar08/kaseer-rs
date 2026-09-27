//! Bridge between the app and `kaseer_core::sync`: the ureq transport for
//! the frozen batch contract, the process-wide drain lock, and the
//! fire-and-forget entry points used by app start and shift close. The
//! Settings screen calls [`drain_once`] on the background executor so a
//! slow cloud never freezes the UI.

use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::{App, AppContext as _};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::sync::{self, BatchHttp, DrainOutcome, Outbox};

/// Shared agent: pooled connections, hard timeouts so a dead network
/// never hangs a drain thread.
struct UreqBatchHttp {
    agent: ureq::Agent,
}

impl Default for UreqBatchHttp {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build(),
        }
    }
}

impl BatchHttp for UreqBatchHttp {
    fn post_batch(&self, base_url: &str, token: &str, body: &str) -> Result<String, String> {
        let url = format!("{}/api/sync/batch", base_url.trim_end_matches('/'));
        match self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .send_string(body)
        {
            Ok(response) => response.into_string().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(status, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(format!("HTTP {status}: {detail}"))
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// At most one drain at a time: app start, shift close, and the button
/// can otherwise overlap and double-send the same chunk. `try_lock`
/// turns that race into a polite "already running".
static DRAIN_LOCK: Mutex<()> = Mutex::new(());

/// Runs one full drain synchronously against the real ledger and
/// outbox. Blocking (SQLite + HTTP): run it on a background thread.
pub fn drain_once(forced: bool) -> Result<DrainOutcome, String> {
    let _guard = DRAIN_LOCK
        .try_lock()
        .map_err(|_| "A sync is already running".to_string())?;
    let outbox = Outbox::open(&sync::resolve_outbox_path()).map_err(|e| e.to_string())?;
    let ledger = open_repo(&resolve_db_path()).map_err(|e| e.to_string())?;
    sync::drain(&ledger, &outbox, &UreqBatchHttp::default(), forced)
}

/// Fire-and-forget drain on the background executor: automatic trigger
/// (respects the backoff window), no UI waits on it.
pub fn spawn_background_drain(cx: &mut App) {
    cx.background_spawn(async move {
        let _ = drain_once(false);
    })
    .detach();
}
