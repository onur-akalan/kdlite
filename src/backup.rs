//! Cift yonlu esitleme: secilen yerel klasorler buluttaki karsiliklariyla
//! ayni tutulur.
//!
//! Kurallar:
//! - Bilgisayarda degisen dosya yuklenir; eskisi kDrive surum gecmisinde kalir.
//! - Bulutta degisen dosya indirilir.
//! - Hicbir sey kalici silinmez: bilgisayarda silinen dosyanin buluttaki
//!   kopyasi kDrive cop kutusuna, bulutta silinen dosyanin yerel kopyasi
//!   masaustu cop kutusuna gider. Toplu silmelerde once onay istenir.
//! - Iki tarafta da degisen dosyanin iki hali de saklanir.
//! - Bulutta yeni beliren dosya yerelde "yalnizca bulutta" kisayolu olur.

use crate::api::{Api, Entry};
use crate::config;
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORKERS: usize = 4;
/// Son birkac saniyede degismis dosyalar hala yaziliyor olabilir; bir
/// sonraki taramaya birakilir.
const SETTLE_SECS: i64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Yedeklenecek yerel klasor (mutlak yol).
    pub local: String,
    /// Buluttaki yedek klasoru altindaki adi.
    pub name: String,
    /// Buluttaki ust klasor (baglanan koke gore yol). Bos birakilirsa
    /// ayardaki genel yedek koku ("Yedekler/<bilgisayar>") kullanilir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRec {
    size: u64,
    mtime: i64,
    id: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RootState {
    /// Goreli klasor yolu -> buluttaki klasor kimligi ("" = yedegin koku).
    dirs: BTreeMap<String, u64>,
    /// Goreli dosya yolu -> en son yuklenen hali.
    files: BTreeMap<String, FileRec>,
    /// Bu kaydin ait oldugu buluttaki ust klasor. Hedef degisirse kayit
    /// sifirlanir ki dosyalar yeni yere gitsin.
    #[serde(default)]
    base: Option<String>,
    /// Yalnizca bulutta tutulan dosyalar (yerelde kisayol var).
    #[serde(default)]
    cloud_only: BTreeSet<String>,
    /// Yalnizca bilgisayarda tutulan dosyalar (yedeklenmez).
    #[serde(default)]
    local_only: BTreeSet<String>,
    /// Silme esitlemesi bu yedek icin baslatildi mi. Ilk calismada, daha
    /// once yerelde silinmis dosyalar buluttan silinmez; yalnizca unutulur.
    #[serde(default)]
    deletes_synced: bool,
    /// Bulutta silinmis olup yerelden kaldirilmasi icin onay bekleyenler.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    cloud_missing: BTreeSet<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    roots: BTreeMap<String, RootState>,
}

fn sources_path() -> PathBuf {
    config::config_dir().join("backups.json")
}

fn state_path() -> PathBuf {
    config::config_dir().join("backup-state.json")
}

pub fn load_sources() -> Vec<Source> {
    std::fs::read_to_string(sources_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_sources(s: &[Source]) -> Result<()> {
    std::fs::create_dir_all(config::config_dir())?;
    write_atomic(&sources_path(), &serde_json::to_vec_pretty(s)?)
}

fn load_state() -> Result<State> {
    match std::fs::read_to_string(state_path()) {
        Ok(t) => serde_json::from_str(&t).context("yedek durum dosyasi bozuk"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn save_state(st: &State) -> Result<()> {
    write_atomic(&state_path(), &serde_json::to_vec(st)?)
}

/// Basit desen: "*.ext" (sonek), "onek*" (onek) ya da tam ad.
pub fn matches(pattern: &str, name: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        pattern == name
    }
}

fn excluded(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| matches(p, name))
}

fn mtime_secs(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct LocalFile {
    rel: String,
    dir_rel: String,
    name: String,
    abs: PathBuf,
    size: u64,
    mtime: i64,
}

#[derive(Default)]
struct Walk {
    files: Vec<LocalFile>,
    skipped: u64,
    unreadable: u64,
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir, name)
    }
}

fn walk(dir: &Path, rel: &str, patterns: &[String], out: &mut Walk) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("okunamadi: {}: {}", dir.display(), e);
            out.unreadable += 1;
            return;
        }
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let Ok(name) = ent.file_name().into_string() else {
            log::warn!("gecersiz karakterli ad atlandi: {}", ent.path().display());
            out.skipped += 1;
            continue;
        };
        if excluded(patterns, &name) {
            out.skipped += 1;
            continue;
        }
        // symlink_metadata: sembolik baglar izlenmez.
        let Ok(meta) = ent.path().symlink_metadata() else {
            out.unreadable += 1;
            continue;
        };
        let child_rel = join_rel(rel, &name);
        if meta.is_dir() {
            // Onbellek olarak isaretlenmis klasorler (ornegin cargo target).
            if ent.path().join("CACHEDIR.TAG").exists() {
                out.skipped += 1;
                continue;
            }
            walk(&ent.path(), &child_rel, patterns, out);
        } else if meta.is_file() {
            out.files.push(LocalFile {
                rel: child_rel,
                dir_rel: rel.to_string(),
                name,
                abs: ent.path(),
                size: meta.len(),
                mtime: mtime_secs(&meta),
            });
        } else {
            out.skipped += 1;
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub scanned: u64,
    pub uploaded: u64,
    pub uploaded_bytes: u64,
    pub already: u64,
    pub failed: u64,
    pub skipped: u64,
    pub postponed: u64,
    /// Yerelde silindigi icin kDrive cop kutusuna tasinan dosya sayisi.
    #[serde(default)]
    pub deleted: u64,
    /// Yerelde silinmis ama onay bekledigi icin bulutta birakilan dosya sayisi.
    #[serde(default)]
    pub pending_deletes: u64,
    /// Yalnizca bulutta oldugu icin yerelde kisayolu olusturulan dosya sayisi.
    #[serde(default)]
    pub pulled: u64,
    /// Bulutta degistigi icin indirilen dosya sayisi.
    #[serde(default)]
    pub downloaded: u64,
    /// Bulutta tasindigi ya da adi degistigi icin yerelde de tasinan dosya sayisi.
    #[serde(default)]
    pub moved: u64,
    /// Bulutta silindigi icin yerelde cop kutusuna tasinan dosya sayisi.
    #[serde(default)]
    pub removed: u64,
    /// Bulutta silinmis ama onay bekledigi icin yerelde birakilan dosya sayisi.
    #[serde(default)]
    pub pending_cloud_deletes: u64,
    /// Iki tarafta da degistigi icin iki hali de saklanan dosya sayisi.
    #[serde(default)]
    pub conflicts: u64,
    /// Ilk birkac hata mesaji (arayuzde gostermek icin).
    #[serde(default)]
    pub errors: Vec<String>,
}

const MAX_ERRORS: usize = 20;

/// Bir dosyanin nerede duracagi.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Place {
    /// Yalnizca bulutta: yerel dosya cop kutusuna gider, yerine kDrive
    /// klasorundeki dosyayi gosteren bir kisayol (sembolik bag) kalir.
    Cloud,
    /// Bulutta ve bilgisayarda.
    Both,
    /// Yalnizca bilgisayarda: buluttaki kopya kDrive cop kutusuna gider ve
    /// dosya bir daha yedeklenmez.
    Local,
}

