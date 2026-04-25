use anyhow::{Result, Context};
use reqwest::Client;
use once_cell::sync::Lazy;
use crate::models::{Mod, FlagEntry, GitHubRelease};
use crate::APP_VERSION;

pub const CVRMG_MODS_JSON: &str = "https://api.cvrmg.com/v1/mods";
pub const CVRMG_FLAGS_JSON: &str = "https://gist.githubusercontent.com/Nirv-git/1963e20d855c401349820a93b4d2639b/raw/cvrModFlags.json";
pub const MELON_LOADER_URL: &str = "https://github.com/LavaGang/MelonLoader/releases/latest/download/MelonLoader.x64.zip";
pub const MELON_LOADER_RELEASES_API: &str = "https://api.github.com/repos/LavaGang/MelonLoader/releases/latest";

pub static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent(format!("CVRMelonAssistant-Linux/{}", APP_VERSION))
        .timeout(std::time::Duration::from_secs(240))
        .build()
        .expect("Failed to build HTTP client")
});

pub async fn fetch_mods() -> Result<Vec<Mod>> {
    let resp = HTTP_CLIENT.get(CVRMG_MODS_JSON).send().await?;
    let body = resp.text().await?;
    let mut mods: Vec<Mod> = serde_json::from_str(&body)?;

    mods.sort_by(|a, b| {
        let cat_cmp = a.display_category().cmp(&b.display_category());
        if cat_cmp != std::cmp::Ordering::Equal {
            return cat_cmp;
        }
        let a_name = a.versions.first().map(|v| v.name.as_str()).unwrap_or("");
        let b_name = b.versions.first().map(|v| v.name.as_str()).unwrap_or("");
        a_name.cmp(b_name)
    });

    Ok(mods)
}

pub async fn fetch_flags() -> Result<Vec<FlagEntry>> {
    let resp = HTTP_CLIENT.get(CVRMG_FLAGS_JSON).send().await?;
    let body = resp.text().await?;
    Ok(serde_json::from_str(&body)?)
}

pub async fn fetch_melon_loader_release() -> Result<GitHubRelease> {
    let resp = HTTP_CLIENT
        .get(MELON_LOADER_RELEASES_API)
        .send()
        .await
        .context("Failed to reach GitHub API")?;
    let body = resp.text().await?;
    let release: GitHubRelease = serde_json::from_str(&body)
        .context("Failed to parse GitHub release JSON")?;
    Ok(release)
}

pub async fn download_to_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = HTTP_CLIENT.get(url).send().await?;
    let bytes = resp.bytes().await?;
    Ok(bytes.to_vec())
}

pub fn flag_symbol(flag: i32) -> &'static str {
    match flag {
        1 => "★",
        2 => "♥",
        3 => "ⓘ",
        _ => "",
    }
}

/// Compare two version strings. Returns true if `remote` is strictly newer than `local`.
/// Handles semver (1.2.3), semver with pre-release (1.2.3-beta.1), and 4-part (1.2.3.4).
/// Strips leading 'v' from either string.
pub fn is_newer_version(local: &str, remote: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

#[derive(Eq, PartialEq, Debug)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    build: u64,
    /// Pre-release: None = stable (sorts higher), Some(tag) = pre-release (sorts lower)
    pre:   Option<String>,
}

fn parse_version(s: &str) -> Version {
    let s = s.trim().trim_start_matches('v');
    // Split off pre-release tag e.g. "1.2.3-alpha.1" → ("1.2.3", Some("alpha.1"))
    let (numeric, pre) = match s.find('-') {
        Some(i) => (&s[..i], Some(s[i + 1..].to_string())),
        None    => (s, None),
    };
    let parts: Vec<u64> = numeric.split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    Version {
        major: parts.first().copied().unwrap_or(0),
        minor: parts.get(1).copied().unwrap_or(0),
        patch: parts.get(2).copied().unwrap_or(0),
        build: parts.get(3).copied().unwrap_or(0),
        // Stable (no pre-release) sorts HIGHER than any pre-release.
        // We encode this by flipping: None → high sentinel, Some → low.
        pre:   pre,
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare numeric parts first
        let num_cmp = (self.major, self.minor, self.patch, self.build)
            .cmp(&(other.major, other.minor, other.patch, other.build));
        if num_cmp != std::cmp::Ordering::Equal { return num_cmp; }

        // Stable > pre-release
        match (&self.pre, &other.pre) {
            (None,    None)    => std::cmp::Ordering::Equal,
            (None,    Some(_)) => std::cmp::Ordering::Greater, // stable > pre
            (Some(_), None)    => std::cmp::Ordering::Less,    // pre < stable
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}
