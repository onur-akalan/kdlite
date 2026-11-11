//! FUSE dosya sistemi. Okuma istege bagli indirmeyle calisir.
//!
//! Yazma (ayarda `writable` acikken) kademelidir ve bulutta hicbir seyi
//! silmez ya da tasimaz:
//! - Yeni dosya ve duzenlenen dosya once yerelde "bekleyen" olarak tutulur;
//!   kapandiktan birkac saniye sonra buluta yuklenir (duzenlenen dosyanin
//!   eski hali kDrive surum gecmisinde kalir).
//! - Henuz yuklenmemis bekleyen dosya silinebilir ve yeniden adlandirilabilir;
//!   var olan bir dosyanin uzerine tasinirsa (duzenleyicilerin "gecici dosyaya
//!   yaz, sonra uzerine tasi" kaydetme yontemi) o dosyanin yeni surumu olur.
//! - Silinen dosya ve bos klasor kDrive cop kutusuna tasinir (geri alinabilir).
//! - Buluttaki bir dosyayi ya da klasoru tasimak, yeniden adlandirmak
//!   reddedilir (EPERM).

use crate::api::{Api, Entry};
use crate::cache::Cache;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use libc::{c_int, EEXIST, EIO, EISDIR, ENOENT, ENOTDIR, EPERM, EROFS};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Buluta yuklenmeyi bekleyen ve yuklenemeyen dosya sayilari (arayuz icin).
pub static PENDING: AtomicU64 = AtomicU64::new(0);
pub static FAILED: AtomicU64 = AtomicU64::new(0);

/// Henuz buluta yuklenmemis yeni dosyalarin gecici inode araligi.
const STAGE_BASE: u64 = 1 << 50;
const STAGED_TTL: Duration = Duration::from_secs(1);
/// Dosya kapandiktan sonra yuklemeden once beklenen sure; gecici dosyalarin
/// (kaydetme sirasinda olusturulup hemen tasinanlar) buluta gitmesini onler.
const UPLOAD_DELAY: Duration = Duration::from_secs(5);
const UPLOAD_RETRY: Duration = Duration::from_secs(60);

/// Yerelde tutulan, buluta yuklenmeyi bekleyen dosya icerigi.
struct Staged {
    seq: u64,
    path: PathBuf,
    file: Arc<File>,
    /// Yuklenecegi bulut klasoru ve oradaki adi.
    parent_id: u64,
    name: String,
    open: u32,
    /// Buluttakinden farkli icerik var.
    dirty: bool,
    /// Her degisiklikte artar; yukleme sirasinda degisirse yeniden yuklenir.
    gen: u64,
    closed_at: Instant,
    uploading: bool,
    next_try: Instant,
    failed: bool,
    /// Haric tutulan adlar (kilit ve gecici dosyalar) hic yuklenmez.
    local_only: bool,
}

struct WriteCfg {
    dir: PathBuf,
    exclude: Vec<String>,
}

const ROOT_INO: u64 = 1;
/// kDrive dosya kimlikleri bu ofsetle inode numarasina cevrilir (1 kok icin ayrili).
const INO_BASE: u64 = 1 << 40;
const KERNEL_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Node {
    entry: Entry,
    parent: u64,
}

struct Nodes {
    by_ino: HashMap<u64, Node>,
    children: HashMap<u64, (Instant, Vec<u64>)>,
    /// inode -> bekleyen icerik.
    staged: HashMap<u64, Staged>,
    /// Yuklenince kimligi degisen dosyalar: eski inode -> yeni inode.
    alias: HashMap<u64, u64>,
    seq: u64,
}

impl Nodes {
    fn real(&self, mut ino: u64) -> u64 {
        for _ in 0..4 {
            match self.alias.get(&ino) {
                Some(n) => ino = *n,
                None => break,
            }
        }
        ino
    }

    fn counts(&self) {
        let pending = self.staged.values().filter(|s| s.dirty && !s.local_only).count() as u64;
        let failed = self.staged.values().filter(|s| s.failed).count() as u64;
        PENDING.store(pending, Ordering::Relaxed);
        FAILED.store(failed, Ordering::Relaxed);
    }

    fn drop_child(&mut self, parent: u64, ino: u64) {
        if let Some((_, kids)) = self.children.get_mut(&parent) {
            kids.retain(|k| *k != ino);
        }
    }

    fn add_child(&mut self, parent: u64, ino: u64) {
        if let Some((_, kids)) = self.children.get_mut(&parent) {
            if !kids.contains(&ino) {
                kids.push(ino);
            }
        }
    }
}

pub struct Inner {
    api: Arc<Api>,
    pub cache: Arc<Cache>,
    nodes: Mutex<Nodes>,
    root_id: u64,
    dir_ttl: Duration,
    uid: u32,
    gid: u32,
    /// Yazma acikken dolu.
    write: Option<WriteCfg>,
    /// Yukleyiciyi uyandirir (nodes kilidiyle birlikte kullanilir).
    wake: Condvar,
}

fn safe_name(name: &str) -> String {
    name.replace('/', "_")
}

