//! kDrive API istemcisi: listeleme, indirme, klasor olusturma, yukleme ve
//! cop kutusuna tasima. Kalici silme, tasima ya da yeniden adlandirma istegi
//! bilerek yoktur; silinen her sey kDrive cop kutusuna gider (geri alinabilir).

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Body, Client, RequestBuilder, Response};
use reqwest::header::RANGE;
use reqwest::StatusCode;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix saniyesi.
    pub mtime: i64,
}

pub struct Api {
    http: Client,
    base: String,
    drive_id: u64,
    token: String,
}

const PAGE: u32 = 200;
const TRIES: u32 = 4;
/// Bundan buyuk dosyalar parcali oturumla yuklenir (resmi istemciyle ayni esik).
const SESSION_THRESHOLD: u64 = 100 * 1024 * 1024;
const CHUNK_MIN: u64 = 10 * 1024 * 1024;
const CHUNK_MAX: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Uploaded {
    pub id: u64,
    pub size: u64,
}

fn parse_uploaded(v: &Value) -> Result<Uploaded> {
    let data = v.get("data").ok_or_else(|| anyhow!("yukleme yanitinda data yok"))?;
    let f = data.get("file").filter(|x| x.is_object()).unwrap_or(data);
    let id = f
        .get("id")
        .and_then(as_u64)
        .ok_or_else(|| anyhow!("yukleme yanitinda dosya kimligi yok"))?;
    let size = f.get("size").and_then(as_u64).unwrap_or(0);
    Ok(Uploaded { id, size })
}

fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

