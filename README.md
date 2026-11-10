# kdlite

kDrive için Linux istemcisi. İki işi var:

1. **Bağlama (Lite Sync benzeri):** bulutun tamamı salt okunur bir klasör
   olarak görünür, dosyalar açıldığında iner ve yerel önbellekte kalır.
2. **Yedekleme:** seçilen yerel klasörler tek yönlü olarak kDrive'a akar.

Güvenlik sınırları: bağlama yalnızca GET isteği kullanır ve salt okunurdur.
Yedekleme yalnızca klasör oluşturur ve dosya yükler. Kodda silme, taşıma veya
yeniden adlandırma isteği yoktur; yerel dosyalar yalnızca okunur.

## Kurulum

```bash
cargo build --release
./target/release/kdlite init
```

`~/.config/kdlite/config.toml` içine iki değer yazılır:

- `token`: Infomaniak Manager'da oluşturulan, kDrive yetkili API token'ı
- `drive_id`: web arayüzünde adres çubuğundaki `.../drive/<kimlik>/...` sayısı

Dosyanın izinleri 600 olmalıdır; değilse kdlite çalışmayı reddeder.

## Kullanım

```bash
kdlite ls                      # bağlantı testi: kökü listeler
kdlite ls "03 Fotoğraflar"     # bir alt klasörü listeler
kdlite mount                   # ~/kDrive-bulut altına bağlar, Ctrl-C ile ayrılır
kdlite pin "01 Belgeler"       # klasörü her zaman yerelde tutar (çevrimdışı)
kdlite unpin "01 Belgeler"
kdlite status                  # önbellek kullanımı
```

## Tepsi servisi ve pencere

```bash
./install.sh               # ~/.local/bin/kdlite ve uygulama menüsü kaydı
./install.sh --autostart   # ayrıca oturum açılışında tepside başlat
kdlite tray                # servisi başlatır ve pencereyi açar
kdlite tray --hidden       # yalnızca tepsi
kdlite window              # çalışan servisin penceresini açar
kdlite quit                # servisi kapatır, bağlantıyı ayırır
```

`kdlite tray` tek bir süreçtir: kDrive bağlantısı, otomatik yedekleme, yerel
API (yalnızca 127.0.0.1) ve KDE tepsi simgesi. Boşta yaklaşık 10 MB kullanır.
Bağlantı kurulamazsa (örneğin açılışta ağ yokken) dakikada bir yeniden dener.

Pencere ayrı bir süreçtir (`qml6`, Kirigami ile yerel KDE görünümü), yalnızca
açıkken ~57 MB kullanır. Tepsi simgesine sol tık açar ve kapatır. Gereken
paketler: `qt6-declarative`, `kirigami`, `qqc2-desktop-style`.

Tepsi simgesindeki rozet durumu gösterir: onay (her şey yedekli), eşitleme
(yükleniyor), duraklatma, uyarı (yüklenemeyen dosya var), çevrimdışı (kDrive
bağlı değil). `kdlite pause` / `kdlite resume` ya da tepsi menüsü ve
penceredeki düğme yedeklemeyi duraklatır ve sürdürür; durum yeniden
başlatmada korunur. Duraklatınca o an yüklenen dosya biter, sıradakine
geçilmez.

Dolphin'de bir klasöre sağ tık → **kdlite** altında "Bu klasörü kDrive'a
yedekle" ve "Yedeklemeden çıkar" bulunur (`install.sh` kurar; sonuç masaüstü
bildirimiyle gösterilir).

**Dosya nerede dursun** (Dolphin'de sağ tık → kdlite, ya da penceredeki bulut
düğmesi):

- *Yalnızca bulutta tut*: bulutta birebir durduğu o an doğrulanan yerel dosya
  çöp kutusuna gider (`gio trash`), yerine `~/kDrive-bulut` içindeki dosyayı
  gösteren bir kısayol (sembolik bağ) kalır; açınca buluttan iner.
