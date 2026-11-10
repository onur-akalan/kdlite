//! Yerel web arayuzu: yedeklenecek klasorleri secme, durumu izleme ve
//! yedeklemeyi baslatma. Yalnizca 127.0.0.1 uzerinde dinler.

use crate::api::Api;
use crate::backup::{self, Backup, Progress, Source, Stats};
use crate::config;
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

const PAGE: &str = include_str!("ui.html");

#[derive(Clone, Serialize, Deserialize)]
struct LastRun {
    at: i64,
    stats: Option<Stats>,
    error: Option<String>,
}

pub struct Ui {
    pub api: Arc<Api>,
    pub root_id: u64,
    pub backup_root: String,
    exclude: Mutex<Vec<String>>,
    /// Yedeklenmeyecekler dosyasinin son gorulen degisiklik zamani.
    ignore_seen: Mutex<Option<SystemTime>>,
    /// Izleme calismiyorsa tarama araligi.
    pub interval: Duration,
    /// Izleme calisirken guvenlik taramasi araligi.
    pub rescan: Duration,
    pub mountpoint: PathBuf,
    pub port: u16,
    progress: Arc<Progress>,
    running: AtomicBool,
    /// Calisan is yedekleme degil "yer ac" ise true.
    freeing: AtomicBool,
    /// Son "yer ac" isleminin sonucu (pencerede gosterilir).
    notice: Mutex<Value>,
    /// false = duraklatildi. Durum "paused" dosyasiyla kalicidir.
    auto: AtomicBool,
    cancel: Arc<AtomicBool>,
    /// Otomatik donguyu uyandirir: tam tarama istegi ya da degisen yedekler.
    wake: (Mutex<Wake>, std::sync::Condvar),
    watcher: Mutex<Option<crate::watch::Handle>>,
    last: Mutex<BTreeMap<String, LastRun>>,
    quit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Ayar dosyasinin yolu (ayarlar penceresi icin).
    pub config_path: PathBuf,
    /// Ayarlar kaydedildi: servis kapanip yeni ayarlarla yeniden baslasin.
    pub restart: AtomicBool,
    /// Yeniden baslarken pencere mutlaka acilsin (hesaptan cikis -> sihirbaz).
    pub show_after: AtomicBool,
    /// Yerelde silinen buluttan da silinsin mi (ayar).
    pub sync_deletes: bool,
    /// Toplu silme icin onay verilen yedekler (bir sonraki calismada uygulanir).
    confirmed: Mutex<BTreeSet<String>>,
    /// Silme onayi icin bildirimi gosterilen yedekler (ad -> dosya sayisi).
    asked: Mutex<BTreeMap<String, u64>>,
    /// Bagli bulut klasoru (baglaninca tepsi tarafindan verilir).
    mount: Mutex<Option<Arc<crate::fs::Inner>>>,
    /// Indirilmis dosyalarin son olculen boyutu (dosya, bayt, kalici bayt).
    usage: Mutex<Option<(Instant, (usize, u64, u64))>>,
}

fn autostart_path() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => config::home().join(".config"),
    };
    base.join("autostart/kdlite.desktop")
}

fn set_autostart(on: bool) -> Result<()> {
    let path = autostart_path();
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    let installed = config::home().join(".local/bin/kdlite");
    let exe = if installed.exists() { installed } else { std::env::current_exe()? };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=kdlite\nComment=Mounts your cloud drive and backs up selected folders\nExec={} tray --hidden\nIcon=folder-cloud\nStartupNotify=false\n",
        exe.display()
    );
    std::fs::write(&path, text)?;
    Ok(())
}

fn last_path() -> PathBuf {
    config::config_dir().join("backup-last.json")
}

#[derive(Default)]
struct Wake {
    /// Butun yedekler taransin (surdur, klasor ekle).
    full: bool,
    /// Dosya degisikligi gorulen yedekler.
    dirty: BTreeSet<String>,
    /// Bekleyen degisikliklerin ilk ve son olay zamani.
    first: Option<Instant>,
    last: Option<Instant>,
}

/// Son olaydan sonra bu kadar sessizlik beklenir (dosya yazimi bitsin).
const QUIET: Duration = Duration::from_secs(4);
/// Surekli yazilan bir dosya digerlerini en fazla bu kadar bekletir.
const MAX_WAIT: Duration = Duration::from_secs(30);

fn paused_path() -> PathBuf {
    config::config_dir().join("paused")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 3 <= b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then(|| percent_decode(v))
    })
}

/// `dir` altindaki, eski bulut klasorune bakan kisayollari yenisine cevirir.
fn retarget_links(dir: &Path, old: &Path, new: &Path, count: &mut u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for ent in rd.flatten() {
        let path = ent.path();
        let Ok(meta) = path.symlink_metadata() else { continue };
        if meta.is_dir() {
            retarget_links(&path, old, new, count);
        } else if meta.file_type().is_symlink() {
            let Ok(target) = std::fs::read_link(&path) else { continue };
            let Ok(rest) = target.strip_prefix(old) else { continue };
            let to = new.join(rest);
            if std::fs::remove_file(&path).is_ok() && std::os::unix::fs::symlink(&to, &path).is_ok() {
                *count += 1;
            }
        }
    }
}

/// Bulut klasoru tasindiysa eski (bos) klasoru kaldirir.
pub fn clean_old_mount() {
    let note = config::config_dir().join("old-mount");
    if let Ok(old) = std::fs::read_to_string(&note) {
        let _ = std::fs::remove_dir(old.trim());
        let _ = std::fs::remove_file(&note);
    }
}

