//! Yerel dosya onbellegi. Bir dosya ilk okundugunda arka planda bastan sona
//! indirilir; okumalar ihtiyac duyduklari kisim gelene kadar bekler. Tamamlanan
//! dosyalar onbellekte kalir, ust sinir asilinca en eski kullanilanlar silinir.

use crate::api::{Api, Entry};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// Okuma, inen kismin bu kadar otesindeyse (ornegin videoda ileri sarma)
/// beklemek yerine dogrudan aralik istegi denenir.
const SEEK_AHEAD: u64 = 16 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;

struct DlState {
    have: u64,
    done: bool,
    err: Option<String>,
}

struct Dl {
    state: Mutex<DlState>,
    cv: Condvar,
}

pub struct Cache {
    dir: PathBuf,
    max_bytes: u64,
    api: Arc<Api>,
    active: Mutex<HashMap<String, Arc<Dl>>>,
    pinned: Mutex<HashSet<u64>>,
    /// Su an acik olan dosyalar (kimlik -> acik tutamac sayisi); tahliye edilmez.
    open: Mutex<HashMap<u64, u32>>,
    /// Sunucu aralik isteklerini desteklemiyorsa tekrar denememek icin.
    range_unsupported: Mutex<bool>,
}

fn key(e: &Entry) -> String {
    format!("{}_{}_{}", e.id, e.mtime, e.size)
}

fn id_of(name: &str) -> Option<u64> {
    name.split('_').next()?.parse().ok()
}