- *Bulutta ve bilgisayarda tut*: kısayolun yerine dosya geri indirilir;
  yalnızca bilgisayarda olan dosya yeniden yedeklenir.
- *Yalnızca bilgisayarda tut*: buluttaki kopya kDrive çöp kutusuna gider ve
  dosya bir daha yedeklenmez.

**Silme** (`sync_deletes = true`): yedeklenen klasörde silinen dosyanın (ya da
kısayolun) buluttaki kopyası kDrive çöp kutusuna taşınır; oradan geri alınabilir.
Bir seferde 20'den fazla dosya ya da klasördeki her şey silinmiş görünürse
(bağlanmamış disk gibi bir hata olabilir) önce pencerede onay istenir. Özellik
açıldığı ilk taramada, daha önce silinmiş dosyalar buluttan silinmez. Boş kalan
bulut klasörleri silinmez. `~/kDrive-bulut` içinde silinen dosya ve boş klasör
de kDrive çöp kutusuna gider. Kalıcı silme hiçbir yerde yoktur.

**İlk kurulum:** ayar dosyası yokken `kdlite tray` bir sihirbaz açar: token'ı
yapıştırırsın, sürücüyü ve kök klasörü seçersin; `config.toml` 600 izinle
yazılır ve servis kendiliğinden başlar. (`kdlite init` ile elle de yazılabilir.)

**Buluttaki yer:** "Klasör ekle…" yerel klasörden sonra kDrive'daki hedefi
sorar. Seçilen bulut klasörünün içinde yerel klasörün adıyla bir klasör
oluşturulur. "Varsayılan yer" `Yedekler/<bilgisayar adı>` olur. Hedefi
değiştirmek için klasörü listeden çıkarıp yeniden ekle; eski yerdeki kopya
silinmez.

**Dolphin rozetleri:** `./install.sh --dolphin` küçük bir eklenti derleyip
kurar (`dolphin/`, C++; `cmake` ve `kio` gerekir). Yedeklenen klasörlerdeki
dosyaların simgesinde durum görünür: yeşil onay (bulutta), eşitleme (yüklenmeyi
bekliyor), duraklatma, uyarı. Alt klasörlerde ve hariç tutulan dosyalarda rozet
yoktur. Eklenti yalnızca `~/.config/kdlite` altındaki durum dosyalarını okur.
Qt'nin kullanıcı klasöründeki eklentiyi görmesi için betik
`~/.config/plasma-workspace/env/kdlite.sh` dosyasına `QT_PLUGIN_PATH` yazar;
oturumu kapatıp açınca etkin olur.

**Bulut klasörüne yazma** (`writable = true`, varsayılan): `~/kDrive-bulut`
içine dosya kaydedilebilir, var olan dosya düzenlenebilir, klasör
oluşturulabilir. Yazılan dosya önce yerelde bekler (`~/.cache/kdlite/staged`),
kapandıktan 5 saniye sonra yüklenir; düzenlenen dosyanın eski hali kDrive sürüm
geçmişinde kalır. Henüz yüklenmemiş dosya silinebilir ve yeniden
adlandırılabilir; var olan bir dosyanın üzerine taşınırsa (düzenleyicilerin
"geçici dosyaya yaz, üzerine taşı" kaydı) onun yeni sürümü olur. Yedeklenmeyecekler
listesindeki adlar (kilit ve geçici dosyalar) hiç yüklenmez. Buluttaki bir
dosyayı ya da klasörü taşımak, yeniden adlandırmak reddedilir (EPERM).
Yüklenemeyen dosya diskte kalır, dakikada bir ve sonraki açılışta yeniden denenir.

