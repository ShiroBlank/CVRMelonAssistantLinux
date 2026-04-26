// ui.rs — GTK4 GUI for CVR MelonLoader Assistant (Linux)

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use gtk4::*;

use crate::api;
use crate::config::Config;
use crate::install;
use crate::models::Mod;
use crate::steam;
use crate::APP_VERSION;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct AppState {
    install_dir: Option<PathBuf>,
    /// All mods from the API (with installed_file_path / installed_version filled in)
    mods: Vec<Mod>,
    /// Category names that the user has collapsed
    collapsed_categories: HashSet<String>,
}

type SharedState = Arc<Mutex<AppState>>;

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn build_ui(app: &Application) {
    crate::log("INFO", "build_ui: start");
    let config = Config::load();
    crate::log("INFO", "build_ui: config loaded");
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));

    {
        let mut s = state.lock().unwrap();
        if let Some(dir) = &config.install_folder {
            let p = PathBuf::from(dir);
            if steam::is_valid_install_dir(&p) { s.install_dir = Some(p); }
        }
        if s.install_dir.is_none() { s.install_dir = steam::find_steam_install(); }
    }

    crate::log("INFO", "build_ui: creating window");
    let window = ApplicationWindow::builder()
        .application(app)
        .title(&format!("CVR MelonLoader Assistant v{} — Linux", APP_VERSION))
        .default_width(1150)
        .default_height(720)
        .build();

    // ── App icon — Wayland + X11 ──────────────────────────────────────────────
    // On Wayland the compositor resolves window icons via:
    //   1. A .desktop file (matched by app-id) that has Icon=<name>
    //   2. An icon named <name> in the XDG hicolor icon theme
    // GTK's set_icon_name() only works on X11. We must do both ourselves.
    {
        const APP_ID:   &str = "com.cvrmg.melon-assistant";
        const APP_NAME: &str = "CVR MelonLoader Assistant";

        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(
                std::env::var("HOME").unwrap_or_default() + "/.local/share"
            ));

        // ── 1. Write icon to hicolor theme ───────────────────────────────────
        let icon_path = data_dir
            .join("icons").join("hicolor").join("256x256").join("apps")
            .join(format!("{}.png", APP_ID));
        if let Some(p) = icon_path.parent() { let _ = std::fs::create_dir_all(p); }
        let _ = std::fs::write(&icon_path, crate::APP_ICON_PNG);

        // Also write a 48×48 copy (some compositors prefer it)
        let icon_path_48 = data_dir
            .join("icons").join("hicolor").join("48x48").join("apps")
            .join(format!("{}.png", APP_ID));
        if let Some(p) = icon_path_48.parent() { let _ = std::fs::create_dir_all(p); }
        let _ = std::fs::write(&icon_path_48, crate::APP_ICON_PNG);

        // ── 2. Write a .desktop file ─────────────────────────────────────────
        // Use the bare binary name for Exec= so it resolves via $PATH.
        // Using current_exe() would embed a transient path (AppImage mount,
        // cargo run path, etc.) which breaks after the first launch.
        let desktop_dir = data_dir.join("applications");
        let _ = std::fs::create_dir_all(&desktop_dir);
        let desktop_path = desktop_dir.join(format!("{}.desktop", APP_ID));
        let desktop_content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Exec=cvr-melon-assistant\n\
             Icon={}\n\
             Categories=Game;Utility;\n\
             StartupWMClass={}\n\
             StartupNotify=true\n",
            APP_NAME,
            APP_ID,
            APP_ID,
        );
        let _ = std::fs::write(&desktop_path, &desktop_content);

        // ── 3. Flush the icon theme cache ────────────────────────────────────
        // gtk-update-icon-cache tells GTK (and some compositors) about the new icon.
        let hicolor_dir = data_dir.join("icons").join("hicolor");
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .arg("-f").arg("-t").arg(&hicolor_dir)
            .spawn();
        // update-desktop-database refreshes the .desktop file index
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&desktop_dir)
            .spawn();

        // ── 4. GTK icon name (X11 fallback) ──────────────────────────────────
        if let Some(display) = gdk4::Display::default() {
            let theme = gtk4::IconTheme::for_display(&display);
            theme.add_search_path(data_dir.join("icons"));
        }
        window.set_icon_name(Some(APP_ID));
    }

    crate::log("INFO", "build_ui: applying CSS");
    let css = CssProvider::new();
    css.load_from_data(DARK_CSS);
    gtk4::style_context_add_provider_for_display(
        &gdk4::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let root = Box::new(Orientation::Vertical, 0);

    // ── Header ────────────────────────────────────────────────────────────────
    let header = build_header(&state);
    root.append(&header);

    // ── Notebook ──────────────────────────────────────────────────────────────
    let notebook = Notebook::new();
    notebook.set_vexpand(true);

    crate::log("INFO", "build_ui: building mods tab");
    let (mods_tab, mods_listbox, load_btn, update_btn, count_label) = build_mods_tab(&state);
    notebook.append_page(&mods_tab, Some(&Label::new(Some("  Mods  "))));

    let (ml_tab, ml_check_btn) = build_melon_loader_tab(&state);
    notebook.append_page(&ml_tab, Some(&Label::new(Some("  MelonLoader  "))));

    let (options_tab, dir_entry) = build_options_tab(&state, &window, &mods_listbox, &count_label);
    notebook.append_page(&options_tab, Some(&Label::new(Some("  Options  "))));

    let about_tab = build_about_tab();
    notebook.append_page(&about_tab, Some(&Label::new(Some("  About  "))));

    let (debug_tab, debug_scan_btn) = build_debug_tab(&state);
    notebook.append_page(&debug_tab, Some(&Label::new(Some("  Debug  "))));

    root.append(&notebook);

    // ── Auto-check MelonLoader status when the tab is first switched to ───────
    // Page index 1 = MelonLoader tab: auto-check on first visit.
    // Page index 4 = Debug tab: show intro dialog on first ever visit, then auto-scan.
    {
        use std::cell::Cell;
        use std::rc::Rc;
        let ml_checked  = Rc::new(Cell::new(false));
        let dbg_visited = Rc::new(Cell::new(false));
        let ml_check_btn_ref   = ml_check_btn.clone();
        let debug_scan_btn_ref = debug_scan_btn.clone();
        let state_ref = state.clone();
        notebook.connect_switch_page(move |_nb, _page, page_num| {
            let has_dir = state_ref.lock().unwrap().install_dir.is_some();

            if page_num == 1 && !ml_checked.get() && has_dir {
                ml_checked.set(true);
                ml_check_btn_ref.emit_clicked();
            }

            if page_num == 4 && !dbg_visited.get() && has_dir {
                dbg_visited.set(true);
                let already_shown = Config::load().debug_intro_shown;
                let scan_btn = debug_scan_btn_ref.clone();

                if already_shown {
                    // Already confirmed before — just auto-scan
                    scan_btn.emit_clicked();
                } else {
                    // First visit — show the intro dialog, scan only if confirmed
                    let dialog = AlertDialog::builder()
                        .message("Log Scanner")
                        .detail(
                            "The Debug tab reads your MelonLoader log file \
                             (MelonLoader/Latest.log inside your ChilloutVR folder) \
                             and analyses it for common issues such as:\n\n\
                             • Incompatible mods\n\
                             • Assembly load failures\n\
                             • Mods throwing runtime errors\n\
                             • Misplaced or duplicate mods\n\
                             • Missing dependencies\n\n\
                             The log is read locally — nothing is sent anywhere.\n\
                             The file is only generated after launching ChilloutVR \
                             with MelonLoader at least once."
                        )
                        .buttons(["Cancel", "Scan Log"])
                        .cancel_button(0)
                        .default_button(1)
                        .build();

                    dialog.choose(None::<&gtk4::Window>, None::<&gio::Cancellable>,
                        move |response| {
                            if response == Ok(1) {
                                // Mark as shown and proceed
                                let mut cfg = Config::load();
                                cfg.debug_intro_shown = true;
                                let _ = cfg.save();
                                scan_btn.emit_clicked();
                            }
                        }
                    );
                }
            }
        });
    }

    // ── Status bar ────────────────────────────────────────────────────────────
    let status_label = Label::new(Some("Ready — click 'Load Mods' to fetch the mod list."));
    status_label.set_halign(Align::Start);
    status_label.add_css_class("status-label");
    root.append(&status_label);

    crate::log("INFO", "build_ui: setting window child");
    window.set_child(Some(&root));

    // ── Wire options dir entry ────────────────────────────────────────────────
    wire_dir_entry(&dir_entry, &state);

    // ── Auto-load mod list if install dir is already known ────────────────────
    {
        let has_dir = state.lock().unwrap().install_dir.is_some();
        if has_dir {
            let lb = mods_listbox.clone();
            let cl = count_label.clone();
            let st = state.clone();
            let ub = update_btn.clone();
            load_btn.set_sensitive(false);
            load_btn.set_label("Loading…");
            cl.set_label("Fetching mod list from CVRMG…");
            let load_btn = load_btn.clone();
            crate::spawn_async(
                fetch_mod_data(st.clone()),
                move |result| {
                    match result {
                        Ok((mods, unverified, flags)) => {
                            let n = populate_mod_list(&lb, &cl, &st, mods, unverified, flags);
                            if n > 0 {
                                ub.set_label(&format!("⬆  Update Outdated ({})", n));
                                ub.add_css_class("update-badge");
                            }
                        }
                        Err(e) => {
                            cl.set_label("Failed to load mod list — check your connection.");
                            crate::log("ERROR", &format!("auto-load failed: {}", e));
                        }
                    }
                    load_btn.set_sensitive(true);
                    load_btn.set_label("🔄  Refresh Mod List");
                },
            );
        }
    }

    crate::log("INFO", "build_ui: calling window.present()");
    window.present();
    crate::log("INFO", "build_ui: window.present() returned");
}

// ── Header ────────────────────────────────────────────────────────────────────

fn build_header(state: &SharedState) -> Box {
    let hbox = Box::new(Orientation::Horizontal, 10);
    hbox.add_css_class("header-bar");

    let title = Label::new(Some("CVR MelonLoader Assistant"));
    title.add_css_class("header-title");
    title.set_hexpand(true);
    title.set_halign(Align::Start);
    hbox.append(&title);

    let dir_str = {
        let s = state.lock().unwrap();
        s.install_dir.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "⚠ No install directory found".to_string())
    };
    let dir_label = Label::new(Some(&dir_str));
    dir_label.add_css_class("dir-label");
    dir_label.set_ellipsize(pango::EllipsizeMode::Start);
    dir_label.set_max_width_chars(45);
    hbox.append(&dir_label);

    hbox
}

// ── Mods Tab ──────────────────────────────────────────────────────────────────