/// Konum degistirme isleminin sonucu.
#[derive(Default, Debug, Clone, Serialize)]
pub struct PlaceStats {
    pub done: u64,
    pub bytes: u64,
    /// Oldugu gibi birakilan dosya sayisi.
    pub kept: u64,
    pub errors: Vec<String>,
}

impl PlaceStats {
    fn error(&mut self, msg: String) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(msg);
        }
    }
}

struct Item {
    rel: String,
    abs: PathBuf,
    link: bool,
}

/// Yerelde silinen dosya sayisi bunu asarsa buluttan silmeden once onay istenir.
const DELETE_LIMIT: usize = 20;
/// Klasorun tum icerigi silinmisse onay istenen en kucuk dosya sayisi.
const EMPTIED_MIN: usize = 5;

/// Calisan bir yedeklemenin anlik durumu; arayuz buradan okur.
#[derive(Default)]
pub struct Progress {
    pub source: Mutex<String>,
    pub current: Mutex<String>,
    pub total_files: AtomicU64,
    pub done_files: AtomicU64,
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
}

impl Progress {
    fn begin(&self, source: &str, files: u64, bytes: u64) {
        *self.source.lock().unwrap() = source.to_string();
        self.current.lock().unwrap().clear();
        self.total_files.store(files, Ordering::Relaxed);
        self.total_bytes.store(bytes, Ordering::Relaxed);
        self.done_files.store(0, Ordering::Relaxed);
        self.done_bytes.store(0, Ordering::Relaxed);
    }
}

