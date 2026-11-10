//! Kurulum sihirbazi: ayar dosyasi yokken (ya da token / surucu bosken)
//! `kdlite tray` bu kipte acilir. Pencere token'i sorar, suruculeri ve kok
//! klasoru sectirir; ayar dosyasi 600 izinle yazilir ve servis normal kipte
//! yeniden baslar. Yalnizca 127.0.0.1 uzerinde dinler; token gunluge yazilmaz.

use crate::api::Api;
use crate::config;
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use tiny_http::{Header, Method, Response, Server};

/// Ayar dosyasi yoksa ya da token / drive_id doldurulmamissa true.
pub fn needed(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return !path.exists();
    };
    let Ok(v) = text.parse::<toml::Table>() else {
        return false; // bozuk dosya: normal hata mesaji gosterilsin
    };
    let token_empty = v.get("token").and_then(|t| t.as_str()).is_none_or(|t| t.trim().is_empty());
    let drive_zero = v.get("drive_id").and_then(|d| d.as_integer()).unwrap_or(0) == 0;
    token_empty || drive_zero
}

fn base_url() -> String {
    std::env::var("KDLITE_BASE_URL").unwrap_or_else(|_| "https://api.infomaniak.com".into())
}

fn clean_token(t: &str) -> Result<String> {
    let t = t.trim();
    if t.is_empty() {
        bail!("{}", tr!("the token is empty"));
    }
    if t.len() > 512 || !t.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~+/=".contains(c)) {
        bail!("{}", tr!("the token contains invalid characters"));
    }
    Ok(t.to_string())
}

fn explain(e: anyhow::Error) -> anyhow::Error {
    let s = format!("{:#}", e);
    if s.contains("HTTP 401") || s.contains("HTTP 403") {
        anyhow!("{}", tr!("the token was not accepted; make sure it was created with the drive scope"))
    } else {
        anyhow!("{}", tr!("could not reach kDrive: {}", s))
    }
}

/// `key = ...` satirini degistirir; yoksa sona ekler.
pub fn set_line(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let is_key = l.trim_start().strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('='));
            if is_key && !found {
                found = true;
                format!("{} = {}", key, value)
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(format!("{} = {}", key, value));
    }
    out.join("\n") + "\n"
}

fn save(path: &Path, token: &str, drive_id: u64, root_id: u64) -> Result<()> {
    let old = std::fs::read_to_string(path).unwrap_or_else(|_| config::TEMPLATE.to_string());
    let mut text = set_line(&old, "token", &format!("\"{}\"", token));
    text = set_line(&text, "drive_id", &drive_id.to_string());
    text = set_line(&text, "root_id", &root_id.to_string());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .context(tr!("could not write the settings file"))?;
    f.write_all(text.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn handle(path: &Path, url: &str, body: &Value) -> Result<Value> {
    let s = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let n = |k: &str| body.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    match url {
        "/api/setup/drives" => {
            let token = clean_token(s("token"))?;
            let drives = Api::new(&base_url(), 0, &token)?.drives().map_err(explain)?;
            if drives.is_empty() {
                bail!("{}", tr!("this token cannot access any kDrive"));
            }
            Ok(json!({ "drives": drives.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>() }))
        }
        "/api/setup/folders" => {
            let token = clean_token(s("token"))?;
            let mut dirs: Vec<Value> = Api::new(&base_url(), n("drive_id"), &token)?
                .list_dir(1)
                .map_err(explain)?
                .into_iter()
                .filter(|e| e.is_dir)
                .map(|e| json!({ "id": e.id, "name": e.name }))
                .collect();
            dirs.sort_by_key(|d| d["name"].as_str().unwrap_or("").to_lowercase());
            Ok(json!({ "dirs": dirs }))
        }
        "/api/setup/save" => {
            let token = clean_token(s("token"))?;
            let (drive, root) = (n("drive_id"), n("root_id").max(1));
            if drive == 0 {
                bail!("{}", tr!("no drive selected"));
            }
            // Kaydetmeden once secimin calistigini dogrula.
            Api::new(&base_url(), drive, &token)?.list_dir(root).map_err(explain)?;
            save(path, &token, drive, root)?;
            Ok(json!({ "saved": true }))
        }
        _ => bail!("bulunamadi"),
    }
}

/// Sihirbaz sunucusunu calistirir. Kurulum bitince sureci normal kipte
/// yeniden baslatir (geri donmez); pencere kapatilip cikilirsa Ok doner.
pub fn run(path: PathBuf, port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let server = Server::http(&addr).map_err(|e| anyhow!("{} dinlenemedi: {}", addr, e))?;
    let mut window = crate::tray::spawn_window(port)?;
    log::info!("kurulum sihirbazi acildi");
    loop {
        let Some(mut req) = server.recv_timeout(std::time::Duration::from_secs(1))? else {
            if window.try_wait().is_ok_and(|s| s.is_some()) {
                return Ok(()); // pencere kapatildi: kurulumdan vazgecildi
            }
            continue;
        };
        let header = |name: &str| {
            req.headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str().to_string())
        };
        let host_ok = header("Host").is_some_and(|h| h == addr || h == format!("localhost:{}", port));
        let is_post = *req.method() == Method::Post;
        let url = req.url().split('?').next().unwrap_or("").to_string();
        let mut saved = false;
        let (code, out) = if !host_ok || (is_post && header("X-Kdlite").is_none()) {
            (403, json!({ "error": "gecersiz istek" }))
        } else if !is_post {
            (200, json!({ "setup": true, "lang": crate::i18n::lang() }))
        } else if url == "/api/quit" {
            let _ = req.respond(Response::from_string("{}"));
            return Ok(());
        } else {
            let mut text = String::new();
            let _ = req.as_reader().take(64 * 1024).read_to_string(&mut text);
            let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            match handle(&path, &url, &body) {
                Ok(v) => {
                    saved = url == "/api/setup/save";
                    (200, v)
                }
                Err(e) => (400, json!({ "error": format!("{:#}", e) })),
            }
        };
        let resp = Response::from_string(out.to_string())
            .with_status_code(code)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
        let _ = req.respond(resp);
        if saved {
            // Ayni portu birakip normal kipte yeniden basla. Pencere acik
            // kaldigi icin ikincisi acilmasin diye --hidden eklenir.
            drop(server);
            let mut args: Vec<String> = std::env::args().skip(1).collect();
            if !args.iter().any(|a| a == "--hidden") {
                args.push("--hidden".into());
            }
            let err = std::process::Command::new(std::env::current_exe()?).args(args).exec();
            return Err(anyhow!("yeniden baslatilamadi: {}", err));
        }
    }
}
