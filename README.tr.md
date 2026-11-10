# kdlite

Linux ve KDE Plasma için hafif bir bulut sürücü istemcisi.

kdlite bulut sürücünü normal bir klasör gibi gösterir, dosyaları yalnızca
açtığında indirir ve seçtiğin yerel klasörleri yedekler. Sistem tepsisinde
sessizce çalışır.

**Şu an desteklenen:** [Infomaniak kDrive](https://www.infomaniak.com/en/ksuite/kdrive).
Başka sağlayıcılar planlanıyor.

[English](README.md)

## Ne yapar

- **Sürücün bir klasör olarak.** `~/kDrive-bulut` içinde dosyalara göz at,
  aç, kaydet ve düzenle. Dosyalar ilk kullanımda iner.
- **Otomatik yedekleme.** Yedeklenecek klasörleri ve buluttaki yerlerini seç.
  Değişiklikler birkaç saniye içinde yüklenir.
- **Her dosyanın nerede duracağına sen karar verirsin.** Yalnızca bulutta,
  bulutta ve bilgisayarda ya da yalnızca bilgisayarda. Seçmek için dosyaya
  sağ tıkla.
- **Hiçbir şey yanlışlıkla kaybolmaz.** Silinen dosyalar bulutun çöp kutusuna
  gider, kalıcı silinmez. Toplu silmelerde önce onay istenir.
- **KDE'ye uyumlu.** Durumu gösteren tepsi simgesi, yerel ayar penceresi,
  Dolphin sağ tık menüsü ve dosyalarda durum rozetleri.
- **Küçük.** Boştayken yaklaşık 10 MB bellek kullanan tek bir Rust süreci.
- **Türkçe ve İngilizce**, sistem diline göre seçilir.

## Kurulum

Rust, FUSE 3 ve pencere için `qt6-declarative`, `kirigami` ve
`qqc2-desktop-style` gerekir.

```bash
git clone https://github.com/onur-akalan/kdlite.git
cd kdlite
cargo build --release
./install.sh
```

Yönetici yetkisi gerekmez. `install.sh` için isteğe bağlı seçenekler:

| Seçenek | Ne yapar |
|---|---|
| `--autostart` | kdlite'ı oturum açılışında başlatır |
| `--dolphin` | Dolphin'de dosyalara durum rozeti ekler (`cmake` ve `kio` gerekir) |
| `--remove` | Kaldırır; ayarların korunur |

## İlk açılış

Uygulama menüsünden **kdlite**'ı aç. Kısa bir sihirbaz API token'ını sorar,
sürücünü seçtirir ve kurulum biter.

kDrive için token'ı Infomaniak Manager'da *Geliştirici → API token'ları*
bölümünden `drive` yetkisiyle oluştur. Token yalnızca bilgisayarında, yalnızca
senin okuyabildiğin bir dosyada saklanır.

## Günlük kullanım

- **Tepsi simgesi:** tıklayınca pencere açılır. Menüde duraklat, sürdür ve
  *Şimdi yedekle* bulunur.
- **Pencere:** klasör ekle, durumlarını gör, *Ayarlar*'ı aç.
- **Dolphin:** bir dosyaya ya da klasöre sağ tıkla ve **kdlite** menüsünü aç.

### Dosya nerede durur

| Seçim | Ne olur |
|---|---|
| Yalnızca bulutta tut | Dosya bilgisayardan kalkar; yerinde buluttaki kopyayı açan bir kısayol kalır. |
| Bulutta ve bilgisayarda tut | Dosya iki yerde de durur ve eşit tutulur. |
| Yalnızca bilgisayarda tut | Buluttaki kopya bulutun çöp kutusuna gider; dosya artık yedeklenmez. |

## Verilerin nasıl korunur

- kdlite hiçbir şeyi kalıcı olarak silmez. Silinenler bulutun çöp kutusuna
  gider ve geri alınabilir.
- Bir dosya bilgisayardan ancak bulutta birebir aynısı olduğu doğrulandıktan
  sonra kaldırılır.
- Bir seferde 20'den fazla dosya ya da bir klasörün tüm içeriği kaybolursa,
  kdlite buluta dokunmadan önce sorar.
- Düzenlenen dosyaların eski halleri bulutun sürüm geçmişinde kalır.

## Ayarlar

Her şey pencerede *Ayarlar* altında. Aynı değerler
`~/.config/kdlite/config.toml` dosyasında durur.

Yedeklemede atlanacak dosya ve klasörlerin adlarını
`~/.config/kdlite/ignore` dosyasına, `.gitignore` gibi satır satır yaz:

```
node_modules
*.tmp
```

## Komut satırı

```bash
kdlite backup add ~/Documents   # bir klasörü yedekle
kdlite backup run               # değişiklikleri şimdi yükle
kdlite pause                    # yedeklemeyi duraklat
kdlite --help                   # diğer her şey
```

## Henüz desteklenmeyenler

- Bulut klasörünün içinde taşıma ve yeniden adlandırma
- Çift yönlü eşitleme
- kDrive dışındaki sağlayıcılar

## Katkı

Hata bildirimleri ve katkılar memnuniyetle karşılanır. Yeni bir dil eklemek
için `lang/tr.json` dosyasını kopyala, değerleri çevir ve dosyayı
`src/i18n.rs` içine kaydet.

## Lisans

MIT. Ayrıntı için [LICENSE](LICENSE) dosyasına bak.

kdlite bağımsız bir projedir ve Infomaniak ile bağlantılı değildir.
