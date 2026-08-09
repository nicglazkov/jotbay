//! Jotbay for Windows and Linux.
//!
//! Unlike the macOS app, which shells out to the `jotbay` binary, this links
//! `jotbay-core` directly. It is already a Rust process, so there is nothing to
//! gain from spawning one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};
use jotbay_core::limits::Severity;
use jotbay_core::settings::{Settings, Theme};
use jotbay_core::setup::{self, SetupCapabilities};
use jotbay_core::{ActivityEvent, CommitInfo, NodeStatus, SyncReport, Jotbay, JotbayStatus};

/// How often the tray re-reads git. Closing the window hides to the tray, so
/// for most of Jotbay's life this poll is the only thing keeping any on-screen
/// indicator honest. The webview may not even be running.
const TRAY_POLL: Duration = Duration::from_secs(10);

/// Set while a sync is in flight, from the tray menu or from the window, so
/// the icon can show "syncing" immediately instead of waiting for the next
/// poll to infer it.
static SYNCING: AtomicBool = AtomicBool::new(false);

/// What the tray is currently displaying, so a poll that finds no change costs
/// nothing and the icon does not flicker.
static SHOWN: Mutex<Option<TrayState>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TrayState {
    Idle,
    Syncing,
    Attention,
}

impl TrayState {
    /// Full-colour PNGs, not macOS-style templates: an AppIndicator draws the
    /// bytes as given, so a black-plus-alpha glyph would be invisible on
    /// Ubuntu's dark panel. State is carried by the plate colour, see
    /// `lib/icons/tray-idle.svg` for the reasoning.
    fn icon(self) -> &'static [u8] {
        match self {
            TrayState::Idle => &include_bytes!("../../../icons/generated/tray/idle@2x.png")[..],
            TrayState::Syncing => &include_bytes!("../../../icons/generated/tray/syncing@2x.png")[..],
            TrayState::Attention => {
                &include_bytes!("../../../icons/generated/tray/attention@2x.png")[..]
            }
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            TrayState::Idle => "Jotbay: everything in sync",
            TrayState::Syncing => "Jotbay: syncing",
            TrayState::Attention => "Jotbay: needs attention",
        }
    }
}

/// Mirrors the macOS menu bar glyph in `JotbayApp.swift`, deliberately: a
/// dirty working tree is not "attention", because notes are edited constantly
/// and an icon that shouts at every keystroke gets ignored. Only things the
/// user must act on count.
fn tray_state() -> TrayState {
    if SYNCING.load(Ordering::Relaxed) {
        return TrayState::Syncing;
    }
    match jotbay().and_then(|v| v.status(false).map_err(|e| e.to_string())) {
        Ok(s) => {
            let blocked = s.warnings.iter().any(|w| w.severity == Severity::Blocked);
            let node_error = s.nodes.iter().any(|n| n.last_error.is_some());
            if s.rebase_in_progress || blocked || node_error {
                TrayState::Attention
            } else {
                TrayState::Idle
            }
        }
        // Being unable to read the jotbay at all is precisely what the user
        // needs to see. Leaving the healthy glyph up would be a lie.
        Err(_) => TrayState::Attention,
    }
}

/// Recompute and, only if it changed, push the new glyph to the tray. Safe to
/// call from any thread.
fn update_tray(app: &AppHandle) {
    let next = tray_state();

    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    if *shown == Some(next) {
        return;
    }
    *shown = Some(next);
    drop(shown);

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("jotbay-tray") {
            if let Ok(icon) = Image::from_bytes(next.icon()) {
                let _ = tray.set_icon(Some(icon));
            }
            let _ = tray.set_tooltip(Some(next.tooltip()));
        }
    });
}

/// Holds the tray on "syncing" for as long as it is alive, and clears it on the
/// way out, including when the sync returned an error, which is the case that
/// matters most.
struct SyncGuard(AppHandle);

impl SyncGuard {
    fn new(app: &AppHandle) -> Self {
        SYNCING.store(true, Ordering::Relaxed);
        update_tray(app);
        Self(app.clone())
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNCING.store(false, Ordering::Relaxed);
        update_tray(&self.0);
    }
}

