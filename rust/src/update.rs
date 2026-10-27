use serde_json::Value;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASES_API: &str = "https://api.github.com/repos/shawnrodis/SmartLifeTuyaDesktop/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/shawnrodis/SmartLifeTuyaDesktop/releases/latest";
pub const UPDATE_PERIOD_MS: i64 = 24 * 60 * 60 * 1000;

pub fn expected_update_asset_name_for(version: &str, os: &str) -> String {
    if os == "macos" {
        format!("SmartLifeTuyaDesktop-v{}.dmg", version)
    } else {
        format!("SmartLifeTuyaDesktop-Setup-v{}.exe", version)
    }
}

pub fn expected_update_asset_name(version: &str) -> String {
    expected_update_asset_name_for(version, std::env::consts::OS)
}

/// Pick this platform's asset out of a release's asset list —
/// an exact name match (the same strictness the download
/// verification applies again before anything runs/opens).
pub fn select_update_asset<'a>(assets: &'a [Value], version: &str, os: &str) -> Option<&'a Value> {
    let want = expected_update_asset_name_for(version, os);
    assets.iter().find(|a| {
        a.get("name")
            .and_then(|n| n.as_str())
            .map(|n| n == want)
            .unwrap_or(false)
    })
}

/// What "install" means once the asset is downloaded & verified.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UpdateInstall {
    /// Windows: run the Setup .exe, then exit so it can replace
    /// the running app.
    RunInstallerAndExit,
    /// macOS: open the .dmg and let the user drag the app into
    /// Applications; the app keeps running.
    OpenDiskImage,
}

pub fn update_install_for(os: &str) -> UpdateInstall {
    if os == "macos" {
        UpdateInstall::OpenDiskImage
    } else {
        UpdateInstall::RunInstallerAndExit
    }
}

pub fn parse_version(s: &str) -> Vec<u64> {
    s.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// True when `latest` is a strictly newer X.Y.Z than `current`
/// (numeric: 1.10.0 > 1.9.0).
pub fn version_newer(latest: &str, current: &str) -> bool {
    let a = parse_version(latest);
    let b = parse_version(current);
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub page_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
}

/// Pull THIS platform's update asset out of a GitHub "latest
/// release" response (the Setup .exe on Windows, the .dmg on
/// macOS — select_update_asset).
pub fn parse_release(v: &Value) -> Option<ReleaseInfo> {
    let tag = v.get("tag_name")?.as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    let page_url = v.get("html_url")?.as_str()?.to_string();
    let assets = v.get("assets")?.as_array()?;
    let asset = select_update_asset(assets, &version, std::env::consts::OS)?;
    Some(ReleaseInfo {
        version,
        page_url,
        asset_name: asset.get("name")?.as_str()?.to_string(),
        asset_url: asset.get("browser_download_url")?.as_str()?.to_string(),
        asset_size: asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
    })
}

