use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Infomaniak API token (kDrive okuma yetkisiyle).
    pub token: String,
    /// kDrive kimligi; web arayuzundeki adreste /drive/<kimlik>/ olarak gorunur.
    pub drive_id: u64,
    /// Baglanacak klasorun kimligi. 1 = surucunun koku.
    #[serde(default = "default_root_id")]
    pub root_id: u64,
    #[serde(default = "default_mountpoint")]
    pub mountpoint: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    /// Onbellegin ust siniri (GB). Sabitlenen dosyalar bu sinira dahil degildir.
    #[serde(default = "default_cache_max_gb")]
    pub cache_max_gb: f64,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Klasor listelerinin kac saniye taze sayilacagi.
    #[serde(default = "default_dir_ttl")]
    pub dir_ttl_secs: u64,
    /// Yedeklerin buluttaki yeri (root_id klasorune gore). Bos birakilirsa
    /// "Yedekler/<bilgisayar adi>" kullanilir.
    #[serde(default)]
    pub backup_root: Option<String>,
    /// Surekli yedeklemede iki tarama arasindaki sure.
    #[serde(default = "default_backup_interval")]
    pub backup_interval_secs: u64,
    /// Dosya izleme (inotify) calisirken guvenlik icin yapilan tam taramanin
    /// araligi. Izleme kurulamazsa backup_interval_secs kullanilir.
    #[serde(default = "default_rescan")]
    pub rescan_secs: u64,
    /// Baglanan klasore yazilabilsin mi (yeni dosya, duzenleme, klasor
    /// olusturma, tasima, silme). Silinenler kDrive cop kutusuna gider.
    #[serde(default = "default_true")]
    pub writable: bool,
    /// Yedeklenen klasorde silinen dosya buluttan da (kDrive cop kutusuna)
    /// silinsin mi.
    #[serde(default = "default_true")]
    pub sync_deletes: bool,
    /// Interface language: "auto" (follow the system), "en", "tr"...
    #[serde(default = "default_language")]
    pub language: String,
    /// Yedeklemede atlanacak dosya ve klasor adlari ("*.ext", "onek*" ya da tam ad).
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
}

fn default_language() -> String {
    "auto".into()
}

fn default_true() -> bool {
    true
}

fn default_rescan() -> u64 {
    600
}

fn default_backup_interval() -> u64 {
    120
}

pub fn default_exclude() -> Vec<String> {
    [
        ".git",
        "node_modules",
        ".venv",
        "__pycache__",
        ".dart_tool",
        ".gradle",
        ".cache",
        ".Trash-*",
        "*.part",
        "*.tmp",
        "*.swp",
        "*~",
        ".~lock*",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_root_id() -> u64 {
    1
}
fn default_mountpoint() -> String {
    "~/kDrive-bulut".into()
}
fn default_cache_dir() -> String {
    "~/.cache/kdlite".into()
}
fn default_cache_max_gb() -> f64 {
    20.0
}
fn default_base_url() -> String {
    "https://api.infomaniak.com".into()
}
fn default_dir_ttl() -> u64 {
    60
}

pub const TEMPLATE: &str = r#"# kdlite ayarlari. Bu dosyada API token'i durur; izinleri 600 olmali.

# Infomaniak Manager > Gelistirici > API token'lari bolumunden olusturulan token.
# Yalnizca kDrive (drive) yetkisi yeterli.
token = ""

# kDrive kimligi: web arayuzunde adres cubugunda .../drive/<kimlik>/files/... olarak gorunur.
drive_id = 0

# Baglanacak klasor. 1 = surucunun koku.
root_id = 1

# Arayuz dili / interface language: "auto" (sistem dili), "en", "tr".
language = "auto"

# Yedeklenen klasorde sildigin dosya buluttan da silinsin mi (kDrive cop
# kutusuna gider, oradan geri alinabilir). false = bulutta kalir.
sync_deletes = true

# Baglanan klasore yazma: yeni dosya, duzenleme, klasor olusturma. Buluttan
# silme ve tasima her durumda kapalidir. false = salt okunur.
writable = true

mountpoint = "~/kDrive-bulut"
cache_dir = "~/.cache/kdlite"
cache_max_gb = 20
base_url = "https://api.infomaniak.com"
dir_ttl_secs = 60

# Yedekleme: buluttaki hedef (root_id klasorune gore). Bos ise "Backups/<bilgisayar adi>".
# backup_root = "Yedekler/bilgisayarim"
# Yerel degisiklikler aninda yuklenir (inotify). rescan_secs: buluttaki
# degisikliklere bakma (ve tam tarama) araligi. backup_interval_secs:
# yalnizca izleme kurulamazsa kullanilan tarama araligi.
rescan_secs = 600
backup_interval_secs = 120
# Yedeklemede atlanacaklar. Tanimlanmazsa makul bir varsayilan liste kullanilir
# (.git, node_modules, .venv, __pycache__, gecici dosyalar...).
# exclude = [".git", "node_modules"]
"#;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

pub fn config_dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("kdlite"),
        _ => home().join(".config/kdlite"),
    }
}

/// Yedeklenmeyecekler dosyasi (.gitignore benzeri): satir basina bir ad.
pub fn ignore_path() -> PathBuf {
    config_dir().join("ignore")
}

/// Dosya varsa icindeki desenleri doner ("#" ile baslayan ve bos satirlar atlanir).
pub fn read_ignore() -> Option<Vec<String>> {
    let text = std::fs::read_to_string(ignore_path()).ok()?;
    Some(
        text.lines()
            .map(|l| l.trim().trim_end_matches('/'))
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.contains('/'))
            .map(str::to_string)
            .collect(),
    )
}

pub const IGNORE_HEADER: &str = "\
# kdlite: yedeklenmeyecek dosya ve klasor adlari (.gitignore gibi).
#
# Her satira bir ad yaz. Kullanilabilen bicimler:
#   node_modules     tam ad
#   *.tmp            bu uzantiyla bitenler
#   .Trash-*         bu onekle baslayanlar
#
# Yol degil yalnizca ad eslestirilir. Bir klasor adi yazarsan, o adi tasiyan
# her klasor ve icindeki her sey atlanir. \"#\" ile baslayan satirlar yorumdur.
# Dosyayi kaydettiginde birkac saniye icinde uygulanir.

";

pub fn default_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let meta = std::fs::metadata(path).with_context(|| {
            format!(
                "ayar dosyasi bulunamadi: {} (olusturmak icin: kdlite init)",
                path.display()
            )
        })?;
        if meta.permissions().mode() & 0o077 != 0 {
            bail!(
                "{} baskalarinca okunabilir; icinde token var. Duzeltmek icin: chmod 600 {}",
                path.display(),
                path.display()
            );
        }
        let text = std::fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&text).context("ayar dosyasi okunamadi")?;
        // Yedeklenmeyecekler listesi ayri bir metin dosyasindaysa o gecerlidir.
        if let Some(list) = read_ignore() {
            cfg.exclude = list;
        }
        if cfg.token.trim().is_empty() {
            bail!("{} icinde token bos", path.display());
        }
        if cfg.drive_id == 0 {
            bail!("{} icinde drive_id ayarlanmamis", path.display());
        }
        Ok(cfg)
    }

    pub fn mountpoint(&self) -> PathBuf {
        expand(&self.mountpoint)
    }
    pub fn cache_dir(&self) -> PathBuf {
        expand(&self.cache_dir)
    }
}
