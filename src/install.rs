use std::path::{Path, PathBuf};
use std::fs;
use std::io::{Read, Write};
use anyhow::{Result, Context};
use zip::ZipArchive;
use crate::api;
use crate::config::Config;
use crate::models::Mod;

// ── MelonLoader ───────────────────────────────────────────────────────────────

pub fn is_melon_loader_installed(install_dir: &Path) -> bool {
    // ChilloutVR always runs via Proton (Windows game). MelonLoader installs
    // version.dll as a proxy loader alongside MelonLoader/ directory containing
    // the managed assemblies. We check for both to confirm a real install.
    let version_dll = install_dir.join("version.dll");
    let ml_dir      = install_dir.join("MelonLoader");
    version_dll.exists() && ml_dir.exists()
}

/// Read the installed MelonLoader version from the version file it writes on first run,
/// or fall back to reading it from the AssemblyInfo embedded in MelonLoader.dll.
pub fn get_installed_melon_loader_version(install_dir: &Path) -> Option<String> {
    // MelonLoader writes a version file at MelonLoader/Data/MelonLoader.ver on newer builds
    let ver_file = install_dir.join("MelonLoader").join("Data").join("MelonLoader.ver");
    if let Ok(v) = fs::read_to_string(&ver_file) {
        let v = v.trim().to_string();
        if !v.is_empty() { return Some(v); }
    }

    // Fall back: scan MelonLoader.dll for a version string in its PE metadata
    let dll_paths = [
        install_dir.join("MelonLoader").join("net6").join("MelonLoader.dll"),
        install_dir.join("MelonLoader").join("MelonLoader.dll"),
        install_dir.join("MelonLoader").join("Dependencies").join("MelonLoader.dll"),
    ];
    for dll_path in &dll_paths {
        if dll_path.exists() {
            if let Some(ver) = extract_version_from_dll(dll_path) {
                return Some(ver);
            }
        }
    }

    // If installed but version unreadable, return a placeholder
    if is_melon_loader_installed(install_dir) {
        return Some("(unknown)".to_string());
    }

    None
}

/// Scan a .dll's raw bytes for a version string of the form "X.Y.Z.W" or "vX.Y.Z".
fn extract_version_from_dll(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    // Look for UTF-16LE version strings embedded in PE resources (common in .NET assemblies)
    // Heuristic: find a run of bytes that matches \d+\.\d+\.\d+ when decoded as ASCII
    let s = String::from_utf8_lossy(&bytes);
    // Find "AssemblyVersion" or "AssemblyFileVersion" nearby
    let markers = ["AssemblyVersion(\"", "AssemblyFileVersion(\"", "MelonLoader v"];
    for marker in &markers {
        if let Some(pos) = s.find(marker) {
            let after = &s[pos + marker.len()..];
            let end = after.find(|c: char| !c.is_ascii_digit() && c != '.' && c != 'v').unwrap_or(after.len());
            let ver = after[..end].trim_start_matches('v').to_string();
            if ver.contains('.') && ver.len() < 20 {
                return Some(ver);
            }
        }
    }

    // Generic version pattern scan: find X.Y.Z.W in the string
    let re_simple = regex::Regex::new(r"\b(\d+\.\d+\.\d+(?:\.\d+)?)\b").ok()?;
    // Walk through matches and find ones that look like version numbers (not IP addresses etc.)
    for cap in re_simple.captures_iter(&s) {
        let v = cap[1].to_string();
        let parts: Vec<u64> = v.split('.').filter_map(|p| p.parse().ok()).collect();
        // Plausible MelonLoader version: major < 5, none > 999
        if parts.len() >= 3 && parts[0] < 5 && parts.iter().all(|&n| n < 1000) {
            return Some(v);
        }
    }

    None
}

pub fn remove_melon_loader(install_dir: &Path) -> Result<()> {
    // The Windows MelonLoader zip drops these files in the game root:
    //   version.dll  — the proxy loader
    //   dobby.dll    — hooking library (some versions)
    // And the MelonLoader/ directory containing all managed assemblies.
    for f in &[
        install_dir.join("version.dll"),
        install_dir.join("dobby.dll"),
    ] {
        if f.exists() {
            fs::remove_file(f).with_context(|| format!("Failed to remove {:?}", f))?;
        }
    }
    let ml_dir = install_dir.join("MelonLoader");
    if ml_dir.exists() {
        fs::remove_dir_all(&ml_dir).context("Failed to remove MelonLoader directory")?;
    }
    Ok(())
}