/// Her kaynak icin buluta yuklenmis dosya sayisi ve toplam boyut.
pub fn summary() -> BTreeMap<String, (u64, u64, u64, u64)> {
    load_state()
        .map(|st| {
            st.roots
                .into_iter()
                .map(|(name, rs)| {
                    let bytes = rs.files.values().map(|f| f.size).sum();
                    (name, (rs.files.len() as u64, bytes, rs.cloud_only.len() as u64, rs.local_only.len() as u64))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub struct Backup<'a> {
    pub api: &'a Api,
    /// Yedeklerin altina konacagi klasorun kimligi (ornegin Private).
    pub root_id: u64,
    /// O klasore gore yedek koku, ornegin "Yedekler/bilgisayar".
    pub backup_root: String,
    pub exclude: Vec<String>,
    pub progress: Option<std::sync::Arc<Progress>>,
    /// Ayarlanirsa (duraklatma) calisan yedekleme siradaki dosyaya gecmeden durur.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// kDrive'in baglandigi yerel klasor ("yalnizca bulutta" kisayollari icin).
    pub mountpoint: PathBuf,
    /// Yerelde silinen dosya buluttan da (cop kutusuna) silinsin mi.
    pub sync_deletes: bool,
    /// Cok sayida silme icin kullanici onay verdi.
    pub confirm_deletes: bool,
    /// Buluttaki degisiklikler bilgisayara yansitilsin mi (tam taramalarda).
    pub pull_cloud: bool,
    /// Bulutta silinen cok sayida dosyanin yerelden kaldirilmasina onay verildi.
    pub confirm_cloud_deletes: bool,
}

/// Buluttaki bir esitleme klasorunun o anki icerigi.
#[derive(Default)]
struct CloudTree {
    files: BTreeMap<String, Entry>,
    dirs: BTreeMap<String, u64>,
    /// Butun klasorler okunabildi mi; okunamadiysa "bulutta yok" denemez.
    complete: bool,
}

/// Yerel dosyayi masaustu cop kutusuna tasir; basardiysa true.
fn trash_local(path: &Path) -> bool {
    let _ = std::process::Command::new("gio")
        .arg("trash")
        .arg("--")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    path.symlink_metadata().is_err()
}

/// `rel` dosyasinin ust klasorlerinden, bulutta artik olmayan ve bosalmis
/// olanlari kaldirir.
fn prune_empty(local: &Path, rel: &str, cloud_dirs: &BTreeMap<String, u64>) {
    let mut dir = rel.rsplit_once('/').map(|(d, _)| d.to_string());
    while let Some(d) = dir {
        if cloud_dirs.contains_key(&d) || std::fs::remove_dir(local.join(&d)).is_err() {
            break;
        }
        dir = d.rsplit_once('/').map(|(p, _)| p.to_string());
    }
}

/// "rapor.pdf" -> "rapor (cakisan kopya bilgisayar 2026-11-11 142530).pdf"
fn conflict_name(name: &str) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = now_secs() as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    let tag = format!(
        "({} {} {:04}-{:02}-{:02} {:02}{:02}{:02})",
        tr!("conflicting copy"),
        hostname(),
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    );
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} {}{}", &name[..i], tag, &name[i..]),
        _ => format!("{} {}", name, tag),
    }
}


impl Backup<'_> {
    /// Bir yedegin buluttaki ust klasoru (koke gore yol).
    pub fn base_of(&self, source: &Source) -> String {
        source.dest.clone().unwrap_or_else(|| self.backup_root.clone())
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// `parent` altinda `name` adli klasoru bulur, yoksa olusturur.
    fn child_dir(&self, parent: u64, name: &str) -> Result<u64> {
        let existing = self.api.list_dir(parent)?;
        if let Some(e) = existing.iter().find(|e| e.name == name) {
            if !e.is_dir {
                bail!("bulutta '{}' adinda bir dosya var, klasor olusturulamiyor", name);
            }
            return Ok(e.id);
        }
        log::info!("bulutta klasor olusturuluyor: {}", name);
        self.api.create_dir(parent, name)
    }

    /// Bir yedegin goreli klasorunun buluttaki kimligini doner; gerekirse
    /// ust klasorleriyle birlikte olusturur. Sonuclar durumda saklanir.
    fn ensure_dir(&self, rs: &mut RootState, source: &Source, rel: &str) -> Result<u64> {
        if let Some(id) = rs.dirs.get(rel) {
            return Ok(*id);
        }
        let id = if rel.is_empty() {
            let mut id = self.root_id;
            for part in self.base_of(source).split('/').filter(|p| !p.is_empty()) {
                id = self.child_dir(id, part)?;
            }
            self.child_dir(id, &source.name)?
        } else {
            let (parent_rel, name) = match rel.rsplit_once('/') {
                Some((p, n)) => (p, n),
                None => ("", rel),
            };
            let parent = self.ensure_dir(rs, source, parent_rel)?;
            self.child_dir(parent, name)?
        };
        rs.dirs.insert(rel.to_string(), id);
        Ok(id)
    }

    /// Bir kaynagi tarar ve eksik ya da degismis dosyalari yukler.
    pub fn run(&self, source: &Source) -> Result<Stats> {
        let local = PathBuf::from(&source.local);
        if !local.is_dir() {
            bail!("yerel klasor yok: {}", source.local);
        }
        let mut state = load_state()?;
        let mut rs = state.roots.remove(&source.name).unwrap_or_default();
        // Hedef klasor degistiyse eski kimlikler gecersiz: bastan coz.
        let want = self.base_of(source);
        if rs.base.clone().unwrap_or_else(|| self.backup_root.clone()) != want {
            log::info!("{}: buluttaki hedef degisti, yeni yere yedeklenecek: {}", source.name, want);
            rs = RootState::default();
        }
        rs.base = Some(want);

        let mut w = Walk::default();
        walk(&local, "", &self.exclude, &mut w);
        let unreadable = w.unreadable;
        let mut stats = Stats {
            scanned: w.files.len() as u64,
            skipped: w.skipped,
            failed: w.unreadable,
            ..Default::default()
        };

        // 1. Durum kaydina gore degismemis olanlari ele.
        let now = now_secs();
        let mut pending: Vec<LocalFile> = Vec::new();
        for f in w.files {
            // Yerelde gercek dosya duruyor: "yalnizca bulutta" isareti eskimis
            // olabilir (yarida kesilen bir islemden kalma).
            rs.cloud_only.remove(&f.rel);
            match rs.files.get(&f.rel) {
                Some(r) if r.size == f.size && r.mtime == f.mtime => {}
                _ if rs.local_only.contains(&f.rel) => {} // yalnizca bilgisayarda
                _ if now - f.mtime < SETTLE_SECS => stats.postponed += 1,
                _ => pending.push(f),
            }
        }

        // 2. Gereken klasorleri olustur; bulutta zaten ayni haliyle duran
        //    dosyalari (durum kaydi kaybolmus olabilir) yeniden yukleme.
        let mut by_dir: BTreeMap<String, Vec<LocalFile>> = BTreeMap::new();
        for f in pending {
            by_dir.entry(f.dir_rel.clone()).or_default().push(f);
        }
        let mut jobs: VecDeque<(u64, LocalFile)> = VecDeque::new();
        let mut early_errors: Vec<String> = Vec::new();
        for (dir_rel, files) in by_dir {
            if self.cancelled() {
                break;
            }
            let dir_id = match self.ensure_dir(&mut rs, source, &dir_rel) {
                Ok(id) => id,
                Err(e) => {
                    log::warn!("klasor hazirlanamadi: {}: {:#}", dir_rel, e);
                    stats.failed += files.len() as u64;
                    continue;
                }
            };
            let remote: HashMap<String, Entry> = match self.api.list_dir(dir_id) {
                Ok(l) => l.into_iter().map(|e| (e.name.clone(), e)).collect(),
                Err(e) => {
                    // Kayitli klasor kimligi gecersiz olabilir; bir sonraki
                    // taramada yeniden cozulsun.
                    log::warn!("bulut klasoru listelenemedi: {}: {:#}", dir_rel, e);
                    rs.dirs.remove(&dir_rel);
                    stats.failed += files.len() as u64;
                    continue;
                }
            };
            for f in files {
                match remote.get(&f.name) {
                    Some(r) if !r.is_dir && r.size == f.size && r.mtime == f.mtime => {
                        rs.files.insert(
                            f.rel.clone(),
                            FileRec {
                                size: f.size,
                                mtime: f.mtime,
                                id: r.id,
                            },
                        );
                        stats.already += 1;
                    }
                    // Kayittan beri bulutta da degismis: iki hali de sakla.
                    Some(r)
                        if !r.is_dir
                            && rs
                                .files
                                .get(&f.rel)
                                .is_some_and(|rec| rec.id != r.id || rec.size != r.size || rec.mtime != r.mtime) =>
                    {
                        match self.keep_both(&f.abs, r) {
                            Ok(copy) => {
                                log::info!("iki tarafta da degismis, yerel hali ayri saklandi: {}/{} -> {}", source.name, f.rel, copy);
                                rs.files.insert(f.rel.clone(), FileRec { size: r.size, mtime: r.mtime, id: r.id });
                                rs.cloud_only.remove(&f.rel);
                                stats.conflicts += 1;
                            }
                            Err(e) => {
                                log::warn!("cakisma cozulemedi: {}/{}: {:#}", source.name, f.rel, e);
                                stats.failed += 1;
                                early_errors.push(format!("{}: {:#}", f.rel, e));
                            }
                        }
                    }
                    _ => jobs.push_back((dir_id, f)),
                }
            }
        }

        // 3. Yuklemeleri paralel yap; durum dosyasini arada bir kaydet ki
        //    kesinti olursa yapilan is kaybolmasin.
        let total = jobs.len();
        if total > 0 {
            log::info!("{}: {} dosya yuklenecek", source.name, total);
        }
        if let Some(p) = &self.progress {
            p.begin(&source.name, total as u64, jobs.iter().map(|(_, f)| f.size).sum());
        }
        let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let queue = Mutex::new(jobs);
        let done: Mutex<Vec<(String, FileRec)>> = Mutex::new(Vec::new());
        let uploaded = AtomicU64::new(0);
        let bytes = AtomicU64::new(0);
        let failed = AtomicU64::new(0);
        let shared = Mutex::new((&mut state, &mut rs, Instant::now()));
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(total.max(1)) {
                scope.spawn(|| loop {
                    if self.cancelled() {
                        break;
                    }
                    let Some((dir_id, f)) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    if let Some(p) = &self.progress {
                        *p.current.lock().unwrap() = f.rel.clone();
                    }
                    let fsize = f.size;
                    match self.api.upload_file(dir_id, &f.name, &f.abs, f.size, f.mtime) {
                        Ok(up) => {
                            log::info!("yuklendi: {}/{} ({} bayt)", source.name, f.rel, f.size);
                            uploaded.fetch_add(1, Ordering::Relaxed);
                            bytes.fetch_add(f.size, Ordering::Relaxed);
                            let rec = FileRec {
                                size: f.size,
                                mtime: f.mtime,
                                id: up.id,
                            };
                            done.lock().unwrap().push((f.rel, rec));
                        }
                        Err(e) => {
                            log::warn!("yuklenemedi: {}/{}: {:#}", source.name, f.rel, e);
                            failed.fetch_add(1, Ordering::Relaxed);
                            let mut errs = errors.lock().unwrap();
                            if errs.len() < MAX_ERRORS {
                                errs.push(format!("{}: {:#}", f.rel, e));
                            }
                        }
                    }
                    if let Some(p) = &self.progress {
                        p.done_files.fetch_add(1, Ordering::Relaxed);
                        p.done_bytes.fetch_add(fsize, Ordering::Relaxed);
                    }
                    // Ara kayit.
                    let mut guard = shared.lock().unwrap();
                    let (st, rs, last) = &mut *guard;
                    for (rel, rec) in done.lock().unwrap().drain(..) {
                        rs.files.insert(rel, rec);
                    }
                    if last.elapsed() > Duration::from_secs(20) {
                        let snapshot = std::mem::take(&mut **rs);
                        st.roots.insert(source.name.clone(), snapshot);
                        if let Err(e) = save_state(st) {
                            log::warn!("durum kaydedilemedi: {:#}", e);
                        }
                        **rs = st.roots.remove(&source.name).unwrap_or_default();
                        *last = Instant::now();
                    }
                });
            }
        });
        drop(shared);
        for (rel, rec) in done.into_inner().unwrap() {
            rs.files.insert(rel, rec);
        }
        stats.uploaded = uploaded.into_inner();
        stats.uploaded_bytes = bytes.into_inner();
        stats.failed += failed.into_inner();
        stats.errors = early_errors;
        stats.errors.extend(errors.into_inner().unwrap());
        stats.errors.truncate(MAX_ERRORS);

        // 4. Yerelde silinen dosyalar buluttan da kDrive cop kutusuna tasinir.
        let gone = |rel: &String| {
            matches!(local.join(rel).symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        };
        rs.local_only.retain(|rel| !gone(rel));
        if self.sync_deletes && unreadable == 0 && !self.cancelled() {
            let missing: Vec<String> = rs.files.keys().filter(|rel| gone(rel)).cloned().collect();
            // Klasorun tamami bosalmissa (baglanmamis disk olabilir) onay iste;
            // birkac dosyalik kucuk klasorlerde bu kural uygulanmaz.
            let everything = missing.len() >= EMPTIED_MIN && missing.len() == rs.files.len();
            if !rs.deletes_synced {
                // Bu ozellikten once silinmis olanlar: bulutta kalsin, takibi birak.
                for rel in &missing {
                    rs.files.remove(rel);
                    rs.cloud_only.remove(rel);
                }
                rs.deletes_synced = true;
            } else if (missing.len() > DELETE_LIMIT || everything) && !self.confirm_deletes {
                // Toplu silme (ya da klasorun bosalmasi) bir hata olabilir: onay bekle.
                stats.pending_deletes = missing.len() as u64;
            } else {
                for rel in missing {
                    let Some(id) = rs.files.get(&rel).map(|r| r.id) else { continue };
                    match self.api.trash(id) {
                        Ok(()) => {
                            log::info!("yerelde silindi, kDrive cop kutusuna tasindi: {}/{}", source.name, rel);
                            stats.deleted += 1;
                            rs.files.remove(&rel);
                            rs.cloud_only.remove(&rel);
                        }
                        Err(e) if format!("{:#}", e).contains("HTTP 404") => {
                            rs.files.remove(&rel); // bulutta zaten yok
                            rs.cloud_only.remove(&rel);
                        }
                        Err(e) => {
                            log::warn!("buluttan silinemedi: {}/{}: {:#}", source.name, rel, e);
                            stats.failed += 1;
                            if stats.errors.len() < MAX_ERRORS {
                                stats.errors.push(format!("{}: {}: {:#}", rel, tr!("could not delete from the cloud"), e));
                            }
                        }
                    }
                }
            }
        }

        // 5. Buluttaki degisiklikler bilgisayara yansir.
        if self.pull_cloud && self.sync_deletes && rs.deletes_synced && stats.pending_deletes == 0 && unreadable == 0 && !self.cancelled() {
            if let Err(e) = self.from_cloud(source, &local, &mut rs, &mut stats) {
                log::warn!("{}: buluttaki degisiklikler alinamadi: {:#}", source.name, e);
            }
        } else {
            // Bu calismada bakilmadi: bekleyen soru yerinde dursun.
            stats.pending_cloud_deletes = rs.cloud_missing.len() as u64;
        }

        state.roots.insert(source.name.clone(), rs);
        save_state(&state)?;
        Ok(stats)
    }

    /// Esitleme klasorunun buluttaki kimligini yoldan cozer (olusturmadan).
    fn find_root(&self, source: &Source) -> Result<Option<u64>> {
        let mut id = self.root_id;
        let base = self.base_of(source);
        for part in base.split('/').filter(|p| !p.is_empty()).chain(std::iter::once(source.name.as_str())) {
            match self.api.list_dir(id)?.into_iter().find(|e| e.is_dir && e.name == part) {
                Some(e) => id = e.id,
                None => return Ok(None),
            }
        }
        Ok(Some(id))
    }

    fn cloud_walk(&self, dir_id: u64, rel: &str, out: &mut CloudTree) {
        if self.cancelled() {
            out.complete = false;
            return;
        }
        let list = match self.api.list_dir(dir_id) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("bulut klasoru listelenemedi: {}: {:#}", rel, e);
                out.complete = false;
                return;
            }
        };
        out.dirs.insert(rel.to_string(), dir_id);
        for e in list {
            if excluded(&self.exclude, &e.name) {
                continue;
            }
            let child = join_rel(rel, &e.name);
            if e.is_dir {
                self.cloud_walk(e.id, &child, out);
            } else {
                out.files.insert(child, e);
            }
        }
    }

    fn excluded_rel(&self, rel: &str) -> bool {
        rel.split('/').any(|part| excluded(&self.exclude, part))
    }

    /// Buluttaki dosyayi indirip `path` yerine koyar (once gecici dosyaya).
    fn fetch(&self, e: &Entry, path: &Path) -> Result<()> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!(".{}.kdlite.part", name));
        let res = (|| -> Result<()> {
            let mut resp = self.api.download(e.id).context(tr!("could not download from the cloud"))?;
            let mut f = std::fs::File::create(&tmp)?;
            let n = std::io::copy(&mut resp, &mut f)?;
            if n != e.size {
                bail!("{}", tr!("the downloaded file has the wrong size"));
            }
            // Buluttaki zamani koru ki dosya yeniden yuklenmesin.
            f.set_modified(UNIX_EPOCH + Duration::from_secs(e.mtime.max(0) as u64))?;
            if let Ok(old) = std::fs::metadata(path) {
                let _ = f.set_permissions(old.permissions());
            }
            drop(f);
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    }

    /// Iki tarafta da degismis dosya: yerel hali yeni bir adla saklanir (bir
    /// sonraki calismada o da yuklenir), buluttaki hali asil ada indirilir.
    /// Saklanan kopyanin adini doner.
    fn keep_both(&self, path: &Path, e: &Entry) -> Result<String> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let copy = conflict_name(&name);
        let copy_path = path.with_file_name(&copy);
        if copy_path.symlink_metadata().is_ok() {
            bail!("{}", tr!("a conflicting copy with the same name already exists"));
        }
        std::fs::rename(path, &copy_path)?;
        if let Err(err) = self.fetch(e, path) {
            let _ = std::fs::rename(&copy_path, path); // geri al
            return Err(err);
        }
        Ok(copy)
    }

    /// Buluttaki agaci kayitla karsilastirir ve farklari bilgisayara uygular:
    /// degisen dosya indirilir, tasinan tasinir, yeni dosya icin kisayol
    /// konur, silinen dosyanin yerel kopyasi cop kutusuna gider.
    fn from_cloud(&self, source: &Source, local: &Path, rs: &mut RootState, stats: &mut Stats) -> Result<()> {
        let Some(root_id) = self.find_root(source)? else {
            return Ok(()); // bulutta klasor henuz yok
        };
        let cloud_root = self.mountpoint.join(self.base_of(source)).join(&source.name);
        let mut tree = CloudTree { complete: true, ..Default::default() };
        self.cloud_walk(root_id, "", &mut tree);
        let dry = std::env::var_os("KDLITE_DRY_SYNC").is_some();
        // Kisayollar bulut klasorune bakar: bagli degilse olusturma.
        let mount_ok = cloud_root.is_dir();
        let note = |stats: &mut Stats, rel: &str, e: anyhow::Error| {
            log::warn!("{}/{}: {:#}", source.name, rel, e);
            stats.failed += 1;
            if stats.errors.len() < MAX_ERRORS {
                stats.errors.push(format!("{}: {:#}", rel, e));
            }
        };
        let rec_of = |e: &Entry| FileRec { size: e.size, mtime: e.mtime, id: e.id };
        let by_id: HashMap<u64, String> = rs.files.iter().map(|(k, v)| (v.id, k.clone())).collect();

        for (rel, e) in &tree.files {
            if self.cancelled() {
                return Ok(());
            }
            if rs.local_only.contains(rel) {
                continue;
            }
            let path = local.join(rel);
            if !rs.files.contains_key(rel) {
                // Kayitta yok: bulutta tasinmis ya da yeni.
                if path.symlink_metadata().is_ok() {
                    continue; // yerelde ayni adda bir sey var; yukleme adimi ilgilenir
                }
                let from = by_id.get(&e.id).filter(|old| !tree.files.contains_key(*old) && rs.files.contains_key(*old)).cloned();
                if let Some(old) = from {
                    let old_path = local.join(&old);
                    let Ok(meta) = old_path.symlink_metadata() else { continue };
                    let link = meta.file_type().is_symlink() && rs.cloud_only.contains(&old);
                    if !link && !meta.is_file() {
                        continue;
                    }
                    log::info!("bulutta tasindi: {}/{} -> {}", source.name, old, rel);
                    if dry {
                        continue;
                    }
                    let done = (|| -> Result<()> {
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        if link {
                            std::os::unix::fs::symlink(cloud_root.join(rel), &path)?;
                            std::fs::remove_file(&old_path)?;
                        } else {
                            std::fs::rename(&old_path, &path)?;
                        }
                        Ok(())
                    })();
                    match done {
                        Ok(()) => {
                            if let Some(r) = rs.files.remove(&old) {
                                rs.files.insert(rel.clone(), r);
                            }
                            if rs.cloud_only.remove(&old) {
                                rs.cloud_only.insert(rel.clone());
                            }
                            stats.moved += 1;
                            prune_empty(local, &old, &tree.dirs);
                        }
                        Err(err) => {
                            note(stats, rel, err);
                            continue;
                        }
                    }
                    // Tasinirken icerigi de degismis olabilir: asagida bakilir.
                } else {
                    if !mount_ok {
                        continue; // bulut klasoru bagli degil; sonraki taramada
                    }
                    let target = cloud_root.join(rel);
                    log::info!("bulutta yeni: {}/{}", source.name, rel);
                    if dry {
                        continue;
                    }
                    if let Some(parent) = path.parent() {
                        if std::fs::create_dir_all(parent).is_err() {
                            continue;
                        }
                    }
                    if std::os::unix::fs::symlink(&target, &path).is_ok() {
                        rs.files.insert(rel.clone(), rec_of(e));
                        rs.cloud_only.insert(rel.clone());
                        stats.pulled += 1;
                    }
                    continue;
                }
            }
            let Some(r) = rs.files.get(rel).cloned() else { continue };
            if r.id == e.id && r.size == e.size && r.mtime == e.mtime {
                continue;
            }
            // Bulutta degismis.
            if rs.cloud_only.contains(rel) {
                rs.files.insert(rel.clone(), rec_of(e));
                continue;
            }
            let Ok(meta) = path.symlink_metadata() else { continue }; // yerelde silinmis: silme adimi ilgilenir
            if !meta.is_file() {
                continue;
            }
            let (size, mtime) = (meta.len(), mtime_secs(&meta));
            if size == e.size && mtime == e.mtime {
                rs.files.insert(rel.clone(), rec_of(e)); // zaten ayni
            } else if size == r.size && mtime == r.mtime {
                log::info!("bulutta degisti, indiriliyor: {}/{}", source.name, rel);
                if dry {
                    continue;
                }
                match self.fetch(e, &path) {
                    Ok(()) => {
                        rs.files.insert(rel.clone(), rec_of(e));
                        stats.downloaded += 1;
                    }
                    Err(err) => note(stats, rel, err),
                }
            } else {
                log::info!("iki tarafta da degismis: {}/{}", source.name, rel);
                if dry {
                    continue;
                }
                match self.keep_both(&path, e) {
                    Ok(_) => {
                        rs.files.insert(rel.clone(), rec_of(e));
                        stats.conflicts += 1;
                    }
                    Err(err) => note(stats, rel, err),
                }
            }
        }

        if !tree.complete {
            stats.pending_cloud_deletes = rs.cloud_missing.len() as u64;
            return Ok(());
        }

        // Kayitta olup bulutta artik bulunmayanlar: bulutta silinmis.
        let gone: Vec<String> = rs
            .files
            .keys()
            .filter(|rel| !tree.files.contains_key(*rel) && !self.excluded_rel(rel))
            .cloned()
            .collect();
        let mut waiting: Vec<String> = Vec::new(); // yerelde hala duran, kaldirilacak olanlar
        for rel in &gone {
            match local.join(rel).symlink_metadata() {
                Err(_) => {
                    // Iki tarafta da yok.
                    rs.files.remove(rel);
                    rs.cloud_only.remove(rel);
                }
                Ok(meta) if meta.is_file() => {
                    let same = rs.files.get(rel).is_some_and(|r| r.size == meta.len() && r.mtime == mtime_secs(&meta));
                    if same {
                        waiting.push(rel.clone());
                    } else if !dry {
                        // Yerelde de degismis: duzenleme silmeye baskin gelir, yeniden yuklenir.
                        rs.files.remove(rel);
                    }
                }
                Ok(meta) if meta.file_type().is_symlink() && rs.cloud_only.contains(rel) => waiting.push(rel.clone()),
                Ok(_) => {
                    rs.files.remove(rel);
                    rs.cloud_only.remove(rel);
                }
            }
        }
        let everything = waiting.len() >= EMPTIED_MIN && waiting.len() == rs.files.len();
        if (waiting.len() > DELETE_LIMIT || everything) && !self.confirm_cloud_deletes {
            log::info!("{}: bulutta silinmis {} dosya icin onay bekleniyor", source.name, waiting.len());
            stats.pending_cloud_deletes = waiting.len() as u64;
            rs.cloud_missing = waiting.into_iter().collect();
        } else {
            rs.cloud_missing.clear();
            for rel in waiting {
                log::info!("bulutta silindi, yerel kopya cop kutusuna tasiniyor: {}/{}", source.name, rel);
                if dry {
                    continue;
                }
                let path = local.join(&rel);
                let ok = if rs.cloud_only.contains(&rel) {
                    std::fs::remove_file(&path).is_ok() // yalnizca kisayol
                } else {
                    trash_local(&path)
                };
                if !ok {
                    note(stats, &rel, anyhow!("{}", tr!("could not be moved to the trash")));
                    continue;
                }
                rs.files.remove(&rel);
                rs.cloud_only.remove(&rel);
                stats.removed += 1;
                prune_empty(local, &rel, &tree.dirs);
            }
        }
        if !dry {
            // Bulutta olan klasorler (bos olsalar da) yerelde de bulunsun.
            for d in tree.dirs.keys().filter(|d| !d.is_empty()) {
                let p = local.join(d);
                if p.symlink_metadata().is_err() {
                    let _ = std::fs::create_dir_all(&p);
                }
            }
            rs.dirs = tree.dirs;
        }
        if stats.pulled + stats.downloaded + stats.moved + stats.removed + stats.conflicts > 0 {
            log::info!(
                "{}: buluttan: {} yeni kisayol, {} indirildi, {} tasindi, {} kaldirildi, {} cakisma",
                source.name, stats.pulled, stats.downloaded, stats.moved, stats.removed, stats.conflicts
            );
        }
        Ok(())
    }

    /// Bulutta silinmis olup onay bekleyen dosyalar yerelde kalsin: takipten
    /// cikarilir, bir sonraki calismada buluta yeniden yuklenirler.
    pub fn restore_cloud_missing(&self, source: &Source) -> Result<u64> {
        let mut state = load_state()?;
        let Some(rs) = state.roots.get_mut(&source.name) else { return Ok(0) };
        let list = std::mem::take(&mut rs.cloud_missing);
        for rel in &list {
            if !rs.cloud_only.contains(rel) {
                rs.files.remove(rel);
            }
        }
        save_state(&state)?;
        Ok(list.len() as u64)
    }

    /// Yerelde silinmis gorunen dosyalarin takibini birakir (bulutta kalirlar).
    pub fn forget_missing(&self, source: &Source) -> Result<u64> {
        let local = PathBuf::from(&source.local);
        let mut state = load_state()?;
        let Some(rs) = state.roots.get_mut(&source.name) else { return Ok(0) };
        let missing: Vec<String> = rs
            .files
            .keys()
            .filter(|rel| matches!(local.join(rel).symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound))
            .cloned()
            .collect();
        for rel in &missing {
            rs.files.remove(rel);
            rs.cloud_only.remove(rel);
        }
        save_state(&state)?;
        Ok(missing.len() as u64)
    }

    fn collect(&self, root: &Path, path: &Path, out: &mut Vec<Item>) {
        let Ok(meta) = path.symlink_metadata() else { return };
        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => return,
        };
        if meta.is_dir() {
            if path.join("CACHEDIR.TAG").exists() {
                return;
            }
            let Ok(rd) = std::fs::read_dir(path) else { return };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for ent in entries {
                let name = ent.file_name().to_string_lossy().into_owned();
                if !excluded(&self.exclude, &name) {
                    self.collect(root, &ent.path(), out);
                }
            }
        } else if meta.is_file() || meta.file_type().is_symlink() {
            out.push(Item {
                rel,
                abs: path.to_path_buf(),
                link: meta.file_type().is_symlink(),
            });
        }
    }

    /// Verilen dosyalarin (klasorse icindekilerin) konumunu degistirir.
    pub fn place(&self, source: &Source, paths: &[PathBuf], mode: Place) -> Result<PlaceStats> {
        let root = PathBuf::from(&source.local);
        if !root.is_dir() {
            bail!("yerel klasor yok: {}", source.local);
        }
        let mut state = load_state()?;
        let mut rs = state.roots.remove(&source.name).unwrap_or_default();
        if rs.base.clone().unwrap_or_else(|| self.backup_root.clone()) != self.base_of(source) {
            bail!("{}", tr!("the folder is not synced to its new place yet; try again when the sync finishes"));
        }
        let cloud_root = self.mountpoint.join(self.base_of(source)).join(&source.name);
        let mut items = Vec::new();
        for p in paths {
            self.collect(&root, p, &mut items);
        }
        if let Some(p) = &self.progress {
            p.begin(&source.name, items.len() as u64, 0);
        }
        let mut out = PlaceStats::default();
        let mut listings: HashMap<String, Option<HashMap<String, Entry>>> = HashMap::new();
        for item in &items {
            if self.cancelled() {
                out.kept += 1;
                continue;
            }
            let res = match mode {
                Place::Cloud => self.to_cloud(item, &mut rs, &cloud_root, &mut listings),
                Place::Both => self.to_both(item, &mut rs, &cloud_root),
                Place::Local => self.to_local(item, &mut rs, &cloud_root),
            };
            match res {
                Ok(Some(bytes)) => {
                    out.done += 1;
                    out.bytes += bytes;
                }
                Ok(None) => out.kept += 1,
                Err(e) => {
                    out.kept += 1;
                    out.error(format!("{}: {:#}", item.rel, e));
                }
            }
            if let Some(p) = &self.progress {
                p.done_files.fetch_add(1, Ordering::Relaxed);
            }
        }
        state.roots.insert(source.name.clone(), rs);
        save_state(&state)?;
        log::info!("{}: konum degisti ({:?}): {} dosya, {} oldugu gibi", source.name, mode, out.done, out.kept);
        Ok(out)
    }

    /// Yalnizca bulutta: bulutta birebir durdugu o an dogrulanan yerel dosya
    /// cop kutusuna gider, yerine kDrive klasorundeki dosyaya kisayol kalir.
    fn to_cloud(
        &self,
        item: &Item,
        rs: &mut RootState,
        cloud_root: &Path,
        listings: &mut HashMap<String, Option<HashMap<String, Entry>>>,
    ) -> Result<Option<u64>> {
        if item.link || rs.local_only.contains(&item.rel) {
            return Ok(None);
        }
        let meta = item.abs.symlink_metadata()?;
        let (size, mtime) = (meta.len(), mtime_secs(&meta));
        let uploaded = rs.files.get(&item.rel).is_some_and(|r| r.size == size && r.mtime == mtime);
        if !uploaded {
            bail!("{}", tr!("not synced yet"));
        }
        let (dir_rel, name) = match item.rel.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n),
            None => (String::new(), item.rel.as_str()),
        };
        // Bulutu simdi sor: kayda degil, o anki listeye guven.
        let listing = match listings.get(&dir_rel) {
            Some(l) => l,
            None => {
                let l = match rs.dirs.get(&dir_rel) {
                    Some(id) => Some(self.api.list_dir(*id)?.into_iter().map(|e| (e.name.clone(), e)).collect()),
                    None => None,
                };
                listings.entry(dir_rel.clone()).or_insert(l)
            }
        };
        let in_cloud = listing
            .as_ref()
            .and_then(|l| l.get(name))
            .is_some_and(|r| !r.is_dir && r.size == size && r.mtime == mtime);
        if !in_cloud {
            bail!("{}", tr!("no identical copy found in the cloud"));
        }
        let target = cloud_root.join(&item.rel);
        if target.symlink_metadata().is_err() {
            bail!("{}", tr!("the kDrive folder is not mounted or the file is not visible there"));
        }
        if !trash_local(&item.abs) {
            bail!("{}", tr!("could not be moved to the trash"));
        }
        std::os::unix::fs::symlink(&target, &item.abs).context(tr!("could not create the shortcut"))?;
        rs.cloud_only.insert(item.rel.clone());
        Ok(Some(size))
    }

    /// Bulutta ve bilgisayarda: yalnizca buluttaki dosya geri indirilir;
    /// yalnizca bilgisayardaki dosya yeniden yedeklenmek uzere isaretlenir.
    fn to_both(&self, item: &Item, rs: &mut RootState, _cloud_root: &Path) -> Result<Option<u64>> {
        if !item.link {
            return Ok(rs.local_only.remove(&item.rel).then_some(0));
        }
        if !rs.cloud_only.contains(&item.rel) {
            return Ok(None); // bizim olusturmadigimiz bir bag
        }
        let rec = rs.files.get(&item.rel).cloned().ok_or_else(|| anyhow!("{}", tr!("record not found")))?;
        // Dogrudan buluttan indir: kendi bagladigimiz klasorden okumak,
        // kapanis sirasinda surecin kendi kendini beklemesine yol acabilir.
        let entry = Entry { id: rec.id, name: String::new(), is_dir: false, size: rec.size, mtime: rec.mtime };
        self.fetch(&entry, &item.abs)?; // kisayolun yerine gecer
        rs.cloud_only.remove(&item.rel);
        Ok(Some(rec.size))
    }

    /// Yalnizca bilgisayarda: buluttaki kopya kDrive cop kutusuna gider.
    fn to_local(&self, item: &Item, rs: &mut RootState, cloud_root: &Path) -> Result<Option<u64>> {
        if item.link {
            if !rs.cloud_only.contains(&item.rel) {
                return Ok(None);
            }
            self.to_both(item, rs, cloud_root)?; // once geri indir
        }
        let size = item.abs.metadata().map(|m| m.len()).unwrap_or(0);
        if let Some(rec) = rs.files.get(&item.rel).cloned() {
            match self.api.trash(rec.id) {
                Ok(()) => {}
                Err(e) if format!("{:#}", e).contains("HTTP 404") => {}
                Err(e) => return Err(e.context(tr!("could not delete from the cloud"))),
            }
            rs.files.remove(&item.rel);
        } else if rs.local_only.contains(&item.rel) {
            return Ok(None);
        }
        rs.local_only.insert(item.rel.clone());
        Ok(Some(size))
    }

    /// Yerel dosyalari buluttakilerle (ad ve boyut) karsilastirir.
    /// (eksik ya da farkli dosyalarin goreli yollari, kontrol edilen sayi)
    pub fn verify(&self, source: &Source) -> Result<(Vec<String>, u64)> {
        let local = PathBuf::from(&source.local);
        let mut w = Walk::default();
        walk(&local, "", &self.exclude, &mut w);
        let mut by_dir: BTreeMap<String, Vec<LocalFile>> = BTreeMap::new();
        for f in w.files {
            by_dir.entry(f.dir_rel.clone()).or_default().push(f);
        }
        // Buluttaki agaci kokten yuruyerek coz (durum kaydina guvenmeden).
        let mut base = self.root_id;
        let parent = self.base_of(source);
        for part in parent
            .split('/')
            .filter(|p| !p.is_empty())
            .chain(std::iter::once(source.name.as_str()))
        {
            base = self
                .api
                .list_dir(base)?
                .into_iter()
                .find(|e| e.is_dir && e.name == part)
                .map(|e| e.id)
                .ok_or_else(|| anyhow!("bulutta yedek klasoru yok: {}", part))?;
        }
        let mut listings: HashMap<String, Option<HashMap<String, Entry>>> = HashMap::new();
        let mut dir_ids: HashMap<String, Option<u64>> = HashMap::new();
        dir_ids.insert(String::new(), Some(base));
        let mut problems = Vec::new();
        let mut checked = 0;
        for (dir_rel, files) in by_dir {
            let listing = self.remote_listing(&dir_rel, &mut dir_ids, &mut listings)?;
            for f in files {
                checked += 1;
                let ok = listing
                    .as_ref()
                    .and_then(|l| l.get(&f.name))
                    .is_some_and(|r| !r.is_dir && r.size == f.size);
                if !ok {
                    problems.push(f.rel);
                }
            }
        }
        Ok((problems, checked))
    }

    fn remote_listing(
        &self,
        rel: &str,
        dir_ids: &mut HashMap<String, Option<u64>>,
        listings: &mut HashMap<String, Option<HashMap<String, Entry>>>,
    ) -> Result<Option<HashMap<String, Entry>>> {
        if let Some(l) = listings.get(rel) {
            return Ok(l.clone());
        }
        let id = match dir_ids.get(rel) {
            Some(id) => *id,
            None => {
                let (parent, name) = rel.rsplit_once('/').unwrap_or(("", rel));
                let id = self
                    .remote_listing(parent, dir_ids, listings)?
                    .and_then(|l| l.get(name).filter(|e| e.is_dir).map(|e| e.id));
                dir_ids.insert(rel.to_string(), id);
                id
            }
        };
        let listing = match id {
            Some(id) => Some(
                self.api
                    .list_dir(id)?
                    .into_iter()
                    .map(|e| (e.name.clone(), e))
                    .collect::<HashMap<_, _>>(),
            ),
            None => None,
        };
        listings.insert(rel.to_string(), listing.clone());
        Ok(listing)
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bilgisayar".into())
}