fn build_mods_tab(state: &SharedState) -> (Box, ListBox, Button, Button, Label) {
    let vbox = Box::new(Orientation::Vertical, 0);

    // ── Toolbar ──────────────────────────────────────────────────────────────
    let toolbar = Box::new(Orientation::Horizontal, 6);
    toolbar.add_css_class("toolbar");

    let load_btn     = Button::with_label("🔄  Refresh Mod List");
    let install_btn  = Button::with_label("⬇  Install Selected");
    let update_btn   = Button::with_label("⬆  Update Outdated");
    let uninstall_btn = Button::with_label("🗑  Uninstall Selected");
    let select_all   = Button::with_label("☑ All");
    let deselect_all = Button::with_label("☐ None");
    let search = SearchEntry::new();
    search.set_placeholder_text(Some("Search mods…"));
    search.set_hexpand(true);

    load_btn.add_css_class("action-button");
    install_btn.add_css_class("install-button");
    update_btn.add_css_class("update-button");
    uninstall_btn.add_css_class("danger-button");
    select_all.add_css_class("small-button");
    deselect_all.add_css_class("small-button");

    toolbar.append(&load_btn);
    toolbar.append(&install_btn);
    toolbar.append(&update_btn);
    toolbar.append(&uninstall_btn);
    toolbar.append(&select_all);
    toolbar.append(&deselect_all);
    toolbar.append(&search);
    vbox.append(&toolbar);

    // ── Column headers ────────────────────────────────────────────────────────
    let col_header = Box::new(Orientation::Horizontal, 0);
    col_header.add_css_class("col-header");
    for (text, expand, width) in &[
        ("", false, 28i32),
        ("", false, 20i32),
        ("Name / Description", true, -1),
        ("Author", false, 140),
        ("Latest", false, 90),
        ("Installed", false, 90),
        ("Status", false, 90),
    ] {
        let lbl = Label::new(Some(text));
        lbl.add_css_class("col-header-label");
        lbl.set_hexpand(*expand);
        if !expand { lbl.set_width_chars(*width / 8); }
        col_header.append(&lbl);
    }
    vbox.append(&col_header);

    // ── List ──────────────────────────────────────────────────────────────────
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    let listbox = ListBox::new();
    listbox.add_css_class("mods-listbox");
    listbox.set_selection_mode(SelectionMode::None);
    scrolled.set_child(Some(&listbox));
    vbox.append(&scrolled);

    // ── Bottom bar ────────────────────────────────────────────────────────────
    let bottom = Box::new(Orientation::Horizontal, 10);
    bottom.add_css_class("bottom-bar");
    let count_label = Label::new(Some("No mods loaded"));
    count_label.set_hexpand(true);
    count_label.set_halign(Align::End);
    bottom.append(&count_label);
    vbox.append(&bottom);

    // ── Wire: Load ────────────────────────────────────────────────────────────
    {
        let listbox = listbox.clone();
        let count_label = count_label.clone();
        let state = state.clone();
        let update_btn = update_btn.clone();
        let btn = load_btn.clone();
        btn.connect_clicked(move |b| {
            // Guard: require an install directory before hitting the network
            {
                let s = state.lock().unwrap();
                if s.install_dir.is_none() {
                    show_error(
                        "No install directory set",
                        "Please go to the Options tab and set your ChilloutVR install path before loading mods.",
                    );
                    return;
                }
            }
            b.set_sensitive(false);
            b.set_label("Loading…");
            let lb = listbox.clone();
            let cl = count_label.clone();
            let st = state.clone();
            let ub = update_btn.clone();
            let bc = b.clone();
            crate::spawn_async(
                fetch_mod_data(st.clone()),
                move |result| {
                    match result {
                        Ok((mods, unverified, flags)) => {
                            let n = populate_mod_list(&lb, &cl, &st, mods, unverified, flags);
                            if n > 0 {
                                ub.set_label(&format!("⬆  Update Outdated ({})", n));
                                ub.add_css_class("update-badge");
                            } else {
                                ub.set_label("⬆  Update Outdated");
                                ub.remove_css_class("update-badge");
                            }
                        }
                        Err(e) => show_error("Failed to load mods", &e.to_string()),
                    }
                    bc.set_sensitive(true);
                    bc.set_label("🔄  Refresh Mod List");
                },
            );
        });
    }

    // ── Wire: Install Selected ────────────────────────────────────────────────
    {
        let listbox = listbox.clone();
        let state = state.clone();
        let count_label = count_label.clone();
        let update_btn = update_btn.clone();
        install_btn.connect_clicked(move |b| {
            let checked = collect_checked_mod_ids(&listbox);
            if checked.is_empty() {
                show_error("Nothing selected", "Check the boxes next to mods you want to install.");
                return;
            }
            b.set_sensitive(false);
            b.set_label("Installing…");
            let st = state.clone();
            let lb = listbox.clone();
            let cl = count_label.clone();
            let ub = update_btn.clone();
            let bc = b.clone();
            let dir = match st.lock().unwrap().install_dir.clone() {
                Some(d) => d,
                None => {
                    show_error("No install directory", "Set your ChilloutVR path in Options.");
                    bc.set_sensitive(true);
                    bc.set_label("⬇  Install Selected");
                    return;
                }
            };
            let mods_snapshot = st.lock().unwrap().mods.clone();
            let all_mods = mods_snapshot.clone();
            let targets: Vec<Mod> = mods_snapshot.into_iter()
                .filter(|m| checked.contains(&m._id))
                .filter(|m| !m.is_unverified)  // never auto-install unverified mods
                .collect();
            crate::spawn_async(
                async move {
                    let mut ok = 0usize;
                    let mut fail = 0usize;
                    for m in &targets {
                        match install::install_mod(m, &dir, &all_mods).await {
                            Ok(_)  => ok += 1,
                            Err(_) => fail += 1,
                        }
                    }
                    Ok((ok, fail, dir))
                },
                move |result| {
                    match result {
                        Ok((ok, fail, _dir)) => {
                            crate::spawn_async(
                                fetch_mod_data(st.clone()),
                                move |r| {
                                    if let Ok((mods, unverified, flags)) = r {
                                        let n = populate_mod_list(&lb, &cl, &st, mods, unverified, flags);
                                        if n > 0 { ub.set_label(&format!("⬆  Update Outdated ({})", n)); }
                                        else     { ub.set_label("⬆  Update Outdated"); }
                                    }
                                    show_info("Install complete", &format!("{} installed, {} failed.", ok, fail));
                                    bc.set_sensitive(true);
                                    bc.set_label("⬇  Install Selected");
                                },
                            );
                        }
                        Err(e) => {
                            show_error("Install failed", &e.to_string());
                            bc.set_sensitive(true);
                            bc.set_label("⬇  Install Selected");
                        }
                    }
                },
            );
        });
    }

    // ── Wire: Update Outdated ─────────────────────────────────────────────────
    {
        let listbox = listbox.clone();
        let state = state.clone();
        let count_label = count_label.clone();
        let update_btn_ref = update_btn.clone();
        update_btn.connect_clicked(move |b| {
            b.set_sensitive(false);
            b.set_label("Updating…");
            let st = state.clone();
            let lb = listbox.clone();
            let cl = count_label.clone();
            let ub = update_btn_ref.clone();
            let bc = b.clone();
            let dir = match st.lock().unwrap().install_dir.clone() {
                Some(d) => d,
                None => {
                    show_error("No install directory", "Set your ChilloutVR path in Options.");
                    bc.set_sensitive(true);
                    bc.set_label("⬆  Update Outdated");
                    return;
                }
            };
            let mods_snapshot = st.lock().unwrap().mods.clone();
            let all_mods = mods_snapshot.clone();
            // Step 1: quarantine broken AND retired mods before updating
            let quarantined_broken  = install::quarantine_broken_mods(&mods_snapshot, &dir);
            let quarantined_retired = install::quarantine_retired_mods(&mods_snapshot, &dir);
            // Step 2: collect mods that need updating — skip unverified, broken, and retired
            let outdated: Vec<Mod> = mods_snapshot.into_iter()
                .filter(|m| install::mod_has_update(m))
                .filter(|m| !m.is_unverified)
                .filter(|m| m.versions.first().map(|v| !v.is_broken() && !v.is_retired()).unwrap_or(true))
                .collect();
            crate::spawn_async(
                async move {
                    let mut updated = 0usize;
                    let mut failed  = 0usize;
                    for m in &outdated {
                        match install::install_mod(m, &dir, &all_mods).await {
                            Ok(_)  => updated += 1,
                            Err(_) => failed  += 1,
                        }
                    }
                    Ok((updated, failed, quarantined_broken, quarantined_retired))
                },
                move |result| {
                    match result {
                        Ok((updated, failed, quarantined_broken, quarantined_retired)) => {
                            crate::spawn_async(
                                fetch_mod_data(st.clone()),
                                move |r| {
                                    if let Ok((mods, unverified, flags)) = r {
                                        let n = populate_mod_list(&lb, &cl, &st, mods, unverified, flags);
                                        if n > 0 { ub.set_label(&format!("⬆  Update Outdated ({})", n)); }
                                        else     { ub.set_label("⬆  Update Outdated"); }
                                    }
                                    let mut summary = String::new();
                                    if updated > 0 || failed > 0 {
                                        summary.push_str(&format!("{} mod(s) updated", updated));
                                        if failed > 0 { summary.push_str(&format!(", {} failed", failed)); }
                                        summary.push('.');
                                    }
                                    if !quarantined_broken.is_empty() {
                                        if !summary.is_empty() { summary.push('\n'); }
                                        summary.push_str(&format!(
                                            "\n⚠ {} mod(s) moved to Mods/Broken/ (marked broken by CVRMG):\n",
                                            quarantined_broken.len()
                                        ));
                                        for q in &quarantined_broken {
                                            summary.push_str(&format!("  • {}\n", q.mod_name));
                                        }
                                    }
                                    if !quarantined_retired.is_empty() {
                                        if !summary.is_empty() { summary.push('\n'); }
                                        summary.push_str(&format!(
                                            "\n📦 {} mod(s) moved to Mods/Retired/ (retired by CVRMG):\n",
                                            quarantined_retired.len()
                                        ));
                                        for q in &quarantined_retired {
                                            summary.push_str(&format!("  • {}\n", q.mod_name));
                                        }
                                    }
                                    if summary.is_empty() {
                                        summary = "All installed mods are already up to date!".into();
                                    }
                                    show_info("Update complete", &summary);
                                    bc.set_sensitive(true);
                                    bc.set_label("⬆  Update Outdated");
                                },
                            );
                        }
                        Err(e) => {
                            show_error("Update failed", &e.to_string());
                            bc.set_sensitive(true);
                            bc.set_label("⬆  Update Outdated");
                        }
                    }
                },
            );
        });
    }

    // ── Wire: Uninstall Selected ──────────────────────────────────────────────
    {
        let listbox = listbox.clone();
        let state = state.clone();
        let count_label = count_label.clone();
        let update_btn = update_btn.clone();
        uninstall_btn.connect_clicked(move |b| {
            let checked = collect_checked_mod_ids(&listbox);
            if checked.is_empty() {
                show_error("Nothing selected", "Check the boxes next to mods you want to uninstall.");
                return;
            }

            // Build mod names for the confirmation message
            let mod_names: Vec<String> = {
                let s = state.lock().unwrap();
                s.mods.iter()
                    .filter(|m| checked.contains(&m._id) && m.installed_file_path.is_some())
                    .map(|m| m.versions.first()
                        .map(|v| v.name.clone())
                        .unwrap_or_else(|| "Unknown".into()))
                    .collect()
            };

            let confirm = Config::load().confirm_uninstall;

            // Clone everything needed by both the immediate path and the dialog callback
            let do_uninstall = {
                let state = state.clone();
                let listbox = listbox.clone();
                let count_label = count_label.clone();
                let update_btn = update_btn.clone();
                let b = b.clone();
                let checked = checked.clone();
                move || {
                    b.set_sensitive(false);
                    let st = state.clone();
                    let lb = listbox.clone();
                    let cl = count_label.clone();
                    let ub = update_btn.clone();
                    let bc = b.clone();
                    let mods_snapshot = st.lock().unwrap().mods.clone();
                    let mut ok = 0usize;
                    for m in &mods_snapshot {
                        if checked.contains(&m._id) {
                            if let Some(path) = &m.installed_file_path {
                                if install::uninstall_mod(path).is_ok() { ok += 1; }
                            }
                        }
                    }
                    crate::spawn_async(
                        fetch_mod_data(st.clone()),
                        move |result| {
                            if let Ok((mods, unverified, flags)) = result {
                                let n = populate_mod_list(&lb, &cl, &st, mods, unverified, flags);
                                if n > 0 { ub.set_label(&format!("⬆  Update Outdated ({})", n)); }
                                else     { ub.set_label("⬆  Update Outdated"); }
                            }
                            show_info("Uninstall complete", &format!("{} mod(s) removed.", ok));
                            bc.set_sensitive(true);
                            bc.set_label("🗑  Uninstall Selected");
                        },
                    );
                }
            };

            if !confirm {
                do_uninstall();
                return;
            }

            // Build confirmation message listing the mods to be removed
            let detail = if mod_names.is_empty() {
                "This will permanently delete the selected mod files.".to_string()
            } else {
                format!(
                    "This will permanently delete {} mod file(s):\n\n{}\n\nThis cannot be undone.",
                    mod_names.len(),
                    mod_names.iter().map(|n| format!("  • {}", n)).collect::<Vec<_>>().join("\n")
                )
            };

            // gtk4 0.7 AlertDialog uses button arrays indexed by position.
            // Index 0 = "Cancel", index 1 = "Uninstall"
            let dialog = AlertDialog::builder()
                .message("Uninstall mods?")
                .detail(&detail)
                .buttons(["Cancel", "Uninstall"])
                .cancel_button(0)
                .default_button(0)
                .build();

            dialog.choose(None::<&gtk4::Window>, None::<&gio::Cancellable>, move |response| {
                // Button index 1 = "Uninstall"
                if response == Ok(1) {
                    do_uninstall();
                }
            });
        });
    }

    // ── Wire: Select / Deselect All ───────────────────────────────────────────
    {
        let lb = listbox.clone();
        select_all.connect_clicked(move |_| set_all_checks(&lb, true));
    }
    {
        let lb = listbox.clone();
        deselect_all.connect_clicked(move |_| set_all_checks(&lb, false));
    }

    // ── Wire: Search ──────────────────────────────────────────────────────────
    {
        let lb = listbox.clone();
        let state_ref = state.clone();
        search.connect_search_changed(move |entry| {
            let q = entry.text().to_lowercase();
            let collapsed = {
                let s = state_ref.lock().unwrap();
                s.collapsed_categories.clone()
            };
            let mut row = lb.first_child();
            while let Some(r) = row {
                let next = r.next_sibling();
                let wname = r.widget_name().to_string();
                if wname.starts_with("cat:") {
                    // Category headers always visible
                    r.set_visible(true);
                } else if wname.starts_with("mod:") {
                    // widget name: "mod:CATKEY|searchable text"
                    let after_prefix = &wname["mod:".len()..];
                    let (cat_key, search_text) = after_prefix
                        .split_once('|')
                        .unwrap_or((after_prefix, after_prefix));
                    if q.is_empty() {
                        r.set_visible(!collapsed.contains(cat_key));
                    } else {
                        r.set_visible(search_text.contains(&q));
                    }
                } else {
                    r.set_visible(q.is_empty() || wname.contains(&q));
                }
                row = next;
            }
        });
    }

    (vbox, listbox, load_btn, update_btn, count_label)
}