/// Every command opens Jotbay fresh. Sync passes are seconds apart at most
/// and git is the real source of truth, so caching a handle would only create
/// a way for the UI to show something git no longer agrees with.
fn jotbay() -> Result<Jotbay, String> {
    // for_app, not discover: this process's working directory is wherever the
    // launcher left it, which on Windows is the install directory.
    Jotbay::for_app().map_err(|e| e.to_string())
}

/// Every read command below is `async` for the same reason as `do_sync`: a
/// sync Tauri command runs on the main thread, and these all spawn git
/// subprocesses - with `refresh: true`, a network fetch. The 20-second status
/// poll was doing exactly that, so the window froze for the duration of every
/// poll and indefinitely when a fetch stalled; on Windows the whole app was
/// observed frozen and its close button dead for seconds at a time.
/// Everything the first-run screen needs, in one call: is there a vault at all,
/// and which of the three routes can this machine actually offer?
#[derive(serde::Serialize)]
struct SetupState {
    has_vault: bool,
    capabilities: SetupCapabilities,
}

#[tauri::command]
async fn get_setup_state() -> Result<SetupState, String> {
    tauri::async_runtime::spawn_blocking(|| SetupState {
        has_vault: Jotbay::exists(),
        capabilities: setup::capabilities(),
    })
    .await
    .map_err(|e| e.to_string())
}

