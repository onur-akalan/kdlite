//! Tek surecli servis: kDrive baglantisi, otomatik yedekleme, yerel API ve
//! KDE sistem tepsisi simgesi ayni surecte calisir. Yonetim penceresi ayri
//! bir surectir (qml6) ve yalnizca acikken bellek kullanir.

use crate::tr;
use crate::ui::Ui;
use crate::{mount_session, Ctx};
use anyhow::{anyhow, Context, Result};
use ksni::blocking::TrayMethods;
use serde_json::Value;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const QML: &str = include_str!("window.qml");
// Tepsi simgesi duruma gore degisir. (Plasma tepsisi kaplama rozetlerini
// cizmedigi icin simgenin tamami degistirilir.)
const ICON: &str = "folder-cloud";
const BADGE_OK: &str = "state-ok";
const BADGE_SYNC: &str = "state-sync";
const BADGE_PAUSE: &str = "state-pause";
const BADGE_WARN: &str = "state-warning";
const BADGE_OFFLINE: &str = "state-offline";
const BADGE_NONE: &str = ICON;

fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("kdlite")
}

/// Yonetim penceresini ayri bir surec olarak baslatir.
pub fn spawn_window(port: u16) -> Result<Child> {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let qml = dir.join("window.qml");
    let text = QML
        .replace("@PORT@", &port.to_string())
        .replace("@LANG@", crate::i18n::lang())
        .replace("@STRINGS@", crate::i18n::catalog_json());
    std::fs::write(&qml, text)?;
    let mut cmd = Command::new("qml6");
    cmd.arg(&qml)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        cmd.env("QT_QUICK_CONTROLS_STYLE", "org.kde.desktop");
    }
    // Yazilimsal cizim: bu basit pencere icin GPU surucusunu yuklemeye
    // gerek yok, bellek kullanimi yaklasik yariya iner.
    if std::env::var_os("QT_QUICK_BACKEND").is_none() {
        cmd.env("QT_QUICK_BACKEND", "software");
    }
    cmd.spawn()
        .context("pencere acilamadi (qml6 bulunamadi; qt6-declarative paketi gerekli)")
}

type Window = Arc<Mutex<Option<Child>>>;

fn window_open(w: &Window) -> bool {
    let mut guard = w.lock().unwrap();
    match guard.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        _ => {
            *guard = None;
            false
        }
    }
}

fn close_window(w: &Window) {
    if let Some(mut child) = w.lock().unwrap().take() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let _ = child.wait();
    }
}

fn show_window(w: &Window, port: u16) {
    if window_open(w) {
        return;
    }
    match spawn_window(port) {
        Ok(child) => *w.lock().unwrap() = Some(child),
        Err(e) => log::warn!("{:#}", e),
    }
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, U[i])
}