// ── Mod data fetching (runs on Tokio thread, no GTK) ─────────────────────────

/// Fetch mods + flags from the API. Safe to run on the Tokio thread pool.
async fn fetch_mod_data(state: SharedState)
    -> anyhow::Result<(Vec<Mod>, Vec<Mod>, Vec<crate::models::FlagEntry>)>
// returns (official_mods, unverified_mods, flags)
{
    let mods  = api::fetch_mods().await?;
    let flags = api::fetch_flags().await.unwrap_or_default();

    // Scan installed mod files from disk — this now returns MelonInfo too
    let installed = {
        let s = state.lock().unwrap();
        s.install_dir.clone()
            .map(|d| install::scan_installed_mods(&d))
            .unwrap_or_default()
    };

    crate::log("INFO", &format!("fetch_mod_data: {} API mods, {} installed files", mods.len(), installed.len()));

    let flag_map: HashMap<i64, i32> = flags.iter().map(|f| (f._id, f.flag)).collect();

    // Build fast-lookup maps from the installed files
    // key: lowercase mod name from MelonInfoAttribute → (path, installed_version)
    let mut installed_by_melon_name: HashMap<String, (PathBuf, String)> = HashMap::new();
    // key: lowercase filename stem → (path, installed_version_or_empty)
    let mut installed_by_filename: HashMap<String, (PathBuf, String)> = HashMap::new();
    // key: md5 hash → path (last-resort fallback)
    let mut installed_by_hash: HashMap<String, PathBuf> = HashMap::new();

    for (path, hash, melon_info) in &installed {
        installed_by_hash.insert(hash.to_lowercase(), path.clone());

        let filename_stem = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        if let Some(info) = melon_info {
            installed_by_melon_name.insert(
                info.name.to_lowercase(),
                (path.clone(), info.version.clone()),
            );
            // Also index under the filename in case name differs
            installed_by_filename.insert(filename_stem, (path.clone(), info.version.clone()));
        } else {
            // No MelonInfo — just index by filename with unknown version
            installed_by_filename.insert(filename_stem, (path.clone(), String::new()));
        }
    }

    let mut mods = mods;
    let mut matched = 0usize;

    for m in &mut mods {
        m.flag = flag_map.get(&m._id).copied().unwrap_or(0);

        // Try each version entry (newest first, which is how the API returns them)
        'outer: for ver in &m.versions {
            let api_name_lower = ver.name.to_lowercase();

            // ── Strategy 1: MelonInfo name matches the version's name (primary — same as original) ──
            if let Some((path, inst_ver)) = installed_by_melon_name.get(&api_name_lower) {
                m.installed_file_path = Some(path.display().to_string());
                m.installed_version   = Some(inst_ver.clone());
                crate::log("INFO", &format!("  matched '{}' by MelonInfo name, installed={:?} latest={:?}",
                    ver.name, inst_ver, ver.mod_version));
                matched += 1;
                break 'outer;
            }

            // ── Strategy 2: MelonInfo name matches one of the mod's aliases ──
            for alias in &m.aliases {
                if let Some((path, inst_ver)) = installed_by_melon_name.get(&alias.to_lowercase()) {
                    m.installed_file_path = Some(path.display().to_string());
                    m.installed_version   = Some(inst_ver.clone());
                    crate::log("INFO", &format!("  matched '{}' by alias '{}', installed={:?}",
                        ver.name, alias, inst_ver));
                    matched += 1;
                    break 'outer;
                }
            }

            // ── Strategy 3: filename from download URL ────────────────────────
            if let Some(url) = &ver.download_link {
                let expected_stem = url.split('/').last().unwrap_or("")
                    .trim_end_matches(".dll").to_lowercase();
                if !expected_stem.is_empty() {
                    if let Some((path, inst_ver)) = installed_by_filename.get(&expected_stem) {
                        m.installed_file_path = Some(path.display().to_string());
                        m.installed_version   = Some(inst_ver.clone());
                        crate::log("INFO", &format!("  matched '{}' by filename stem '{}', installed={:?}",
                            ver.name, expected_stem, inst_ver));
                        matched += 1;
                        break 'outer;
                    }
                }
            }

            // ── Strategy 4: API hash match ────────────────────────────────────
            if let Some(api_hash) = &ver.hash {
                let h = api_hash.trim().to_lowercase();
                if !h.is_empty() {
                    if let Some(path) = installed_by_hash.get(&h) {
                        // Hash matched the *latest* version — so it's up to date
                        m.installed_file_path = Some(path.display().to_string());
                        m.installed_version   = ver.mod_version.clone();
                        crate::log("INFO", &format!("  matched '{}' by hash, up to date", ver.name));
                        matched += 1;
                        break 'outer;
                    }
                }
            }
        }
    }

    crate::log("INFO", &format!("fetch_mod_data: matched {}/{} installed files", matched, installed.len()));

    // ── Collect unverified mods: installed files not matched to any API entry ──
    // Build a set of all matched paths
    let matched_paths: std::collections::HashSet<String> = mods.iter()
        .filter_map(|m| m.installed_file_path.as_deref())
        .map(|s| s.to_lowercase())
        .collect();

    let mut unverified: Vec<crate::models::Mod> = Vec::new();
    for (path, _hash, melon_info) in &installed {
        let path_str = path.display().to_string();
        if matched_paths.contains(&path_str.to_lowercase()) { continue; }

        // Build a synthetic Mod entry for this unrecognised file
        let (name, version, author) = if let Some(info) = melon_info {
            (info.name.clone(), info.version.clone(), info.author.clone())
        } else {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_string();
            (stem, String::new(), String::new())
        };

        let syn_version = crate::models::ModVersion {
            _version: 0,
            name: name.clone(),
            mod_version: if version.is_empty() { None } else { Some(version.clone()) },
            mod_type: None,
            author: if author.is_empty() { None } else { Some(author) },
            description: Some("Not listed in the CVRMG mod repository.".into()),
            download_link: None,
            source_link: None,
            hash: None,
            update_date: None,
            chillout_vr_version: None,
            loader_version: None,
            approval_status: 0,
        };

        let syn_mod = crate::models::Mod {
            _id: 0,
            upload_date: None,
            category: Some("__unverified__".into()),
            aliases: Vec::new(),
            versions: vec![syn_version],
            installed_file_path: Some(path_str),
            installed_version: if version.is_empty() { None } else { Some(version) },
            installed_in_broken_dir: false,
            installed_in_retired_dir: false,
            flag: 0,
            is_unverified: true,
        };
        unverified.push(syn_mod);
        crate::log("INFO", &format!("  unverified: {} ({})", name, path.display()));
    }

    crate::log("INFO", &format!("fetch_mod_data: {} unverified mods", unverified.len()));
    Ok((mods, unverified, flags))
}

