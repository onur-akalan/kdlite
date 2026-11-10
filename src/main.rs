mod api;
mod backup;
mod cache;
mod config;
mod fs;
mod pins;
mod setup;
mod tray;
mod ui;
mod watch;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use fuser::MountOption;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "kdlite", version, about = "kDrive'i istege bagli indirmeyle baglayan salt okunur istemci")]
struct Cli {
    /// Ayar dosyasi (varsayilan: ~/.config/kdlite/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ornek ayar dosyasi olusturur
    Init,
    /// Surucuyu baglar (on planda calisir; Ctrl-C ile ayrilir)
    Mount,
    /// Baglamadan bir klasoru listeler (baglanti testi icin)
    Ls {
        /// Koke gore yol; bos birakilirsa kok
        path: Option<String>,
        /// Yol yerine dogrudan klasor kimligi
        #[arg(long)]
        id: Option<u64>,
    },
    /// Bir dosya ya da klasoru her zaman yerelde tutulacak sekilde sabitler
    Pin { path: String },
    /// Sabitlemeyi kaldirir
    Unpin { path: String },
    /// Sabitlenen yollari listeler
    Pins,
    /// Onbellek kullanimini gosterir
    Status,
    /// Token'in erisebildigi suruculeri listeler
    Drives,
    /// kDrive cop kutusundaki son ogeleri listeler
    TrashLs,
    /// Tarayicida acilan arayuz: klasor secme, durum, otomatik yedekleme
    Ui {
        #[arg(long, default_value_t = 47800)]
        port: u16,
        /// Tarayiciyi kendiliginden acma
        #[arg(long)]
        no_open: bool,
    },
    /// Servis: baglanti + otomatik yedekleme + sistem tepsisi simgesi (tek surec)
    Tray {
        #[arg(long, default_value_t = 47800)]
        port: u16,
        /// Pencereyi acmadan yalnizca tepside basla (oturum acilisi icin)
        #[arg(long)]
        hidden: bool,
    },
    /// Calisan servisin yonetim penceresini acar
    Window {
        #[arg(long, default_value_t = 47800)]
        port: u16,
    },
    /// Calisan servisi kapatir (baglantiyi ayirir)
    Quit {
        #[arg(long, default_value_t = 47800)]
        port: u16,
    },
    /// Calisan servisin yedeklemesini duraklatir
    Pause {
        #[arg(long, default_value_t = 47800)]
        port: u16,
    },
    /// Duraklatilan yedeklemeyi surdurur
    Resume {
        #[arg(long, default_value_t = 47800)]
        port: u16,
    },
    /// Dosya yoneticisinin sag tik menusu icin: klasoru yedege ekler / cikarir
    #[command(hide = true)]
    Menu {
        /// add | remove | sync | cloud | both | local
        action: String,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value_t = 47800)]
        port: u16,
    },
    /// Yerel klasorleri kDrive'a tek yonlu yedekler
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// Bir klasoru yedeklenecekler listesine ekler
    Add {
        path: PathBuf,
        /// Buluttaki adi (varsayilan: klasorun adi)
        #[arg(long)]
        name: Option<String>,
    },
    /// Bir klasoru listeden cikarir (buluttaki yedek silinmez)
    Remove { name: String },
    /// Yedeklenen klasorleri listeler
    List,
    /// Bir kez tarar, eksik ve degismis dosyalari yukler
    Run,
    /// Surekli calisir: belirli araliklarla tarar ve yukler
    Watch,
    /// Yerel dosyalarin bulutta ayni boyutla durdugunu dogrular
    Verify,
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

pub(crate) struct Ctx {
    pub(crate) cfg: config::Config,
    pub(crate) api: Arc<api::Api>,
}

fn ctx(path: &Option<PathBuf>) -> Result<Ctx> {
    let p = path.clone().unwrap_or_else(config::default_path);
    let cfg = config::Config::load(&p)?;
    let api = Arc::new(api::Api::new(&cfg.base_url, cfg.drive_id, &cfg.token)?);
    Ok(Ctx { cfg, api })
}

fn make_cache(c: &Ctx) -> Result<Arc<cache::Cache>> {
    let dir = c.cfg.cache_dir().join("files");
    let max = (c.cfg.cache_max_gb.max(0.0) * (1u64 << 30) as f64) as u64;
    Ok(Arc::new(
        cache::Cache::new(dir, max, c.api.clone()).context("onbellek klasoru acilamadi")?,
    ))
}