impl Inner {
    pub fn new(api: Arc<Api>, cache: Arc<Cache>, root_id: u64, dir_ttl: Duration) -> Inner {
        Inner {
            api,
            cache,
            nodes: Mutex::new(Nodes {
                by_ino: HashMap::new(),
                children: HashMap::new(),
                staged: HashMap::new(),
                alias: HashMap::new(),
                seq: 0,
            }),
            root_id,
            dir_ttl,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            write: None,
            wake: Condvar::new(),
        }
    }

    /// Yazmayi acar: bekleyen dosyalar `dir` altinda tutulur.
    pub fn with_write(mut self, dir: PathBuf, exclude: Vec<String>) -> std::io::Result<Inner> {
        std::fs::create_dir_all(&dir)?;
        // Onceki calismadan kalanlarla cakismamak icin sayaci ilerlet.
        let max = std::fs::read_dir(&dir)?
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().split('.').next()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.nodes.get_mut().unwrap().seq = max;
        self.write = Some(WriteCfg { dir, exclude });
        Ok(self)
    }

    pub fn writable(&self) -> bool {
        self.write.is_some()
    }

    fn local_only(&self, name: &str) -> bool {
        self.write
            .as_ref()
            .is_some_and(|w| w.exclude.iter().any(|p| crate::backup::matches(p, name)))
    }

    fn parent_dir_id(&self, nodes: &Nodes, parent_ino: u64) -> Result<u64, c_int> {
        if parent_ino == ROOT_INO {
            return Ok(self.root_id);
        }
        match nodes.by_ino.get(&parent_ino) {
            Some(n) if n.entry.is_dir => Ok(n.entry.id),
            Some(_) => Err(ENOTDIR),
            None => Err(ENOENT),
        }
    }

    fn write_meta(path: &Path, parent_id: u64, name: &str) {
        let meta = serde_json::json!({ "parent_id": parent_id, "name": name });
        let _ = std::fs::write(path.with_extension("json"), meta.to_string());
    }