// ── GTK list population (runs on main thread) ─────────────────────────────────

/// Re-render the mod list using data already in AppState — no network call.
/// Used when display settings change (e.g. grouping or quarantine visibility toggle).
fn repopulate_from_state(listbox: &ListBox, count_label: &Label, state: &SharedState) {
    let (mods, unverified) = {
        let s = state.lock().unwrap();
        // mods are the official ones; unverified were display-only, not stored in state.
        // We can only re-render official mods — unverified will be absent until next refresh.
        (s.mods.clone(), Vec::new())
    };
    if mods.is_empty() { return; } // nothing loaded yet — don't blank the list
    populate_mod_list(listbox, count_label, state, mods, unverified, Vec::new());
}


/// Must be called on the GTK main thread.
fn populate_mod_list(
    listbox: &ListBox,
    count_label: &Label,
    state: &SharedState,
    mods: Vec<Mod>,
    unverified: Vec<Mod>,
    _flags: Vec<crate::models::FlagEntry>,
) -> usize {
    // Save into shared state — only official mods go here
    {
        let mut s = state.lock().unwrap();
        s.mods = mods.clone();
    }

    // Read display preferences
    let cfg = Config::load();
    let group_broken_retired  = cfg.show_broken_retired_category;
    let show_quarantined      = cfg.show_quarantined_mods;

    // Helper: is a mod's installed file in a quarantine dir?
    let is_quarantined = |m: &Mod| -> bool {
        m.installed_file_path.as_deref()
            .map(|p| { let l = p.to_lowercase(); l.contains("/broken/") || l.contains("/retired/") })
            .unwrap_or(false)
    };

    let n_updates = mods.iter()
        .filter(|m| m.installed_file_path.is_some() && install::mod_has_update(m))
        .count();

    let total           = mods.len();
    // Installed count: quarantined mods only count if the option to show them is on
    let installed_count = mods.iter()
        .filter(|m| m.installed_file_path.is_some())
        .filter(|m| show_quarantined || !is_quarantined(m))
        .count()
        + unverified.len();

    // Partition mods: normal vs broken/retired (for grouping option)
    let (broken_retired, normal): (Vec<&Mod>, Vec<&Mod>) = if group_broken_retired {
        mods.iter().partition(|m| {
            m.versions.first().map(|v| v.is_broken() || v.is_retired()).unwrap_or(false)
        })
    } else {
        (Vec::new(), mods.iter().collect())
    };

    // Rebuild list rows
    while let Some(child) = listbox.first_child() { listbox.remove(&child); }

    let collapsed = {
        let s = state.lock().unwrap();
        s.collapsed_categories.clone()
    };

    // Helper to append a category header and wire its toggle
    let append_category = |lb: &ListBox, st: &SharedState, cat_name: &str| {
        let is_collapsed = collapsed.contains(cat_name);
        let row = make_category_row(cat_name, is_collapsed);
        let cat_key = cat_name.to_string();
        let st2 = st.clone();
        let lb2 = lb.clone();
        // GestureClick fires reliably on mouse clicks; connect_activate only
        // fires for keyboard activation in GTK4's ListBox.
        let gesture = gtk4::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            toggle_category(&lb2, &cat_key, &st2);
        });
        row.add_controller(gesture);
        lb.append(&row);
    };

    // ── Normal mods by category ───────────────────────────────────────────────
    let mut current_cat = String::new();
    for m in &normal {
        if !show_quarantined && is_quarantined(m) { continue; }

        let cat = m.display_category();
        if cat != current_cat {
            current_cat = cat.clone();
            append_category(listbox, state, &cat);
        }
        let mod_row = make_mod_row(m);
        // Tag with category so the toggle handler can find it
        // Replace the __pending__ placeholder with the real category key
        {
            let existing = mod_row.widget_name().to_string();
            let new_name = existing.replacen("mod:__pending__", &format!("mod:{}", current_cat.to_lowercase()), 1);
            mod_row.set_widget_name(&new_name);
        }
        if collapsed.contains(&current_cat) {
            mod_row.set_visible(false);
        }
        listbox.append(&mod_row);
    }

    // ── Broken / Retired category ─────────────────────────────────────────────
    if !broken_retired.is_empty() {
        let cat_name = "Broken / Retired";
        append_category(listbox, state, cat_name);
        for m in &broken_retired {
            if !show_quarantined && is_quarantined(m) { continue; }
            let mod_row = make_mod_row(m);
            {
                let existing = mod_row.widget_name().to_string();
                let new_name = existing.replacen("mod:__pending__", &format!("mod:{}", cat_name.to_lowercase()), 1);
                mod_row.set_widget_name(&new_name);
            }
            if collapsed.contains(cat_name) { mod_row.set_visible(false); }
            listbox.append(&mod_row);
        }
    }

    // ── Quarantined files ─────────────────────────────────────────────────────
    if show_quarantined {
        let quarantined_unmatched: Vec<&Mod> = unverified.iter()
            .filter(|m| is_quarantined(m))
            .collect();
        if !quarantined_unmatched.is_empty() {
            let cat_name = "Quarantined (Not in CVRMG)";
            append_category(listbox, state, cat_name);
            for m in quarantined_unmatched {
                let mod_row = make_mod_row(m);
                {
                let existing = mod_row.widget_name().to_string();
                let new_name = existing.replacen("mod:__pending__", &format!("mod:{}", cat_name.to_lowercase()), 1);
                mod_row.set_widget_name(&new_name);
            }
                if collapsed.contains(cat_name) { mod_row.set_visible(false); }
                listbox.append(&mod_row);
            }
        }
    }

    // ── Unverified mods ───────────────────────────────────────────────────────
    let unverified_active: Vec<&Mod> = unverified.iter()
        .filter(|m| !is_quarantined(m))
        .collect();
    if !unverified_active.is_empty() {
        let cat_name = "User Installed — NOT VERIFIED BY CVRMG STAFF";
        append_category(listbox, state, cat_name);
        for m in unverified_active {
            let mod_row = make_mod_row(m);
            {
                let existing = mod_row.widget_name().to_string();
                let new_name = existing.replacen("mod:__pending__", &format!("mod:{}", cat_name.to_lowercase()), 1);
                mod_row.set_widget_name(&new_name);
            }
            if collapsed.contains(cat_name) { mod_row.set_visible(false); }
            listbox.append(&mod_row);
        }
    }

    count_label.set_label(&format!(
        "{} mods  •  {} installed  •  {} update(s) available",
        total, installed_count, n_updates
    ));

    n_updates
}

// ── Category collapse toggle ──────────────────────────────────────────────────

/// Toggle the collapsed state of a category in the listbox.
/// Walks all rows, hides/shows those tagged with `"mod:<cat_key>"`,
/// and flips the chevron on the category header row.
fn toggle_category(listbox: &ListBox, cat_name: &str, state: &SharedState) {
    // Update the collapsed set in state
    let now_collapsed = {
        let mut s = state.lock().unwrap();
        if s.collapsed_categories.contains(cat_name) {
            s.collapsed_categories.remove(cat_name);
            false
        } else {
            s.collapsed_categories.insert(cat_name.to_string());
            true
        }
    };

    let mod_tag  = format!("mod:{}", cat_name.to_lowercase());
    let cat_tag  = format!("cat:{}", cat_name.to_lowercase());

    let mut row = listbox.first_child();
    while let Some(r) = row {
        let next = r.next_sibling();
        let name = r.widget_name().to_string();

        if name.starts_with(&mod_tag) && (name.len() == mod_tag.len() || name.as_bytes().get(mod_tag.len()) == Some(&b'|')) {
            r.set_visible(!now_collapsed);
        } else if name == cat_tag {
            // Update chevron label inside the category header
            if let Some(child) = r.first_child() {  // the hbox
                let mut hchild = child.first_child();
                while let Some(w) = hchild {
                    let wnext = w.next_sibling();
                    if w.widget_name() == "chevron" {
                        if let Some(lbl) = w.downcast_ref::<Label>() {
                            lbl.set_text(if now_collapsed { "▶" } else { "▼" });
                        }
                    }
                    hchild = wnext;
                }
            }
        }

        row = next;
    }
}

// ── Row builders ──────────────────────────────────────────────────────────────

fn make_category_row(name: &str, collapsed: bool) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(true); // clickable to toggle
    row.add_css_class("category-row");
    // Store category name in widget-name for the toggle handler
    row.set_widget_name(&format!("cat:{}", name.to_lowercase()));

    let hbox = Box::new(Orientation::Horizontal, 6);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(3);
    hbox.set_margin_bottom(3);

    // Chevron indicator
    let chevron = Label::new(Some(if collapsed { "▶" } else { "▼" }));
    chevron.set_widget_name("chevron");
    chevron.add_css_class("category-chevron");
    hbox.append(&chevron);

    let lbl = Label::new(Some(name));
    lbl.set_halign(Align::Start);
    lbl.set_hexpand(true);
    if name.contains("NOT VERIFIED") {
        lbl.add_css_class("category-label-unverified");
    } else if name == "Broken / Retired" || name.contains("Quarantined") {
        lbl.add_css_class("category-label-broken-retired");
    } else {
        lbl.add_css_class("category-label");
    }
    hbox.append(&lbl);

    row.set_child(Some(&hbox));
    row
}