fn cmd_init(path: &Option<PathBuf>) -> Result<()> {
    let p = path.clone().unwrap_or_else(config::default_path);
    if p.exists() {
        bail!("{} zaten var; uzerine yazilmadi", p.display());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&p)?;
    f.write_all(config::TEMPLATE.as_bytes())?;
    println!("Olusturuldu: {}", p.display());
    println!("Icine token ve drive_id degerlerini yazin, sonra: kdlite ls");
    Ok(())
}

fn cmd_ls(c: Ctx, path: Option<String>, id: Option<u64>) -> Result<()> {
    let entries = match id {
        Some(id) => c.api.list_dir(id)?,
        None => {
            let cache = make_cache(&c)?;
            let inner = fs::Inner::new(c.api.clone(), cache, c.cfg.root_id, Duration::from_secs(60));
            let (ino, entry) = inner
                .resolve(path.as_deref().unwrap_or(""))
                .map_err(|e| anyhow::anyhow!("yol cozulemedi (errno {})", e))?;
            if entry.as_ref().is_some_and(|e| !e.is_dir) {
                bail!("bu bir dosya, klasor degil");
            }
            inner
                .children(ino)
                .map_err(|e| anyhow::anyhow!("listelenemedi (errno {})", e))?
                .into_iter()
                .map(|(_, e)| e)
                .collect()
        }
    };
    println!("{:>12}  {:>10}  {}", "kimlik", "boyut", "ad");
    for e in &entries {
        let size = if e.is_dir { "<klasor>".to_string() } else { human(e.size) };
        println!("{:>12}  {:>10}  {}", e.id, size, e.name);
    }
    println!("{} oge", entries.len());
    Ok(())
}

/// kDrive'i salt okunur baglar; donen oturum birakilinca baglanti ayrilir.
pub(crate) fn mount_session(c: &Ctx) -> Result<(fuser::BackgroundSession, Arc<fs::Inner>)> {
    let mp = c.cfg.mountpoint();
    std::fs::create_dir_all(&mp).with_context(|| format!("{} olusturulamadi", mp.display()))?;
    if std::fs::read_dir(&mp)?.next().is_some() {
        bail!("{} bos degil; bos bir klasor secin", mp.display());
    }
    // Baglamadan once baglantiyi ve token'i dogrula.
    let top = c
        .api
        .list_dir(c.cfg.root_id)
        .context("kDrive'a baglanilamadi; token ve drive_id degerlerini kontrol edin")?;
    log::info!("baglanti tamam, kokte {} oge var", top.len());

    let cache = make_cache(c)?;
    cache.evict();
    let mut inner = fs::Inner::new(
        c.api.clone(),
        cache,
        c.cfg.root_id,
        Duration::from_secs(c.cfg.dir_ttl_secs),
    );
    if c.cfg.writable {
        inner = inner
            .with_write(c.cfg.cache_dir().join("staged"), c.cfg.exclude.clone())
            .context("bekleme klasoru olusturulamadi")?;
    }
    let inner = Arc::new(inner);
    inner.spawn_uploader();
    pins::spawn(inner.clone());

    let opts = [
        if c.cfg.writable { MountOption::RW } else { MountOption::RO },
        MountOption::FSName("kdlite".into()),
        MountOption::Subtype("kdlite".into()),
        MountOption::NoAtime,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::DefaultPermissions,
    ];
    let session = fuser::spawn_mount2(fs::KdFs { inner: inner.clone() }, &mp, &opts)
        .with_context(|| format!("{} baglanamadi", mp.display()))?;
    Ok((session, inner))
}

fn cmd_mount(c: Ctx) -> Result<()> {
    let (session, inner) = mount_session(&c)?;
    println!("Baglandi: {}. Ayirmak icin Ctrl-C.", c.cfg.mountpoint().display());

    let (tx, rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })?;
    let _ = rx.recv();
    inner.flush_pending();
    drop(session);
    println!("Ayrildi.");
    Ok(())
}

fn cmd_status(c: Ctx) -> Result<()> {
    let cache = make_cache(&c)?;
    let (n, total, _) = cache.usage();
    println!("Onbellek klasoru : {}", c.cfg.cache_dir().display());
    println!("Dosya sayisi     : {}", n);
    println!("Kullanilan       : {} (sinir {} GB, sabitlenenler haric)", human(total), c.cfg.cache_max_gb);
    println!("Diskte bos       : {}", human(cache.free_space()));
    let p = pins::load();
    println!("Sabitlenen yol   : {}", p.len());
    for x in p {
        println!("  {}", x);
    }
    Ok(())
}

