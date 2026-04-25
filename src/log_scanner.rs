//! log_scanner.rs — MelonLoader log analysis, ported from the Lumbot MelonScanner (Java).
//!
//! Reads `MelonLoader/Latest.log` from the ChilloutVR install directory and
//! produces a structured `ScanReport` with categorised findings.

use std::path::{Path, PathBuf};
use std::fs;

// ── Public result types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Ok,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    pub category: String,
    pub message:  String,
}

impl Finding {
    fn ok(cat: &str, msg: &str)   -> Self { Self { severity: Severity::Ok,      category: cat.into(), message: msg.into() } }
    fn info(cat: &str, msg: &str) -> Self { Self { severity: Severity::Info,     category: cat.into(), message: msg.into() } }
    fn warn(cat: &str, msg: &str) -> Self { Self { severity: Severity::Warning,  category: cat.into(), message: msg.into() } }
    fn err(cat: &str, msg: &str)  -> Self { Self { severity: Severity::Error,    category: cat.into(), message: msg.into() } }
}

#[derive(Debug, Clone)]
pub struct LoadedMod {
    pub name:     String,
    pub version:  Option<String>,
    pub author:   Option<String>,
    pub hash:     Option<String>,
    pub assembly: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanReport {
    pub log_path:     PathBuf,
    pub header_lines: Vec<String>,

    pub ml_version:   Option<String>,
    pub game_name:    Option<String>,
    pub game_version: Option<String>,
    pub game_path:    Option<String>,
    pub os_type:      Option<String>,
    pub arch:         Option<String>,
    pub is_il2cpp:    bool,
    pub is_mono:      bool,

    pub loaded_mods:    Vec<LoadedMod>,
    pub loaded_plugins: Vec<LoadedMod>,

    pub findings:  Vec<Finding>,
    pub truncated: bool,
    pub line_count: usize,
}

// ── Entry points ──────────────────────────────────────────────────────────────

pub fn log_path(install_dir: &Path) -> PathBuf {
    install_dir.join("MelonLoader").join("Latest.log")
}

pub fn scan(install_dir: &Path) -> Result<ScanReport, String> {
    let path = log_path(install_dir);
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Could not read log file at {}: {}", path.display(), e))?;
    Ok(parse_log(&content, path))
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn parse_log(content: &str, log_path: PathBuf) -> ScanReport {
    let mut report = ScanReport {
        log_path,
        header_lines:   Vec::new(),
        ml_version:     None,
        game_name:      None,
        game_version:   None,
        game_path:      None,
        os_type:        None,
        arch:           None,
        is_il2cpp:      false,
        is_mono:        false,
        loaded_mods:    Vec::new(),
        loaded_plugins: Vec::new(),
        findings:       Vec::new(),
        truncated:      false,
        line_count:     0,
    };

    // MelonLoader log lines look like:  [HH:MM:SS.mmm] <content>
    // BUT multi-line content is sometimes embedded with literal \n inside a single
    // Windows CRLF line. We split on CRLF first, then on bare \n within a segment.
    let raw_lines: Vec<&str> = content.lines().collect();
    report.line_count = raw_lines.len();

    if report.line_count > 20000 {
        report.truncated = true;
    }

    // Expand embedded \n within lines so we parse each logical sub-line
    let mut lines: Vec<String> = Vec::with_capacity(report.line_count);
    for raw in &raw_lines {
        for sub in raw.split('\n') {
            let s = sub.trim_end_matches('\r').to_string();
            if !s.trim().is_empty() {
                lines.push(s);
            }
        }
    }

    let scan_limit = lines.len().min(20000);

    // Collect header lines (first 25 non-blank)
    {
        let mut h = 0;
        for line in &lines {
            let t = line.trim();
            if !t.is_empty() {
                report.header_lines.push(t.to_string());
                h += 1;
                if h >= 25 { break; }
            }
        }
    }

    // ── Issue accumulators ────────────────────────────────────────────────────
    let mut type_load_failures:  Vec<String> = Vec::new();
    let mut incompatible_pairs:  Vec<String> = Vec::new();
    let mut mods_erroring:       Vec<String> = Vec::new();
    let mut misplaced_mods:      Vec<String> = Vec::new();
    let mut misplaced_plugs:     Vec<String> = Vec::new();
    let mut missing_deps:        Vec<String> = Vec::new();
    let mut duplicates:          Vec<String> = Vec::new();
    let mut old_mods:            Vec<String> = Vec::new();
    let mut known_errors:        Vec<String> = Vec::new();
    // "ModName is missing dependency DepName" pairs
    let mut missing_dep_pairs:   Vec<(String, String)> = Vec::new();

    // ── Mod listing state machine ─────────────────────────────────────────────
    let mut listing_plugins     = false;
    let mut listing_mods        = false;
    let mut pre_listing         = false;
    let mut remaining_count     = 0i32;

    // Temp mod accumulator during the named listing
    let mut tmp_name:     Option<String> = None;
    let mut tmp_version:  Option<String> = None;
    let mut tmp_author:   Option<String> = None;
    let mut tmp_hash:     Option<String> = None;
    let mut tmp_assembly: Option<String> = None;

    // Incompatibility parsing state
    let mut reading_incompat    = false;
    let mut incompat_source:    Option<String> = None;

    // Missing-dependency block state (Lumbot pattern)
    let mut reading_missing_deps     = false;
    let mut missing_dep_mod_source:  Option<String> = None;

    // ── Main parse loop ───────────────────────────────────────────────────────
    let mut i = 0;
    while i < scan_limit {
        let line = lines[i].trim();
        i += 1;

        if line.is_empty() || line.len() > 1200 { continue; }

        let content_part = strip_ts(line);

        // ── Meta: MelonLoader version ─────────────────────────────────────────
        if report.ml_version.is_none() && content_part.starts_with("MelonLoader v") {
            let ver = content_part["MelonLoader v".len()..]
                .split_whitespace().next().unwrap_or("").to_string();
            if !ver.is_empty() { report.ml_version = Some(ver); }
        }

        // ── Meta: OS / Arch / Game ────────────────────────────────────────────
        if content_part.starts_with("OS: ") {
            report.os_type = Some(content_part["OS: ".len()..].trim().to_string());
        }
        if content_part.starts_with("Game Arch:") {
            report.arch = Some(content_part.split("Arch:").nth(1).unwrap_or("").trim().to_string());
        }
        if content_part.starts_with("Game Type:") {
            let lc = content_part.to_lowercase();
            if lc.contains("il2cpp") { report.is_il2cpp = true; }
            if lc.contains("mono")   { report.is_mono   = true; }
        }
        if content_part.starts_with("Game Name:") {
            report.game_name = Some(content_part.split("Name:").nth(1).unwrap_or("").trim().to_string());
        }
        if content_part.starts_with("Game Version:") {
            let v = content_part.split("Version:").nth(1).unwrap_or("").trim().to_string();
            if !v.is_empty() { report.game_version = Some(v); }
        }
        if content_part.contains("Game::BasePath") || content_part.contains("Game::ApplicationPath") {
            if let Some(p) = content_part.split('=').nth(1) {
                let p = p.trim().to_string();
                if report.game_path.is_none() && !p.is_empty() {
                    if p.contains('\'') {
                        known_errors.push("Your game path contains a single-quote (') character which can break MelonLoader.".into());
                    }
                    report.game_path = Some(p);
                }
            }
        }

        // ── Security: compromised ML ──────────────────────────────────────────
        if line.contains("<Transmtn.Get GET api") || line.starts_with("authcookie_") {
            known_errors.push("⚠ COMPROMISED: Your MelonLoader installation is leaking sensitive credentials. Reinstall MelonLoader immediately.".into());
        }

        // ── Type load failures ────────────────────────────────────────────────
        // Pattern:  Failed to load all types in assembly <AssemblyName>, Version=...
        if content_part.starts_with("Failed to load all types in assembly ") {
            let rest = &content_part["Failed to load all types in assembly ".len()..];
            // Assembly name ends at first comma
            let name = rest.split(',').next().unwrap_or(rest).trim().to_string();
            if !name.is_empty() && !type_load_failures.contains(&name) {
                type_load_failures.push(name);
            }
        }

        // ── Missing dependencies block ────────────────────────────────────────
        // "Some mods are missing dependencies, which you may have to install."
        // "- 'ModName' is missing the following dependencies:"
        // "    - 'DepName'"
        if content_part.to_lowercase().contains("some mods are missing dependencies") {
            reading_missing_deps = true;
            continue;
        }
        if reading_missing_deps {
            let trimmed = content_part.trim_start_matches([' ', '-']).trim();
            if trimmed.starts_with('\'') || trimmed.starts_with('"') {
                if trimmed.to_lowercase().contains("is missing the following dependencies") {
                    // "- 'ModName' is missing the following dependencies:"
                    missing_dep_mod_source = extract_quoted(trimmed);
                } else if let Some(ref src) = missing_dep_mod_source.clone() {
                    // "    - 'DepName'"
                    if let Some(dep) = extract_quoted(trimmed) {
                        missing_dep_pairs.push((src.clone(), dep));
                    }
                }
                continue;
            } else if !trimmed.is_empty() && !trimmed.starts_with('[') {
                // Non-bullet non-empty line ends the block
                reading_missing_deps    = false;
                missing_dep_mod_source  = None;
                // fall through
            }
        }

        // ── Incompatibility block ─────────────────────────────────────────────
        // "Some Melons are marked as incompatible with each other."
        // "- 'OSC' is incompatible with the following Melons:"
        // "    - 'CVRParamLib'"
        if content_part.contains("is incompatible with the following Melons:") {
            // Extract the mod that declares incompatibility
            if let Some(name) = extract_quoted(content_part) {
                incompat_source = Some(name);
                reading_incompat = true;
            }
            continue;
        }
        if reading_incompat {
            // Indented bullet:  "    - 'ModName'"
            let trimmed = content_part.trim_start_matches([' ', '-']).trim();
            if trimmed.starts_with('\'') || trimmed.starts_with('"') {
                if let Some(incompatible_with) = extract_quoted(trimmed) {
                    if let Some(ref src) = incompat_source {
                        incompatible_pairs.push(format!(
                            "'{}' is incompatible with '{}'", src, incompatible_with
                        ));
                    }
                }
                continue;
            } else {
                // Non-indented line ends the block
                reading_incompat = false;
                incompat_source  = None;
                // Fall through to process this line normally
            }
        }

        // ── Misplaced mods/plugins ────────────────────────────────────────────
        if content_part.contains("is in the Plugins Folder:") || content_part.contains("Melon is a Plugin") {
            if let Some(name) = content_part.split('\'').nth(1) {
                if !misplaced_mods.contains(&name.to_string()) {
                    misplaced_mods.push(name.to_string());
                }
            }
        }
        if content_part.contains("is in the Mods Folder:") || content_part.contains("Melon is a Mod") {
            if let Some(name) = content_part.split('\'').nth(1) {
                if !misplaced_plugs.contains(&name.to_string()) {
                    misplaced_plugs.push(name.to_string());
                }
            }
        }

        // ── Duplicate mods ────────────────────────────────────────────────────
        if content_part.contains("Duplicate File") || content_part.contains("Duplicate Mod") || content_part.contains("Duplicate Plugin") {
            // Extract filename from path
            let name = line.rsplit('\\').next().unwrap_or("").replace(".dll", "");
            if !name.is_empty() && !duplicates.contains(&name) {
                duplicates.push(name);
            }
        }
        if content_part.contains("An item with the same key has already been added") {
            let name = line.rsplit(':').next().unwrap_or("").trim().to_string();
            if !name.is_empty() && !duplicates.contains(&name) { duplicates.push(name); }
        }

        // ── Missing assemblies ────────────────────────────────────────────────
        // Only match "Could not load file or assembly '...'" — NOT "Could not
        // load type of field '...'" which produces cascading noise like
        // "VideoRemote.VideoRemoteMod:VideoPlayerListMain".
        if content_part.contains("Could not load file or assembly '") {
            if let Some(dep) = content_part.split('\'').nth(1) {
                // Strip ", Version=..." suffix — we only want the assembly name
                let dep = dep.split(',').next().unwrap_or(dep).trim().to_string();
                // Filter out:
                //   • UnityEngine / System.* (runtime internals)
                //   • Anything containing ':' (type field references, not assemblies)
                //   • Anything that looks like a namespace path (contains '.' AND ':')
                let is_real_assembly = !dep.contains("UnityEngine")
                    && !dep.starts_with("System.")
                    && !dep.contains(':')   // "Mod.Class:field" is a type, not an assembly
                    && !dep.is_empty();
                if is_real_assembly && !missing_deps.contains(&dep) {
                    missing_deps.push(dep);
                }
            }
        }

        // ── Old / incompatible mods (failed to resolve) ───────────────────────
        if content_part.contains("[ERROR] Failed to Resolve Melons for") {
            // Path: extract filename stem
            let name = line.rsplit('\\').next().unwrap_or("")
                .split('.').next().unwrap_or("").replace('_', " ");
            if !name.is_empty() {
                if content_part.to_lowercase().contains("exception") && !content_part.to_lowercase().contains("typeloadexception") {
                    if !mods_erroring.contains(&name) { mods_erroring.push(name); }
                } else if !old_mods.contains(&name) {
                    old_mods.push(name);
                }
            }
        }

        // ── Mod throwing errors ───────────────────────────────────────────────
        // Pattern 1: [16:40:54.622] [CVRGoesBrrr] System.NullReferenceException: ...
        //   — a mod-namespaced line followed immediately by an exception class
        // Pattern 2: [time] [ModName] [ERROR] ...
        // Pattern 3: [time] [ERROR] ...
        {
            // Extract the bracketed source tag right after the timestamp: [ModName]
            let after_ts = content_part;
            if after_ts.starts_with('[') {
                let end = after_ts.find(']').unwrap_or(0);
                if end > 0 {
                    let source = &after_ts[1..end];
                    let rest   = after_ts[end+1..].trim();

                    let is_ml_internal = matches!(source,
                        "MelonLoader" | "Il2CppAssemblyGenerator" | "MelonStartScreen" |
                        "WholesomeLoader" | "TotallyWholesome" | "CVRModUpdater.Loader"
                    );

                    // [Mod] [ERROR] or [Mod] [WARNING]
                    if rest.starts_with("[ERROR]") || rest.starts_with("[Error]") {
                        if !is_ml_internal && !mods_erroring.contains(&source.to_string()) {
                            mods_erroring.push(source.replace('_', " ").to_string());
                        }
                    }

                    // [Mod] ExceptionType: message  — detect by exception class names
                    let is_exception = rest.starts_with("System.")
                        || rest.contains("Exception:")
                        || rest.contains("Exception was thrown");
                    if is_exception && !is_ml_internal {
                        let mod_name = source.replace('_', " ").to_string();
                        if !mods_erroring.contains(&mod_name) {
                            mods_erroring.push(mod_name);
                        }
                    }
                }
            }

            // Generic [ERROR] at the top level (no mod tag)
            if after_ts.starts_with("[ERROR]") || after_ts.starts_with("[Error]") {
            }
        }

        // ── Known one-liner error patterns ────────────────────────────────────
        check_known_patterns(line, &mut known_errors);

        // ── Mod listing state machine ─────────────────────────────────────────
        let is_separator = content_part.starts_with("---") && content_part.len() >= 20;

        // "Loading Mods from ..." / "Loading Plugins from ..."
        if !pre_listing && !listing_mods && !listing_plugins
            && (content_part.starts_with("Loading Mods") || content_part.starts_with("Loading Plugins"))
        {
            pre_listing     = true;
            listing_plugins = content_part.contains("Plugin");
            listing_mods    = !listing_plugins;
            continue;
        }

        // "N Mods loaded." / "N Plugins loaded."
        if let Some(count) = parse_loaded_count(content_part) {
            let is_plugin = content_part.contains("Plugin");
            remaining_count  = count;
            listing_mods     = !is_plugin;
            listing_plugins  = is_plugin;
            pre_listing      = false;
            continue;
        }

        // "No Mods loaded." / "0 Mods loaded."
        if content_part.contains("No Mods loaded") || content_part.contains("0 Mods loaded")
            || content_part.contains("No Plugins loaded") || content_part.contains("0 Plugins loaded")
        {
            listing_mods    = false;
            listing_plugins = false;
            pre_listing     = false;
            continue;
        }

        // Inside the listing
        if listing_mods || listing_plugins {
            if is_separator {
                // Commit current mod/plugin if complete
                if let Some(name) = tmp_name.take() {
                    // Discard garbage entries from the pre-load assembly block
                    if !name.starts_with("Melon Assembly loaded")
                        && !name.starts_with("Support Module")
                        && !name.starts_with("Failed to load")
                    {
                        let entry = LoadedMod {
                            name,
                            version:  tmp_version.take(),
                            author:   tmp_author.take(),
                            hash:     tmp_hash.take(),
                            assembly: tmp_assembly.take(),
                        };
                        if listing_plugins { report.loaded_plugins.push(entry); }
                        else               { report.loaded_mods.push(entry); }
                    } else {
                        // Clear temps without committing
                        tmp_version.take(); tmp_author.take();
                        tmp_hash.take(); tmp_assembly.take();
                    }

                    if remaining_count > 0 {
                        remaining_count -= 1;
                        if remaining_count == 0 {
                            listing_mods    = false;
                            listing_plugins = false;
                        }
                    }
                }
                continue;
            }

            // Skip pre-load assembly info lines — not actual named mod entries
            if content_part.starts_with("Melon Assembly loaded:") {
                continue;
            }

            // Hash / Assembly lines
            if content_part.starts_with("SHA256 Hash:") {
                tmp_hash = Some(content_part["SHA256 Hash:".len()..].trim().trim_matches('\'').to_string());
                continue;
            }
            if content_part.starts_with("Assembly:") {
                tmp_assembly = Some(content_part["Assembly:".len()..].trim().to_string());
                continue;
            }
            // "by Author"
            if content_part.starts_with("by ") {
                tmp_author = Some(content_part[3..].trim().to_string());
                continue;
            }

            // Misplaced inside listing
            if content_part.contains("Failed to load Melon '") {
                if content_part.contains("Melon is a Plugin") {
                    if let Some(n) = content_part.split('\'').nth(1) { misplaced_plugs.push(n.to_string()); }
                } else if content_part.contains("Melon is a Mod") {
                    if let Some(n) = content_part.split('\'').nth(1) { misplaced_mods.push(n.to_string()); }
                }
                continue;
            }

            // Name + version line: "ModName v1.2.3"
            if tmp_name.is_none() && !content_part.is_empty() && !content_part.starts_with('[') {
                // Try to split on " v" to get name + version
                if let Some(v_pos) = find_version_split(content_part) {
                    tmp_name    = Some(content_part[..v_pos].trim().to_string());
                    tmp_version = Some(content_part[v_pos+2..].trim().to_string());
                } else {
                    tmp_name = Some(content_part.trim().to_string());
                }
                continue;
            }
        }
    }

    // ── Compile findings ──────────────────────────────────────────────────────

    // 1. Header info (always first)
    match &report.ml_version {
        Some(v) => report.findings.push(Finding::info("MelonLoader", &format!("Version: {}", v))),
        None    => report.findings.push(Finding::warn("MelonLoader",
            "MelonLoader version not found — the log may be incomplete or MelonLoader may not have started.")),
    }

    if let Some(game) = &report.game_name {
        let mut msg = format!("Game: {}", game);
        if let Some(ver) = &report.game_version { msg.push_str(&format!("  v{}", ver)); }
        if report.is_il2cpp { msg.push_str("  [Il2Cpp]"); }
        if report.is_mono   { msg.push_str("  [Mono]"); }
        report.findings.push(Finding::info("Game", &msg));
    }
    if let Some(os) = &report.os_type {
        let mut msg = format!("OS: {}", os);
        if let Some(arch) = &report.arch { msg.push_str(&format!("  Arch: {}", arch)); }
        report.findings.push(Finding::info("System", &msg));
    }

    let mod_count  = report.loaded_mods.len();
    let plug_count = report.loaded_plugins.len();
    if mod_count > 0 || plug_count > 0 {
        report.findings.push(Finding::info("Mods Loaded",
            &format!("{} mod(s)  •  {} plugin(s)", mod_count, plug_count)));
    }

    // 2. Issues (errors first, then warnings)

    if !known_errors.is_empty() {
        for e in &known_errors {
            report.findings.push(Finding::err("Known Error", e));
        }
    }

    if !duplicates.is_empty() {
        report.findings.push(Finding::err("Duplicate Mods",
            &format!("Remove duplicate mod files:\n{}", fmt_list(&duplicates))));
    }

    if !misplaced_mods.is_empty() {
        report.findings.push(Finding::err("Misplaced Mods",
            &format!("These mods are in the Plugins folder — move them to Mods/:\n{}", fmt_list(&misplaced_mods))));
    }
    if !misplaced_plugs.is_empty() {
        report.findings.push(Finding::err("Misplaced Plugins",
            &format!("These plugins are in the Mods folder — move them to Plugins/:\n{}", fmt_list(&misplaced_plugs))));
    }

    if !incompatible_pairs.is_empty() {
        report.findings.push(Finding::err("Incompatible Mods",
            &format!("The following mods conflict and won't both load:\n{}", fmt_list(&incompatible_pairs))));
    }

    if !missing_dep_pairs.is_empty() {
        // Group by dependency name: BTKUILib → [ASTExtension, JoinMe, ...]
        let mut dep_to_mods: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for (mod_name, dep) in &missing_dep_pairs {
            dep_to_mods
                .entry(dep.clone())
                .or_default()
                .push(mod_name.clone());
        }
        // Deduplicate mod lists and sort
        for mods in dep_to_mods.values_mut() {
            mods.sort();
            mods.dedup();
        }
        let lines: Vec<String> = dep_to_mods.iter()
            .map(|(dep, mods)| format!("{} — needed by: {}", dep, mods.join(", ")))
            .collect();
        report.findings.push(Finding::err("Missing Dependencies",
            &format!("Install the following missing dependencies:\n{}", fmt_list(&lines))));
    }

    if !type_load_failures.is_empty() {
        report.findings.push(Finding::err("Assembly Load Failures",
            &format!(
                "Failed to load all types in these assemblies — the mod may be outdated or incompatible:\n{}",
                fmt_list(&type_load_failures)
            )));
    }

    if !missing_deps.is_empty() {
        // Only show deps not already covered by the structured missing-deps block above
        let already_shown: std::collections::HashSet<&str> =
            missing_dep_pairs.iter().map(|(_, d)| d.as_str()).collect();
        let filtered: Vec<_> = missing_deps.iter()
            .filter(|d| !d.contains("UnityEngine") && !d.starts_with("System."))
            .filter(|d| !already_shown.contains(d.as_str()))
            .collect();
        if !filtered.is_empty() {
            report.findings.push(Finding::err("Missing Assemblies",
                &format!("Could not load:\n{}", fmt_list_ref(&filtered))));
        }
    }

    if !old_mods.is_empty() {
        report.findings.push(Finding::warn("Incompatible Mods",
            &format!("These mods failed to load (likely outdated for current ML/game version):\n{}", fmt_list(&old_mods))));
    }

    if !mods_erroring.is_empty() {
        report.findings.push(Finding::warn("Mods Throwing Errors",
            &format!("These mods logged exceptions during this session:\n{}", fmt_list(&mods_erroring))));
    }

    if report.truncated {
        report.findings.push(Finding::warn("Log Truncated",
            &format!("Log exceeds 20,000 lines ({} total). Some entries may not have been scanned.", report.line_count)));
    }

    // 3. Summary
    let issue_count = report.findings.iter()
        .filter(|f| f.severity == Severity::Error || f.severity == Severity::Warning)
        .count();

    if issue_count == 0 && report.ml_version.is_some() {
        report.findings.push(Finding::ok("Summary",
            &format!("No issues detected. {} mod(s)/plugin(s) loaded cleanly.", mod_count + plug_count)));
    } else if issue_count > 0 {
        report.findings.push(Finding::err("Summary",
            &format!("{} issue(s) found. Review the findings above.", issue_count)));
    }

    report
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Strip the `[HH:MM:SS.mmm] ` timestamp prefix from a log line.
fn strip_ts(line: &str) -> &str {
    // Lines start with [digits/colons/dots] followed by a space
    if !line.starts_with('[') { return line; }
    if let Some(close) = line.find(']') {
        let rest = &line[close + 1..];
        rest.trim_start_matches(' ')
    } else {
        line
    }
}

/// Find the position of " v" that precedes a version number (e.g. "ModName v1.2.3")
fn find_version_split(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut pos = 0;
    while pos + 2 < bytes.len() {
        if bytes[pos] == b' ' && bytes[pos+1] == b'v' && bytes[pos+2].is_ascii_digit() {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

/// Parse "N Mods loaded." or "N Plugins loaded." returning N.
fn parse_loaded_count(s: &str) -> Option<i32> {
    // e.g. "45 Mods loaded."  or "1 Plugin loaded."
    let first = s.split_whitespace().next()?;
    let n: i32 = first.parse().ok()?;
    if s.contains("Mod") || s.contains("Plugin") { Some(n) } else { None }
}

/// Extract the first single- or double-quoted string from a line.
fn extract_quoted(s: &str) -> Option<String> {
    for q in &['\'', '"'] {
        if let Some(start) = s.find(*q) {
            if let Some(end) = s[start+1..].find(*q) {
                return Some(s[start+1..start+1+end].to_string());
            }
        }
    }
    None
}

fn check_known_patterns(line: &str, out: &mut Vec<String>) {
    let patterns: &[(&str, &str)] = &[
        ("System.BadImageFormatException",
         "An invalid or incompatible assembly is in your Mods or Plugins folder."),
        ("Il2CppAssemblyGenerator",
         "Il2CppAssemblyGenerator failed. Delete the MelonLoader folder and reinstall MelonLoader."),
        ("Applied USER32.dll::SetTimer patch",
         "MelonLoader may have crashed due to the Start Screen. Try the launch option: --melonloader.disablestartscreen"),
        ("Contacting RemoteAPI...",
         "Unity failed to initialise graphics. Make sure your GPU drivers are up to date."),
    ];

    for (needle, message) in patterns {
        if line.contains(needle) {
            let msg = message.to_string();
            if !out.contains(&msg) { out.push(msg); }
        }
    }
}

fn fmt_list(items: &[String]) -> String {
    items.iter().map(|s| format!("• {}", s)).collect::<Vec<_>>().join("\n")
}
fn fmt_list_ref(items: &[&String]) -> String {
    items.iter().map(|s| format!("• {}", s)).collect::<Vec<_>>().join("\n")
}