    fn remove_files(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("json"));
    }

    /// Bos bir bekleyen dosya olusturur (kilit tutulurken cagrilir).
    fn new_staged(&self, nodes: &mut Nodes, parent_id: u64, name: &str, from: Option<&Path>) -> std::io::Result<Staged> {
        let w = self.write.as_ref().ok_or_else(|| std::io::Error::from_raw_os_error(EROFS))?;
        nodes.seq += 1;
        let path = w.dir.join(format!("{}.data", nodes.seq));
        match from {
            Some(src) => {
                std::fs::copy(src, &path)?;
            }
            None => {
                File::create(&path)?;
            }
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path)?;
        Self::write_meta(&path, parent_id, name);
        let now = Instant::now();
        Ok(Staged {
            seq: nodes.seq,
            path,
            file: Arc::new(file),
            parent_id,
            name: name.to_string(),
            open: 0,
            dirty: false,
            gen: 0,
            closed_at: now,
            uploading: false,
            next_try: now,
            failed: false,
            local_only: self.local_only(name),
        })
    }

    /// Buluttaki bir dosyayi duzenlemeye hazirlar: icerigi indirip bekleyen
    /// kopyaya alir. `truncate` ise indirmeden bos baslar.
    fn stage_existing(&self, ino: u64, truncate: bool) -> Result<(), c_int> {
        let node = {
            let nodes = self.nodes.lock().unwrap();
            if nodes.staged.contains_key(&ino) {
                return Ok(());
            }
            nodes.by_ino.get(&ino).cloned().ok_or(ENOENT)?
        };
        if node.entry.is_dir {
            return Err(EISDIR);
        }
        let keep = !truncate && node.entry.size > 0;
        if keep {
            self.cache.ensure_full(&node.entry).map_err(|e| {
                log::warn!("duzenleme icin indirilemedi: {}: {}", node.entry.name, e);
                EIO
            })?;
        }
        let mut nodes = self.nodes.lock().unwrap();
        if nodes.staged.contains_key(&ino) {
            return Ok(());
        }
        let parent_id = self.parent_dir_id(&nodes, node.parent)?;
        let src = self.cache.path_of(&node.entry);
        let mut st = self
            .new_staged(&mut nodes, parent_id, &node.entry.name, keep.then_some(src.as_path()))
            .map_err(|e| {
                log::warn!("bekleyen kopya olusturulamadi: {}", e);
                EIO
            })?;
        if truncate && node.entry.size > 0 {
            st.dirty = true;
            st.gen = 1;
        }
        nodes.staged.insert(ino, st);
        nodes.counts();
        Ok(())
    }

    /// `parent` klasorunde yeni bir bekleyen dosya olusturur; inode'unu doner.
    fn create_file(&self, parent: u64, name: &str) -> Result<u64, c_int> {
        if self.write.is_none() {
            return Err(EROFS);
        }
        let kids = self.children(parent)?;
        if kids.iter().any(|(_, e)| e.name == name) {
            return Err(EEXIST);
        }
        let mut nodes = self.nodes.lock().unwrap();
        let parent_id = self.parent_dir_id(&nodes, parent)?;
        let mut st = self.new_staged(&mut nodes, parent_id, name, None).map_err(|e| {
            log::warn!("dosya olusturulamadi: {}", e);
            EIO
        })?;
        st.dirty = true; // bos dosya da yuklenir
        st.open = 1;
        let ino = STAGE_BASE + st.seq;
        nodes.staged.insert(ino, st);
        nodes.by_ino.insert(
            ino,
            Node {
                entry: Entry {
                    id: 0,
                    name: name.to_string(),
                    is_dir: false,
                    size: 0,
                    mtime: 0,
                },
                parent,
            },
        );
        nodes.add_child(parent, ino);
        nodes.counts();
        Ok(ino)
    }

    fn make_dir(&self, parent: u64, name: &str) -> Result<(u64, Entry), c_int> {
        if self.write.is_none() {
            return Err(EROFS);
        }
        // Dosya yoneticilerinin cop kutusu ve gecici klasorleri buluta gitmesin.
        if self.local_only(name) {
            return Err(EPERM);
        }
        let kids = self.children(parent)?;
        if kids.iter().any(|(_, e)| e.name == name) {
            return Err(EEXIST);
        }
        let parent_id = self.dir_id(parent)?;
        let id = self.api.create_dir(parent_id, name).map_err(|e| {
            log::warn!("klasor olusturulamadi: {}: {:#}", name, e);
            EIO
        })?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let entry = Entry {
            id,
            name: name.to_string(),
            is_dir: true,
            size: 0,
            mtime: now,
        };
        let ino = INO_BASE + id;
        let mut nodes = self.nodes.lock().unwrap();
        nodes.by_ino.insert(ino, Node { entry: entry.clone(), parent });
        nodes.add_child(parent, ino);
        nodes.children.insert(ino, (Instant::now(), Vec::new()));
        Ok((ino, entry))
    }

    /// Henuz yuklenmemis bekleyen dosya dogrudan silinir; buluttaki dosya
    /// kDrive cop kutusuna tasinir.
    fn unlink(&self, parent: u64, name: &str) -> Result<(), c_int> {
        if self.write.is_none() {
            return Err(EROFS);
        }
        let kids = self.children(parent)?;
        let (ino, entry) = kids.into_iter().find(|(_, e)| e.name == name).ok_or(ENOENT)?;
        if entry.is_dir {
            return Err(EISDIR);
        }
        if ino < STAGE_BASE {
            if self.nodes.lock().unwrap().staged.get(&ino).is_some_and(|s| s.uploading) {
                return Err(libc::EBUSY);
            }
            self.api.trash(entry.id).map_err(|e| {
                log::warn!("cop kutusuna tasinamadi: {}: {:#}", name, e);
                EIO
            })?;
            log::info!("kDrive cop kutusuna tasindi: {}", name);
            let mut nodes = self.nodes.lock().unwrap();
            if let Some(s) = nodes.staged.remove(&ino) {
                Self::remove_files(&s.path);
            }
            nodes.by_ino.remove(&ino);
            nodes.drop_child(parent, ino);
            nodes.counts();
            return Ok(());
        }
        let mut nodes = self.nodes.lock().unwrap();
        let removable = nodes.staged.get(&ino).is_some_and(|s| !s.uploading);
        if !removable {
            return Err(libc::EBUSY);
        }
        if let Some(s) = nodes.staged.remove(&ino) {
            Self::remove_files(&s.path);
        }
        nodes.by_ino.remove(&ino);
        nodes.drop_child(parent, ino);
        nodes.counts();
        Ok(())
    }

    /// Bos bir bulut klasorunu kDrive cop kutusuna tasir.
    fn remove_dir(&self, parent: u64, name: &str) -> Result<(), c_int> {
        if self.write.is_none() {
            return Err(EROFS);
        }
        let kids = self.children(parent)?;
        let (ino, entry) = kids.into_iter().find(|(_, e)| e.name == name).ok_or(ENOENT)?;
        if !entry.is_dir {
            return Err(ENOTDIR);
        }
        // Guncel listeye bak: bos degilse silme.
        self.nodes.lock().unwrap().children.remove(&ino);
        if !self.children(ino)?.is_empty() {
            return Err(libc::ENOTEMPTY);
        }
        self.api.trash(entry.id).map_err(|e| {
            log::warn!("klasor cop kutusuna tasinamadi: {}: {:#}", name, e);
            EIO
        })?;
        log::info!("kDrive cop kutusuna tasindi: {}/", name);
        let mut nodes = self.nodes.lock().unwrap();
        nodes.by_ino.remove(&ino);
        nodes.children.remove(&ino);
        nodes.drop_child(parent, ino);
        Ok(())
    }

    /// Buluttaki bir dosyayi ya da klasoru tasir ve/veya adini degistirir.
    /// Hedefte ayni adda bir dosya varsa o kDrive cop kutusuna gider.
    fn rename_cloud(
        &self,
        parent: u64,
        src: u64,
        new_parent: u64,
        new_name: &str,
        target: Option<(u64, Entry)>,
    ) -> Result<(), c_int> {
        if self.write.is_none() {
            return Err(EROFS);
        }
        let (id, old_name, new_parent_id) = {
            let nodes = self.nodes.lock().unwrap();
            if nodes.staged.get(&src).is_some_and(|s| s.uploading || s.dirty) {
                return Err(libc::EBUSY); // yuklenmeyi bekleyen degisiklik var
            }
            let node = nodes.by_ino.get(&src).ok_or(ENOENT)?;
            (node.entry.id, node.entry.name.clone(), self.parent_dir_id(&nodes, new_parent)?)
        };
        if new_parent == parent && new_name == old_name {
            return Ok(());
        }
        let fail = |what: &str, e: anyhow::Error| {
            log::warn!("{}: {}: {:#}", what, old_name, e);
            EIO
        };
        if let Some((t, e)) = &target {
            if e.is_dir || *t >= STAGE_BASE {
                return Err(libc::EEXIST);
            }
            self.api.trash(e.id).map_err(|err| fail("hedefteki dosya cop kutusuna tasinamadi", err))?;
            let mut nodes = self.nodes.lock().unwrap();
            nodes.by_ino.remove(t);
            nodes.drop_child(new_parent, *t);
        }
        if new_parent != parent {
            // Hedef klasorde eski adla bir sey varsa once adi degistir.
            let clash = new_name != old_name && self.children(new_parent)?.iter().any(|(_, e)| e.name == old_name);
            if clash {
                self.api.rename(id, new_name).map_err(|e| fail("adi degistirilemedi", e))?;
                self.api.move_to(id, new_parent_id).map_err(|e| fail("tasinamadi", e))?;
            } else {
                self.api.move_to(id, new_parent_id).map_err(|e| fail("tasinamadi", e))?;
                if new_name != old_name {
                    self.api.rename(id, new_name).map_err(|e| fail("adi degistirilemedi", e))?;
                }
            }
        } else {
            self.api.rename(id, new_name).map_err(|e| fail("adi degistirilemedi", e))?;
        }
        log::info!("bulutta tasindi: {} -> {}", old_name, new_name);
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(n) = nodes.by_ino.get_mut(&src) {
            n.entry.name = new_name.to_string();
            n.parent = new_parent;
        }
        nodes.drop_child(parent, src);
        nodes.add_child(new_parent, src);
        Ok(())
    }

    /// Henuz yuklenmemis bekleyen dosyalar yerelde, buluttaki ogeler bulutta
    /// tasinir. Bekleyen dosyanin hedefinde buluttaki bir dosya varsa,
    /// bekleyen icerik onun yeni surumu olur.
    fn rename(&self, parent: u64, name: &str, new_parent: u64, new_name: &str) -> Result<(), c_int> {
        let (src, _) = self
            .children(parent)?
            .into_iter()
            .find(|(_, e)| e.name == name)
            .ok_or(ENOENT)?;
        let target = self.children(new_parent)?.into_iter().find(|(_, e)| e.name == new_name);
        if src < STAGE_BASE {
            return self.rename_cloud(parent, src, new_parent, new_name, target);
        }
        let mut nodes = self.nodes.lock().unwrap();
        let movable = nodes.staged.get(&src).is_some_and(|s| !s.uploading);
        if !movable {
            return Err(EPERM);
        }
        let new_parent_id = self.parent_dir_id(&nodes, new_parent)?;
        match target {
            Some((t, e)) if e.is_dir || t == src => {
                let _ = e;
                return Err(EPERM);
            }
            // Hedef de bekleyen yeni dosya: onu at, yerine gec.
            Some((t, _)) if t >= STAGE_BASE => {
                if nodes.staged.get(&t).is_some_and(|s| s.uploading) {
                    return Err(EPERM);
                }
                if let Some(old) = nodes.staged.remove(&t) {
                    Self::remove_files(&old.path);
                }
                nodes.by_ino.remove(&t);
                nodes.drop_child(new_parent, t);
            }
            // Hedef buluttaki dosya: icerik onun yeni surumu olur.
            Some((t, _)) => {
                let mut s = nodes.staged.remove(&src).ok_or(ENOENT)?;
                if let Some(old) = nodes.staged.remove(&t) {
                    Self::remove_files(&old.path);
                }
                s.parent_id = new_parent_id;
                s.name = new_name.to_string();
                s.local_only = false;
                s.dirty = true;
                s.gen += 1;
                s.closed_at = Instant::now();
                Self::write_meta(&s.path, s.parent_id, &s.name);
                nodes.staged.insert(t, s);
                nodes.by_ino.remove(&src);
                nodes.drop_child(parent, src);
                nodes.alias.insert(src, t);
                nodes.counts();
                self.wake.notify_all();
                return Ok(());
            }
            None => {}
        }
        let local_only = self.local_only(new_name);
        if let Some(s) = nodes.staged.get_mut(&src) {
            s.parent_id = new_parent_id;
            s.name = new_name.to_string();
            s.local_only = local_only;
            s.closed_at = Instant::now();
            Self::write_meta(&s.path, s.parent_id, &s.name);
        }
        if let Some(n) = nodes.by_ino.get_mut(&src) {
            n.entry.name = new_name.to_string();
            n.parent = new_parent;
        }
        nodes.drop_child(parent, src);
        nodes.add_child(new_parent, src);
        nodes.counts();
        self.wake.notify_all();
        Ok(())
    }

    /// Bekleyen bir dosyayi buluta yukler ve dugumunu gercek kimligine cevirir.
    fn upload_one(&self, ino: u64) {
        let (path, parent_id, name, gen) = {
            let mut nodes = self.nodes.lock().unwrap();
            let Some(s) = nodes.staged.get_mut(&ino) else { return };
            s.uploading = true;
            (s.path.clone(), s.parent_id, s.name.clone(), s.gen)
        };
        let res = std::fs::metadata(&path).map_err(anyhow::Error::from).and_then(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let up = self.api.upload_file(parent_id, &name, &path, m.len(), mtime)?;
            Ok((up, m.len(), mtime))
        });
        let mut nodes = self.nodes.lock().unwrap();
        let Some(s) = nodes.staged.get_mut(&ino) else {
            return; // bu arada silindi
        };
        s.uploading = false;
        let (up, size, mtime) = match res {
            Ok(v) => v,
            Err(e) => {
                log::warn!("yuklenemedi (yeniden denenecek): {}: {:#}", name, e);
                s.failed = true;
                s.next_try = Instant::now() + UPLOAD_RETRY;
                nodes.counts();
                return;
            }
        };
        log::info!("yuklendi: {} ({} bayt)", name, size);
        s.failed = false;
        if s.gen != gen || s.open > 0 {
            // Yukleme sirasinda degisti ya da hala acik: bekleyen olarak kalsin.
            s.dirty = s.gen != gen;
            nodes.counts();
            return;
        }
        let s = nodes.staged.remove(&ino).unwrap();
        let parent = nodes.by_ino.get(&ino).map(|n| n.parent).unwrap_or(ROOT_INO);
        let entry = Entry {
            id: up.id,
            name: name.clone(),
            is_dir: false,
            size,
            mtime,
        };
        // Icerigi onbellege devret: yeniden indirmeye gerek kalmasin.
        if self.cache.adopt(&entry, &s.path).is_err() {
            let _ = std::fs::remove_file(&s.path);
        }
        let _ = std::fs::remove_file(s.path.with_extension("json"));
        let new_ino = INO_BASE + up.id;
        if new_ino != ino {
            nodes.by_ino.remove(&ino);
            nodes.drop_child(parent, ino);
            nodes.alias.insert(ino, new_ino);
        }
        nodes.by_ino.insert(new_ino, Node { entry, parent });
        nodes.add_child(parent, new_ino);
        nodes.counts();
    }

    /// Yukleyici dongusu: kapanmis ve bekleme suresi dolmus dosyalari yukler.
    pub fn spawn_uploader(self: &Arc<Self>) {
        let Some(w) = &self.write else { return };
        // Onceki calismadan kalan bekleyenleri yukle.
        let leftovers: Vec<PathBuf> = std::fs::read_dir(&w.dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        let me = self.clone();
        std::thread::spawn(move || {
            for meta in leftovers {
                me.recover(&meta);
            }
            loop {
                let due: Vec<u64> = {
                    let mut nodes = me.nodes.lock().unwrap();
                    loop {
                        let now = Instant::now();
                        let waiting = nodes.staged.values().any(|s| s.dirty && !s.local_only && !s.uploading);
                        let due: Vec<u64> = nodes
                            .staged
                            .iter()
                            .filter(|(_, s)| {
                                s.dirty && !s.local_only && !s.uploading && s.open == 0
                                    && now >= s.closed_at + UPLOAD_DELAY && now >= s.next_try
                            })
                            .map(|(ino, _)| *ino)
                            .collect();
                        if !due.is_empty() {
                            break due;
                        }
                        nodes = if waiting {
                            me.wake.wait_timeout(nodes, Duration::from_secs(1)).unwrap().0
                        } else {
                            me.wake.wait(nodes).unwrap()
                        };
                    }
                };
                for ino in due {
                    me.upload_one(ino);
                }
            }
        });
    }

    /// Onceki calismadan kalan bir bekleyen dosyayi yukler.
    fn recover(&self, meta: &Path) {
        let data = meta.with_extension("data");
        let info: Option<(u64, String)> = std::fs::read_to_string(meta)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| Some((v.get("parent_id")?.as_u64()?, v.get("name")?.as_str()?.to_string())));
        let (Some((parent_id, name)), Ok(m)) = (info, std::fs::metadata(&data)) else {
            Self::remove_files(&data);
            return;
        };
        if self.local_only(&name) {
            Self::remove_files(&data);
            return;
        }
        let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        match self.api.upload_file(parent_id, &name, &data, m.len(), mtime) {
            Ok(_) => {
                log::info!("onceki oturumdan kalan dosya yuklendi: {}", name);
                Self::remove_files(&data);
            }
            Err(e) => log::warn!("onceki oturumdan kalan dosya yuklenemedi (sonra denenecek): {}: {:#}", name, e),
        }
    }

    /// Kapanirken: degismis bekleyen dosyalari hemen yukler. Yuklenemeyenler
    /// diskte kalir ve bir sonraki baslangicta yeniden denenir.
    pub fn flush_pending(&self) {
        let pending: Vec<(PathBuf, u64, String, bool)> = {
            let nodes = self.nodes.lock().unwrap();
            nodes
                .staged
                .values()
                .map(|s| (s.path.clone(), s.parent_id, s.name.clone(), s.dirty && !s.local_only))
                .collect()
        };
        for (path, parent_id, name, upload) in pending {
            if !upload {
                Self::remove_files(&path);
                continue;
            }
            let Ok(m) = std::fs::metadata(&path) else { continue };
            let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            match self.api.upload_file(parent_id, &name, &path, m.len(), mtime) {
                Ok(_) => {
                    log::info!("kapanirken yuklendi: {}", name);
                    Self::remove_files(&path);
                }
                Err(e) => log::warn!("kapanirken yuklenemedi (sonraki baslangicta denenecek): {}: {:#}", name, e),
            }
        }
    }

    fn dir_id(&self, ino: u64) -> Result<u64, c_int> {
        if ino == ROOT_INO {
            return Ok(self.root_id);
        }
        match self.nodes.lock().unwrap().by_ino.get(&ino) {
            Some(n) if n.entry.is_dir => Ok(n.entry.id),
            Some(_) => Err(ENOTDIR),
            None => Err(ENOENT),
        }
    }

    fn node(&self, ino: u64) -> Option<Node> {
        self.nodes.lock().unwrap().by_ino.get(&ino).cloned()
    }

    /// Bir klasorun cocuklarini (inode, giris) olarak doner; liste tazeyse
    /// aga gitmez.
    pub fn children(&self, ino: u64) -> Result<Vec<(u64, Entry)>, c_int> {
        let dir_id = self.dir_id(ino)?;
        {
            let nodes = self.nodes.lock().unwrap();
            if let Some((at, kids)) = nodes.children.get(&ino) {
                if at.elapsed() < self.dir_ttl {
                    return Ok(kids
                        .iter()
                        .filter_map(|k| nodes.by_ino.get(k).map(|n| (*k, n.entry.clone())))
                        .collect());
                }
            }
        }
        let listed = match self.api.list_dir(dir_id) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("{:#}", e);
                // Ag hatasinda eldeki eski liste varsa onu kullan.
                let nodes = self.nodes.lock().unwrap();
                return match nodes.children.get(&ino) {
                    Some((_, kids)) => Ok(kids
                        .iter()
                        .filter_map(|k| nodes.by_ino.get(k).map(|n| (*k, n.entry.clone())))
                        .collect()),
                    None => Err(EIO),
                };
            }
        };
        let mut nodes = self.nodes.lock().unwrap();
        let mut kids = Vec::with_capacity(listed.len());
        let mut out = Vec::with_capacity(listed.len());
        let mut seen: HashMap<String, u32> = HashMap::new();
        for mut e in listed {
            // Ayni klasorde ayni ada sahip iki oge olursa ikincisini ayirt et.
            let mut name = safe_name(&e.name);
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                name = format!("{} ({})", name, e.id);
            }
            e.name = name;
            let child_ino = INO_BASE + e.id;
            nodes.by_ino.insert(
                child_ino,
                Node {
                    entry: e.clone(),
                    parent: ino,
                },
            );
            kids.push(child_ino);
            out.push((child_ino, e));
        }
        // Henuz yuklenmemis yeni dosyalar sunucu listesinde yok: ekle.
        let pending: Vec<u64> = nodes
            .by_ino
            .iter()
            .filter(|(k, n)| **k >= STAGE_BASE && n.parent == ino)
            .map(|(k, _)| *k)
            .collect();
        for k in pending {
            if let Some(n) = nodes.by_ino.get(&k) {
                if out.iter().any(|(_, e): &(u64, Entry)| e.name == n.entry.name) {
                    continue;
                }
                out.push((k, n.entry.clone()));
                kids.push(k);
            }
        }
        nodes.children.insert(ino, (Instant::now(), kids));
        Ok(out)
    }

    /// Koke gore yolu (ornegin "03 Fotograflar/2024") cozer.
    pub fn resolve(&self, path: &str) -> Result<(u64, Option<Entry>), c_int> {
        let mut ino = ROOT_INO;
        let mut entry = None;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let kids = self.children(ino)?;
            let (k, e) = kids.into_iter().find(|(_, e)| e.name == part).ok_or(ENOENT)?;
            ino = k;
            entry = Some(e);
        }
        Ok((ino, entry))
    }

    /// Bir inode'un ozniteliklerini ve cekirdekte ne kadar onbelleklenecegini doner.
    fn attr_of(&self, ino: u64) -> Option<(Duration, FileAttr)> {
        if ino == ROOT_INO {
            return Some((KERNEL_TTL, self.attr(ino, None)));
        }
        let nodes = self.nodes.lock().unwrap();
        let ino = nodes.real(ino);
        let node = nodes.by_ino.get(&ino)?;
        match nodes.staged.get(&ino).and_then(|s| s.file.metadata().ok()) {
            Some(m) => {
                let mut e = node.entry.clone();
                e.size = m.len();
                e.mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                Some((STAGED_TTL, self.attr(ino, Some(&e))))
            }
            None => Some((KERNEL_TTL, self.attr(ino, Some(&node.entry)))),
        }
    }

    fn attr(&self, ino: u64, e: Option<&Entry>) -> FileAttr {
        let (fperm, dperm) = if self.write.is_some() { (0o644, 0o755) } else { (0o444, 0o555) };
        let (kind, size, mtime, perm) = match e {
            Some(e) if !e.is_dir => (FileType::RegularFile, e.size, e.mtime, fperm),
            Some(e) => (FileType::Directory, 0, e.mtime, dperm),
            None => (FileType::Directory, 0, 0, dperm),
        };
        let t = if mtime > 0 {
            UNIX_EPOCH + Duration::from_secs(mtime as u64)
        } else {
            SystemTime::now()
        };
        FileAttr {
            ino,
            size,
            blocks: size.div_ceil(512),
            atime: t,
            mtime: t,
            ctime: t,
            crtime: t,
            kind,
            perm,
            nlink: if kind == FileType::Directory { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 128 * 1024,
            flags: 0,
        }
    }
}

