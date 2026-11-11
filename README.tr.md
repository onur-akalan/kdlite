# kdlite

Linux ve KDE Plasma için hafif bir bulut sürücü istemcisi.

kdlite seçtiğin klasörleri bulut sürücünle eşit tutar, bilgisayarında neyin
yer kaplayacağına dosya dosya senin karar vermeni sağlar ve sürücünün
tamamını normal bir klasör gibi gösterir. Sistem tepsisinde sessizce çalışır.

**Şu an desteklenen:** [Infomaniak kDrive](https://www.infomaniak.com/en/ksuite/kdrive).
Başka sağlayıcılar planlanıyor.

[English](README.md)

## Ne yapar

- **Seçtiğin klasörlerde çift yönlü eşitleme.** Bilgisayarından bir klasör
  ekle ya da buluttaki bir klasörü bilgisayarına getir. Bilgisayardaki
  değişiklikler birkaç saniyede yüklenir; bulutta yapılan değişiklikler
  (telefon, web, başka bilgisayar) birkaç dakika içinde gelir.
- **Her dosyanın nerede duracağına sen karar verirsin.** Yalnızca bulutta,
  bulutta ve bilgisayarda ya da yalnızca bilgisayarda. Seçmek için dosyaya
  ya da klasöre sağ tıkla.
- **Sürücünün tamamı bir klasör olarak.** `~/kDrive-bulut` içinde her şeye
  göz at, aç, kaydet, taşı ve adını değiştir (yerini *Ayarlar*'dan
  değiştirebilirsin); eşitlemediğin klasörler de orada. Dosyalar ilk
  kullanımda iner.
- **Hiçbir şey yanlışlıkla kaybolmaz.** Silinen dosyalar, kaldırıldıkları
  tarafın çöp kutusuna gider, kalıcı silinmez. Toplu silmelerde önce onay
  istenir. Bir dosya iki tarafta da değiştiyse iki hali de saklanır.
- **KDE'ye uyumlu.** Durumu gösteren tepsi simgesi, yerel ayar penceresi,
  Dolphin sağ tık menüsü ve dosyalarda durum rozetleri.
- **Küçük.** Boştayken yaklaşık 10 MB bellek kullanan tek bir Rust süreci.
- **Türkçe ve İngilizce**, sistem diline göre seçilir.

## Kurulum

kdlite, KDE Plasma 6 kurulu her x86_64 Linux dağıtımında çalışır. Arch
Linux, Fedora 45 ve Debian 13 üzerinde denendi.

**1. kdlite'ın ihtiyaç duyduklarını kur** (çoğu Plasma masaüstünde zaten
vardır):

| Dağıtım | Komut |
|---|---|
| Arch, Manjaro | `sudo pacman -S fuse3 libnotify qt6-declarative kirigami qqc2-desktop-style` |
| Fedora | `sudo dnf install fuse3 libnotify qt6-qtdeclarative-devel kf6-kirigami kf6-qqc2-desktop-style` |
| Debian, Ubuntu | `sudo apt install fuse3 libnotify-bin qml-qt6 qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-dialogs qml6-module-org-kde-kirigami qml6-module-org-kde-desktop` |