fn make_mod_row(m: &Mod) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.add_css_class("mod-row");

    let ver = m.versions.first();
    let is_installed  = m.installed_file_path.is_some();
    let has_update    = is_installed && (install::mod_has_update(m) || install::mod_version_outdated(m));

    if has_update   { row.add_css_class("mod-has-update"); }
    if is_installed { row.add_css_class("mod-installed"); }
    if m.is_unverified { row.add_css_class("unverified-row"); }
    if ver.map(|v| v.is_broken() || v.is_retired()).unwrap_or(false) {
        row.add_css_class("broken-retired-row");
    }

    let hbox = Box::new(Orientation::Horizontal, 6);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(5);
    hbox.set_margin_bottom(5);

    // Checkbox (pre-checked if installed)
    let check = CheckButton::new();
    check.set_active(is_installed);
    check.set_widget_name(&m._id.to_string());
    hbox.append(&check);

    // Flag symbol
    let flag_lbl = Label::new(Some(api::flag_symbol(m.flag)));
    flag_lbl.set_width_chars(2);
    hbox.append(&flag_lbl);

    // Name + description
    let name_box = Box::new(Orientation::Vertical, 1);
    name_box.set_hexpand(true);
    let name_str = ver.map(|v| v.name.as_str()).unwrap_or("?");
    let name_lbl = Label::new(Some(name_str));
    name_lbl.set_halign(Align::Start);
    name_lbl.add_css_class("mod-name");
    name_box.append(&name_lbl);
    if let Some(desc) = ver.and_then(|v| v.description.as_deref()).filter(|d| !d.is_empty()) {
        let d = Label::new(Some(desc));
        d.set_halign(Align::Start);
        d.set_ellipsize(pango::EllipsizeMode::End);
        d.set_max_width_chars(55);
        d.add_css_class("mod-desc");
        name_box.append(&d);
    }
    hbox.append(&name_box);

    // Author
    let author = ver.and_then(|v| v.author.as_deref()).unwrap_or("—");
    let author_lbl = Label::new(Some(author));
    author_lbl.set_width_chars(16);
    author_lbl.set_ellipsize(pango::EllipsizeMode::End);
    author_lbl.add_css_class("mod-meta");
    hbox.append(&author_lbl);

    // Latest version (from API)
    let latest = ver.and_then(|v| v.mod_version.as_deref()).unwrap_or("?");
    let latest_lbl = Label::new(Some(latest));
    latest_lbl.set_width_chars(10);
    latest_lbl.add_css_class("mod-meta");
    hbox.append(&latest_lbl);

    // Installed version
    let inst_str = m.installed_version.as_deref().unwrap_or("—");
    let inst_lbl = Label::new(Some(inst_str));
    inst_lbl.set_width_chars(10);
    if is_installed { inst_lbl.add_css_class("installed-ver"); }
    hbox.append(&inst_lbl);

    // Status badge
    let path_lower = m.installed_file_path.as_deref()
        .map(|p| p.to_lowercase())
        .unwrap_or_default();
    let is_in_broken_dir  = path_lower.contains("/broken/");
    let is_in_retired_dir = path_lower.contains("/retired/");

    let status_str = if is_in_broken_dir {
        "⚠ BROKEN DIR"
    } else if is_in_retired_dir {
        "📦 RETIRED DIR"
    } else if is_installed && ver.map(|v| v.is_broken()).unwrap_or(false) {
        "→ QUARANTINE"
    } else if is_installed && ver.map(|v| v.is_retired()).unwrap_or(false) {
        "→ RETIRE"
    } else if has_update {
        "⬆ UPDATE"
    } else if is_installed {
        "✓ OK"
    } else if ver.map(|v| v.is_broken()).unwrap_or(false) {
        "⚠ BROKEN"
    } else if ver.map(|v| v.is_retired()).unwrap_or(false) {
        "● RETIRED"
    } else {
        ""
    };
    let status_lbl = Label::new(Some(status_str));
    status_lbl.set_width_chars(12);
    if status_str.contains("BROKEN DIR")  { status_lbl.add_css_class("badge-broken"); }
    else if status_str.contains("RETIRED DIR") { status_lbl.add_css_class("badge-retired"); }
    else if status_str.contains("QUARANTINE") || status_str.contains("→ RETIRE") {
                                            status_lbl.add_css_class("badge-quarantine"); }
    else if status_str.contains("BROKEN") { status_lbl.add_css_class("badge-broken"); }
    else if has_update                    { status_lbl.add_css_class("badge-update"); }
    else if is_installed                  { status_lbl.add_css_class("badge-ok"); }
    else if status_str.contains("RETIRED"){ status_lbl.add_css_class("badge-retired"); }
    hbox.append(&status_lbl);

    row.set_child(Some(&hbox));

    // Widget name encodes "mod:CATEGORY_KEY|SEARCHABLE_TEXT"
    // populated_mod_list overwrites the category part after construction
    row.set_widget_name(&format!("mod:__pending__|{} {} {}",
        ver.map(|v| v.name.as_str()).unwrap_or(""),
        ver.and_then(|v| v.description.as_deref()).unwrap_or(""),
        m.display_category()
    ).to_lowercase());

    row
}

// ── MelonLoader Tab ────────────────────────────────────────────────────────────

fn build_melon_loader_tab(state: &SharedState)
    -> (Box, Button)
{
    let vbox = Box::new(Orientation::Vertical, 12);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(24);
    vbox.set_margin_end(24);

    // Simple toast label — shown briefly then hidden
    let toast_label = Label::new(None);
    toast_label.add_css_class("toast-label");
    toast_label.set_visible(false);
    toast_label.set_halign(Align::Center);

    let title = Label::new(Some("MelonLoader Management"));
    title.add_css_class("section-label");
    title.set_halign(Align::Start);
    vbox.append(&title);

    let desc = Label::new(Some(
        "MelonLoader is the mod loader required to run ChilloutVR mods.\n\
         On Linux it runs via Proton/Wine. See the tip below for launch options."
    ));
    desc.set_halign(Align::Start);
    desc.set_wrap(true);
    desc.add_css_class("info-text");
    vbox.append(&desc);

    // ── Version info grid ─────────────────────────────────────────────────────
    let grid = Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(16);
    grid.set_margin_top(12);

    let mk_lbl = |text: &str| -> Label {
        let l = Label::new(Some(text));
        l.set_halign(Align::Start);
        l
    };

    grid.attach(&mk_lbl("Status:"),           0, 0, 1, 1);
    grid.attach(&mk_lbl("Installed version:"), 0, 1, 1, 1);
    grid.attach(&mk_lbl("Latest release:"),   0, 2, 1, 1);

    let status_label = Label::new(Some("—"));
    status_label.set_halign(Align::Start);
    status_label.add_css_class("ml-status");
    grid.attach(&status_label, 1, 0, 1, 1);

    let installed_ver_label = Label::new(Some("—"));
    installed_ver_label.set_halign(Align::Start);
    installed_ver_label.add_css_class("ml-version");
    grid.attach(&installed_ver_label, 1, 1, 1, 1);

    let latest_ver_label = Label::new(Some("—"));
    latest_ver_label.set_halign(Align::Start);
    latest_ver_label.add_css_class("ml-version");
    grid.attach(&latest_ver_label, 1, 2, 1, 1);

    vbox.append(&grid);

    // ── Buttons ───────────────────────────────────────────────────────────────
    let btn_box = Box::new(Orientation::Horizontal, 8);
    btn_box.set_margin_top(16);

    let check_btn       = Button::with_label("🔍  Check Status");
    let install_btn     = Button::with_label("⬇  Install / Update MelonLoader");
    let remove_btn      = Button::with_label("🗑  Remove MelonLoader");

    check_btn.add_css_class("action-button");
    install_btn.add_css_class("install-button");
    remove_btn.add_css_class("danger-button");

    btn_box.append(&check_btn);
    btn_box.append(&install_btn);
    btn_box.append(&remove_btn);
    vbox.append(&btn_box);

    // Update button label (shown when update available)
    let update_ml_hint = Label::new(None);
    update_ml_hint.set_halign(Align::Start);
    update_ml_hint.add_css_class("update-hint");
    vbox.append(&update_ml_hint);

    // Progress bar
    let progress = ProgressBar::new();
    progress.set_visible(false);
    vbox.append(&progress);

    // ── Wire: Check ──────────────────────────────────────────────────────────
    {
        let state = state.clone();
        let sl = status_label.clone();
        let ivl = installed_ver_label.clone();
        let lvl = latest_ver_label.clone();
        let hint = update_ml_hint.clone();
        check_btn.connect_clicked(move |_| {
            sl.set_label("Checking…");
            ivl.set_label("—");
            lvl.set_label("—");
            hint.set_label("");
            let state = state.clone();
            let sl = sl.clone();
            let ivl = ivl.clone();
            let lvl = lvl.clone();
            let hint = hint.clone();
            let dir = match state.lock().unwrap().install_dir.clone() {
                Some(d) => d,
                None => { sl.set_label("⚠ No install directory set"); return; }
            };
            let is_installed  = install::is_melon_loader_installed(&dir);
            let installed_ver = install::get_installed_melon_loader_version(&dir);
            // Update local labels immediately (no network needed for these)
            if is_installed {
                sl.set_label("✅  MelonLoader is installed");
                sl.remove_css_class("status-err");
                sl.add_css_class("status-ok");
                ivl.set_label(installed_ver.as_deref().unwrap_or("(version unknown)"));
            } else {
                sl.set_label("❌  MelonLoader is NOT installed");
                sl.remove_css_class("status-ok");
                sl.add_css_class("status-err");
                ivl.set_label("—");
            }
            // Fetch latest release tag via Tokio
            crate::spawn_async(
                async move { api::fetch_melon_loader_release().await },
                move |result| {
                    match result {
                        Ok(rel) => {
                            let tag = rel.tag_name.trim_start_matches('v').to_string();
                            lvl.set_label(&tag);
                            if is_installed {
                                if let Some(inst_v) = &installed_ver {
                                    if api::is_newer_version(inst_v, &tag) {
                                        hint.set_label(&format!(
                                            "⬆  Update available: {} → {}  (click Install / Update)",
                                            inst_v, tag
                                        ));
                                        hint.add_css_class("update-available");
                                    } else {
                                        hint.set_label("✓  MelonLoader is up to date");
                                        hint.remove_css_class("update-available");
                                    }
                                }
                            }
                        }
                        Err(e) => lvl.set_label(&format!("(network error: {})", e)),
                    }
                },
            );
        });
    }

    // ── Wire: Install / Update ────────────────────────────────────────────────
    {
        let state = state.clone();
        let sl = status_label.clone();
        let ivl = installed_ver_label.clone();
        let hint = update_ml_hint.clone();
        let prog = progress.clone();
        install_btn.connect_clicked(move |b| {
            let install_dir = state.lock().unwrap().install_dir.clone();
            let Some(dir) = install_dir else {
                show_error("No install directory", "Set the ChilloutVR path in Options first.");
                return;
            };
            b.set_sensitive(false);
            b.set_label("Installing…");
            prog.set_visible(true);
            prog.pulse();
            let sl = sl.clone();
            let ivl = ivl.clone();
            let hint = hint.clone();
            let prog = prog.clone();
            let bc = b.clone();
            crate::spawn_async(
                async move { install::install_melon_loader(&dir).await.map(|_| dir) },
                move |result| {
                    match result {
                        Ok(dir) => {
                            sl.set_label("✅  MelonLoader installed successfully!");
                            sl.add_css_class("status-ok");
                            let new_ver = install::get_installed_melon_loader_version(&dir)
                                .unwrap_or_else(|| "(unknown)".into());
                            ivl.set_label(&new_ver);
                            hint.set_label("✓  MelonLoader is up to date");
                            hint.remove_css_class("update-available");
                        }
                        Err(e) => {
                            sl.set_label("❌  Install failed");
                            sl.add_css_class("status-err");
                            show_error("Install failed", &e.to_string());
                        }
                    }
                    bc.set_sensitive(true);
                    bc.set_label("⬇  Install / Update MelonLoader");
                    prog.set_visible(false);
                },
            );
        });
    }

    // ── Wire: Remove ─────────────────────────────────────────────────────────
    {
        let state = state.clone();
        let sl = status_label.clone();
        let ivl = installed_ver_label.clone();
        let hint = update_ml_hint.clone();
        remove_btn.connect_clicked(move |_| {
            let install_dir = state.lock().unwrap().install_dir.clone();
            let Some(dir) = install_dir else {
                show_error("No install directory", "Set the ChilloutVR path in Options first.");
                return;
            };
            if !install::is_melon_loader_installed(&dir) {
                show_info("Not installed", "MelonLoader is not currently installed.");
                return;
            }
            match install::remove_melon_loader(&dir) {
                Ok(_) => {
                    sl.set_label("MelonLoader removed.");
                    sl.remove_css_class("status-ok");
                    sl.remove_css_class("status-err");
                    ivl.set_label("—");
                    hint.set_label("");
                    show_info("Removed", "MelonLoader has been removed successfully.");
                }
                Err(e) => show_error("Remove failed", &e.to_string()),
            }
        });
    }

    // ── Tip — click to copy launch argument ───────────────────────────────────
    const LAUNCH_ARG: &str = "WINEDLLOVERRIDES=\"version=n,b\" %command%";

    let tip_btn = Button::new();
    tip_btn.add_css_class("note-text-btn");
    tip_btn.set_margin_top(24);

    let tip_inner = Box::new(Orientation::Horizontal, 8);
    let tip_label = Label::new(Some(&format!(
        "💡 Tip: Add  {}  to ChilloutVR's Steam launch options to enable MelonLoader.\n\
         Click to copy the launch argument to clipboard.", LAUNCH_ARG
    )));
    tip_label.set_halign(Align::Start);
    tip_label.set_wrap(true);
    tip_label.set_xalign(0.0);
    tip_inner.append(&tip_label);

    let copy_icon = Label::new(Some("📋"));
    copy_icon.set_valign(Align::Start);
    tip_inner.append(&copy_icon);
    tip_btn.set_child(Some(&tip_inner));

    // Wire clipboard copy + toast label
    let toast_label_ref = toast_label.clone();
    tip_btn.connect_clicked(move |_| {
        // Copy to clipboard
        if let Some(display) = gdk4::Display::default() {
            let clipboard = display.clipboard();
            clipboard.set_text(LAUNCH_ARG);
        }
        // Show toast label briefly
        toast_label_ref.set_text("✓ Launch argument copied to clipboard!");
        toast_label_ref.set_visible(true);
        let tl = toast_label_ref.clone();
        glib::timeout_add_seconds_local_once(3, move || {
            tl.set_visible(false);
        });
    });

    vbox.append(&tip_btn);
    vbox.append(&toast_label);

    let spacer = Box::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    vbox.append(&spacer);

    (vbox, check_btn)
}

