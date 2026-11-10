//! Dosya degisikligi izleyicisi (inotify). Yedeklenen klasorlerde bir dosya
//! olusturulunca ya da degisince ilgili yedegin adini bildirir; boylece
//! duzenli tarama yerine degisiklik aninda yukleme yapilir.
//!
//! Haric tutulan klasorler (.git, node_modules...) hic izlenmez. Yeni
//! olusturulan alt klasorler kendiliginden izlemeye alinir.

use crate::backup::{self, Source};
use notify::event::{AccessKind, AccessMode, EventKind};
use notify::{RecursiveMode, Watcher as _};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

enum Msg {
    Fs(notify::Result<notify::Event>),
    Resync,
    Exclude(Vec<String>),
}

#[derive(Clone)]
pub struct Handle(mpsc::Sender<Msg>);

impl Handle {
    /// Yedek listesi degisti: izlenen klasorleri yeniden kur.
    pub fn resync(&self) {
        let _ = self.0.send(Msg::Resync);
    }

    /// Haric tutma listesi degisti.
    pub fn set_exclude(&self, list: Vec<String>) {
        let _ = self.0.send(Msg::Exclude(list));
        self.resync();
    }
}

fn skip_dir(exclude: &[String], path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    exclude.iter().any(|p| backup::matches(p, &name)) || path.join("CACHEDIR.TAG").exists()
}

struct State {
    watcher: notify::RecommendedWatcher,
    exclude: Vec<String>,
    /// Izlenen kok klasor -> yedek adi.
    roots: HashMap<PathBuf, String>,
    /// Izlenen her klasor -> ait oldugu kok.
    dirs: HashMap<PathBuf, PathBuf>,
    failed: bool,
}

impl State {
    /// `dir` ve altindaki (haric tutulmayan) klasorleri izlemeye alir.
    fn watch_tree(&mut self, root: &Path, dir: &Path) {
        if self.dirs.contains_key(dir) {
            return;
        }
        if let Err(e) = self.watcher.watch(dir, RecursiveMode::NonRecursive) {
            if !self.failed {
                log::warn!("izlenemedi: {}: {} (duzenli taramaya guvenilecek)", dir.display(), e);
            }
            self.failed = true;
            return;
        }
        self.dirs.insert(dir.to_path_buf(), root.to_path_buf());
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for ent in rd.flatten() {
            let p = ent.path();
            let is_dir = p.symlink_metadata().is_ok_and(|m| m.is_dir());
            if is_dir && !skip_dir(&self.exclude, &p) {
                self.watch_tree(root, &p);
            }
        }
    }

    fn resync(&mut self, sources: &[Source]) {
        let want: HashMap<PathBuf, String> =
            sources.iter().map(|s| (PathBuf::from(&s.local), s.name.clone())).collect();
        let gone: HashSet<PathBuf> = self.roots.keys().filter(|r| !want.contains_key(*r)).cloned().collect();
        if !gone.is_empty() {
            let drop: Vec<PathBuf> =
                self.dirs.iter().filter(|(_, r)| gone.contains(*r)).map(|(d, _)| d.clone()).collect();
            for d in drop {
                let _ = self.watcher.unwatch(&d);
                self.dirs.remove(&d);
            }
        }
        self.failed = false;
        for root in want.keys() {
            // Var olan koklerde de yeniden dolas: kacmis yeni klasorler eklenir.
            let known: Vec<PathBuf> = vec![root.clone()];
            for d in known {
                if !self.dirs.contains_key(&d) {
                    self.watch_tree(root, &d);
                } else {
                    self.rescan_children(root, &d);
                }
            }
        }
        self.roots = want;
        log::info!("{} klasor izleniyor ({} yedek)", self.dirs.len(), self.roots.len());
    }

    /// Zaten izlenen bir agacta, izlenmeyen alt klasorleri ekler.
    fn rescan_children(&mut self, root: &Path, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.symlink_metadata().is_ok_and(|m| m.is_dir()) || skip_dir(&self.exclude, &p) {
                continue;
            }
            if self.dirs.contains_key(&p) {
                self.rescan_children(root, &p);
            } else {
                self.watch_tree(root, &p);
            }
        }
    }

    /// Olayin ait oldugu yedegin adini doner (ilgilenilmeyen olaylarda None).
    fn handle(&mut self, ev: notify::Event) -> Option<String> {
        let relevant = matches!(
            ev.kind,
            EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Remove(_)
                | EventKind::Access(AccessKind::Close(AccessMode::Write))
        );
        if !relevant {
            return None;
        }
        let mut hit = None;
        for path in &ev.paths {
            let Some(parent) = path.parent() else { continue };
            let Some(root) = self.dirs.get(parent).cloned() else { continue };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if self.exclude.iter().any(|p| backup::matches(p, &name)) {
                continue;
            }
            if path.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                if skip_dir(&self.exclude, path) {
                    continue;
                }
                self.watch_tree(&root, path);
            }
            hit = self.roots.get(&root).cloned().or(hit);
        }
        hit
    }
}

/// Izleyiciyi baslatir. Degisiklik olan yedegin adi `on_change` ile bildirilir.
pub fn spawn(
    exclude: Vec<String>,
    on_change: impl Fn(String) + Send + 'static,
) -> notify::Result<Handle> {
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    let watcher = notify::recommended_watcher(move |res| {
        let _ = tx2.send(Msg::Fs(res));
    })?;
    let mut st = State {
        watcher,
        exclude,
        roots: HashMap::new(),
        dirs: HashMap::new(),
        failed: false,
    };
    std::thread::spawn(move || {
        for msg in rx {
            match msg {
                Msg::Resync => st.resync(&backup::load_sources()),
                Msg::Exclude(list) => st.exclude = list,
                Msg::Fs(Ok(ev)) => {
                    if let Some(name) = st.handle(ev) {
                        on_change(name);
                    }
                }
                Msg::Fs(Err(e)) => log::warn!("izleme hatasi: {}", e),
            }
        }
    });
    let h = Handle(tx);
    h.resync();
    Ok(h)
}
