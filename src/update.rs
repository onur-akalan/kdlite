//! Updates from the GitHub releases of the project.
//!
//! The service looks at the latest release once a day and, if it is newer,
//! shows a notification. Nothing is installed unless the user asks (the
//! notification's or the window's Update button, or `kdlite update`). Then
//! the release archive is downloaded, its SHA-256 checksum is checked, the
//! new program is test-run, and the archive's own install.sh is run (it
//! replaces ~/.local/bin/kdlite and rewrites the menu entries). The service
//! restarts itself with the new program once no sync is running.
//!
//! Only installs from a release archive are updated. When ~/.local/bin/kdlite
//! is a link into a source checkout, kdlite only reports the new version.

use crate::config;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const REPO: &str = "onur-akalan/kdlite";

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    /// Release date, YYYY-MM-DD.
    pub date: String,
    /// Release notes (Markdown).
    pub notes: String,
    archive: String,
    checksum: String,
}

/// Where release information is read from. KDLITE_UPDATE_URL overrides it
/// (for testing against a local server).
fn latest_url() -> String {
    std::env::var("KDLITE_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{}/releases/latest", REPO))
}

/// Downloads must come from this project's release files.
fn allowed(url: &str) -> bool {
    if std::env::var_os("KDLITE_UPDATE_URL").is_some() {
        return url.starts_with("http://127.0.0.1:") || url.starts_with(&format!("https://github.com/{}/releases/download/", REPO));
    }
    url.starts_with(&format!("https://github.com/{}/releases/download/", REPO))
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(format!("kdlite/{}", current()))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()?)
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// true if `new` is a higher version than `old`.
pub fn newer(new: &str, old: &str) -> bool {
    match (parse_version(new), parse_version(old)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Reads the latest release. Returns it only when it is newer than this
/// program; drafts and pre-releases are never offered.
pub fn check() -> Result<Option<Release>> {
    let resp = client()?
        .get(latest_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .context(crate::tr!("could not reach GitHub"))?;
    if !resp.status().is_success() {
        bail!("{}", crate::tr!("GitHub answered {}", resp.status()));
    }
    let v: Value = resp.json().context(crate::tr!("unexpected answer from GitHub"))?;
    if v["draft"].as_bool().unwrap_or(false) || v["prerelease"].as_bool().unwrap_or(false) {
        return Ok(None);
    }
    let tag = v["tag_name"].as_str().unwrap_or("");
    let version = tag.trim_start_matches('v').to_string();
    if !newer(&version, current()) {
        return Ok(None);
    }
    let name = format!("kdlite-{}-x86_64-linux.tar.gz", version);
    let asset = |n: &str| {
        v["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(n)))
            .and_then(|x| x["browser_download_url"].as_str())
            .map(str::to_string)
    };
    let (Some(archive), Some(checksum)) = (asset(&name), asset(&format!("{}.sha256", name))) else {
        bail!("{}", crate::tr!("release {} has no download for this computer", version));
    };
    if !allowed(&archive) || !allowed(&checksum) {
        bail!("{}", crate::tr!("release {} points to an unexpected download address", version));
    }
    Ok(Some(Release {
        version,
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        notes: v["body"].as_str().unwrap_or("").chars().take(20000).collect(),
        date: v["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
        archive,
        checksum,
    }))
}

pub fn installed_path() -> PathBuf {
    config::home().join(".local/bin/kdlite")
}

/// None if this installation can update itself, otherwise the reason.
pub fn blocked() -> Option<String> {
    if std::env::consts::ARCH != "x86_64" {
        return Some(crate::tr!("updates are only published for x86_64 computers"));
    }
    let path = installed_path();
    match path.symlink_metadata() {
        Ok(m) if m.file_type().is_symlink() => Some(crate::tr!("kdlite was installed from source; update it with git pull and install.sh")),
        Ok(m) if m.is_file() => None,
        _ => Some(crate::tr!("kdlite is not installed in ~/.local/bin; run install.sh from the release archive")),
    }
}

fn work_dir() -> PathBuf {
    match std::env::var_os("XDG_CACHE_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("kdlite-update"),
        _ => config::home().join(".cache/kdlite-update"),
    }
}

fn download(url: &str, to: &Path) -> Result<()> {
    let mut resp = client()?.get(url).send()?.error_for_status()?;
    let mut f = std::fs::File::create(to)?;
    std::io::copy(&mut resp, &mut f)?;
    f.sync_all()?;
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

/// Downloads, checks and installs the release. Afterwards the program at
/// installed_path() is the new version; the running process is unchanged.
pub fn install(rel: &Release) -> Result<()> {
    if let Some(why) = blocked() {
        bail!("{}", why);
    }
    let dir = work_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let res = install_in(rel, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    res
}

fn install_in(rel: &Release, dir: &Path) -> Result<()> {
    let name = format!("kdlite-{}-x86_64-linux", rel.version);
    let archive = dir.join(format!("{}.tar.gz", name));
    let sumfile = dir.join("sha256");
    download(&rel.checksum, &sumfile).context(crate::tr!("could not download the checksum"))?;
    download(&rel.archive, &archive).context(crate::tr!("could not download the update"))?;
    let expected = std::fs::read_to_string(&sumfile)?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    if expected.len() != 64 || sha256(&archive)? != expected {
        bail!("{}", crate::tr!("the downloaded update is damaged (checksum does not match)"));
    }
    let out = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(dir).arg("--no-same-owner").output()?;
    if !out.status.success() {
        bail!("{}", crate::tr!("could not unpack the update: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let unpacked = dir.join(&name);
    let program = unpacked.join("kdlite");
    let script = unpacked.join("install.sh");
    if !program.is_file() || !script.is_file() {
        bail!("{}", crate::tr!("the update archive is incomplete"));
    }
    // The new program must start and report the expected version.
    let out = Command::new(&program).arg("--version").output().context(crate::tr!("the new version does not start"))?;
    let said = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || said.split_whitespace().nth(1) != Some(rel.version.as_str()) {
        bail!("{}", crate::tr!("the new version does not start correctly"));
    }
    let out = Command::new("bash")
        .arg(&script)
        .current_dir(&unpacked)
        .env_remove("KDLITE_UPDATE_URL")
        .output()
        .context(crate::tr!("could not run the installer"))?;
    log::info!("install.sh: {}", String::from_utf8_lossy(&out.stdout).trim().replace('\n', " | "));
    if !out.status.success() {
        bail!("{}", crate::tr!("the installer failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Written before restarting into the new version, read once by it.
fn note_path() -> PathBuf {
    config::config_dir().join("updated-from")
}

pub fn leave_note() {
    let _ = std::fs::create_dir_all(config::config_dir());
    let _ = std::fs::write(note_path(), current());
}

/// The version that was replaced, if this start follows an update.
pub fn take_note() -> Option<String> {
    let old = std::fs::read_to_string(note_path()).ok()?;
    let _ = std::fs::remove_file(note_path());
    let old = old.trim().to_string();
    (old != current()).then_some(old)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.3.0", "0.2.1"));
        assert!(newer("v0.2.10", "0.2.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.2.1", "0.2.1"));
        assert!(!newer("0.2.0", "0.2.1"));
        assert!(!newer("garbage", "0.2.1"));
    }
}