// ── Options Tab ───────────────────────────────────────────────────────────────

fn build_options_tab(
    state: &SharedState,
    window: &ApplicationWindow,
    listbox: &ListBox,
    count_label: &Label,
) -> (Box, Entry) {
    let vbox = Box::new(Orientation::Vertical, 12);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(24);
    vbox.set_margin_end(24);

    let dir_lbl = Label::new(Some("ChilloutVR Install Directory:"));
    dir_lbl.set_halign(Align::Start);
    dir_lbl.add_css_class("section-label");
    vbox.append(&dir_lbl);

    let row = Box::new(Orientation::Horizontal, 6);
    let entry = Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("/path/to/ChilloutVR"));
    {
        let s = state.lock().unwrap();
        if let Some(d) = &s.install_dir { entry.set_text(&d.display().to_string()); }
    }
    row.append(&entry);

    // Browse button
    {
        let e = entry.clone();
        let st = state.clone();
        let w = window.clone();
        let btn = Button::with_label("Browse…");
        btn.add_css_class("small-button");
        btn.connect_clicked(move |_| {
            let dialog = FileDialog::new();
            dialog.set_title("Select ChilloutVR Install Directory");
            let e = e.clone(); let st = st.clone();
            dialog.select_folder(Some(&w), None::<&gio::Cancellable>, move |res| {
                if let Ok(f) = res {
                    if let Some(path) = f.path() {
                        if steam::is_valid_install_dir(&path) {
                            e.set_text(&path.display().to_string());
                            let mut s = st.lock().unwrap();
                            s.install_dir = Some(path.clone());
                            let mut cfg = Config::load();
                            cfg.install_folder = Some(path.display().to_string());
                            let _ = cfg.save();
                        } else {
                            show_error("Invalid Directory",
                                "Could not find ChilloutVR.x86_64 and ChilloutVR_Data/Plugins here.");
                        }
                    }
                }
            });
        });
        row.append(&btn);
    }

    // Auto-detect
    {
        let e = entry.clone();
        let st = state.clone();
        let btn = Button::with_label("Auto-detect ChilloutVR");
        btn.add_css_class("small-button");
        btn.connect_clicked(move |_| {
            if let Some(dir) = steam::find_steam_install() {
                e.set_text(&dir.display().to_string());
                let mut s = st.lock().unwrap();
                s.install_dir = Some(dir.clone());
                let mut cfg = Config::load();
                cfg.install_folder = Some(dir.display().to_string());
                let _ = cfg.save();
            } else {
                show_error("Not Found",
                    "Could not auto-detect ChilloutVR. Is Steam installed and has the game been launched?");
            }
        });
        row.append(&btn);
    }
    vbox.append(&row);

    // Quick-open folder buttons
    let fl = Label::new(Some("Quick Open:"));
    fl.set_halign(Align::Start);
    fl.add_css_class("section-label");
    fl.set_margin_top(16);
    vbox.append(&fl);

    let btn_row = Box::new(Orientation::Horizontal, 8);
    for (lbl, sub) in &[
        ("📁 Game Folder", ""),
        ("📁 Mods", "Mods"),
        ("📁 Plugins", "Plugins"),
        ("📁 MelonLoader", "MelonLoader"),
        ("📁 UserData", "UserData"),
    ] {
        let b = Button::with_label(lbl);
        b.add_css_class("folder-button");
        let st = state.clone();
        let sub = sub.to_string();
        b.connect_clicked(move |_| {
            let s = st.lock().unwrap();
            if let Some(dir) = &s.install_dir {
                let t = if sub.is_empty() { dir.clone() } else { dir.join(&sub) };
                install::open_folder(&t.display().to_string());
            }
        });
        btn_row.append(&b);
    }
    vbox.append(&btn_row);

    // ── Behaviour settings ────────────────────────────────────────────────────
    let behaviour_lbl = Label::new(Some("Behaviour:"));
    behaviour_lbl.set_halign(Align::Start);
    behaviour_lbl.add_css_class("section-label");
    behaviour_lbl.set_margin_top(16);
    vbox.append(&behaviour_lbl);

    let confirm_check = CheckButton::with_label("Show confirmation dialog before uninstalling mods");
    confirm_check.set_active(Config::load().confirm_uninstall);
    confirm_check.add_css_class("options-check");
    confirm_check.connect_toggled(|check| {
        let mut cfg = Config::load();
        cfg.confirm_uninstall = check.is_active();
        let _ = cfg.save();
    });
    vbox.append(&confirm_check);

    let group_check = CheckButton::with_label(
        "Group broken and retired mods into a single \"Broken / Retired\" category"
    );
    group_check.set_active(Config::load().show_broken_retired_category);
    group_check.add_css_class("options-check");
    {
        let state = state.clone();
        let listbox = listbox.clone();
        let count_label = count_label.clone();
        group_check.connect_toggled(move |check| {
            let mut cfg = Config::load();
            cfg.show_broken_retired_category = check.is_active();
            let _ = cfg.save();
            repopulate_from_state(&listbox, &count_label, &state);
        });
    }
    vbox.append(&group_check);

    let quarantine_check = CheckButton::with_label(
        "Show mods that have been moved to Broken/ or Retired/ directories in the mod list"
    );
    quarantine_check.set_active(Config::load().show_quarantined_mods);
    quarantine_check.add_css_class("options-check");
    {
        let state = state.clone();
        let listbox = listbox.clone();
        let count_label = count_label.clone();
        quarantine_check.connect_toggled(move |check| {
            let mut cfg = Config::load();
            cfg.show_quarantined_mods = check.is_active();
            let _ = cfg.save();
            repopulate_from_state(&listbox, &count_label, &state);
        });
    }
    vbox.append(&quarantine_check);

    let deps_check = CheckButton::with_label(
        "Automatically install missing mod dependencies when installing a mod"
    );
    deps_check.set_active(Config::load().auto_install_deps);
    deps_check.add_css_class("options-check");
    {
        deps_check.connect_toggled(|check| {
            let mut cfg = Config::load();
            cfg.auto_install_deps = check.is_active();
            let _ = cfg.save();
        });
    }
    vbox.append(&deps_check);

    let cfg_info = Label::new(Some(&format!(
        "Config: {}",
        Config::config_path().display()
    )));
    cfg_info.set_halign(Align::Start);
    cfg_info.add_css_class("info-text");
    cfg_info.set_margin_top(20);
    vbox.append(&cfg_info);

    let spacer = Box::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    vbox.append(&spacer);

    (vbox, entry)
}

// ── About Tab ─────────────────────────────────────────────────────────────────