**2. kdlite'ı indir ve kur.**
[Sürümler sayfasından](https://github.com/onur-akalan/kdlite/releases) en son
`kdlite-…-x86_64-linux.tar.gz` dosyasını indir, sonra:

```bash
tar xzf kdlite-*-x86_64-linux.tar.gz
cd kdlite-*-x86_64-linux
./install.sh
```

Yönetici yetkisi gerekmez; her şey ev klasörüne kurulur. `install.sh` için
isteğe bağlı seçenekler:

| Seçenek | Ne yapar |
|---|---|
| `--autostart` | kdlite'ı oturum açılışında başlatır |
| `--dolphin` | Dolphin'de dosyalara durum rozeti ekler (küçük bir eklenti derler, aşağıya bak) |
| `--remove` | Kaldırır; ayarların korunur |

Rozet eklentisi senin bilgisayarında derlenir; bir derleyici ve KDE
geliştirme dosyaları gerekir:

| Dağıtım | Paketler |
|---|---|
| Arch, Manjaro | `cmake gcc kio` |
| Fedora | `cmake gcc-c++ kf6-kio-devel` |
| Debian, Ubuntu | `cmake g++ libkf6kio-dev` |

Rozetler, oturumu kapatıp açtıktan sonra görünür.

### Kaynaktan derleme

Yukarıdaki paketlere ek olarak Rust gerekir.

```bash
git clone https://github.com/onur-akalan/kdlite.git
cd kdlite
cargo build --release
./install.sh
```

## İlk açılış

Uygulama menüsünden **kdlite**'ı aç. Kısa bir sihirbaz API token'ını sorar,
sürücünü seçtirir ve kurulum biter.

kDrive için token'ı Infomaniak Manager'da *Geliştirici → API token'ları*
bölümünden `drive` yetkisiyle oluştur. Token yalnızca bilgisayarında, yalnızca
senin okuyabildiğin bir dosyada saklanır.

## Günlük kullanım

- **Tepsi simgesi:** tıklayınca pencere açılır. Menüde duraklat, sürdür ve
  *Şimdi eşitle* bulunur.
- **Pencere:** klasör ekle, durumlarını gör, *Ayarlar*'ı aç. *Klasör ekle…*
  bilgisayarındaki bir klasörü eşitler; *Buluttan ekle…* buluttaki bir
  klasörü bilgisayarına getirir, içindekiler başta yalnızca bulutta duran
  kısayollar olur.
- **Dolphin:** bir dosyaya ya da klasöre sağ tıkla ve **kdlite** menüsünü aç.

### Dosya nerede durur

| Seçim | Ne olur |
|---|---|
| Yalnızca bulutta tut | Dosya bilgisayardan kalkar; yerinde buluttaki kopyayı açan bir kısayol kalır. |
| Bulutta ve bilgisayarda tut | Dosya iki yerde de durur ve eşit tutulur. |
| Yalnızca bilgisayarda tut | Buluttaki kopya bulutun çöp kutusuna gider; dosya artık eşitlenmez. |

Aynı üç seçenek eşitlenen her klasörde çalışır; her dosya ve klasör durumunu
Dolphin'de küçük bir rozetle gösterir.

### Eşitleme nasıl çalışır

| Ne olur | Sonuç |
|---|---|
| Bilgisayarda bir dosyayı değiştirir ya da eklersin | Birkaç saniyede yüklenir. |
| Bilgisayarda bir dosyayı silersin | Buluttaki kopya bulutun çöp kutusuna gider. |
| Bulutta bir dosya değişir | Yeni hali indirilir. |
| Buluta bir dosya eklenir | Burada yalnızca bulutta duran bir kısayol olarak görünür. |
| Bulutta bir dosya taşınır ya da adı değişir | Burada da taşınır ya da adı değişir. |
| Bulutta bir dosya silinir | Buradaki kopya masaüstü çöp kutusuna gider. |
| Dosya iki tarafta da değişti | İkisi de saklanır; seninkinin adına “çakışan kopya” eklenir. |

kdlite buluta 10 dakikada bir (*Ayarlar*'dan değiştirilebilir) ve *Şimdi
eşitle*'ye bastığında bakar.

## Verilerin nasıl korunur

- kdlite hiçbir şeyi kalıcı olarak silmez. Silinenler bulutun çöp kutusuna
  gider ve geri alınabilir.
- Bir dosya bilgisayardan ancak bulutta birebir aynısı olduğu doğrulandıktan
  sonra kaldırılır.
- Bir tarafta bir seferde 20'den fazla dosya ya da bir klasörün tüm içeriği
  kaybolursa, kdlite diğer taraftan kaldırmadan önce sorar.
- Düzenlenen dosyaların eski halleri bulutun sürüm geçmişinde kalır.

## Ayarlar

Her şey pencerede *Ayarlar* altında. Aynı değerler
`~/.config/kdlite/config.toml` dosyasında durur.

Eşitlemede atlanacak dosya ve klasörlerin adlarını
`~/.config/kdlite/ignore` dosyasına, `.gitignore` gibi satır satır yaz:

```
node_modules
*.tmp
```

## Komut satırı

```bash
kdlite backup add ~/Documents   # bir klasörü eşitle
kdlite backup run               # şimdi eşitle
kdlite pause                    # eşitlemeyi duraklat
kdlite --help                   # diğer her şey
```

## Henüz desteklenmeyenler

- kDrive dışındaki sağlayıcılar
- Buluttaki değişikliğin anında fark edilmesi (bir sonraki kontrolde alınır)

## Katkı

Hata bildirimleri ve katkılar memnuniyetle karşılanır. Yeni bir dil eklemek
için `lang/tr.json` dosyasını kopyala, değerleri çevir ve dosyayı
`src/i18n.rs` içine kaydet.

## Lisans

MIT. Ayrıntı için [LICENSE](LICENSE) dosyasına bak.

kdlite bağımsız bir projedir ve Infomaniak ile bağlantılı değildir.
