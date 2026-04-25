use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mod {
    pub _id: i64,
    #[serde(default)]
    pub upload_date: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub versions: Vec<ModVersion>,
    // Runtime fields (not from JSON)
    #[serde(skip)]
    pub installed_file_path: Option<String>,
    #[serde(skip)]
    pub installed_version: Option<String>,
    #[serde(skip)]
    pub installed_in_broken_dir: bool,
    #[serde(skip)]
    pub installed_in_retired_dir: bool,
    #[serde(skip)]
    pub flag: i32,
    /// True for mods found on disk that have no matching CVRMG API entry
    #[serde(skip)]
    pub is_unverified: bool,
}

impl Mod {
    pub fn display_category(&self) -> String {
        self.category.clone().unwrap_or_else(|| "Uncategorized".to_string())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModVersion {
    #[serde(default)]
    pub _version: i64,
    pub name: String,
    #[serde(rename = "modVersion", default)]
    pub mod_version: Option<String>,
    #[serde(rename = "modType", default)]
    pub mod_type: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "downloadLink", default)]
    pub download_link: Option<String>,
    #[serde(rename = "sourceLink", default)]
    pub source_link: Option<String>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(rename = "updateDate", default)]
    pub update_date: Option<String>,
    #[serde(rename = "ChilloutVRVersion", default)]
    pub chillout_vr_version: Option<String>,
    #[serde(rename = "loaderVersion", default)]
    pub loader_version: Option<String>,
    #[serde(rename = "approvalStatus", default)]
    pub approval_status: i32,
}

impl ModVersion {
    pub fn is_broken(&self) -> bool {
        self.approval_status == 2
    }
    pub fn is_retired(&self) -> bool {
        self.approval_status == 3
    }
    pub fn is_plugin(&self) -> bool {
        self.mod_type.as_deref()
            .map(|t| t.eq_ignore_ascii_case("plugin"))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlagEntry {
    pub _id: i64,
    pub flag: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone)]
pub struct InstalledModInfo {
    pub name: String,
    pub version: String,
    pub author: String,
}
