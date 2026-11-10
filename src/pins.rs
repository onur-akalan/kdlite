//! Sabitleme: listedeki dosya ve klasorler her zaman yerelde tutulur
//! (cevrimdisi erisim) ve onbellek temizliginden etkilenmez.

use crate::api::Entry;
use crate::config;
use crate::fs::Inner;
use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Liste degisince indirme dongusunu hemen uyandirir.
static KICK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub fn wake() {
    *KICK.0.lock().unwrap() = true;
    KICK.1.notify_all();
}

/// `path`, listedeki bir yolun kendisi ya da altinda mi; oyleyse o yol.
pub fn covering<'a>(pins: &'a [String], path: &str) -> Option<&'a String> {
    pins.iter().find(|p| path == p.as_str() || path.starts_with(&format!("{}/", p)))
}

/// Bir bulut yolunun altindaki butun dosyalar.
pub fn files_under(inner: &Inner, path: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Ok((ino, entry)) = inner.resolve(path) {
        collect(inner, ino, entry, &mut out);
    }
    out
}

const INTERVAL: Duration = Duration::from_secs(600);
/// Diskte bundan az bos yer kalirsa sabitleme indirmeleri durur.
const MIN_FREE: u64 = 10 * 1024 * 1024 * 1024;

pub fn path() -> PathBuf {
    config::config_dir().join("pins.txt")
}

pub fn normalize(p: &str) -> String {
    p.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

pub fn load() -> Vec<String> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .map(normalize)
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn save(pins: &[String]) -> Result<()> {
    std::fs::create_dir_all(config::config_dir())?;
    let mut text = pins.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    std::fs::write(path(), text)?;
    Ok(())
}

fn collect(inner: &Inner, ino: u64, entry: Option<Entry>, out: &mut Vec<Entry>) {
    match entry {
        Some(e) if !e.is_dir => out.push(e),
        _ => {
            if let Ok(kids) = inner.children(ino) {
                for (k, e) in kids {
                    collect(inner, k, Some(e), out);
                }
            }
        }
    }
}

fn sync_once(inner: &Arc<Inner>) {
    let pins = load();
    let mut files = Vec::new();
    for p in &pins {
        match inner.resolve(p) {
            Ok((ino, entry)) => collect(inner, ino, entry, &mut files),
            Err(_) => log::warn!("sabitlenen yol bulunamadi: {}", p),
        }
    }
    inner.cache.set_pinned(files.iter().map(|e| e.id).collect::<HashSet<_>>());
    let missing: Vec<&Entry> = files.iter().filter(|e| !inner.cache.is_cached(e)).collect();
    if !missing.is_empty() {
        log::info!("sabitleme: {} dosya indirilecek", missing.len());
    }
    for e in missing {
        if *KICK.0.lock().unwrap() {
            break; // liste degisti: yeni listeyle bastan
        }
        if inner.cache.free_space() < MIN_FREE {
            log::warn!("diskte 10 GB'tan az yer kaldi; sabitleme indirmeleri durduruldu");
            break;
        }
        if let Err(err) = inner.cache.ensure_full(e) {
            log::warn!("sabitleme indirilemedi: {}: {}", e.name, err);
        }
    }
}

pub fn spawn(inner: Arc<Inner>) {
    std::thread::spawn(move || loop {
        sync_once(&inner);
        let g = KICK.0.lock().unwrap();
        let (mut g, _) = KICK.1.wait_timeout_while(g, INTERVAL, |k| !*k).unwrap();
        *g = false;
    });
}