**Ayarlar:** penceredeki dişli düğmesi ayarlar sayfasını açar (otomatik
başlatma, yazma izni, önbellek sınırı, varsayılan yedek yeri, güvenlik taraması).
Kaydedince servis yeni ayarlarla yeniden başlar. "Yedeklenmeyecekler" listesi
`~/.config/kdlite/ignore` dosyasıdır (.gitignore gibi, satır başına bir ad);
"Listeyi düzenle…" onu metin düzenleyicide açar, kaydedilince birkaç saniyede
uygulanır. Her klasörün yanındaki dişli, o klasörün buluttaki yerini değiştirir.

Aynı arayüz tarayıcıdan da açılabilir: `http://127.0.0.1:47800/`.

## Yedekleme

```bash
kdlite backup add ~/Documents          # listeye ekler
kdlite backup add ~/notlar --name Notlar
kdlite backup list
kdlite backup run                      # bir kez tarar ve yükler
kdlite backup watch                    # sürekli: backup_interval_secs'te bir tarar
kdlite backup verify                   # yereldeki her dosya bulutta aynı boyutla var mı
kdlite backup remove Documents         # listeden çıkarır, buluttakine dokunmaz
```

- Servis (`kdlite tray`) yedeklenen klasörleri inotify ile izler: dosya
  oluşturulunca ya da değişince, yazım bittikten birkaç saniye sonra yalnızca
  o yedek taranıp yüklenir. Güvenlik için açılışta ve `rescan_secs` (3600)
  aralığıyla tam tarama yapılır. Hariç tutulan klasörler izlenmez. İzleme
  kurulamazsa `backup_interval_secs` aralığıyla taramaya dönülür.
- Hedef: `Yedekler/<bilgisayar adı>/<ad>` (ayarda `backup_root` ile değişir).
- Değişen dosya aynı adla yeniden
  yüklenir, eski hali kDrive sürüm geçmişinde durur.
- Dosyanın değişiklik zamanı korunur. Durum dosyası kaybolsa bile bulutta
  aynı boyut ve zamanla duran dosyalar yeniden yüklenmez.
- 100 MB üstü dosyalar parçalı yüklenir.
- Atlananlar: `.git`, `node_modules`, `.venv`, `__pycache__`, geçici dosyalar,
  `CACHEDIR.TAG` içeren klasörler (örneğin cargo `target`), sembolik bağlar.
  Liste ayarda `exclude` ile değiştirilebilir.
- Son 3 saniyede değişmiş dosyalar bir sonraki taramaya bırakılır.

## Nasıl çalışır

- Klasör listeleri `dir_ttl_secs` (varsayılan 60 sn) boyunca bellekte tutulur.
- Bir dosya ilk okunduğunda arka planda baştan sona indirilir; okuma, ihtiyaç
  duyduğu kısım gelir gelmez devam eder. İleri sarmada sunucu destekliyorsa
  doğrudan aralık isteği yapılır.
- Önbellek `cache_max_gb` sınırını aşınca en eski kullanılan dosyalar silinir.
  Açık dosyalar ve sabitlenenler silinmez.
- Sabitlenen yollar bağlantı açılınca ve 10 dakikada bir eşitlenir. Diskte
  10 GB'tan az yer kalırsa sabitleme indirmeleri durur.
- İnternet yokken: daha önce listelenmiş klasörler ve önbellekteki dosyalar
  erişilebilir kalır, diğerleri G/Ç hatası verir.

## Bilinen sınırlar

- Bağlanan klasöre yazılamaz (bilerek); buluta veri yalnızca yedeklemeyle gider.
- Yedekleme değişiklikleri anında değil, tarama aralığında fark eder.
- Boş klasörler yedeklenmez.
- Buluttaki değişiklikler en geç `dir_ttl_secs` sonra görünür.
- Klasör listeleri yeniden başlatmalar arasında saklanmaz.

## Lisans

MIT. Ayrıntı için `LICENSE` dosyasına bak.

Bu proje Infomaniak ile bağlantılı değildir; kDrive, Infomaniak Network SA’nın ürünüdür.
