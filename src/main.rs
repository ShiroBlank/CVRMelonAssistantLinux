mod models;
mod api;
mod install;
mod steam;
mod config;
mod ui;
mod melon_dll;

use gtk4::prelude::*;
use gtk4::Application;
use config::Config;
use glib;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Twemoji frog icon embedded at compile time — no runtime file dependency.
pub static APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// Global Tokio runtime — created once, used by all async operations.
/// reqwest requires a Tokio runtime; glib::spawn_future_local alone is not enough.
pub static TOKIO: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn main() {
    // Truncate log on each startup
    let log_path = Config::log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&log_path, "");

    log("INFO", &format!("=== CVR MelonLoader Assistant v{} ===", APP_VERSION));
    log("INFO", &format!("Log: {}", log_path.display()));

    // ── Create Tokio runtime ──────────────────────────────────────────────────
    log("INFO", "Creating Tokio runtime…");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");
    TOKIO.set(rt).expect("Runtime already set");
    log("INFO", "Tokio runtime ready");

    // ── Environment ───────────────────────────────────────────────────────────
    log("INFO", "--- Environment ---");
    for var in &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "GDK_BACKEND", "HOME"] {
        log("INFO", &format!("  {} = {}", var,
            std::env::var(var).unwrap_or_else(|_| "(not set)".into())));
    }

    // ── GTK runtime version ───────────────────────────────────────────────────
    log("INFO", &format!("GTK runtime: {}.{}.{}",
        gtk4::major_version(), gtk4::minor_version(), gtk4::micro_version()));

    // ── Config + Steam detection ──────────────────────────────────────────────
    let cfg = Config::load();
    log("INFO", &format!("Config: {}", Config::config_path().display()));
    log("INFO", &format!("Saved install folder: {:?}", cfg.install_folder));

    log("INFO", "Steam detection…");
    match steam::find_steam_install() {
        Some(dir) => log("INFO",  &format!("  Found CVR at: {}", dir.display())),
        None      => log("WARN",  "  CVR not found via Steam"),
    }

    // ── GTK Application ───────────────────────────────────────────────────────
    log("INFO", "Building GTK4 Application…");
    let app = Application::builder()
        .application_id("com.cvrmg.melon-assistant")
        .build();

    app.connect_activate(|a| {
        log("INFO", "activate() — building UI");
        ui::build_ui(a);
        log("INFO", "build_ui() returned");
    });

    log("INFO", "app.run()…");
    let exit = app.run_with_args(&std::env::args().collect::<Vec<_>>());
    log("INFO", &format!("app.run() exited: {:?}", exit));
}

/// Spawn an async task on the Tokio runtime and, when it completes, deliver
/// the result back to the GTK main thread via a glib idle callback.
///
/// The callback runs on the GTK main thread, so it is safe to touch GTK widgets
/// inside it. GTK widget handles must be wrapped in `SendWrapper` before being
/// captured by the callback closure — see usage in ui.rs.
pub fn spawn_async<F, T, C>(future: F, callback: C)
where
    F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
    C: FnOnce(anyhow::Result<T>) + 'static,
{
    let rt = TOKIO.get().expect("Tokio runtime not initialised");

    // Wrap the callback in a SendWrapper so it can be moved into the Tokio
    // thread. It will only ever be called inside glib::idle_add_once, which
    // runs on the GTK main thread — so the Send requirement is satisfied at
    // runtime even though GTK types aren't Send by default.
    let callback = send_wrapper::SendWrapper::new(callback);

    rt.spawn(async move {
        let result = future.await;
        glib::idle_add_once(move || {
            // Take the callback out of the wrapper and call it.
            // We are now on the GTK main thread.
            send_wrapper::SendWrapper::take(callback)(result);
        });
    });
}

/// Log to both stderr and the log file.
pub fn log(level: &str, msg: &str) {
    eprintln!("[{}] {}", level, msg);
    Config::log(msg, level);
}