/// Durumdan (durum rozeti, ipucu metni) uretir.
fn describe(st: &Value, mounted: bool) -> (&'static str, String) {
    let n = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let mount_note = if mounted { String::new() } else { format!("\n{}", tr!("The kDrive folder is not mounted right now")) };
    let mount_note = mount_note.as_str();
    let empty = Vec::new();
    let srcs = st.get("sources").and_then(|s| s.as_array()).unwrap_or(&empty);
    if st.get("running").and_then(|r| r.as_bool()).unwrap_or(false) {
        let p = &st["progress"];
        let source = p.get("source").and_then(|s| s.as_str()).unwrap_or("");
        let text = if st.get("freeing").and_then(|r| r.as_bool()).unwrap_or(false) {
            tr!("Changing where files are kept")
        } else if n(p, "total_files") > 0 {
            tr!(
                "Backing up: {}\n{}/{} files, {} / {}",
                source,
                n(p, "done_files"),
                n(p, "total_files"),
                human(n(p, "done_bytes")),
                human(n(p, "total_bytes"))
            )
        } else {
            tr!("Scanning: {}", source)
        };
        return (BADGE_SYNC, text + mount_note);
    }
    if st.get("paused").and_then(|r| r.as_bool()).unwrap_or(false) {
        return (
            BADGE_PAUSE,
            format!("{}{}", tr!("Backup paused"), mount_note),
        );
    }
    let failed: Vec<&str> = srcs
        .iter()
        .filter(|s| {
            let last = &s["last"];
            !s["exists"].as_bool().unwrap_or(true)
                || last.get("error").is_some_and(|e| !e.is_null())
                || last.get("stats").map(|x| n(x, "failed")).unwrap_or(0) > 0
        })
        .filter_map(|s| s["name"].as_str())
        .collect();
    if !failed.is_empty() {
        return (
            BADGE_WARN,
            format!("{}{}", tr!("Some files could not be backed up: {}", failed.join(", ")), mount_note),
        );
    }
    let waiting: Vec<&str> = srcs
        .iter()
        .filter(|s| s["last"].get("stats").map(|x| n(x, "pending_deletes")).unwrap_or(0) > 0)
        .filter_map(|s| s["name"].as_str())
        .collect();
    if !waiting.is_empty() {
        return (
            BADGE_WARN,
            format!("{}{}", tr!("Deleted files are waiting for your answer: {}", waiting.join(", ")), mount_note),
        );
    }
    let text = if srcs.is_empty() {
        tr!("No folders selected yet")
    } else {
        let files: u64 = srcs.iter().map(|s| n(s, "files")).sum();
        let bytes: u64 = srcs.iter().map(|s| n(s, "bytes")).sum();
        tr!("{} folders backed up\n{} files, {} in the cloud", srcs.len(), files, human(bytes))
    };
    let badge = match (mounted, srcs.is_empty()) {
        (false, _) => BADGE_OFFLINE,
        (true, true) => BADGE_NONE,
        (true, false) => BADGE_OK,
    };
    (badge, text + mount_note)
}

struct KdTray {
    badge: &'static str,
    tip: String,
    port: u16,
    window: Window,
    ui: Arc<Ui>,
    mountpoint: PathBuf,
    quit: mpsc::Sender<()>,
}