fn eio(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

impl Cache {
    pub fn new(dir: PathBuf, max_bytes: u64, api: Arc<Api>) -> io::Result<Cache> {
        fs::create_dir_all(&dir)?;
        // Onceki calismadan yarim kalan indirmeleri temizle.
        for ent in fs::read_dir(&dir)? {
            let ent = ent?;
            if ent.file_name().to_string_lossy().ends_with(".part") {
                let _ = fs::remove_file(ent.path());
            }
        }
        Ok(Cache {
            dir,
            max_bytes,
            api,
            active: Mutex::new(HashMap::new()),
            pinned: Mutex::new(HashSet::new()),
            open: Mutex::new(HashMap::new()),
            range_unsupported: Mutex::new(false),
        })
    }

    fn final_path(&self, e: &Entry) -> PathBuf {
        self.dir.join(key(e))
    }

    fn part_path(&self, e: &Entry) -> PathBuf {
        self.dir.join(format!("{}.part", key(e)))
    }

    /// Tamamlanmis dosyanin onbellekteki yolu.
    pub fn path_of(&self, e: &Entry) -> PathBuf {
        self.final_path(e)
    }

    /// Yerelde hazir olan icerigi (az once yuklenen dosya) onbellege devralir.
    pub fn adopt(&self, e: &Entry, from: &Path) -> io::Result<()> {
        fs::rename(from, self.final_path(e))
    }

    pub fn is_cached(&self, e: &Entry) -> bool {
        self.final_path(e).is_file()
    }

    pub fn set_pinned(&self, ids: HashSet<u64>) {
        *self.pinned.lock().unwrap() = ids;
    }

    /// Dosya acildiginda cagrilir: en son kullanim zamanini gunceller ve
    /// dosyayi kapanana kadar tahliyeden korur.
    pub fn opened(&self, e: &Entry) {
        *self.open.lock().unwrap().entry(e.id).or_insert(0) += 1;
        if let Ok(f) = File::open(self.final_path(e)) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    pub fn closed(&self, id: u64) {
        let mut open = self.open.lock().unwrap();
        if let Some(n) = open.get_mut(&id) {
            *n -= 1;
            if *n == 0 {
                open.remove(&id);
            }
        }
        drop(open);
        self.evict();
    }

    fn start(self: &Arc<Self>, e: &Entry) -> Arc<Dl> {
        let k = key(e);
        let mut active = self.active.lock().unwrap();
        if let Some(dl) = active.get(&k) {
            return dl.clone();
        }
        let dl = Arc::new(Dl {
            state: Mutex::new(DlState {
                have: 0,
                done: false,
                err: None,
            }),
            cv: Condvar::new(),
        });
        active.insert(k.clone(), dl.clone());
        drop(active);

        let me = self.clone();
        let e = e.clone();
        let dl2 = dl.clone();
        std::thread::spawn(move || {
            let res = me.run_download(&e, &dl2);
            {
                let mut st = dl2.state.lock().unwrap();
                match res {
                    Ok(()) => st.done = true,
                    Err(err) => {
                        log::warn!("indirme basarisiz: {} ({}): {}", e.name, e.id, err);
                        st.err = Some(err.to_string());
                        let _ = fs::remove_file(me.part_path(&e));
                    }
                }
            }
            me.active.lock().unwrap().remove(&k);
            dl2.cv.notify_all();
            me.evict();
        });
        dl
    }

    fn run_download(&self, e: &Entry, dl: &Dl) -> io::Result<()> {
        let part = self.part_path(e);
        let mut out = File::create(&part)?;
        let mut resp = self.api.download(e.id).map_err(|x| eio(x.to_string()))?;
        let mut buf = vec![0u8; CHUNK];
        let mut total: u64 = 0;
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            total += n as u64;
            dl.state.lock().unwrap().have = total;
            dl.cv.notify_all();
        }
        out.flush()?;
        if total != e.size {
            return Err(eio(format!(
                "boyut uyusmuyor: beklenen {}, gelen {}",
                e.size, total
            )));
        }
        drop(out);
        fs::rename(&part, self.final_path(e))?;
        log::info!("indirildi: {} ({} bayt)", e.name, total);
        Ok(())
    }

    fn pread(path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let f = File::open(path)?;
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let n = f.read_at(&mut buf[got..], offset + got as u64)?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        Ok(buf)
    }

    /// Dosyanin [offset, offset+size) araligini doner; gerekiyorsa indirir.
    pub fn read(self: &Arc<Self>, e: &Entry, offset: u64, size: u32) -> io::Result<Vec<u8>> {
        if offset >= e.size || size == 0 {
            return Ok(Vec::new());
        }
        let end = (offset + size as u64).min(e.size);
        let len = (end - offset) as usize;
        let fin = self.final_path(e);
        if fin.is_file() {
            return Self::pread(&fin, offset, len);
        }

        let dl = self.start(e);
        let have = dl.state.lock().unwrap().have;
        if offset > have + SEEK_AHEAD && !*self.range_unsupported.lock().unwrap() {
            match self.api.download_range(e.id, offset, end) {
                Ok(Some(bytes)) => return Ok(bytes),
                Ok(None) => {
                    log::info!("sunucu aralik istegini desteklemiyor; sirali indirme beklenecek");
                    *self.range_unsupported.lock().unwrap() = true;
                }
                Err(err) => log::debug!("aralik istegi basarisiz: {}", err),
            }
        }

        let mut st = dl.state.lock().unwrap();
        while st.have < end && !st.done && st.err.is_none() {
            st = dl.cv.wait(st).unwrap();
        }
        if let Some(err) = &st.err {
            return Err(eio(err.clone()));
        }
        let done = st.done;
        drop(st);
        if done {
            return Self::pread(&fin, offset, len);
        }
        // Indirme suruyor: yarim dosyadan oku. Bu arada bitip yeniden
        // adlandirildiysa tamamlanmis dosyadan oku.
        match Self::pread(&self.part_path(e), offset, len) {
            Ok(b) if b.len() == len => Ok(b),
            _ => Self::pread(&fin, offset, len),
        }
    }

    /// Dosyanin tamamini onbellege indirir ve bitmesini bekler (sabitleme icin).
    pub fn ensure_full(self: &Arc<Self>, e: &Entry) -> io::Result<()> {
        if self.is_cached(e) {
            return Ok(());
        }
        if e.size == 0 {
            File::create(self.final_path(e))?;
            return Ok(());
        }
        let dl = self.start(e);
        let mut st = dl.state.lock().unwrap();
        while !st.done && st.err.is_none() {
            st = dl.cv.wait(st).unwrap();
        }
        match &st.err {
            Some(err) => Err(eio(err.clone())),
            None => Ok(()),
        }
    }

    /// (dosya sayisi, toplam bayt, sabitlenmis bayt)
    pub fn usage(&self) -> (usize, u64, u64) {
        let pinned = self.pinned.lock().unwrap();
        let (mut n, mut total, mut pin) = (0, 0, 0);
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.ends_with(".part") {
                    continue;
                }
                if let Ok(m) = ent.metadata() {
                    n += 1;
                    total += m.len();
                    if id_of(&name).is_some_and(|id| pinned.contains(&id)) {
                        pin += m.len();
                    }
                }
            }
        }
        (n, total, pin)
    }

    /// Verilen dosyalarin indirilmis kopyalarini siler (acik olanlar haric).
    /// (silinen dosya sayisi, bayt) doner.
    pub fn release(&self, ids: &HashSet<u64>) -> (usize, u64) {
        self.pinned.lock().unwrap().retain(|i| !ids.contains(i));
        let open: HashSet<u64> = self.open.lock().unwrap().keys().copied().collect();
        let (mut n, mut bytes) = (0, 0);
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return (0, 0);
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            let Some(id) = id_of(&name) else { continue };
            if name.ends_with(".part") || !ids.contains(&id) || open.contains(&id) {
                continue;
            }
            let len = ent.metadata().map(|m| m.len()).unwrap_or(0);
            if fs::remove_file(ent.path()).is_ok() {
                n += 1;
                bytes += len;
            }
        }
        (n, bytes)
    }

    /// Ust sinir asildiysa, sabitlenmemis dosyalari en eski kullanilandan
    /// baslayarak siler.
    pub fn evict(&self) {
        let pinned = self.pinned.lock().unwrap().clone();
        let open: HashSet<u64> = self.open.lock().unwrap().keys().copied().collect();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total: u64 = 0;
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return;
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            if name.ends_with(".part") {
                continue;
            }
            let Ok(m) = ent.metadata() else { continue };
            let id = id_of(&name);
            if id.is_some_and(|id| pinned.contains(&id)) {
                continue;
            }
            total += m.len();
            // Acik dosyalar sinira sayilir ama silinmez.
            if id.is_some_and(|id| open.contains(&id)) {
                continue;
            }
            files.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), ent.path()));
        }
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                log::info!("onbellekten cikarildi: {}", path.display());
                total -= len;
            }
        }
    }

    pub fn free_space(&self) -> u64 {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(self.dir.as_os_str().as_bytes()) else {
            return u64::MAX;
        };
        unsafe {
            let mut s: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut s) == 0 {
                s.f_bavail as u64 * s.f_frsize as u64
            } else {
                u64::MAX
            }
        }
    }
}