impl Ui {
    pub fn new(
        api: Arc<Api>,
        root_id: u64,
        backup_root: String,
        exclude: Vec<String>,
        interval: Duration,
        rescan: Duration,
        mountpoint: PathBuf,
        port: u16,
    ) -> Ui {
        let last = std::fs::read_to_string(last_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ui {
            api,
            root_id,
            backup_root,
            exclude: Mutex::new(exclude),
            ignore_seen: Mutex::new(std::fs::metadata(config::ignore_path()).and_then(|m| m.modified()).ok()),
            interval,
            rescan,
            mountpoint,
            port,
            progress: Arc::new(Progress::default()),
            running: AtomicBool::new(false),
            freeing: AtomicBool::new(false),
            notice: Mutex::new(Value::Null),
            auto: AtomicBool::new(!paused_path().exists()),
            cancel: Arc::new(AtomicBool::new(false)),
            wake: (Mutex::new(Wake::default()), std::sync::Condvar::new()),
            watcher: Mutex::new(None),
            last: Mutex::new(last),
            quit: Mutex::new(None),
            config_path: config::default_path(),
            restart: AtomicBool::new(false),
            show_after: AtomicBool::new(false),
            sync_deletes: true,
            confirmed: Mutex::new(BTreeSet::new()),
            asked: Mutex::new(BTreeMap::new()),
            mount: Mutex::new(None),
            usage: Mutex::new(None),
        }
    }

    /// "/api/quit" istegi geldiginde haber verilecek kanal.
    pub fn set_quit(&self, tx: std::sync::mpsc::Sender<()>) {
        *self.quit.lock().unwrap() = Some(tx);
    }

    pub fn excludes(&self) -> Vec<String> {
        self.exclude.lock().unwrap().clone()
    }

    /// Yedeklenmeyecekler dosyasi degistiyse yeni listeyi uygular. Duzenli
    /// olarak cagrilir (yalnizca bir dosya durumu sorgusu).
    pub fn poll_ignore(&self) {
        let now = std::fs::metadata(config::ignore_path()).and_then(|m| m.modified()).ok();
        {
            let mut seen = self.ignore_seen.lock().unwrap();
            if *seen == now {
                return;
            }
            *seen = now;
        }
        let Some(list) = config::read_ignore() else { return };
        log::info!("yedeklenmeyecekler listesi yenilendi ({} desen)", list.len());
        *self.exclude.lock().unwrap() = list.clone();
        if let Ok(data) = serde_json::to_vec(&list) {
            let _ = std::fs::write(config::config_dir().join("exclude.json"), data);
        }
        if let Some(h) = self.watcher.lock().unwrap().as_ref() {
            h.set_exclude(list);
        }
        self.kick();
    }

    /// Yedeklenmeyecekler dosyasini (yoksa gecerli listeyle olusturup) metin
    /// duzenleyicide acar.
    fn open_ignore(&self) -> Result<()> {
        let path = config::ignore_path();
        if !path.exists() {
            let mut text = config::IGNORE_HEADER.to_string();
            for p in self.excludes() {
                text.push_str(&p);
                text.push('\n');
            }
            std::fs::write(&path, text).context(tr!("could not create the list file"))?;
            *self.ignore_seen.lock().unwrap() = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        }
        std::process::Command::new("xdg-open")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context(tr!("could not open a text editor"))?;
        Ok(())
    }

    pub fn paused(&self) -> bool {
        !self.auto.load(Ordering::SeqCst)
    }

    /// Duraklatir (calisan yedekleme siradaki dosyaya gecmeden durur) ya da
    /// surdurur (hemen bir tarama baslar).
    pub fn set_paused(&self, on: bool) {
        self.auto.store(!on, Ordering::SeqCst);
        self.cancel.store(on, Ordering::SeqCst);
        if on {
            let _ = std::fs::write(paused_path(), b"");
        } else {
            let _ = std::fs::remove_file(paused_path());
            self.kick();
        }
    }

    fn kick(&self) {
        self.wake.0.lock().unwrap().full = true;
        self.wake.1.notify_all();
    }

    /// Izleyici bir yedekte degisiklik gordu.
    fn mark_dirty(&self, name: String) {
        let mut w = self.wake.0.lock().unwrap();
        let now = Instant::now();
        w.dirty.insert(name);
        w.first.get_or_insert(now);
        w.last = Some(now);
        self.wake.1.notify_all();
    }

    fn resync_watcher(&self) {
        if let Some(h) = self.watcher.lock().unwrap().as_ref() {
            h.resync();
        }
    }

    fn watching(&self) -> bool {
        self.watcher.lock().unwrap().is_some()
    }

    pub fn run_in_background(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            me.run_all(None);
        });
    }

    /// Listedeki kaynaklari (ya da yalnizca `only` icindekileri) yedekler.
    /// Zaten calisiyorsa hicbir sey yapmaz. Henuz yazimi bitmedigi icin
    /// ertelenen dosyasi olan yedeklerin adlarini doner.
    fn run_all(&self, only: Option<&BTreeSet<String>>) -> BTreeSet<String> {
        let mut postponed = BTreeSet::new();
        if self.running.swap(true, Ordering::SeqCst) {
            return only.cloned().unwrap_or_default();
        }
        // Elle baslatilan yedekleme duraklatilmisken de calisir.
        self.cancel.store(false, Ordering::SeqCst);
        for src in backup::load_sources() {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            if only.is_some_and(|o| !o.contains(&src.name)) {
                continue;
            }
            let mut b = self.backup();
            b.confirm_deletes = self.confirmed.lock().unwrap().remove(&src.name);
            *self.progress.source.lock().unwrap() = src.name.clone();
            let res = b.run(&src);
            if res.as_ref().is_ok_and(|s| s.postponed > 0) {
                postponed.insert(src.name.clone());
            }
            let entry = match res {
                Ok(stats) => LastRun {
                    at: now(),
                    stats: Some(stats),
                    error: None,
                },
                Err(e) => LastRun {
                    at: now(),
                    stats: None,
                    error: Some(format!("{:#}", e)),
                },
            };
            let mut last = self.last.lock().unwrap();
            last.insert(src.name.clone(), entry);
            if let Ok(data) = serde_json::to_vec(&*last) {
                let _ = std::fs::write(last_path(), data);
            }
        }
        self.progress.source.lock().unwrap().clear();
        self.progress.current.lock().unwrap().clear();
        self.running.store(false, Ordering::SeqCst);
        postponed
    }

    fn backup(&self) -> Backup<'_> {
        Backup {
            api: &self.api,
            root_id: self.root_id,
            backup_root: self.backup_root.clone(),
            exclude: self.excludes(),
            progress: Some(self.progress.clone()),
            cancel: Some(self.cancel.clone()),
            mountpoint: self.mountpoint.clone(),
            sync_deletes: self.sync_deletes,
            confirm_deletes: false,
        }
    }

    /// Bir yolu (yedeklenen klasorun ya da kDrive klasorunun icinde) ait
    /// oldugu yedege ve yerel karsiligina cevirir. Son bilesen bag ise izlenmez.
    fn locate(&self, path: &str) -> Option<(Source, PathBuf)> {
        let p = Path::new(path);
        let abs = match (p.parent().and_then(|d| std::fs::canonicalize(d).ok()), p.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => return None,
        };
        let sources = backup::load_sources();
        if let Ok(rel) = abs.strip_prefix(&self.mountpoint) {
            // kDrive klasorundeki dosya: yedegin yerel karsiligini bul.
            return sources.into_iter().find_map(|s| {
                let remote = Path::new(s.dest.as_deref().unwrap_or(&self.backup_root)).join(&s.name);
                let rest = rel.strip_prefix(&remote).ok()?.to_path_buf();
                let local = Path::new(&s.local).join(rest);
                Some((s, local))
            });
        }
        sources.into_iter().find(|s| abs.starts_with(&s.local)).map(|s| (s, abs))
    }

    pub fn set_mount(&self, inner: Arc<crate::fs::Inner>) {
        *self.mount.lock().unwrap() = Some(inner);
        self.write_drive_file();
    }

    /// Dolphin eklentisi icin: bulut klasorunun yeri ve bilgisayarda da
    /// tutulan yollar.
    fn write_drive_file(&self) {
        let v = json!({ "mountpoint": self.mountpoint, "pins": crate::pins::load() });
        let path = config::config_dir().join("drive.json");
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// Bulut klasorunun durumu (pencere icin).
    fn drive_state(&self) -> Value {
        let inner = self.mount.lock().unwrap().clone();
        let usage = inner.as_ref().map(|i| {
            let mut u = self.usage.lock().unwrap();
            match *u {
                Some((at, v)) if at.elapsed() < Duration::from_secs(10) => v,
                _ => {
                    let v = i.cache.usage();
                    *u = Some((Instant::now(), v));
                    v
                }
            }
        });
        let (files, bytes, kept) = usage.unwrap_or((0, 0, 0));
        json!({
            "mounted": inner.is_some(),
            "files": files,
            "bytes": bytes,
            "kept_bytes": kept,
            "pins": crate::pins::load(),
        })
    }

    /// Bulut klasorundeki bir yolun buluttaki goreli yolu. Mutlak yol bulut
    /// klasorunun icinde olmali; goreli yol dogrudan bulut yolu sayilir.
    fn cloud_rel(&self, path: &str) -> Option<String> {
        let p = Path::new(path);
        if !p.is_absolute() {
            return Some(crate::pins::normalize(path));
        }
        if p == self.mountpoint {
            return Some(String::new());
        }
        let abs = match (p.parent().and_then(|d| std::fs::canonicalize(d).ok()), p.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => return None,
        };
        let rel = abs.strip_prefix(&self.mountpoint).ok()?;
        Some(crate::pins::normalize(&rel.to_string_lossy()))
    }

    /// Bulut klasorundeki ogeleri bilgisayarda da tutar (`on`) ya da yalnizca
    /// bulutta birakir: indirilmis kopyalar silinir, buluta dokunulmaz.
    fn pin(&self, paths: &[String], on: bool) -> Result<Value> {
        let inner = self
            .mount
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow!("{}", tr!("the cloud folder is not connected right now")))?;
        let mut pins = crate::pins::load();
        let mut released: Vec<String> = Vec::new();
        let mut done = 0u64;
        for p in paths {
            let rel = self.cloud_rel(p).ok_or_else(|| anyhow!("{}", tr!("this is not inside the cloud folder")))?;
            if rel.is_empty() {
                bail!("{}", tr!("the whole drive cannot be chosen; pick a folder or file inside it"));
            }
            if inner.resolve(&rel).is_err() {
                bail!("{}", tr!("“{}” was not found in the cloud", rel));
            }
            if on {
                if crate::pins::covering(&pins, &rel).is_none() {
                    // Altindaki eski kayitlar gereksizlesir.
                    pins.retain(|x| !x.starts_with(&format!("{}/", rel)));
                    pins.push(rel);
                }
            } else {
                pins.retain(|x| x != &rel && !x.starts_with(&format!("{}/", rel)));
                if let Some(parent) = crate::pins::covering(&pins, &rel) {
                    bail!("{}", tr!("this is inside “{}”, which is kept on this computer; change that folder instead", parent));
                }
                released.push(rel);
            }
            done += 1;
        }
        crate::pins::save(&pins)?;
        self.write_drive_file();
        crate::pins::wake();
        if !released.is_empty() {
            std::thread::spawn(move || {
                let ids: std::collections::HashSet<u64> =
                    released.iter().flat_map(|r| crate::pins::files_under(&inner, r)).map(|e| e.id).collect();
                let (n, bytes) = inner.cache.release(&ids);
                log::info!("bilgisayardaki kopyalar silindi: {} dosya, {} bayt", n, bytes);
            });
        }
        *self.usage.lock().unwrap() = None;
        Ok(json!({ "drive": true, "mode": if on { "both" } else { "cloud" }, "done": done }))
    }

    /// Dosyalarin konumunu degistirir (yalnizca bulutta / iki yerde /
    /// yalnizca bilgisayarda). Calisan yedekleme varsa bitmesini bekler.
    fn place(&self, paths: &[String], mode: backup::Place) -> Result<Value> {
        let mut groups: BTreeMap<String, (Source, Vec<PathBuf>)> = BTreeMap::new();
        // Bulut klasorunun icindeki (bir yedege ait olmayan) ogeler.
        let mut drive: Vec<String> = Vec::new();
        for p in paths {
            match self.locate(p) {
                Some((src, local)) => groups.entry(src.name.clone()).or_insert_with(|| (src, Vec::new())).1.push(local),
                None if self.cloud_rel(p).is_some() && Path::new(p).is_absolute() => drive.push(p.clone()),
                None => bail!("{}", tr!("this file is not inside a backed-up folder")),
            }
        }
        if !drive.is_empty() {
            if mode == backup::Place::Local {
                bail!("{}", tr!("files in the cloud folder live in the cloud; they can be kept on this computer too, but not on this computer only"));
            }
            let out = self.pin(&drive, mode == backup::Place::Both)?;
            if groups.is_empty() {
                return Ok(out);
            }
        }
        if groups.is_empty() {
            bail!("{}", tr!("no file selected"));
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.running.swap(true, Ordering::SeqCst) {
            if Instant::now() > deadline {
                bail!("{}", tr!("a backup is running right now; try again when it finishes"));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        self.freeing.store(true, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        let b = self.backup();
        let (mut done, mut bytes, mut kept) = (0, 0, 0);
        let mut errors: Vec<String> = Vec::new();
        for (src, list) in groups.values() {
            match b.place(src, list, mode) {
                Ok(st) => {
                    done += st.done;
                    bytes += st.bytes;
                    kept += st.kept;
                    errors.extend(st.errors);
                }
                Err(e) => errors.push(format!("{}: {:#}", src.name, e)),
            }
        }
        self.progress.source.lock().unwrap().clear();
        self.progress.current.lock().unwrap().clear();
        self.freeing.store(false, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        if mode == backup::Place::Both {
            self.kick(); // yeniden yedeklenecekler hemen gitsin
        }
        let mode_name = match mode {
            backup::Place::Cloud => "cloud",
            backup::Place::Both => "both",
            backup::Place::Local => "local",
        };
        errors.truncate(5);
        let out = json!({ "at": now(), "mode": mode_name, "done": done, "bytes": bytes, "kept": kept, "errors": errors });
        *self.notice.lock().unwrap() = out.clone();
        Ok(out)
    }

    /// Onay bekleyen toplu silme varsa bir kez masaustu bildirimiyle sorar.
    /// Bildirim kapatilirsa soru pencerede ve tepsi simgesinde durur.
    pub fn prompt_deletes(self: &Arc<Self>) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }
        let pending: Vec<(String, u64)> = self
            .last
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.stats.as_ref()?.pending_deletes)))
            .collect();
        let mut asked = self.asked.lock().unwrap();
        for (name, count) in pending {
            if count == 0 {
                asked.remove(&name);
                continue;
            }
            if asked.insert(name.clone(), count) == Some(count) {
                continue;
            }
            let me = self.clone();
            std::thread::spawn(move || {
                let out = std::process::Command::new("notify-send")
                    .args(["-a", "kdlite", "-i", "dialog-warning", "-u", "critical", "--wait"])
                    .args(["-A", &format!("yes={}", tr!("Delete from cloud too"))])
                    .args(["-A", &format!("no={}", tr!("Keep in cloud"))])
                    .arg(tr!("{}: {} files were deleted on this computer", name, count))
                    .arg(tr!("Delete them from the cloud too? They go to the cloud trash and can be restored."))
                    .output();
                let answer = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
                let res = match answer.as_str() {
                    "yes" => me.resolve_deletes(&name, true),
                    "no" => me.resolve_deletes(&name, false),
                    _ => Ok(()),
                };
                if let Err(e) = res {
                    log::warn!("silme onayi uygulanamadi ({}): {:#}", name, e);
                }
            });
        }
    }

    /// Toplu silme onayi: `apply` ise yerelde silinenler buluttan da (cop
    /// kutusuna) silinir, degilse bulutta birakilip takipten cikarilir.
    fn resolve_deletes(self: &Arc<Self>, name: &str, apply: bool) -> Result<()> {
        let src = backup::load_sources()
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow!("{}", tr!("no such backup in the list")))?;
        if apply {
            self.confirmed.lock().unwrap().insert(src.name.clone());
        } else {
            let deadline = Instant::now() + Duration::from_secs(60);
            while self.running.swap(true, Ordering::SeqCst) {
                if Instant::now() > deadline {
                    bail!("{}", tr!("a backup is running right now; try again when it finishes"));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            let res = self.backup().forget_missing(&src);
            self.running.store(false, Ordering::SeqCst);
            res?;
        }
        let me = self.clone();
        let only: BTreeSet<String> = [src.name].into();
        std::thread::spawn(move || {
            me.run_all(Some(&only));
        });
        Ok(())
    }

    pub fn state(&self) -> Value {
        let summary = backup::summary();
        let last = self.last.lock().unwrap();
        let sources: Vec<Value> = backup::load_sources()
            .iter()
            .map(|s| {
                let (files, bytes, cloud_only, local_only) = summary.get(&s.name).copied().unwrap_or((0, 0, 0, 0));
                json!({
                    "local": s.local,
                    "name": s.name,
                    "remote": format!("{}/{}", s.dest.as_deref().unwrap_or(&self.backup_root), s.name)
                        .trim_start_matches('/'),
                    "exists": Path::new(&s.local).is_dir(),
                    "files": files,
                    "bytes": bytes,
                    "cloud_only": cloud_only,
                    "local_only": local_only,
                    "last": last.get(&s.name),
                })
            })
            .collect();
        let p = &self.progress;
        json!({
            "home": config::home(),
            "lang": crate::i18n::lang(),
            "backup_root": self.backup_root,
            "mountpoint": self.mountpoint,
            "drive": self.drive_state(),
            "interval": self.interval.as_secs(),
            "watching": self.watching(),
            "mount_pending": crate::fs::PENDING.load(Ordering::Relaxed),
            "mount_failed": crate::fs::FAILED.load(Ordering::Relaxed),
            "running": self.running.load(Ordering::SeqCst),
            "auto": self.auto.load(Ordering::SeqCst),
            "paused": self.paused(),
            "freeing": self.freeing.load(Ordering::SeqCst),
            "notice": *self.notice.lock().unwrap(),
            "progress": {
                "source": *p.source.lock().unwrap(),
                "current": *p.current.lock().unwrap(),
                "total_files": p.total_files.load(Ordering::Relaxed),
                "done_files": p.done_files.load(Ordering::Relaxed),
                "total_bytes": p.total_bytes.load(Ordering::Relaxed),
                "done_bytes": p.done_bytes.load(Ordering::Relaxed),
            },
            "sources": sources,
        })
    }

    /// Ev klasoru altindaki bir klasorun alt klasorlerini listeler.
    fn ls(&self, path: &str) -> Result<Value> {
        let home = std::fs::canonicalize(config::home())?;
        let dir = std::fs::canonicalize(path).context(tr!("folder not found"))?;
        if !dir.starts_with(&home) {
            bail!("{}", tr!("only the home folder can be browsed"));
        }
        let mut dirs = Vec::new();
        for ent in std::fs::read_dir(&dir)?.flatten() {
            let Ok(meta) = ent.path().symlink_metadata() else { continue };
            if !meta.is_dir() {
                continue;
            }
            let Ok(name) = ent.file_name().into_string() else { continue };
            let p = ent.path();
            let skipped = self.excludes().iter().any(|pat| backup::matches(pat, &name))
                || p.join("CACHEDIR.TAG").exists()
                || p == self.mountpoint;
            dirs.push(json!({
                "name": name,
                "path": p,
                "hidden": name.starts_with('.'),
                "skipped": skipped,
            }));
        }
        dirs.sort_by_key(|d| d["name"].as_str().unwrap_or("").to_lowercase());
        Ok(json!({ "path": dir, "dirs": dirs }))
    }

    fn settings(&self) -> Result<Value> {
        let cfg = config::Config::load(&self.config_path)?;
        // Bagli surucunun adi (alinamazsa kimligi gosterilir).
        let drive = self
            .api
            .drives()
            .ok()
            .and_then(|l| l.into_iter().find(|(id, _)| *id == cfg.drive_id))
            .map(|(_, name)| name)
            .unwrap_or_else(|| format!("kDrive {}", cfg.drive_id));
        Ok(json!({
            "drive": drive,
            "writable": cfg.writable,
            "language": cfg.language,
            "languages": crate::i18n::available(),
            "sync_deletes": cfg.sync_deletes,
            "cache_max_gb": cfg.cache_max_gb,
            "backup_root": self.backup_root,
            "exclude_count": self.excludes().len(),
            "rescan_min": (cfg.rescan_secs / 60).max(1),
            "autostart": autostart_path().exists(),
            "mountpoint": self.mountpoint,
            "config_path": self.config_path,
        }))
    }

    /// Ayarlari dosyaya yazar ve servisin yeniden baslamasini ister.
    fn save_settings(&self, body: &Value) -> Result<()> {
        let old = std::fs::read_to_string(&self.config_path).context(tr!("could not read the settings file"))?;
        let mut text = old.clone();
        let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
        if let Some(w) = body.get("writable").and_then(|v| v.as_bool()) {
            text = crate::setup::set_line(&text, "writable", &w.to_string());
        }
        if let Some(l) = body.get("language").and_then(|v| v.as_str()) {
            let ok = l == "auto" || l == "en" || crate::i18n::available().contains(&l);
            if ok {
                text = crate::setup::set_line(&text, "language", &quote(l));
            }
        }
        if let Some(d) = body.get("sync_deletes").and_then(|v| v.as_bool()) {
            text = crate::setup::set_line(&text, "sync_deletes", &d.to_string());
        }
        if let Some(gb) = body.get("cache_max_gb").and_then(|v| v.as_f64()) {
            if !(1.0..=4000.0).contains(&gb) {
                bail!("{}", tr!("the cache limit must be between 1 and 4000 GB"));
            }
            text = crate::setup::set_line(&text, "cache_max_gb", &format!("{}", gb.round() as u64));
        }
        if let Some(m) = body.get("rescan_min").and_then(|v| v.as_u64()) {
            if !(1..=1440).contains(&m) {
                bail!("{}", tr!("the rescan interval must be between 1 and 1440 minutes"));
            }
            text = crate::setup::set_line(&text, "rescan_secs", &(m * 60).to_string());
        }
        if let Some(r) = body.get("backup_root").and_then(|v| v.as_str()) {
            let clean = r.split('/').map(str::trim).filter(|p| !p.is_empty() && *p != "." && *p != "..").collect::<Vec<_>>().join("/");
            if clean.is_empty() {
                bail!("{}", tr!("the default backup place cannot be empty"));
            }
            if clean != self.backup_root {
                text = crate::setup::set_line(&text, "backup_root", &quote(&clean));
            }
        }
        let mut moved: Option<PathBuf> = None;
        if let Some(m) = body.get("mountpoint").and_then(|v| v.as_str()) {
            let home = config::home();
            let m = m.trim().trim_end_matches('/');
            let new = match m.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(m),
            };
            if new != self.mountpoint {
                if !new.is_absolute() || !new.starts_with(&home) || new == home {
                    bail!("{}", tr!("the cloud folder must be a folder inside your home folder"));
                }
                if backup::load_sources().iter().any(|s| new.starts_with(&s.local) || Path::new(&s.local).starts_with(&new)) {
                    bail!("{}", tr!("the cloud folder cannot be inside a backed-up folder"));
                }
                match std::fs::read_dir(&new) {
                    Ok(mut rd) => {
                        if rd.next().is_some() {
                            bail!("{}", tr!("that folder already has files in it; choose a new or empty folder"));
                        }
                    }
                    Err(_) if new.symlink_metadata().is_ok() => bail!("{}", tr!("that name is already used by a file")),
                    Err(_) => std::fs::create_dir_all(&new).context(tr!("could not create the folder"))?,
                }
                let shown = match new.strip_prefix(&home) {
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => new.display().to_string(),
                };
                text = crate::setup::set_line(&text, "mountpoint", &quote(&shown));
                moved = Some(new);
            }
        }
        if let Some(list) = body.get("exclude").and_then(|v| v.as_array()) {
            let items: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.contains('/'))
                .map(str::to_string)
                .collect();
            if items != self.excludes() {
                let arr = items.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ");
                text = crate::setup::set_line(&text, "exclude", &format!("[{}]", arr));
            }
        }
        // Yazmadan once yeni metnin gecerli oldugunu dogrula.
        toml::from_str::<config::Config>(&text).context(tr!("the settings are not valid"))?;
        if let Some(a) = body.get("autostart").and_then(|v| v.as_bool()) {
            if a != autostart_path().exists() {
                set_autostart(a).context(tr!("could not set up autostart"))?;
            }
        }
        if let (Some(new), true) = (&moved, text != old) {
            // "Yalnizca bulutta" kisayollari yeni yere baksin; eski bos klasor
            // yeniden baslayinca kaldirilir.
            let mut n = 0;
            for s in backup::load_sources() {
                retarget_links(Path::new(&s.local), &self.mountpoint, new, &mut n);
            }
            log::info!("bulut klasoru tasindi: {} -> {} ({} kisayol guncellendi)", self.mountpoint.display(), new.display(), n);
            let _ = std::fs::write(config::config_dir().join("old-mount"), self.mountpoint.to_string_lossy().as_bytes());
        }
        if text != old {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let tmp = self.config_path.with_extension("tmp");
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(text.as_bytes())?;
            drop(f);
            std::fs::rename(&tmp, &self.config_path)?;
            self.restart.store(true, Ordering::SeqCst);
            if let Some(tx) = self.quit.lock().unwrap().as_ref() {
                let _ = tx.send(());
            }
        }
        Ok(())
    }

    /// Hesaptan cikis: token ayar dosyasindan silinir ve servis yeniden
    /// baslayip kurulum sihirbazini acar. Yedek listesi ve durum kayitlari,
    /// onbellek ve buluttaki dosyalar yerinde kalir.
    fn logout(&self) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let old = std::fs::read_to_string(&self.config_path).context(tr!("could not read the settings file"))?;
        let text = crate::setup::set_line(&old, "token", "\"\"");
        let tmp = self.config_path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        drop(f);
        std::fs::rename(&tmp, &self.config_path)?;
        log::info!("hesaptan cikildi; kurulum sihirbazi acilacak");
        self.show_after.store(true, Ordering::SeqCst);
        self.restart.store(true, Ordering::SeqCst);
        if let Some(tx) = self.quit.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
        Ok(())
    }

    /// Bir yedegin buluttaki yerini degistirir. Eski yerdeki kopya silinmez.
    fn edit(&self, name: &str, dest: Option<String>) -> Result<()> {
        let dest = dest.map(|d| {
            d.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").collect::<Vec<_>>().join("/")
        });
        let mut all = backup::load_sources();
        let src = all.iter_mut().find(|s| s.name == name).ok_or_else(|| anyhow!("{}", tr!("no such backup in the list")))?;
        src.dest = dest;
        backup::save_sources(&all)?;
        self.kick();
        Ok(())
    }

    /// Buluttaki bir klasorun alt klasorlerini listeler (hedef secici icin).
    fn cloud_dirs(&self, id: Option<u64>) -> Result<Value> {
        let mut dirs: Vec<Value> = self
            .api
            .list_dir(id.unwrap_or(self.root_id))?
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| json!({ "id": e.id, "name": e.name }))
            .collect();
        dirs.sort_by_key(|d| d["name"].as_str().unwrap_or("").to_lowercase());
        Ok(json!({ "dirs": dirs }))
    }

    /// `dest`: buluttaki ust klasor (koke gore yol); None ise genel yedek koku.
    fn add(&self, path: &str, dest: Option<String>) -> Result<Source> {
        let dest = dest.map(|d| {
            d.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").collect::<Vec<_>>().join("/")
        });
        let abs = std::fs::canonicalize(path).context(tr!("folder not found"))?;
        if !abs.is_dir() {
            bail!("{}", tr!("this is not a folder"));
        }
        let home = std::fs::canonicalize(config::home())?;
        if abs == home || abs == Path::new("/") {
            bail!("{}", tr!("the whole home folder cannot be added; choose folders inside it"));
        }
        if abs.starts_with(&self.mountpoint) {
            bail!("{}", tr!("this folder is kDrive itself"));
        }
        let mut all = backup::load_sources();
        for s in &all {
            let other = Path::new(&s.local);
            if abs == other {
                bail!("{}", tr!("this folder is already backed up"));
            }
            if abs.starts_with(other) {
                bail!("{}", tr!("this folder is already inside the “{}” backup", s.name));
            }
            if other.starts_with(&abs) {
                bail!("{}", tr!("“{}” inside it is backed up separately; remove that first", s.name));
            }
        }
        let base = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("klasor adi cikarilamadi"))?;
        let mut name = base.clone();
        let mut n = 2;
        while all.iter().any(|s| s.name == name) {
            name = format!("{} ({})", base, n);
            n += 1;
        }
        let src = Source {
            local: abs.to_string_lossy().into_owned(),
            name,
            dest,
        };
        all.push(src.clone());
        backup::save_sources(&all)?;
        self.resync_watcher();
        self.kick(); // ilk yedekleme hemen baslasin
        Ok(src)
    }

    /// Listeden cikarir; `name` yedek adi, `path` yerel klasor yoludur
    /// (hangisi verildiyse). Buluttaki kopyaya dokunulmaz.
    fn remove(&self, name: &str, path: &str) -> Result<()> {
        let local = if path.is_empty() {
            None
        } else {
            Some(std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
        };
        let mut all = backup::load_sources();
        let before = all.len();
        all.retain(|s| match &local {
            Some(l) => Path::new(&s.local) != l,
            None => s.name != name,
        });
        if all.len() == before {
            if let Some(l) = &local {
                if let Some(s) = all.iter().find(|s| l.starts_with(&s.local)) {
                    bail!("{}", tr!("this folder is inside the “{}” backup; it cannot be removed separately", s.name));
                }
                bail!("{}", tr!("this folder is not being backed up"));
            }
            bail!("{}", tr!("no such backup in the list"));
        }
        backup::save_sources(&all)?;
        self.resync_watcher();
        Ok(())
    }

    fn handle(self: &Arc<Self>, req: &mut Request) -> (u16, &'static str, String) {
        let json_ok = |v: Value| (200, "application/json", v.to_string());
        let json_err = |code: u16, msg: String| (code, "application/json", json!({ "error": msg }).to_string());

        // Baska sitelerin tarayici uzerinden bu sunucuya istek atmasini engelle.
        let header = |name: &str| {
            req.headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str().to_string())
        };
        let host_ok = header("Host").is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port)
        });
        if !host_ok {
            return json_err(403, "gecersiz istek".into());
        }
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();
        let is_post = *req.method() == Method::Post;
        if is_post && header("X-Kdlite").is_none() {
            return json_err(403, "gecersiz istek".into());
        }
        let mut body = String::new();
        if is_post {
            let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
        }
        let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let field = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();

        match (is_post, path.as_str()) {
            (false, "/") => (200, "text/html; charset=utf-8", PAGE.to_string()),
            (false, "/api/state") => json_ok(self.state()),
            (false, "/api/ls") => {
                let p = query_param(&url, "path").unwrap_or_else(|| config::home().to_string_lossy().into_owned());
                match self.ls(&p) {
                    Ok(v) => json_ok(v),
                    Err(e) => json_err(400, format!("{:#}", e)),
                }
            }
            (false, "/api/cloud") => {
                let id = query_param(&url, "id").and_then(|s| s.parse().ok());
                match self.cloud_dirs(id) {
                    Ok(v) => json_ok(v),
                    Err(e) => json_err(400, format!("{:#}", e)),
                }
            }
            (true, "/api/add") => match self.add(
                &field("path"),
                body.get("dest").and_then(|v| v.as_str()).map(str::to_string),
            ) {
                Ok(s) => json_ok(json!({ "name": s.name })),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/remove") => match self.remove(&field("name"), &field("path")) {
                Ok(()) => json_ok(json!({})),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (false, "/api/settings") => match self.settings() {
                Ok(v) => json_ok(v),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/settings") => match self.save_settings(&body) {
                Ok(()) => json_ok(json!({ "restart": self.restart.load(Ordering::SeqCst) })),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/logout") => match self.logout() {
                Ok(()) => json_ok(json!({})),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/ignore/open") => match self.open_ignore() {
                Ok(()) => json_ok(json!({})),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/edit") => match self.edit(
                &field("name"),
                body.get("dest").and_then(|v| v.as_str()).map(str::to_string),
            ) {
                Ok(()) => json_ok(json!({})),
                Err(e) => json_err(400, format!("{:#}", e)),
            },
            (true, "/api/place") => {
                let mode = match field("mode").as_str() {
                    "cloud" => backup::Place::Cloud,
                    "both" => backup::Place::Both,
                    "local" => backup::Place::Local,
                    _ => return json_err(400, "bilinmeyen konum".into()),
                };
                let paths: Vec<String> = body
                    .get("paths")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                match self.place(&paths, mode) {
                    Ok(v) => json_ok(v),
                    Err(e) => json_err(400, format!("{:#}", e)),
                }
            }
            (true, "/api/pin") => {
                let paths: Vec<String> = body
                    .get("paths")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let on = body.get("on").and_then(|v| v.as_bool()).unwrap_or(true);
                match self.pin(&paths, on) {
                    Ok(v) => json_ok(v),
                    Err(e) => json_err(400, format!("{:#}", e)),
                }
            }
            (true, "/api/deletes") => {
                let apply = body.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
                match self.resolve_deletes(&field("name"), apply) {
                    Ok(()) => json_ok(json!({})),
                    Err(e) => json_err(400, format!("{:#}", e)),
                }
            }
            (true, "/api/run") => {
                // "path" verilirse yalnizca o klasoru iceren yedek calisir.
                let path = field("path");
                if path.is_empty() {
                    self.run_in_background();
                    return json_ok(json!({}));
                }
                let abs = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
                let Some(src) = backup::load_sources().into_iter().find(|s| abs.starts_with(&s.local)) else {
                    return json_err(400, tr!("this folder is not being backed up"));
                };
                if self.running.load(Ordering::SeqCst) {
                    return json_err(400, tr!("a backup is already running"));
                }
                let me = self.clone();
                let only: BTreeSet<String> = [src.name.clone()].into();
                std::thread::spawn(move || {
                    me.run_all(Some(&only));
                });
                json_ok(json!({ "name": src.name }))
            }
            (true, "/api/quit") => match self.quit.lock().unwrap().as_ref() {
                Some(tx) => {
                    let _ = tx.send(());
                    json_ok(json!({}))
                }
                None => json_err(400, "bu surec uzaktan kapatilamaz".into()),
            },
            (true, "/api/auto") => {
                let on = body.get("on").and_then(|v| v.as_bool()).unwrap_or(true);
                self.set_paused(!on);
                json_ok(json!({ "auto": on }))
            }
            (true, "/api/pause") => {
                let on = body.get("on").and_then(|v| v.as_bool()).unwrap_or(true);
                self.set_paused(on);
                json_ok(json!({ "paused": on }))
            }
            _ => json_err(404, "bulunamadi".into()),
        }
    }

    /// API sunucusunu ve otomatik yedekleme dongusunu arka planda baslatir.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        let addr = format!("127.0.0.1:{}", self.port);
        let server = Arc::new(Server::http(&addr).map_err(|e| anyhow!("{} dinlenemedi: {}", addr, e))?);

        // Dolphin rozet eklentisi haric tutma listesini buradan okur
        // (token'in durdugu ayar dosyasina bakmasin diye ayri dosya).
        if let Ok(data) = serde_json::to_vec(&self.excludes()) {
            let _ = std::fs::write(config::config_dir().join("exclude.json"), data);
        }

        // Dosya izleyicisi: degisiklik aninda ilgili yedegi isaretler.
        {
            let me = self.clone();
            match crate::watch::spawn(self.excludes(), move |name| me.mark_dirty(name)) {
                Ok(h) => *self.watcher.lock().unwrap() = Some(h),
                Err(e) => log::warn!("dosya izleme kurulamadi, duzenli taramaya donuldu: {}", e),
            }
        }

        // Otomatik yedekleme dongusu: acilista ve arada bir tam tarama,
        // aradaki surede yalnizca degisiklik gorulen yedekler.
        let me = self.clone();
        std::thread::spawn(move || {
            let mut only: Option<BTreeSet<String>> = None; // None = tam tarama
            loop {
                if only.is_none() {
                    me.resync_watcher();
                }
                if me.auto.load(Ordering::SeqCst) && !backup::load_sources().is_empty() {
                    for name in me.run_all(only.as_ref()) {
                        me.mark_dirty(name); // yazimi suren dosya: birazdan yeniden dene
                    }
                }
                // Bir sonraki ise kadar bekle.
                let every = if me.watching() { me.rescan } else { me.interval };
                let deadline = Instant::now() + every;
                let mut w = me.wake.0.lock().unwrap();
                only = loop {
                    let now = Instant::now();
                    if w.full || now >= deadline {
                        *w = Wake::default();
                        break None;
                    }
                    let mut until = deadline;
                    if let (Some(first), Some(last)) = (w.first, w.last) {
                        let ready = (last + QUIET).min(first + MAX_WAIT);
                        if now >= ready {
                            if !me.auto.load(Ordering::SeqCst) {
                                // Duraklatildi: degisiklikler surdurunce tam taramayla gider.
                                *w = Wake::default();
                                continue;
                            }
                            if !me.running.load(Ordering::SeqCst) {
                                let names = std::mem::take(&mut w.dirty);
                                w.first = None;
                                w.last = None;
                                break Some(names);
                            }
                            until = now + Duration::from_secs(2); // elle yedekleme suruyor
                        } else {
                            until = ready.min(deadline);
                        }
                    }
                    w = me.wake.1.wait_timeout(w, until.saturating_duration_since(now)).unwrap().0;
                };
            }
        });

        for _ in 0..2 {
            let server = server.clone();
            let me = self.clone();
            std::thread::spawn(move || loop {
                let Ok(mut req) = server.recv() else { break };
                let (code, ctype, body) = me.handle(&mut req);
                let resp = Response::from_string(body)
                    .with_status_code(code)
                    .with_header(Header::from_bytes("Content-Type", ctype).unwrap())
                    .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
                    .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap());
                let _ = req.respond(resp);
            });
        }
        Ok(())
    }

    pub fn serve(self: Arc<Self>, open: bool) -> Result<()> {
        self.start()?;
        let url = format!("http://127.0.0.1:{}/", self.port);
        println!("Arayuz: {}", url);
        println!("Durdurmak icin Ctrl-C.");
        if open {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        ctrlc::set_handler(move || {
            let _ = tx.send(());
        })?;
        let _ = rx.recv();
        println!("Durduruldu.");
        Ok(())
    }
}