impl Api {
    pub fn new(base: &str, drive_id: u64, token: &str) -> Result<Api> {
        let http = Client::builder()
            .user_agent(concat!("kdlite/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            // Toplam sure siniri yok: buyuk dosyalar uzun surebilir.
            .timeout(None)
            .build()?;
        Ok(Api {
            http,
            base: base.trim_end_matches('/').to_string(),
            drive_id,
            token: token.trim().to_string(),
        })
    }

    /// Istegi gonderir; 429 ve 5xx yanitlarinda ve ag hatalarinda bekleyip
    /// yeniden dener. `build` her denemede istegi bastan kurar.
    fn send(&self, build: impl Fn() -> Result<RequestBuilder>) -> Result<Response> {
        let mut wait = Duration::from_millis(500);
        let mut last = anyhow!("istek yapilamadi");
        for attempt in 1..=TRIES {
            match build()?.bearer_auth(&self.token).send() {
                Ok(resp) => {
                    let st = resp.status();
                    if st.is_success() {
                        return Ok(resp);
                    }
                    let retry = st == StatusCode::TOO_MANY_REQUESTS || st.is_server_error();
                    let body = resp.text().unwrap_or_default();
                    let short: String = body.chars().take(300).collect();
                    last = anyhow!("HTTP {} - {}", st.as_u16(), short);
                    if !retry {
                        return Err(last);
                    }
                }
                Err(e) => last = anyhow!(e),
            }
            if attempt < TRIES {
                std::thread::sleep(wait);
                wait *= 3;
            }
        }
        Err(last)
    }

    fn get(&self, url: &str, range: Option<(u64, u64)>) -> Result<Response> {
        self.send(|| {
            let mut req = self.http.get(url);
            if let Some((start, end)) = range {
                req = req.header(RANGE, format!("bytes={}-{}", start, end.saturating_sub(1)));
            }
            Ok(req)
        })
    }

    /// Bir dosyayi ya da klasoru kDrive cop kutusuna tasir (kalici silmez).
    pub fn trash(&self, file_id: u64) -> Result<()> {
        let url = format!("{}/2/drive/{}/files/{}", self.base, self.drive_id, file_id);
        self.send(|| Ok(self.http.delete(&url)))?;
        Ok(())
    }

    /// Cop kutusundaki ogelerin adlari (dogrulama icin).
    pub fn trash_names(&self) -> Result<Vec<String>> {
        let url = format!("{}/2/drive/{}/trash?limit=50&order_by=deleted_at&order=desc", self.base, self.drive_id);
        let v: Value = self.get(&url, None)?.json()?;
        Ok(v.get("data")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|e| e.get("name")?.as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }

    /// Token'in erisebildigi suruculeri listeler: (kimlik, ad). Kurulum
    /// sihirbazi icin; drive_id gerektirmez.
    pub fn drives(&self) -> Result<Vec<(u64, String)>> {
        let url = format!("{}/2/drive/init?with=drives", self.base);
        let v: Value = self.get(&url, None)?.json().context("surucu listesi okunamadi")?;
        let list = v
            .pointer("/data/drives")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow!("yanitta surucu listesi yok"))?;
        Ok(list
            .iter()
            .filter_map(|d| {
                let id = d.get("id").and_then(as_u64)?;
                let name = d.get("name").and_then(|n| n.as_str()).unwrap_or("kDrive").to_string();
                Some((id, name))
            })
            .collect())
    }

    /// Bir klasorun dogrudan icerigini listeler (tum sayfalar).
    pub fn list_dir(&self, dir_id: u64) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/3/drive/{}/files/{}/files?limit={}",
                self.base, self.drive_id, dir_id, PAGE
            );
            if let Some(c) = &cursor {
                url.push_str("&cursor=");
                url.push_str(c);
            }
            let v: Value = self
                .get(&url, None)
                .with_context(|| format!("klasor {} listelenemedi", dir_id))?
                .json()
                .context("liste yaniti JSON degil")?;
            let data = v
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| anyhow!("liste yanitinda data yok"))?;
            for item in data {
                let (Some(id), Some(name)) = (
                    item.get("id").and_then(as_u64),
                    item.get("name").and_then(|n| n.as_str()),
                ) else {
                    continue;
                };
                let is_dir = item.get("type").and_then(|t| t.as_str()) == Some("dir");
                out.push(Entry {
                    id,
                    name: name.to_string(),
                    is_dir,
                    size: item.get("size").and_then(as_u64).unwrap_or(0),
                    mtime: item
                        .get("last_modified_at")
                        .and_then(as_i64)
                        .or_else(|| item.get("created_at").and_then(as_i64))
                        .unwrap_or(0),
                });
            }
            let has_more = v.get("has_more").and_then(|b| b.as_bool()).unwrap_or(false);
            let next = v.get("cursor").and_then(|c| c.as_str()).map(str::to_string);
            match (has_more, next) {
                (true, Some(c)) if Some(&c) != cursor.as_ref() => cursor = Some(c),
                (true, _) => bail!("sayfalama ilerlemiyor (klasor {})", dir_id),
                _ => break,
            }
        }
        Ok(out)
    }

    fn download_url(&self, file_id: u64) -> String {
        format!(
            "{}/2/drive/{}/files/{}/download",
            self.base, self.drive_id, file_id
        )
    }

    /// Dosyanin tamamini akis olarak indirir.
    pub fn download(&self, file_id: u64) -> Result<Response> {
        self.get(&self.download_url(file_id), None)
    }

    /// Dosyanin [start, end) araligini ister. Sunucu aralik istegini
    /// desteklemiyorsa (206 donmuyorsa) None doner.
    pub fn download_range(&self, file_id: u64, start: u64, end: u64) -> Result<Option<Vec<u8>>> {
        let resp = self.get(&self.download_url(file_id), Some((start, end)))?;
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Ok(None);
        }
        let bytes = resp.bytes()?;
        if bytes.len() as u64 != end - start {
            return Ok(None);
        }
        Ok(Some(bytes.to_vec()))
    }

    /// `parent` altinda yeni bir klasor olusturur ve kimligini doner.
    pub fn create_dir(&self, parent: u64, name: &str) -> Result<u64> {
        let url = format!("{}/3/drive/{}/files/{}/directory", self.base, self.drive_id, parent);
        let body = serde_json::json!({ "name": name });
        let v: Value = self
            .send(|| Ok(self.http.post(&url).json(&body)))
            .with_context(|| format!("klasor olusturulamadi: {}", name))?
            .json()?;
        v.get("data")
            .and_then(|d| d.get("id"))
            .and_then(as_u64)
            .ok_or_else(|| anyhow!("klasor olusturma yanitinda kimlik yok"))
    }

    /// Yerel dosyayi `dir_id` klasorune yukler. Ayni adda dosya varsa yeni
    /// surum olarak eklenir (eski surum kDrive surum gecmisinde kalir).
    pub fn upload_file(&self, dir_id: u64, name: &str, path: &Path, size: u64, mtime: i64) -> Result<Uploaded> {
        let up = if size > SESSION_THRESHOLD {
            self.upload_session(dir_id, name, path, size, mtime)?
        } else {
            self.upload_direct(dir_id, name, path, size, mtime)?
        };
        if up.size != size {
            bail!("yukleme sonrasi boyut uyusmuyor: yerel {}, bulut {}", size, up.size);
        }
        Ok(up)
    }

    fn upload_direct(&self, dir_id: u64, name: &str, path: &Path, size: u64, mtime: i64) -> Result<Uploaded> {
        let url = format!("{}/3/drive/{}/upload", self.base, self.drive_id);
        let query = [
            ("directory_id", dir_id.to_string()),
            ("file_name", name.to_string()),
            ("total_size", size.to_string()),
            ("conflict", "version".to_string()),
            ("last_modified_at", mtime.to_string()),
        ];
        let v: Value = self
            .send(|| {
                let f = File::open(path)?;
                Ok(self
                    .http
                    .post(&url)
                    .query(&query)
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::sized(f, size)))
            })?
            .json()?;
        parse_uploaded(&v)
    }

    fn upload_session(&self, dir_id: u64, name: &str, path: &Path, size: u64, mtime: i64) -> Result<Uploaded> {
        let chunk = (size / 200).clamp(CHUNK_MIN, CHUNK_MAX);
        let total_chunks = size.div_ceil(chunk);
        let start_url = format!("{}/3/drive/{}/upload/session/start", self.base, self.drive_id);
        let body = serde_json::json!({
            "directory_id": dir_id,
            "file_name": name,
            "total_size": size,
            "total_chunks": total_chunks,
            "conflict": "version",
            "last_modified_at": mtime,
        });
        let v: Value = self
            .send(|| Ok(self.http.post(&start_url).json(&body)))
            .context("yukleme oturumu baslatilamadi")?
            .json()?;
        let data = v.get("data").ok_or_else(|| anyhow!("oturum yanitinda data yok"))?;
        let token = data
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("oturum yanitinda token yok"))?;
        let host = data
            .get("upload_url")
            .and_then(|t| t.as_str())
            .filter(|h| !h.is_empty())
            .unwrap_or(&self.base)
            .trim_end_matches('/');
        let chunk_url = format!("{}/3/drive/{}/upload/session/{}/chunk", host, self.drive_id, token);

        // Parcalar bellege alinmaz; her biri dogrudan dosyadan akitilir. Boylece
        // bellek kullanimi dosya ve parca boyutundan bagimsiz kalir.
        let mut sent: u64 = 0;
        for n in 1..=total_chunks {
            let want = (size - sent).min(chunk);
            let offset = sent;
            self.send(|| {
                let mut f = File::open(path)?;
                f.seek(SeekFrom::Start(offset))?;
                Ok(self
                    .http
                    .post(&chunk_url)
                    .query(&[("chunk_number", n.to_string()), ("chunk_size", want.to_string())])
                    .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::sized(f.take(want), want)))
            })
            .with_context(|| format!("parca {}/{} yuklenemedi", n, total_chunks))?;
            sent += want;
        }
        let finish_url = format!(
            "{}/3/drive/{}/upload/session/{}/finish",
            self.base, self.drive_id, token
        );
        let fin = serde_json::json!({ "total_chunks": total_chunks, "last_modified_at": mtime });
        let v: Value = self
            .send(|| Ok(self.http.post(&finish_url).json(&fin)))
            .context("yukleme oturumu kapatilamadi")?
            .json()?;
        parse_uploaded(&v)
    }
}