impl ksni::Tray for KdTray {
    fn id(&self) -> String {
        "kdlite".into()
    }
    fn title(&self) -> String {
        "kdlite".into()
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }
    fn icon_name(&self) -> String {
        self.badge.into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: ICON.into(),
            icon_pixmap: Vec::new(),
            title: "kdlite".into(),
            description: self.tip.clone(),
        }
    }
    /// Sol tik: pencereyi ac / kapat.
    fn activate(&mut self, _x: i32, _y: i32) {
        if window_open(&self.window) {
            close_window(&self.window);
        } else {
            show_window(&self.window, self.port);
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let paused = self.ui.paused();
        vec![
            StandardItem {
                label: tr!("Backup window"),
                icon_name: "folder-cloud".into(),
                activate: Box::new(|t: &mut Self| show_window(&t.window, t.port)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: tr!("Back up now"),
                icon_name: "cloud-upload".into(),
                activate: Box::new(|t: &mut Self| t.ui.run_in_background()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if paused { tr!("Resume backup") } else { tr!("Pause backup") },
                icon_name: if paused { "media-playback-start" } else { "media-playback-pause" }.into(),
                activate: Box::new(|t: &mut Self| t.ui.set_paused(!t.ui.paused())),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: tr!("Open the kDrive folder"),
                icon_name: "document-open-folder".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = Command::new("xdg-open")
                        .arg(&t.mountpoint)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn();
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: tr!("Quit"),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.quit.send(());
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Tek ornek kilidi. Kilit alinamazsa (zaten calisiyor) None doner.
fn single_instance() -> Result<Option<std::fs::File>> {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let f = std::fs::File::create(dir.join("lock"))?;
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    Ok((rc == 0).then_some(f))
}

fn is_mounted(mp: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(mp), mp.parent().map(std::fs::metadata)) {
        (Ok(a), Some(Ok(b))) => a.dev() != b.dev(),
        _ => false,
    }
}

pub fn run(c: Ctx, ui: Arc<Ui>, port: u16, hidden: bool) -> Result<()> {
    let Some(_lock) = single_instance()? else {
        // Zaten calisiyor: gizli baslatma degilse yalnizca pencereyi ac.
        if !hidden {
            spawn_window(port)?;
        }
        return Ok(());
    };

    let (quit_tx, quit_rx) = mpsc::channel::<()>();
    ui.set_quit(quit_tx.clone());
    ui.start()?;
    {
        let tx = quit_tx.clone();
        ctrlc::set_handler(move || {
            let _ = tx.send(());
        })?;
    }

    // Baglanti: kurulamadiysa (ornegin oturum acilisinda ag henuz yoksa)
    // dakikada bir yeniden denenir.
    let c = Arc::new(c);
    let mountpoint = c.cfg.mountpoint();
    crate::ui::clean_old_mount();
    let session: Arc<Mutex<Option<(fuser::BackgroundSession, Arc<crate::fs::Inner>)>>> = Arc::new(Mutex::new(None));
    {
        let (c, session, ui) = (c.clone(), session.clone(), ui.clone());
        std::thread::spawn(move || {
            let mut wait = Duration::from_secs(5);
            loop {
                if session.lock().unwrap().is_none() {
                    match mount_session(&c) {
                        Ok(s) => {
                            ui.set_mount(s.1.clone());
                            *session.lock().unwrap() = Some(s);
                            log::info!("kDrive baglandi: {}", c.cfg.mountpoint().display());
                        }
                        Err(e) => log::warn!("kDrive baglanamadi, yeniden denenecek: {:#}", e),
                    }
                }
                std::thread::sleep(wait);
                wait = Duration::from_secs(60);
            }
        });
    }

    let window: Window = Arc::new(Mutex::new(None));
    let tray = KdTray {
        badge: BADGE_NONE,
        tip: tr!("Starting kdlite…"),
        port,
        window: window.clone(),
        ui: ui.clone(),
        mountpoint: mountpoint.clone(),
        quit: quit_tx.clone(),
    };
    // Oturum acilisinda tepsi henuz hazir olmayabilir; hazir olunca gorunur.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .map_err(|e| anyhow!("tepsi simgesi olusturulamadi: {}", e))?;

    // Simge ve ipucunu yalnizca degistiginde guncelle.
    {
        let (ui, mp, window) = (ui.clone(), mountpoint.clone(), window.clone());
        std::thread::spawn(move || {
            let mut last: Option<(&'static str, String)> = None;
            loop {
                window_open(&window); // kapanan pencere surecini topla
                ui.poll_ignore();
                ui.prompt_deletes();
                let now = describe(&ui.state(), is_mounted(&mp));
                if last.as_ref() != Some(&now) {
                    let (badge, tip) = now.clone();
                    if handle.update(move |t: &mut KdTray| {
                        t.badge = badge;
                        t.tip = tip;
                    })
                    .is_none()
                    {
                        break;
                    }
                    last = Some(now);
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }

    if !hidden {
        show_window(&window, port);
    }
    log::info!("kdlite calisiyor (arayuz portu {})", port);
    let _ = quit_rx.recv();
    log::info!("kapatiliyor");
    let reopen = window_open(&window);
    close_window(&window);
    if let Some((s, inner)) = session.lock().unwrap().take() {
        inner.flush_pending(); // bekleyen yazmalari gonder
        drop(s); // baglantiyi ayirir
    }
    if ui.restart.load(std::sync::atomic::Ordering::SeqCst) {
        // Ayarlar degisti: ayni komutla yeniden basla; pencere aciksa yine acilsin.
        use std::os::unix::process::CommandExt;
        drop(_lock);
        let mut args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--hidden").collect();
        if !reopen && !ui.show_after.load(std::sync::atomic::Ordering::SeqCst) {
            args.push("--hidden".into());
        }
        // Kurulu yol (sembolik bag) korunur ki surec adi degismesin.
        let exe = std::env::args_os().next().map(PathBuf::from).filter(|p| p.is_absolute());
        let exe = match exe {
            Some(p) => p,
            None => std::env::current_exe()?,
        };
        let err = Command::new(exe).args(args).exec();
        return Err(anyhow!("yeniden baslatilamadi: {}", err));
    }
    Ok(())
}