fn build_about_tab() -> Box {
    let vbox = Box::new(Orientation::Vertical, 12);
    vbox.set_halign(Align::Center);
    vbox.set_valign(Align::Center);
    vbox.set_margin_start(32);
    vbox.set_margin_end(32);

    let title = Label::new(Some("CVR MelonLoader Assistant"));
    title.add_css_class("about-title");
    vbox.append(&title);

    let sub = Label::new(Some(&format!("Version {} — Linux Port", APP_VERSION)));
    sub.add_css_class("about-subtitle");
    vbox.append(&sub);

    // Vibe-coded disclaimer
    let disclaimer = Label::new(Some(
        "⚠  This project is vibe coded using Claude (claude.ai).\n\
         It may contain bugs or behave unexpectedly. Use at your own risk."
    ));
    disclaimer.set_justify(Justification::Center);
    disclaimer.set_wrap(true);
    disclaimer.add_css_class("about-disclaimer");
    vbox.append(&disclaimer);

    let body = Label::new(Some(
        "A mod manager for ChilloutVR using MelonLoader, for Linux via Proton.\n\
         This is an unofficial Linux port of the original Windows app by Nirv-git & knah.\n\n\
         Ported and maintained by Kneesox  •  kneesox.moe\n\
         Log scanner ported from Lumbot by Slaynash"
    ));
    body.set_justify(Justification::Center);
    body.set_wrap(true);
    body.add_css_class("about-text");
    vbox.append(&body);

    let links = Box::new(Orientation::Horizontal, 16);
    links.set_halign(Align::Center);
    links.set_margin_top(16);
    for (label, url) in &[
        ("CVRMG Discord",       "https://discord.gg/cvrmg"),
        ("GitHub (Linux Port)", "https://github.com/ShiroBlank/CVRMelonAssistantLinux"),
        ("Original Windows App","https://github.com/Nirv-git/CVRMelonAssistant"),
        ("Kneesox's Website",   "https://kneesox.moe"),
        ("Lumbot (Slaynash)",   "https://github.com/Slaynash/Lumbot"),
        ("MelonLoader Wiki",    "https://melonwiki.xyz"),
    ] {
        let btn = Button::with_label(label);
        btn.add_css_class("link-button");
        let url = url.to_string();
        btn.connect_clicked(move |_| {
            let _ = gtk4::gio::AppInfo::launch_default_for_uri(&url, None::<&gtk4::gio::AppLaunchContext>);
        });
        links.append(&btn);
    }
    vbox.append(&links);
    vbox
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn wire_dir_entry(entry: &Entry, state: &SharedState) {
    let state = state.clone();
    entry.connect_changed(move |e| {
        let text = e.text().to_string();
        let p = std::path::PathBuf::from(&text);
        if steam::is_valid_install_dir(&p) {
            let mut s = state.lock().unwrap();
            s.install_dir = Some(p.clone());
            let mut cfg = Config::load();
            cfg.install_folder = Some(text);
            let _ = cfg.save();
        }
    });
}

/// Collect mod IDs from checked CheckButton widgets inside a ListBox.
fn collect_checked_mod_ids(listbox: &ListBox) -> Vec<i64> {
    let mut ids = Vec::new();
    let mut row = listbox.first_child();
    while let Some(r) = row {
        let next = r.next_sibling();
        if let Some(check) = find_check_in_widget(&r) {
            if check.is_active() {
                if let Ok(id) = check.widget_name().parse::<i64>() {
                    ids.push(id);
                }
            }
        }
        row = next;
    }
    ids
}

fn set_all_checks(listbox: &ListBox, active: bool) {
    let mut row = listbox.first_child();
    while let Some(r) = row {
        let next = r.next_sibling();
        if let Some(check) = find_check_in_widget(&r) {
            check.set_active(active);
        }
        row = next;
    }
}

fn find_check_in_widget(widget: &gtk4::Widget) -> Option<CheckButton> {
    find_check_recursive(widget)
}

fn find_check_recursive(w: &gtk4::Widget) -> Option<CheckButton> {
    if let Some(cb) = w.downcast_ref::<CheckButton>() { return Some(cb.clone()); }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(found) = find_check_recursive(&c) { return Some(found); }
        child = c.next_sibling();
    }
    None
}

fn show_error(title: &str, msg: &str) {
    let d = AlertDialog::builder().message(title).detail(msg).build();
    d.show(None::<&gtk4::Window>);
}

fn show_info(title: &str, msg: &str) {
    let d = AlertDialog::builder().message(title).detail(msg).build();
    d.show(None::<&gtk4::Window>);
}

// ── Debug Tab ─────────────────────────────────────────────────────────────────

fn build_debug_tab(state: &SharedState) -> (Box, Button) {
    let vbox = Box::new(Orientation::Vertical, 0);

    // ── Toolbar ───────────────────────────────────────────────────────────────
    let toolbar = Box::new(Orientation::Horizontal, 8);
    toolbar.add_css_class("toolbar");

    let scan_btn = Button::with_label("🔍  Scan Log");
    scan_btn.add_css_class("action-button");
    toolbar.append(&scan_btn);

    let log_path_label = Label::new(Some("Log: —"));
    log_path_label.set_halign(Align::Start);
    log_path_label.set_hexpand(true);
    log_path_label.set_ellipsize(pango::EllipsizeMode::Start);
    log_path_label.add_css_class("dir-label");
    toolbar.append(&log_path_label);

    let open_log_btn = Button::with_label("📂  Open Log");
    open_log_btn.add_css_class("small-button");
    toolbar.append(&open_log_btn);

    vbox.append(&toolbar);

    // ── Scrolled findings area ────────────────────────────────────────────────
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);

    let findings_box = Box::new(Orientation::Vertical, 4);
    findings_box.set_margin_top(8);
    findings_box.set_margin_bottom(8);
    findings_box.set_margin_start(12);
    findings_box.set_margin_end(12);
    findings_box.set_widget_name("findings-box");

    let placeholder = Label::new(Some(
        "Click '🔍 Scan Log' to analyse your MelonLoader log file.\n\
         The log is read from:  <ChilloutVR>/MelonLoader/Latest.log"
    ));
    placeholder.set_halign(Align::Center);
    placeholder.set_valign(Align::Center);
    placeholder.set_vexpand(true);
    placeholder.add_css_class("info-text");
    findings_box.append(&placeholder);
    findings_box.set_widget_name("findings-box");

    scrolled.set_child(Some(&findings_box));
    vbox.append(&scrolled);

    // ── Status bar ────────────────────────────────────────────────────────────
    let status = Label::new(Some("No scan performed yet."));
    status.set_halign(Align::Start);
    status.set_margin_start(10);
    status.set_margin_top(4);
    status.set_margin_bottom(4);
    status.add_css_class("status-label");
    vbox.append(&status);

    // ── Wire: Scan ────────────────────────────────────────────────────────────
    {
        let state  = state.clone();
        let fb     = findings_box.clone();
        let st     = status.clone();
        let lpl    = log_path_label.clone();

        scan_btn.connect_clicked(move |b| {
            let install_dir = state.lock().unwrap().install_dir.clone();
            let Some(dir) = install_dir else {
                st.set_label("No install directory set — go to Options first.");
                return;
            };

            b.set_sensitive(false);
            b.set_label("Scanning…");
            st.set_label("Reading log file…");

            let log_path = crate::log_scanner::log_path(&dir);
            lpl.set_text(&format!("Log: {}", log_path.display()));

            // Run the scanner synchronously (filesystem only, no network)
            let result = crate::log_scanner::scan(&dir);

            // Clear old findings
            while let Some(child) = fb.first_child() { fb.remove(&child); }

            match result {
                Err(e) => {
                    // Determine if it's specifically a missing file vs another error
                    let log_path_str = crate::log_scanner::log_path(&dir);
                    let is_missing   = !log_path_str.exists();

                    let panel = Box::new(Orientation::Vertical, 12);
                    panel.set_halign(Align::Center);
                    panel.set_valign(Align::Center);
                    panel.set_vexpand(true);
                    panel.set_margin_top(40);

                    if is_missing {
                        let icon = Label::new(Some("📄"));
                        icon.set_halign(Align::Center);
                        // Large emoji via CSS font-size
                        icon.add_css_class("debug-no-log-icon");
                        panel.append(&icon);

                        let title = Label::new(Some("No log file found"));
                        title.set_halign(Align::Center);
                        title.add_css_class("debug-no-log-title");
                        panel.append(&title);

                        let body = Label::new(Some(
                            "MelonLoader/Latest.log does not exist yet.\n\n\
                             Launch ChilloutVR with MelonLoader installed at least once,\n\
                             then come back and click 🔍 Scan Log."
                        ));
                        body.set_halign(Align::Center);
                        body.set_justify(gtk4::Justification::Center);
                        body.set_wrap(true);
                        body.add_css_class("debug-no-log-body");
                        panel.append(&body);

                        let expected = Label::new(Some(&format!(
                            "Expected location:\n{}",
                            log_path_str.display()
                        )));
                        expected.set_halign(Align::Center);
                        expected.set_justify(gtk4::Justification::Center);
                        expected.set_wrap(true);
                        expected.add_css_class("debug-no-log-path");
                        panel.append(&expected);

                        st.set_label("No log file found — launch the game first.");
                    } else {
                        let title = Label::new(Some("❌  Failed to read log"));
                        title.set_halign(Align::Center);
                        title.add_css_class("debug-no-log-title");
                        panel.append(&title);

                        let body = Label::new(Some(&e));
                        body.set_halign(Align::Center);
                        body.set_wrap(true);
                        body.add_css_class("debug-no-log-body");
                        panel.append(&body);

                        st.set_label(&format!("Error reading log: {}", e));
                    }

                    fb.append(&panel);
                }
                Ok(report) => {
                    // Header: ML version + game + loaded mods
                    let header = Box::new(Orientation::Vertical, 4);
                    header.add_css_class("debug-header-box");

                    let summary_parts: Vec<String> = {
                        let mut v = Vec::new();
                        if let Some(ml) = &report.ml_version {
                            v.push(format!("MelonLoader v{}", ml));
                        }
                        if let Some(game) = &report.game_name {
                            let mut gs = game.clone();
                            if let Some(ver) = &report.game_version { gs.push_str(&format!(" v{}", ver)); }
                            v.push(gs);
                        }
                        if let Some(os) = &report.os_type {
                            v.push(os.clone());
                        }
                        v
                    };
                    if !summary_parts.is_empty() {
                        let sl = Label::new(Some(&summary_parts.join("  •  ")));
                        sl.set_halign(Align::Start);
                        sl.add_css_class("debug-summary-line");
                        header.append(&sl);
                    }

                    // Loaded mods / plugins
                    if !report.loaded_mods.is_empty() || !report.loaded_plugins.is_empty() {
                        let ml = report.loaded_mods.len();
                        let pl = report.loaded_plugins.len();
                        let mods_lbl = Label::new(Some(&format!("Loaded: {} mod(s)  •  {} plugin(s)", ml, pl)));
                        mods_lbl.set_halign(Align::Start);
                        mods_lbl.add_css_class("debug-summary-line");
                        header.append(&mods_lbl);

                        // Expandable mod list
                        let mod_list_box = Box::new(Orientation::Vertical, 2);
                        mod_list_box.set_margin_start(16);
                        for m in &report.loaded_mods {
                            let mut txt = m.name.clone();
                            if let Some(v) = &m.version { txt.push_str(&format!("  v{}", v)); }
                            if let Some(a) = &m.author  { txt.push_str(&format!("  by {}", a)); }
                            let l = Label::new(Some(&txt));
                            l.set_halign(Align::Start);
                            l.add_css_class("debug-mod-line");
                            mod_list_box.append(&l);
                        }
                        for p in &report.loaded_plugins {
                            let mut txt = format!("[Plugin] {}", p.name);
                            if let Some(v) = &p.version { txt.push_str(&format!("  v{}", v)); }
                            if let Some(a) = &p.author  { txt.push_str(&format!("  by {}", a)); }
                            let l = Label::new(Some(&txt));
                            l.set_halign(Align::Start);
                            l.add_css_class("debug-mod-line");
                            mod_list_box.append(&l);
                        }
                        header.append(&mod_list_box);
                    }

                    fb.append(&header);

                    // Separator
                    let sep = Separator::new(Orientation::Horizontal);
                    sep.set_margin_top(6);
                    sep.set_margin_bottom(6);
                    fb.append(&sep);

                    // Findings
                    for finding in &report.findings {
                        // Skip bare info lines already shown in header
                        if finding.severity == crate::log_scanner::Severity::Info
                            && (finding.category == "MelonLoader" || finding.category == "Game"
                                || finding.category == "System" || finding.category == "Mods")
                        {
                            continue;
                        }

                        let row = Box::new(Orientation::Vertical, 2);
                        row.set_margin_top(4);
                        row.set_margin_bottom(4);
                        row.add_css_class(match finding.severity {
                            crate::log_scanner::Severity::Ok      => "debug-finding-ok",
                            crate::log_scanner::Severity::Info    => "debug-finding-info",
                            crate::log_scanner::Severity::Warning => "debug-finding-warn",
                            crate::log_scanner::Severity::Error   => "debug-finding-error",
                        });

                        // Category header
                        let icon = match finding.severity {
                            crate::log_scanner::Severity::Ok      => "✅",
                            crate::log_scanner::Severity::Info    => "ℹ",
                            crate::log_scanner::Severity::Warning => "⚠",
                            crate::log_scanner::Severity::Error   => "❌",
                        };
                        let cat_lbl = Label::new(Some(&format!("{} {}", icon, finding.category)));
                        cat_lbl.set_halign(Align::Start);
                        cat_lbl.add_css_class("debug-finding-category");
                        row.append(&cat_lbl);

                        // Message (supports multi-line bullet lists)
                        let msg_lbl = Label::new(Some(&finding.message));
                        msg_lbl.set_halign(Align::Start);
                        msg_lbl.set_wrap(true);
                        msg_lbl.set_xalign(0.0);
                        msg_lbl.set_margin_start(18);
                        msg_lbl.add_css_class("debug-finding-message");
                        row.append(&msg_lbl);

                        fb.append(&row);
                    }

                    let issues = report.findings.iter()
                        .filter(|f| f.severity == crate::log_scanner::Severity::Error
                               || f.severity == crate::log_scanner::Severity::Warning)
                        .count();

                    st.set_label(&format!(
                        "Scan complete — {} line(s) read  •  {} mod(s)  •  {} plugin(s)  •  {} issue(s) found{}",
                        report.line_count,
                        report.loaded_mods.len(),
                        report.loaded_plugins.len(),
                        issues,
                        if report.truncated { "  •  ⚠ Log truncated" } else { "" }
                    ));
                }
            }

            b.set_sensitive(true);
            b.set_label("🔍  Scan Log");
        });
    }

    // ── Wire: Open Log ────────────────────────────────────────────────────────
    {
        let state = state.clone();
        open_log_btn.connect_clicked(move |_| {
            let install_dir = state.lock().unwrap().install_dir.clone();
            if let Some(dir) = install_dir {
                let log = crate::log_scanner::log_path(&dir);
                if log.exists() {
                    install::open_folder(&log.parent().unwrap_or(&dir).display().to_string());
                } else {
                    show_error("Log not found",
                        "MelonLoader/Latest.log does not exist. Has ChilloutVR been launched with MelonLoader at least once?");
                }
            }
        });
    }

    (vbox, scan_btn)
}