pub struct KdFs {
    pub inner: Arc<Inner>,
}

// Aga gidebilecek her islem ayri bir is parcaciginda calisir; boylece yavas
// bir indirme dosya sisteminin geri kalanini kilitlemez.
impl Filesystem for KdFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let inner = self.inner.clone();
        let name = name.to_string_lossy().into_owned();
        std::thread::spawn(move || match inner.children(parent) {
            Ok(kids) => match kids.into_iter().find(|(_, e)| e.name == name) {
                Some((ino, _)) => match inner.attr_of(ino) {
                    Some((ttl, attr)) => reply.entry(&ttl, &attr, 0),
                    None => reply.error(ENOENT),
                },
                None => reply.error(ENOENT),
            },
            Err(e) => reply.error(e),
        });
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.inner.attr_of(ino) {
            Some((ttl, attr)) => reply.attr(&ttl, &attr),
            None => reply.error(ENOENT),
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            let kids = match inner.children(ino) {
                Ok(k) => k,
                Err(e) => return reply.error(e),
            };
            let parent = inner.node(ino).map(|n| n.parent).unwrap_or(ROOT_INO);
            let mut all: Vec<(u64, FileType, String)> = vec![
                (ino, FileType::Directory, ".".into()),
                (parent, FileType::Directory, "..".into()),
            ];
            for (k, e) in kids {
                let kind = if e.is_dir {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                };
                all.push((k, kind, e.name));
            }
            for (i, (k, kind, name)) in all.into_iter().enumerate().skip(offset as usize) {
                if reply.add(k, (i + 1) as i64, kind, name) {
                    break;
                }
            }
            reply.ok();
        });
    }

    // Dosya tutamaclari: 0 = onbellekten okuma, 1 = bekleyen (yerel) kopya.
    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        let inner = self.inner.clone();
        let writing = flags & libc::O_ACCMODE != libc::O_RDONLY;
        if writing && !inner.writable() {
            return reply.error(EROFS);
        }
        let ino = inner.nodes.lock().unwrap().real(ino);
        let Some(n) = inner.node(ino) else {
            return reply.error(ENOENT);
        };
        if n.entry.is_dir {
            return reply.error(EISDIR);
        }
        let take_staged = move |inner: &Inner| -> bool {
            let mut nodes = inner.nodes.lock().unwrap();
            match nodes.staged.get_mut(&ino) {
                Some(s) => {
                    s.open += 1;
                    true
                }
                None => false,
            }
        };
        if take_staged(&inner) {
            return reply.opened(1, 0);
        }
        if !writing {
            inner.cache.opened(&n.entry);
            return reply.opened(0, 0);
        }
        // Buluttaki dosya yazmak icin aciliyor: indirip bekleyen kopyaya al.
        std::thread::spawn(move || match inner.stage_existing(ino, flags & libc::O_TRUNC != 0) {
            Ok(()) if take_staged(&inner) => reply.opened(1, 0),
            Ok(()) => reply.error(EIO),
            Err(e) => reply.error(e),
        });
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let inner = self.inner.clone();
        let name = name.to_string_lossy().into_owned();
        std::thread::spawn(move || match inner.create_file(parent, &name) {
            Ok(ino) => match inner.attr_of(ino) {
                Some((ttl, attr)) => reply.created(&ttl, &attr, 0, 1, 0),
                None => reply.error(EIO),
            },
            Err(e) => reply.error(e),
        });
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let offset = offset.max(0) as u64;
        let (n, staged) = {
            let nodes = self.inner.nodes.lock().unwrap();
            let ino = nodes.real(ino);
            (nodes.by_ino.get(&ino).cloned(), nodes.staged.get(&ino).map(|s| s.file.clone()))
        };
        if let Some(file) = staged {
            let mut buf = vec![0u8; size as usize];
            let mut got = 0;
            while got < buf.len() {
                match file.read_at(&mut buf[got..], offset + got as u64) {
                    Ok(0) => break,
                    Ok(k) => got += k,
                    Err(_) => return reply.error(EIO),
                }
            }
            return reply.data(&buf[..got]);
        }
        let Some(n) = n else {
            return reply.error(ENOENT);
        };
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            match inner.cache.read(&n.entry, offset, size) {
                Ok(data) => reply.data(&data),
                Err(e) => {
                    log::warn!("okuma hatasi: {}: {}", n.entry.name, e);
                    reply.error(EIO)
                }
            }
        });
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let file = {
            let mut nodes = self.inner.nodes.lock().unwrap();
            let ino = nodes.real(ino);
            let Some(s) = nodes.staged.get_mut(&ino) else {
                return reply.error(EROFS);
            };
            s.dirty = true;
            s.gen += 1;
            let f = s.file.clone();
            nodes.counts();
            f
        };
        match file.write_all_at(data, offset.max(0) as u64) {
            Ok(()) => reply.written(data.len() as u32),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            let ino = inner.nodes.lock().unwrap().real(ino);
            if let Some(size) = size {
                if !inner.writable() {
                    return reply.error(EROFS);
                }
                if let Err(e) = inner.stage_existing(ino, size == 0) {
                    return reply.error(e);
                }
                let mut nodes = inner.nodes.lock().unwrap();
                if let Some(s) = nodes.staged.get_mut(&ino) {
                    if s.file.set_len(size).is_err() {
                        return reply.error(EIO);
                    }
                    s.dirty = true;
                    s.gen += 1;
                    if s.open == 0 {
                        s.closed_at = Instant::now();
                    }
                }
                nodes.counts();
                inner.wake.notify_all();
            }
            if let Some(m) = mtime {
                // Yalnizca bekleyen dosyada anlamli (ornegin kopyalarken zamani koruma).
                let t = match m {
                    TimeOrNow::SpecificTime(t) => t,
                    TimeOrNow::Now => SystemTime::now(),
                };
                let mut nodes = inner.nodes.lock().unwrap();
                if let Some(s) = nodes.staged.get_mut(&ino) {
                    let _ = s.file.set_modified(t);
                    s.gen += 1;
                    if s.open == 0 {
                        s.closed_at = Instant::now();
                    }
                }
            }
            // Izin ve sahiplik degisiklikleri sessizce yok sayilir.
            match inner.attr_of(ino) {
                Some((ttl, attr)) => reply.attr(&ttl, &attr),
                None => reply.error(ENOENT),
            }
        });
    }

    fn mkdir(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let inner = self.inner.clone();
        let name = name.to_string_lossy().into_owned();
        std::thread::spawn(move || match inner.make_dir(parent, &name) {
            Ok((ino, e)) => reply.entry(&KERNEL_TTL, &inner.attr(ino, Some(&e)), 0),
            Err(e) => reply.error(e),
        });
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let inner = self.inner.clone();
        let name = name.to_string_lossy().into_owned();
        std::thread::spawn(move || match inner.unlink(parent, &name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        });
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let inner = self.inner.clone();
        let name = name.to_string_lossy().into_owned();
        std::thread::spawn(move || match inner.remove_dir(parent, &name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        });
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let inner = self.inner.clone();
        let (name, newname) = (name.to_string_lossy().into_owned(), newname.to_string_lossy().into_owned());
        std::thread::spawn(move || match inner.rename(parent, &name, newparent, &newname) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        });
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        reply.ok();
    }

    fn fsync(&mut self, _req: &Request, _ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if fh == 1 {
            let mut nodes = self.inner.nodes.lock().unwrap();
            let ino = nodes.real(ino);
            if let Some(s) = nodes.staged.get_mut(&ino) {
                s.open = s.open.saturating_sub(1);
                if s.open == 0 {
                    s.closed_at = Instant::now();
                    if !s.dirty && ino < STAGE_BASE {
                        // Yazmak icin acildi ama degismedi: bekleyen kopyayi birak.
                        if let Some(s) = nodes.staged.remove(&ino) {
                            Inner::remove_files(&s.path);
                        }
                    }
                }
            }
            nodes.counts();
            drop(nodes);
            self.inner.wake.notify_all();
        } else if let Some(n) = self.inner.node(ino) {
            self.inner.cache.closed(n.entry.id);
        }
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        // Dosya yoneticileri kopyalamadan once bos alana bakar; yerel
        // bekleme alaninin bos yerini bildir.
        let free = self.inner.cache.free_space() / 4096;
        reply.statfs(free, free, free, 0, 0, 4096, 255, 4096);
    }
}