pub async fn install_melon_loader(install_dir: &Path) -> Result<()> {
    remove_melon_loader(install_dir)?;

    let bytes = api::download_to_bytes(api::MELON_LOADER_URL).await
        .context("Failed to download MelonLoader")?;

    let cursor = std::io::Cursor::new(bytes);
    let mut zip = ZipArchive::new(cursor).context("Failed to read MelonLoader zip")?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let out_path = install_dir.join(entry.name());
        if entry.name().ends_with('/') {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() { fs::create_dir_all(parent)?; }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::File::create(&out_path)?.write_all(&buf)?;
        }
    }

    fs::create_dir_all(install_dir.join("Mods"))?;
    fs::create_dir_all(install_dir.join("Plugins"))?;
    Ok(())
}

// ── Mod install / update / uninstall ─────────────────────────────────────────

/// The result of a quarantine operation on a single mod.
#[derive(Debug)]
pub struct QuarantineResult {
    pub mod_name: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Walk all installed mods and move any that the API marks as broken into
/// `Mods/~Broken/` (or `Plugins/~Broken/`), creating the directory if needed.
///
/// Returns a list of moves that were performed so the UI can report them.
pub fn quarantine_broken_mods(
    mods: &[Mod],
    install_dir: &Path,
) -> Vec<QuarantineResult> {
    let mut results = Vec::new();

    for m in mods {
        let version = match m.versions.first() {
            Some(v) => v,
            None => continue,
        };

        // Only act on mods that are installed AND now marked broken by the API
        if !version.is_broken() { continue; }
        let installed_path = match &m.installed_file_path {
            Some(p) => PathBuf::from(p),
            None => continue,
        };

        // If the installed version is higher than the latest API version the mod
        // has likely been patched locally — leave it alone.
        if let (Some(installed_ver), Some(api_ver)) = (
            m.installed_version.as_deref(),
            version.mod_version.as_deref(),
        ) {
            if crate::api::is_newer_version(api_ver, installed_ver) {
                Config::log(&format!(
                    "Skipping quarantine of broken mod '{}': installed {} > api {} (locally patched?)",
                    version.name, installed_ver, api_ver
                ), "info");
                continue;
            }
        }

        // If the file is already inside a Broken/ subdirectory, leave it alone
        let path_str = installed_path.to_string_lossy().to_lowercase();
        if path_str.contains("/~broken/") || path_str.contains("\\~broken\\") {
            continue;
        }

        // Determine target Broken/ folder (Mods/Broken or Plugins/Broken)
        let subdir = if version.is_plugin() { "Plugins" } else { "Mods" };
        let broken_dir = install_dir.join(subdir).join("~Broken");
        if let Err(e) = fs::create_dir_all(&broken_dir) {
            Config::log(&format!("Could not create Broken dir {:?}: {}", broken_dir, e), "warn");
            continue;
        }

        let filename = match installed_path.file_name() {
            Some(f) => f,
            None => continue,
        };
        let dest = broken_dir.join(filename);

        match fs::rename(&installed_path, &dest) {
            Ok(_) => {
                let name = version.name.clone();
                Config::log(&format!("Quarantined broken mod '{}': {:?} → {:?}", name, installed_path, dest), "info");
                results.push(QuarantineResult {
                    mod_name: name,
                    from: installed_path,
                    to: dest,
                });
            }
            Err(e) => {
                // rename can fail across filesystems — fall back to copy+delete
                if let Ok(_) = fs::copy(&installed_path, &dest) {
                    let _ = fs::remove_file(&installed_path);
                    let name = version.name.clone();
                    Config::log(&format!("Quarantined (copy) broken mod '{}': {:?} → {:?}", name, installed_path, dest), "info");
                    results.push(QuarantineResult {
                        mod_name: name,
                        from: installed_path,
                        to: dest,
                    });
                } else {
                    Config::log(&format!("Failed to quarantine {:?}: {}", installed_path, e), "warn");
                }
            }
        }
    }

    results
}

/// Walk all installed mods and move any that the API marks as retired into
/// `Mods/~Retired/` (or `Plugins/~Retired/`), creating the directory if needed.
pub fn quarantine_retired_mods(
    mods: &[Mod],
    install_dir: &Path,
) -> Vec<QuarantineResult> {
    let mut results = Vec::new();

    for m in mods {
        let version = match m.versions.first() {
            Some(v) => v,
            None => continue,
        };

        if !version.is_retired() { continue; }
        let installed_path = match &m.installed_file_path {
            Some(p) => PathBuf::from(p),
            None => continue,
        };

        // If the installed version is higher than the latest API version the mod
        // has likely been patched locally — leave it alone.
        if let (Some(installed_ver), Some(api_ver)) = (
            m.installed_version.as_deref(),
            version.mod_version.as_deref(),
        ) {
            if crate::api::is_newer_version(api_ver, installed_ver) {
                Config::log(&format!(
                    "Skipping quarantine of retired mod '{}': installed {} > api {} (locally patched?)",
                    version.name, installed_ver, api_ver
                ), "info");
                continue;
            }
        }

        // Already in a Retired/ subdirectory — leave it alone
        let path_str = installed_path.to_string_lossy().to_lowercase();
        if path_str.contains("/~retired/") || path_str.contains("\\~retired\\") {
            continue;
        }

        let subdir = if version.is_plugin() { "Plugins" } else { "Mods" };
        let retired_dir = install_dir.join(subdir).join("~Retired");
        if let Err(e) = fs::create_dir_all(&retired_dir) {
            Config::log(&format!("Could not create Retired dir {:?}: {}", retired_dir, e), "warn");
            continue;
        }

        let filename = match installed_path.file_name() {
            Some(f) => f,
            None => continue,
        };
        let dest = retired_dir.join(filename);

        let moved = if fs::rename(&installed_path, &dest).is_ok() {
            true
        } else {
            fs::copy(&installed_path, &dest).is_ok() && fs::remove_file(&installed_path).is_ok()
        };

        if moved {
            let name = version.name.clone();
            Config::log(&format!("Moved retired mod '{}': {:?} → {:?}", name, installed_path, dest), "info");
            results.push(QuarantineResult {
                mod_name: name,
                from: installed_path,
                to: dest,
            });
        } else {
            Config::log(&format!("Failed to move retired mod {:?}", installed_path), "warn");
        }
    }

    results
}


/// Returns (path, md5_hash, Option<MelonInfo>) for every .dll found.
/// Recursively scans all subdirectories inside Mods/ and Plugins/.
/// Subdirectories beginning with '~' (e.g. ~Broken, ~Retired) are scanned but
/// their contents are tagged as quarantined by their path.
pub fn scan_installed_mods(install_dir: &Path) -> Vec<(PathBuf, String, Option<crate::melon_dll::MelonInfo>)> {
    let mut found = Vec::new();
    crate::log("INFO", &format!("scan_installed_mods: scanning {}", install_dir.display()));

    for subdir in &["Mods", "Plugins"] {
        let base = install_dir.join(subdir);
        if !base.exists() { continue; }
        scan_dir_recursive(&base, &mut found);
    }

    crate::log("INFO", &format!("scan_installed_mods: {} files found", found.len()));
    found
}

fn scan_dir_recursive(
    dir: &Path,
    out: &mut Vec<(PathBuf, String, Option<crate::melon_dll::MelonInfo>)>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => { crate::log("WARN", &format!("  can't read dir {}: {}", dir.display(), e)); return; }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Recurse into all subdirectories (including ~Broken, ~Retired, and any
            // user-created subdirs). MelonLoader itself recurses all non-~-prefixed dirs.
            scan_dir_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("dll") {
            let hash = match calculate_md5(&path) {
                Some(h) => h,
                None => { crate::log("WARN", &format!("  can't hash: {}", path.display())); continue; }
            };
            let info = crate::melon_dll::read_melon_info(&path);
            crate::log("INFO", &format!("  found: {}  md5={}  melon={:?}",
                path.display(), &hash[..8], info.as_ref().map(|i| format!("{} v{}", i.name, i.version))));
            out.push((path, hash, info));
        }
    }
}

/// Returns true if the installed version is older than the latest API version
/// AND the mod is in the active (non-quarantined) mods directory.
/// Mods in Broken/ or Retired/ subdirectories are never considered as needing
/// an update — they have been intentionally quarantined.
pub fn mod_has_update(mod_info: &Mod) -> bool {
    // Must be installed
    let installed_path = match &mod_info.installed_file_path {
        Some(p) => p.as_str(),
        None => return false,
    };

    // Mods in Broken/ or Retired/ are quarantined — not eligible for updates
    let path_lower = installed_path.to_lowercase();
    if path_lower.contains("/~broken/") || path_lower.contains("/~retired/") {
        return false;
    }

    // Must have a known installed version to compare
    let installed_ver = match &mod_info.installed_version {
        Some(v) if !v.is_empty() => v.as_str(),
        _ => return false,
    };
    let api_ver = match mod_info.versions.first().and_then(|v| v.mod_version.as_deref()) {
        Some(v) if !v.is_empty() => v,
        _ => return false,
    };

    api::is_newer_version(installed_ver, api_ver)
}

/// Alias kept for call-site compatibility — same logic as mod_has_update.
pub fn mod_version_outdated(mod_info: &Mod) -> bool {
    mod_has_update(mod_info)
}

/// Download a mod and write it to disk. Returns (target_path, raw_bytes).
/// Does NOT resolve dependencies — use install_mod for the full flow.
async fn download_and_save(mod_info: &Mod, install_dir: &Path) -> Result<(PathBuf, Vec<u8>)> {
    let version = mod_info.versions.first().context("Mod has no versions")?;
    let download_link = version.download_link.as_deref().context("Mod has no download link")?;

    // Remove existing file if present
    if let Some(existing) = &mod_info.installed_file_path {
        let p = Path::new(existing);
        if p.exists() { fs::remove_file(p)?; }
    }

    let bytes = crate::api::download_to_bytes(download_link).await
        .with_context(|| format!("Failed to download {}", version.name))?;

    // Derive filename from MelonInfoAttribute inside the DLL, fallback to version.name
    let name_from_dll = crate::melon_dll::extract_melon_info(&bytes)
        .map(|i| i.name)
        .filter(|n| !n.trim().is_empty());
    let base_name = name_from_dll.unwrap_or_else(|| version.name.clone());
    let safe: String = base_name.chars().filter(|c| *c != '/' && *c != '\0').collect();
    let safe = safe.trim().to_string();
    let filename = if safe.is_empty() {
        format!("mod_{}.dll", mod_info._id)
    } else if safe.to_lowercase().ends_with(".dll") {
        safe
    } else {
        format!("{}.dll", safe)
    };

    let subdir  = if version.is_plugin() { "Plugins" } else { "Mods" };
    let subdir2 = if version.is_broken() { "~Broken/" }
                  else if version.is_retired() { "~Retired/" }
                  else { "" };
    let target_dir = install_dir.join(subdir).join(subdir2);
    fs::create_dir_all(&target_dir)?;
    let target_path = target_dir.join(&filename);
    fs::File::create(&target_path)?.write_all(&bytes)?;
    crate::log("INFO", &format!("Saved '{}' → {:?}", version.name, target_path));
    Ok((target_path, bytes))
}

/// Install a mod. After saving, reads the AssemblyRef table from the DLL to find
/// hard dependencies, then automatically downloads any that aren't already on disk.
pub async fn install_mod(mod_info: &Mod, install_dir: &Path, all_mods: &[Mod]) -> Result<PathBuf> {
    let (target_path, bytes) = download_and_save(mod_info, install_dir).await?;

    // ── Auto-resolve dependencies from AssemblyRef table ─────────────────────
    if crate::config::Config::load().auto_install_deps {
        let additional_deps = crate::melon_dll::extract_additional_deps(&bytes);
        if !additional_deps.is_empty() {
            crate::log("INFO", &format!(
                "Mod '{}' has {} dep(s): {:?}",
                mod_info.versions.first().map(|v| v.name.as_str()).unwrap_or("?"),
                additional_deps.len(), additional_deps
            ));
        }

        for dep_name in &additional_deps {
            let already_installed = scan_installed_mods(install_dir)
                .iter()
                .any(|(_, _, info)| {
                    info.as_ref()
                        .map(|i| i.name.eq_ignore_ascii_case(dep_name))
                        .unwrap_or(false)
                });

            if already_installed {
                crate::log("INFO", &format!("  dep '{}' already on disk, skipping", dep_name));
                continue;
            }

            let dep_mod = all_mods.iter().find(|m| {
                m.versions.first()
                    .map(|v| v.name.eq_ignore_ascii_case(dep_name))
                    .unwrap_or(false)
                || m.aliases.iter().any(|a| a.eq_ignore_ascii_case(dep_name))
            });

            match dep_mod {
                Some(dep) => {
                    crate::log("INFO", &format!("  auto-installing dep '{}'", dep_name));
                    if let Err(e) = download_and_save(dep, install_dir).await {
                        crate::log("WARN", &format!(
                            "  failed to auto-install dep '{}': {}", dep_name, e));
                    }
                }
                None => {
                    crate::log("WARN", &format!(
                        "  dep '{}' not found in CVRMG API — install manually", dep_name));
                }
            }
        }
    }

    Ok(target_path)
}

pub fn uninstall_mod(file_path: &str) -> Result<()> {
    let p = Path::new(file_path);
    if p.exists() { fs::remove_file(p).with_context(|| format!("Failed to delete {:?}", p))?; }
    Ok(())
}

// ── Utilities ─────────────────────────────────────────────────────────────────

pub fn open_folder(path: &str) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

pub fn calculate_md5(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(format!("{:x}", md5::compute(&bytes)))
}