fn make_backup(c: &Ctx) -> backup::Backup<'_> {
    backup::Backup {
        api: &c.api,
        root_id: c.cfg.root_id,
        backup_root: c
            .cfg
            .backup_root
            .clone()
            .filter(|r| !r.trim_matches('/').is_empty())
            .unwrap_or_else(|| format!("Yedekler/{}", backup::hostname())),
        exclude: c.cfg.exclude.clone(),
        progress: None,
        cancel: None,
        mountpoint: c.cfg.mountpoint(),
        sync_deletes: c.cfg.sync_deletes,
        confirm_deletes: false,
    }
}

fn backup_once(c: &Ctx, quiet: bool) -> bool {
    let b = make_backup(c);
    let mut all_ok = true;
    for src in backup::load_sources() {
        match b.run(&src) {
            Ok(s) => {
                if s.failed > 0 {
                    all_ok = false;
                }
                if !quiet || s.uploaded > 0 || s.failed > 0 {
                    println!(
                        "{}: {} dosya tarandi, {} yuklendi ({}), {} zaten buluttaydi, {} basarisiz, {} ertelendi, {} atlandi",
                        src.name,
                        s.scanned,
                        s.uploaded,
                        human(s.uploaded_bytes),
                        s.already,
                        s.failed,
                        s.postponed,
                        s.skipped
                    );
                }
            }
            Err(e) => {
                all_ok = false;
                eprintln!("{}: yedeklenemedi: {:#}", src.name, e);
            }
        }
    }
    all_ok
}

fn cmd_backup(path: &Option<PathBuf>, cmd: BackupCmd) -> Result<()> {
    match cmd {
        BackupCmd::Add { path: p, name } => {
            let abs = std::fs::canonicalize(&p).with_context(|| format!("klasor bulunamadi: {}", p.display()))?;
            if !abs.is_dir() {
                bail!("{} bir klasor degil", abs.display());
            }
            let local = abs.to_string_lossy().into_owned();
            let name = match name {
                Some(n) => n,
                None => abs
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or_else(|| anyhow::anyhow!("klasor adi cikarilamadi; --name verin"))?,
            };
            if name.contains('/') || name.is_empty() {
                bail!("gecersiz ad: {}", name);
            }
            let mut all = backup::load_sources();
            if all.iter().any(|s| s.local == local) {
                bail!("{} zaten listede", local);
            }
            if all.iter().any(|s| s.name == name) {
                bail!("'{}' adinda baska bir yedek var; --name ile farkli bir ad verin", name);
            }
            all.push(backup::Source { local: local.clone(), name: name.clone(), dest: None });
            backup::save_sources(&all)?;
            println!("Eklendi: {} -> {}", local, name);
            Ok(())
        }
        BackupCmd::Remove { name } => {
            let mut all = backup::load_sources();
            let before = all.len();
            all.retain(|s| s.name != name && s.local != name);
            if all.len() == before {
                bail!("listede boyle bir yedek yok: {}", name);
            }
            backup::save_sources(&all)?;
            println!("Listeden cikarildi: {} (buluttaki yedege dokunulmadi)", name);
            Ok(())
        }
        BackupCmd::List => {
            let c = ctx(path)?;
            let b = make_backup(&c);
            println!("Buluttaki hedef: {}", b.backup_root);
            for s in backup::load_sources() {
                println!("  {}  ->  {}/{}", s.local, b.base_of(&s), s.name);
            }
            Ok(())
        }
        BackupCmd::Run => {
            let c = ctx(path)?;
            if backup::load_sources().is_empty() {
                bail!("yedeklenecek klasor yok; once: kdlite backup add <klasor>");
            }
            if !backup_once(&c, false) {
                bail!("bazi dosyalar yedeklenemedi; ayrinti icin yukaridaki uyarilara bakin");
            }
            Ok(())
        }
        BackupCmd::Watch => {
            let c = ctx(path)?;
            let every = Duration::from_secs(c.cfg.backup_interval_secs.max(10));
            println!("Surekli yedekleme basladi ({} saniyede bir tarama). Durdurmak icin Ctrl-C.", every.as_secs());
            let (tx, rx) = mpsc::channel();
            ctrlc::set_handler(move || {
                let _ = tx.send(());
            })?;
            loop {
                backup_once(&c, true);
                if rx.recv_timeout(every).is_ok() {
                    println!("Durduruldu.");
                    return Ok(());
                }
            }
        }
        BackupCmd::Verify => {
            let c = ctx(path)?;
            let b = make_backup(&c);
            let mut bad = 0;
            for src in backup::load_sources() {
                let (problems, checked) = b.verify(&src)?;
                println!("{}: {} dosya kontrol edildi, {} eksik ya da farkli", src.name, checked, problems.len());
                for p in problems.iter().take(50) {
                    println!("  {}", p);
                }
                if problems.len() > 50 {
                    println!("  ... ve {} tane daha", problems.len() - 50);
                }
                bad += problems.len();
            }
            if bad > 0 {
                bail!("{} dosya bulutta eksik ya da farkli", bad);
            }
            Ok(())
        }
    }
}