/// Create, clone or adopt, whichever the user chose, then remember it.
#[tauri::command]
async fn run_setup(mode: String, value: String, location: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dest = std::path::PathBuf::from(&location);
        let root = match mode.as_str() {
            "create" => setup::create_and_clone(&value, &dest),
            "clone" => setup::clone_existing(&value, &dest),
            "adopt" => setup::adopt(&dest),
            other => Err(jotbay_core::Error::Other(format!("unknown mode {other}"))),
        }
        .map_err(|e| e.to_string())?;

        let vault = Jotbay::open(&root).map_err(|e| e.to_string())?;
        // Record it before syncing: if the first sync fails the vault still
        // exists and the app must open onto it rather than back to first run.
        vault.remember().map_err(|e| e.to_string())?;
        // And leave the machine syncing by itself. This used to happen only in
        // install.sh / install.ps1, so anyone who arrived through the .msi or
        // the .deb finished setup with a tool that synced when asked and at no
        // other time. The opposite of what the graphical route is for.
        let _ = jotbay_core::schedule::ensure();
        let _ = vault.sync();
        Ok(root.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Desktop shortcuts, offered once setup has produced something to point at.
///
/// Returns what was written so the window can name it rather than claim
/// success and leave the user looking for an icon that is not there.
#[tauri::command]
async fn create_shortcuts(app: bool, notes: bool) -> Result<Vec<String>, String> {
    use jotbay_core::shortcut::{self, Target};

    tauri::async_runtime::spawn_blocking(move || {
        let location = shortcut::default_location();
        let mut made = Vec::new();

        if notes {
            let dir = jotbay()?.data_dir();
            let path = shortcut::create(Target::Notes, &dir, &location).map_err(|e| e.to_string())?;
            made.push(path.to_string_lossy().to_string());
        }
        if app {
            let source = shortcut::locate_app()
                .ok_or_else(|| "could not find the Jotbay application".to_string())?;
            let path = shortcut::create(Target::App, &source, &location).map_err(|e| e.to_string())?;
            made.push(path.to_string_lossy().to_string());
        }
        Ok(made)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_status(refresh: bool) -> Result<JotbayStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let v = jotbay()?;
        // A window is open, so ask the other machines to report in. Only on a
        // refresh that already talks to the remote. The offline path exists
        // precisely so opening the app costs nothing when there is no network.
        //
        // Best-effort and ignored: presence is a courtesy to whoever is
        // looking, and must never be why a window fails to load.
        if refresh {
            let _ = jotbay_core::presence::request(v.git());
        }
        v.status(refresh).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `async` so Tauri runs this off the main thread. A blocking git push on the
/// main thread would stall the event loop, and the tray icon change queued by
/// `SyncGuard` would not appear until the sync it is announcing had finished.
#[tauri::command]
async fn do_sync(app: AppHandle) -> Result<SyncReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _syncing = SyncGuard::new(&app);
        jotbay()?.sync().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_log(limit: u32) -> Result<Vec<CommitInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || jotbay()?.log(limit).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn do_upgrade() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| jotbay()?.upgrade().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Everything the settings panel shows, from the same engine call the CLI and
/// the macOS app use. Local reads only, so opening settings sends no request.
#[tauri::command]
async fn get_about() -> Result<jotbay_core::about::About, String> {
    tauri::async_runtime::spawn_blocking(|| jotbay()?.about().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Refresh the release marker, then re-read. `status` is what fetches the
/// marker, so this is a real check rather than a re-read of what was known.
#[tauri::command]
async fn check_updates() -> Result<jotbay_core::about::About, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let jotbay = jotbay()?;
        let _ = jotbay.status(true);
        jotbay.about().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_settings() -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(Settings::load)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_settings(theme: String, verbose: bool) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Load and mutate rather than construct. Building a fresh Settings here
        // silently discarded every field this dialog does not show, vault_path
        // among them, which would have sent the app back to first-run setup the
        // next time it started.
        let mut settings = Settings::load();
        settings.theme = Theme::parse(&theme).unwrap_or_default();
        settings.verbose = verbose;
        settings.save().map_err(|e| e.to_string()).map(|_| settings)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_activity(refresh: bool, limit: usize) -> Result<Vec<ActivityEvent>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        jotbay()?.activity(refresh, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_nodes(refresh: bool) -> Result<Vec<NodeStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        jotbay()?.nodes(refresh).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn forget_node(hostname: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        jotbay()?.forget_node(&hostname).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn data_dir() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(jotbay()?.data_dir().to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn abort_rebase() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| jotbay()?.abort_rebase().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// The file browser's two calls. Read-only by construction. The commands to
/// write anything simply do not exist.
#[tauri::command]
async fn list_notes(path: String) -> Result<Vec<jotbay_core::browse::Entry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = jotbay()?.data_dir();
        jotbay_core::browse::list(&dir, &path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn read_note(path: String) -> Result<jotbay_core::browse::FileContent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = jotbay()?.data_dir();
        jotbay_core::browse::read(&dir, &path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn open_data_dir() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let dir = jotbay()?.data_dir();
        tauri_plugin_opener::open_path(dir.to_string_lossy().to_string(), None::<&str>)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Jotbay", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync Now", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open Jotbay Folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sync, &folder, &quit])?;

    TrayIconBuilder::with_id("jotbay-tray")
        // Replaced within one poll by whatever git actually says; starting on
        // the idle glyph just avoids a flash of the wrong colour.
        .icon(Image::from_bytes(TrayState::Idle.icon()).expect("tray icon"))
        .tooltip(TrayState::Idle.tooltip())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "sync" => {
                // Sync on a worker thread: doing git work on the UI thread
                // would freeze the tray menu for the duration of a push.
                let app = app.clone();
                std::thread::spawn(move || {
                    let _syncing = SyncGuard::new(&app);
                    if let Ok(v) = jotbay() {
                        let _ = v.sync();
                    }
                });
            }
            "folder" => {
                // Off the main thread for the same reason as "sync" above:
                // resolving the data dir spawns git.
                std::thread::spawn(|| {
                    if let Ok(v) = jotbay() {
                        let dir = v.data_dir().to_string_lossy().to_string();
                        let _ = tauri_plugin_opener::open_path(dir, None::<&str>);
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Without this, double-clicking the launcher while the app sits in the
        // tray - the natural way to "reopen" it - started a second full
        // instance with its own window and its own tray icon. Surface the
        // window that already exists instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            build_tray(app.handle())?;

            // Keep the glyph honest whether or not the window exists.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                update_tray(&handle);
                std::thread::sleep(TRAY_POLL);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window leaves the tray agent running, which is what
            // a background sync tool should do. Quit is explicit, from the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_setup_state,
            run_setup,
            create_shortcuts,
            get_status,
            do_sync,
            get_log,
            get_activity,
            do_upgrade,
            get_settings,
            set_settings,
            get_about,
            check_updates,
            get_nodes,
            forget_node,
            data_dir,
            list_notes,
            read_note,
            abort_rebase,
            open_data_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jotbay");
}
