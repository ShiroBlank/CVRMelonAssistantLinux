use std::path::PathBuf;
use std::fs;
use std::io::Write;
use serde::{Deserialize, Serialize};
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub install_folder: Option<String>,
    pub store_type: Option<String>,
    pub close_on_finish: bool,
    pub language_code: Option<String>,
    /// Show a confirmation dialog before uninstalling mods (default: true)
    #[serde(default = "default_true")]
    pub confirm_uninstall: bool,
    /// Show all broken/retired API mods in a single "Broken / Retired" category
    /// instead of their normal category (default: true)
    #[serde(default = "default_true")]
    pub show_broken_retired_category: bool,
    /// Show mods whose files are in Mods/~Broken or Mods/~Retired on disk (default: false)
    #[serde(default)]
    pub show_quarantined_mods: bool,
    /// Whether the user has already seen the Debug tab intro dialog (default: false)
    #[serde(default)]
    pub debug_intro_shown: bool,
    /// Automatically download and install missing dependencies when installing a mod (default: true)
    #[serde(default = "default_true")]
    pub auto_install_deps: bool,
}

fn default_true() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            install_folder: None,
            store_type: None,
            close_on_finish: false,
            language_code: None,
            confirm_uninstall: true,
            show_broken_retired_category: true,
            show_quarantined_mods: false,
            debug_intro_shown: false,
            auto_install_deps: true,
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("CVRMelonAssistant");
        config_dir.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(data) = fs::read_to_string(&path) {
            // If JSON parse fails (e.g. corrupted file), fall back to defaults
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            // No config file yet — use defaults (confirm_uninstall = true)
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&path, data)?;
        Ok(())
    }

    pub fn log_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("CVRMelonAssistant");
        config_dir.join("log.log")
    }

    pub fn log(message: &str, severity: &str) {
        let path = Self::log_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ts = unix_secs_to_timestamp(secs);
        let line = format!("[{}][{}] {}\n", ts, severity.to_uppercase(), message);
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                f.write_all(line.as_bytes())?;
                f.flush()
            });
    }
}

/// Convert a UNIX timestamp (seconds) to a UTC datetime string without any
/// external time crate (e.g. "2025-04-25 14:32:01").
fn unix_secs_to_timestamp(secs: u64) -> String {
    // Days since 1970-01-01
    let days_since_epoch = secs / 86400;
    let time_of_day      = secs % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;

    // Gregorian calendar calculation
    let mut year = 1970u64;
    let mut remaining_days = days_since_epoch;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining_days < days_in_year { break; }
        remaining_days -= days_in_year;
        year += 1;
    }
    let month_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in &month_days {
        if remaining_days < md { break; }
        remaining_days -= md;
        month += 1;
    }
    let day = remaining_days + 1;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, hh, mm, ss)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