fn make_ui(c: &Ctx, port: u16, config_path: PathBuf) -> Arc<ui::Ui> {
    let b = make_backup(c);
    let mut u = ui::Ui::new(
        c.api.clone(),
        c.cfg.root_id,
        b.backup_root.clone(),
        c.cfg.exclude.clone(),
        Duration::from_secs(c.cfg.backup_interval_secs.max(10)),
        Duration::from_secs(c.cfg.rescan_secs.max(60)),
        c.cfg.mountpoint(),
        port,
    );
    u.config_path = config_path;
    u.sync_deletes = c.cfg.sync_deletes;
    Arc::new(u)
}

/// Calisan servisin yerel API'sine istek atar; hata mesajini servis verir.
fn post(port: u16, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{}{}", port, path))
        .header("X-Kdlite", "1")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|_| anyhow::anyhow!("kdlite çalışmıyor; önce uygulama menüsünden başlatın"))?;
    let ok = resp.status().is_success();
    let v: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
    if !ok {
        bail!("{}", v.get("error").and_then(|e| e.as_str()).unwrap_or("istek başarısız"));
    }
    Ok(v)
}

fn notify(icon: &str, title: &str, text: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "kdlite", "-i", icon, title, text])
        .status();
}

/// Onay isteyen bildirim; kullanici onay dugmesine basarsa true.
fn confirm(title: &str, text: &str, yes: &str) -> bool {
    let out = std::process::Command::new("notify-send")
        .args(["-a", "kdlite", "-i", "dialog-warning", "-u", "critical", "--wait", "-t", "60000"])
        .args(["-A", &format!("yes={}", yes), "-A", "no=Vazgeç", title, text])
        .output();
    out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
}

