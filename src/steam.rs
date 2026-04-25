use std::path::{Path, PathBuf};
use std::fs;
use regex::Regex;

const CHILLOUT_APP_ID: &str = "661130";

/// Find ChilloutVR installation via Steam on Linux.
pub fn find_steam_install() -> Option<PathBuf> {
    let steam_roots = get_steam_roots();
    crate::log("INFO", &format!("steam: checking {} root(s): {:?}", steam_roots.len(), steam_roots));

    for root in &steam_roots {
        // Collect every steamapps folder to search: the root's own, plus any
        // extra library folders listed in libraryfolders.vdf
        let mut library_paths = Vec::new();

        // The root's own steamapps dir
        let own_steamapps = root.join("steamapps");
        if own_steamapps.exists() {
            library_paths.push(own_steamapps.clone());
        }

        // Parse libraryfolders.vdf for extra library paths
        for vdf_name in &["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
            let vdf_path = root.join(vdf_name);
            if let Ok(vdf) = fs::read_to_string(&vdf_path) {
                crate::log("INFO", &format!("steam: reading VDF {}", vdf_path.display()));
                for extra in parse_library_paths_from_vdf(&vdf) {
                    let sa = extra.join("steamapps");
                    if sa.exists() && !library_paths.contains(&sa) {
                        crate::log("INFO", &format!("steam: extra library: {}", sa.display()));
                        library_paths.push(sa);
                    }
                }
            }
        }

        crate::log("INFO", &format!("steam: searching {} library path(s)", library_paths.len()));

        for lib in &library_paths {
            let acf = lib.join(format!("appmanifest_{}.acf", CHILLOUT_APP_ID));
            crate::log("INFO", &format!("steam: looking for {}", acf.display()));
            if acf.exists() {
                crate::log("INFO", &format!("steam: found ACF at {}", acf.display()));
                if let Some(dir) = parse_install_dir_from_acf(&acf, lib) {
                    crate::log("INFO", &format!("steam: resolved install dir: {}", dir.display()));
                    return Some(dir);
                } else {
                    crate::log("WARN", "steam: ACF found but could not parse install dir");
                }
            }
        }
    }

    // Last-ditch: just look for the game binary directly under every library we know about
    crate::log("WARN", "steam: ACF search failed, trying direct filesystem scan");
    for root in &steam_roots {
        let common = root.join("steamapps/common");
        if let Some(dir) = scan_common_for_cvr(&common) {
            return Some(dir);
        }
    }

    None
}

/// Parse every `"path"  "/some/dir"` entry from a libraryfolders.vdf.
fn parse_library_paths_from_vdf(vdf: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // Modern VDF uses tabs: "path"\t\t"/home/user/..."
    // Older format: "1" { "path" "/home/user/..." }
    let re = Regex::new(r#""path"\s+"([^"]+)""#).unwrap();
    for cap in re.captures_iter(vdf) {
        let p = PathBuf::from(&cap[1]);
        if p.exists() {
            paths.push(p);
        }
    }
    paths
}

/// Walk steamapps/common looking for a ChilloutVR folder directly
/// (fallback when ACF parsing fails).
fn scan_common_for_cvr(common: &Path) -> Option<PathBuf> {
    if !common.exists() { return None; }
    let entries = fs::read_dir(common).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if is_valid_install_dir(&path) {
            crate::log("INFO", &format!("steam: found CVR via dir scan: {}", path.display()));
            return Some(path);
        }
    }
    None
}

fn get_steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let candidates = [
            // Standard Steam on Linux
            home.join(".local/share/Steam"),
            // Symlink Steam sometimes creates
            home.join(".steam/steam"),
            home.join(".steam/root"),
            // Flatpak Steam
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
            // Snap Steam
            home.join("snap/steam/common/.local/share/Steam"),
        ];
        for c in candidates {
            if c.exists() {
                // Resolve symlinks so we don't search the same dir twice
                let resolved = fs::canonicalize(&c).unwrap_or(c.clone());
                if !roots.contains(&resolved) {
                    roots.push(resolved);
                }
            }
        }
    }

    roots
}

fn parse_install_dir_from_acf(acf_path: &Path, steamapps: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(acf_path).ok()?;

    // "installdir"  "ChilloutVR"  (tabs or spaces as separator)
    let re = Regex::new(r#""installdir"\s+"([^"]+)""#).unwrap();

    for cap in re.captures_iter(&content) {
        let install_dir = steamapps.join("common").join(&cap[1]);
        crate::log("INFO", &format!("steam: ACF installdir candidate: {}", install_dir.display()));
        if is_valid_install_dir(&install_dir) {
            return Some(install_dir);
        }
        // Even if validation fails, return the dir — maybe the binary name differs
        if install_dir.exists() {
            crate::log("WARN", &format!("steam: dir exists but failed validation: {}", install_dir.display()));
            return Some(install_dir);
        }
    }

    None
}

pub fn is_valid_install_dir(path: &Path) -> bool {
    if !path.exists() { return false; }
    // ChilloutVR is Windows-only and always runs via Proton on Linux.
    // The install will always contain ChilloutVR.exe and a ChilloutVR_Data folder.
    let has_exe  = path.join("ChilloutVR.exe").exists();
    let has_data = path.join("ChilloutVR_Data").exists();
    has_exe && has_data
}