const DARK_CSS: &str = r#"
* { font-family: "Inter", "Cantarell", "Segoe UI", sans-serif; }

window, .main-box { background-color: #1a1a2e; color: #e0e0e0; }

.header-bar {
    background: linear-gradient(135deg, #16213e 0%, #0f3460 100%);
    border-bottom: 1px solid #533483;
    padding: 10px 14px;
}
.header-title { font-size: 18px; font-weight: bold; color: #e94560; }
.dir-label    { font-size: 11px; color: #888; }

notebook > header { background-color: #16213e; border-bottom: 1px solid #533483; }
notebook > header > tabs > tab         { color: #999; padding: 7px 18px; background-color: #16213e; }
notebook > header > tabs > tab:checked { color: #e94560; border-bottom: 2px solid #e94560; background-color: #1a1a2e; }

.toolbar {
    background-color: #111827;
    padding: 6px 8px;
    border-bottom: 1px solid #2a2a4a;
}

.col-header { background-color: #0f1726; padding: 4px 8px; border-bottom: 1px solid #2a2a4a; }
.col-header-label { font-size: 11px; color: #666; font-weight: bold; }

.bottom-bar { background-color: #0f1726; padding: 4px 10px; border-top: 1px solid #2a2a4a; font-size: 11px; color: #888; }

.status-label { font-size: 11px; color: #aaa; background-color: #0f1726; border-top: 1px solid #2a2a4a; padding: 3px 10px; }

/* Buttons */
.action-button  { background-color: #533483; color: white; border: none; border-radius: 4px; padding: 5px 12px; font-weight: bold; }
.action-button:hover { background-color: #6a45a0; }
.install-button { background-color: #0f7b6c; color: white; border: none; border-radius: 4px; padding: 5px 12px; font-weight: bold; }
.install-button:hover { background-color: #13a898; }
.update-button  { background-color: #1a5276; color: white; border: none; border-radius: 4px; padding: 5px 12px; font-weight: bold; }
.update-button:hover  { background-color: #2874a6; }
.update-badge   { background-color: #b7770d; }
.update-badge:hover { background-color: #d4ac0d; }
.danger-button  { background-color: #7b1a2f; color: white; border: none; border-radius: 4px; padding: 5px 12px; font-weight: bold; }
.danger-button:hover  { background-color: #b52340; }
.small-button, .folder-button { background-color: #2a2a4a; color: #ccc; border: 1px solid #444; border-radius: 4px; padding: 4px 10px; font-size: 12px; }
.small-button:hover, .folder-button:hover { background-color: #3a3a6a; }
.link-button { background-color: #0f3460; color: #7ec8e3; border: 1px solid #7ec8e3; border-radius: 4px; padding: 6px 16px; }
.link-button:hover { background-color: #1a5090; }

/* Mod list */
.mods-listbox { background-color: #16213e; }
.mods-listbox > row { border-bottom: 1px solid #1f2b50; }
.mods-listbox > row:hover { background-color: #1f2b50; }
.mods-listbox > row.mod-installed  { border-left: 3px solid #4caf50; }
.mods-listbox > row.mod-has-update { background-color: #1a2a1a; border-left: 3px solid #f1c40f; }
.mod-name   { font-weight: bold; color: #e0e0e0; font-size: 13px; }
.mod-desc   { font-size: 11px; color: #777; }
.mod-meta   { font-size: 11px; color: #999; }
.installed-ver { font-size: 11px; color: #4caf50; font-weight: bold; }
.category-row { background-color: #0f1726; }
.category-row:hover { background-color: #171f35; }
.category-chevron { font-size: 10px; color: #666; min-width: 12px; }
.category-label { font-weight: bold; font-size: 12px; color: #e94560; text-transform: uppercase; letter-spacing: 1px; margin: 2px 4px; }

/* Status badges */
.badge-update      { color: #f1c40f; font-weight: bold; font-size: 11px; }
.badge-quarantine  { color: #e67e22; font-weight: bold; font-size: 11px; }
.badge-ok          { color: #4caf50; font-size: 11px; }
.badge-broken      { color: #e74c3c; font-size: 11px; }
.badge-retired     { color: #e67e22; font-size: 11px; }

/* ML tab */
.section-label { font-weight: bold; font-size: 14px; color: #e94560; }
.ml-status     { font-size: 13px; font-weight: bold; color: #ccc; }
.ml-version    { font-size: 13px; color: #ccc; font-family: monospace; }
.status-ok     { color: #4caf50; }
.status-err    { color: #e74c3c; }
.update-hint   { font-size: 12px; color: #ccc; margin-top: 4px; }
.update-available { color: #f1c40f; font-weight: bold; }
.info-text     { font-size: 12px; color: #888; }
.note-text     { font-size: 12px; color: #d4ac0d; background-color: #1a1600; border: 1px solid #5a4a00; border-radius: 4px; padding: 8px 12px; }
.note-text-btn { font-size: 12px; color: #d4ac0d; background-color: #1a1600; border: 1px solid #5a4a00; border-radius: 4px; padding: 8px 12px; }
.note-text-btn:hover { background-color: #2a2200; border-color: #8a7000; }
.toast-label   { font-size: 12px; color: #e0e0e0; background-color: #1e3a1e; border: 1px solid #4caf50; border-radius: 6px; padding: 6px 16px; margin-top: 6px; }

/* About */
.about-title       { font-size: 26px; font-weight: bold; color: #e94560; }
.about-subtitle    { font-size: 13px; color: #888; }
.about-disclaimer  { font-size: 12px; color: #d4ac0d; background-color: #1a1600;
                     border: 1px solid #5a4a00; border-radius: 6px;
                     padding: 8px 16px; margin-top: 4px; }
.about-text        { font-size: 13px; color: #ccc; }

/* Unverified category */
.category-label-unverified { font-weight: bold; font-size: 12px; color: #e67e22;
                              text-transform: uppercase; letter-spacing: 1px; margin: 2px 8px; }
.category-label-broken-retired { font-weight: bold; font-size: 12px; color: #e74c3c;
                                  text-transform: uppercase; letter-spacing: 1px; margin: 2px 8px; }
.mods-listbox > row.unverified-row    { background-color: #1a1000; border-left: 3px solid #e67e22; }
.mods-listbox > row.broken-retired-row { background-color: #1a0000; border-left: 3px solid #e74c3c; }

/* Debug tab */
.debug-header-box       { background-color: #111827; border-radius: 6px; padding: 10px 14px; margin-bottom: 4px; }
.debug-summary-line     { font-size: 13px; color: #ccc; font-weight: bold; }
.debug-mod-line         { font-size: 11px; color: #aaa; }
.debug-finding-ok       { background-color: #0d1f0d; border-left: 3px solid #4caf50; border-radius: 4px; padding: 6px 10px; }
.debug-finding-info     { background-color: #0d1626; border-left: 3px solid #5b9bd5; border-radius: 4px; padding: 6px 10px; }
.debug-finding-warn     { background-color: #1a1600; border-left: 3px solid #f1c40f; border-radius: 4px; padding: 6px 10px; }
.debug-finding-error    { background-color: #1a0000; border-left: 3px solid #e74c3c; border-radius: 4px; padding: 6px 10px; }
.debug-finding-category { font-size: 12px; font-weight: bold; color: #e0e0e0; }
.debug-finding-message  { font-size: 12px; color: #ccc; }
.debug-no-log-icon      { font-size: 48px; margin-bottom: 8px; }
.debug-no-log-title     { font-size: 18px; font-weight: bold; color: #ccc; }
.debug-no-log-body      { font-size: 13px; color: #888; }
.debug-no-log-path      { font-size: 11px; color: #555; font-family: monospace; margin-top: 8px; }


entry, searchentry { background-color: #0f1726; color: #e0e0e0; border: 1px solid #444; border-radius: 4px; padding: 5px; }
entry:focus, searchentry:focus { border-color: #e94560; }
checkbutton check { background-color: #0f1726; border: 1px solid #555; }
checkbutton:checked check { background-color: #e94560; border-color: #e94560; }
.options-check { margin-top: 4px; color: #ccc; font-size: 13px; }
scrolledwindow { background-color: #16213e; }
progressbar trough { background-color: #0f1726; border-radius: 4px; }
progressbar progress { background-color: #e94560; border-radius: 4px; }
"#;