/// Sag tik menusu: secilen dosyalarin konumunu degistirir.
fn cmd_place(mode: &str, paths: &[PathBuf], port: u16) -> Result<()> {
    let what = if paths.len() == 1 {
        format!("“{}”", paths[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    } else {
        format!("{} öğe", paths.len())
    };
    // Buluttan silme: tek dosya disinda once onay al.
    let many = paths.len() > 1 || paths.iter().any(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir()));
    if mode == "local" && many {
        let ok = confirm(
            &format!("{} yalnızca bilgisayarda kalsın mı?", what),
            "Buluttaki kopyalar kDrive çöp kutusuna taşınır ve bu dosyalar artık yedeklenmez.",
            "Buluttan sil",
        );
        if !ok {
            return Ok(());
        }
    }
    let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let res = post(port, "/api/place", serde_json::json!({ "paths": list, "mode": mode }));
    let v = match res {
        Ok(v) => v,
        Err(e) => {
            notify("dialog-warning", &what, &format!("{:#}", e));
            return Ok(());
        }
    };
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let (icon, title, text) = match mode {
        "cloud" => ("cloudstatus", "Yalnızca bulutta", "dosya artık yalnızca bulutta. Yerel kopyalar çöp kutusunda; boşaltınca yer açılır."),
        "both" => ("folder-cloud", "Bulutta ve bilgisayarda", "dosya artık iki yerde de duruyor."),
        _ => ("computer", "Yalnızca bilgisayarda", "dosya artık yalnızca bilgisayarda. Buluttaki kopyalar kDrive çöp kutusunda."),
    };
    let mut body = format!("{} {}", n("done"), text);
    if n("kept") > 0 {
        body.push_str(&format!("\n{} dosya olduğu gibi bırakıldı.", n("kept")));
    }
    if let Some(e) = v.get("errors").and_then(|e| e.as_array()).and_then(|a| a.first()).and_then(|e| e.as_str()) {
        body.push_str(&format!("\n{}", e));
    }
    notify(if n("done") > 0 { icon } else { "dialog-information" }, title, &body);
    Ok(())
}

/// Sag tik menusu: sonucu masaustu bildirimiyle gosterir.
fn cmd_menu(action: &str, path: &std::path::Path, port: u16) -> Result<()> {
    let p = path.to_string_lossy();
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    let res = match action {
        "add" => post(port, "/api/add", serde_json::json!({ "path": p }))
            .map(|_| ("cloud-upload", format!("“{}” yedekleniyor", label), "Değişiklikler kendiliğinden kDrive'a akacak.")),
        "remove" => post(port, "/api/remove", serde_json::json!({ "path": p }))
            .map(|_| ("list-remove", format!("“{}” artık yedeklenmiyor", label), "Buluttaki kopya silinmedi.")),
        "sync" => post(port, "/api/run", serde_json::json!({ "path": p })).map(|v| {
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(&label).to_string();
            ("cloud-upload", format!("“{}” şimdi yedekleniyor", name), "Değişen dosyalar kDrive'a yükleniyor.")
        }),
        _ => bail!("bilinmeyen islem: {}", action),
    };
    match res {
        Ok((icon, title, text)) => notify(icon, &title, text),
        Err(e) => notify("dialog-warning", &format!("“{}”", label), &format!("{:#}", e)),
    }
    Ok(())
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Init => cmd_init(&cli.config),
        Cmd::Mount => cmd_mount(ctx(&cli.config)?),
        Cmd::Ls { path, id } => cmd_ls(ctx(&cli.config)?, path, id),
        Cmd::Status => cmd_status(ctx(&cli.config)?),
        Cmd::TrashLs => {
            for name in ctx(&cli.config)?.api.trash_names()? {
                println!("{}", name);
            }
            Ok(())
        }
        Cmd::Drives => {
            for (id, name) in ctx(&cli.config)?.api.drives()? {
                println!("{:>10}  {}", id, name);
            }
            Ok(())
        }
        Cmd::Backup { cmd } => cmd_backup(&cli.config, cmd),
        Cmd::Ui { port, no_open } => {
            let c = ctx(&cli.config)?;
            make_ui(&c, port, cli.config.clone().unwrap_or_else(config::default_path)).serve(!no_open)
        }
        Cmd::Tray { port, hidden } => {
            // Ilk kurulum: ayar yoksa sihirbaz acilir, bitince servis yeniden baslar.
            let cfg_path = cli.config.clone().unwrap_or_else(config::default_path);
            if setup::needed(&cfg_path) {
                if hidden {
                    bail!("kdlite henuz kurulmadi; kurulum icin: kdlite tray");
                }
                return setup::run(cfg_path, port);
            }
            let c = ctx(&cli.config)?;
            let ui = make_ui(&c, port, cfg_path);
            tray::run(c, ui, port, hidden)
        }
        Cmd::Window { port } => tray::spawn_window(port).map(|_| ()),
        Cmd::Quit { port } => {
            let url = format!("http://127.0.0.1:{}/api/quit", port);
            reqwest::blocking::Client::new()
                .post(url)
                .header("X-Kdlite", "1")
                .header("Content-Type", "application/json")
                .body("{}")
                .send()
                .context("calisan bir kdlite servisi bulunamadi")?;
            println!("Kapatildi.");
            Ok(())
        }
        Cmd::Pause { port } => {
            post(port, "/api/pause", serde_json::json!({ "on": true }))?;
            println!("Duraklatildi.");
            Ok(())
        }
        Cmd::Resume { port } => {
            post(port, "/api/pause", serde_json::json!({ "on": false }))?;
            println!("Surduruluyor.");
            Ok(())
        }
        Cmd::Menu { action, paths, port } => {
            if matches!(action.as_str(), "cloud" | "both" | "local") {
                return cmd_place(&action, &paths, port);
            }
            for p in &paths {
                cmd_menu(&action, p, port)?;
            }
            Ok(())
        }
        Cmd::Pins => {
            for p in pins::load() {
                println!("{}", p);
            }
            Ok(())
        }
        Cmd::Pin { path } => {
            let p = pins::normalize(&path);
            if p.is_empty() {
                bail!("kokun tamami sabitlenemez; bir klasor ya da dosya yolu verin");
            }
            let mut all = pins::load();
            if !all.contains(&p) {
                all.push(p.clone());
                pins::save(&all)?;
            }
            println!("Sabitlendi: {} (calisan baglanti 10 dakika icinde indirmeye baslar)", p);
            Ok(())
        }
        Cmd::Unpin { path } => {
            let p = pins::normalize(&path);
            let mut all = pins::load();
            all.retain(|x| x != &p);
            pins::save(&all)?;
            println!("Sabitleme kaldirildi: {}", p);
            Ok(())
        }
    }
}
