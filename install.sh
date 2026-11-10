#!/usr/bin/env bash
# kdlite'i kullanici hesabina kurar: komutlar ~/.local/bin altina, uygulama
# menusu kaydi ~/.local/share/applications altina. Kaldirmak icin: ./install.sh --remove
#   --autostart   oturum acilisinda servisi tepside (penceresiz) baslat
#   --dolphin     Dolphin durum rozeti eklentisini derle ve kur
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
auto="$HOME/.config/autostart"
menus="$HOME/.local/share/kio/servicemenus"
plugins="$HOME/.local/lib/qt6/plugins"
envfile="$HOME/.config/plasma-workspace/env/kdlite.sh"

if [[ "${1:-}" == "--remove" ]]; then
    "$bin/kdlite" quit 2>/dev/null || true
    rm -f "$bin/kdlite" "$bin/kdlite-tray" "$apps/kdlite.desktop" "$auto/kdlite.desktop" "$auto/kdlite-tray.desktop" "$menus/kdlite.desktop" "$menus/kdlite-files.desktop" \
        "$plugins/kf6/overlayicon/kdliteoverlay.so" "$envfile"
    echo "Kaldirildi (ayarlar ve onbellek yerinde: ~/.config/kdlite, ~/.cache/kdlite)."
    exit 0
fi

[[ -x "$here/target/release/kdlite" ]] || (cd "$here" && cargo build --release)
mkdir -p "$bin" "$apps"
ln -sf "$here/target/release/kdlite" "$bin/kdlite"
rm -f "$bin/kdlite-tray"  # eski Python tepsi betigi
cat > "$apps/kdlite.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=kdlite
GenericName=Cloud drive client
GenericName[tr]=Bulut sürücü istemcisi
Comment=Mounts your cloud drive and backs up selected folders
Comment[tr]=Bulut sürücünü bağlar ve seçilen klasörleri yedekler
Exec=$bin/kdlite tray
Icon=folder-cloud
Categories=Utility;Network;
StartupNotify=false
DESKTOP
# Dolphin sag tik menusu: klasoru yedege ekle / yedekten cikar.
mkdir -p "$menus"
cat > "$menus/kdlite.desktop" <<DESKTOP
[Desktop Entry]
Type=Service
MimeType=inode/directory;
Actions=add;sync;remove;_SEPARATOR_;cloud;both;local;
X-KDE-Submenu=kdlite
Icon=folder-cloud

[Desktop Action add]
Name=Back up this folder
Name[tr]=Bu klasörü yedekle
Icon=cloud-upload
Exec=$bin/kdlite menu add %f

[Desktop Action sync]
Name=Back up now
Name[tr]=Şimdi yedekle
Icon=view-refresh
Exec=$bin/kdlite menu sync %f

[Desktop Action remove]
Name=Stop backing up
Name[tr]=Yedeklemeden çıkar
Icon=list-remove
Exec=$bin/kdlite menu remove %f

[Desktop Action cloud]
Name=Keep in cloud only
Name[tr]=Yalnızca bulutta tut
Icon=cloudstatus
Exec=$bin/kdlite menu cloud %F

[Desktop Action both]
Name=Keep in cloud and on this computer
Name[tr]=Bulutta ve bilgisayarda tut
Icon=folder-cloud
Exec=$bin/kdlite menu both %F

[Desktop Action local]
Name=Keep on this computer only
Name[tr]=Yalnızca bilgisayarda tut
Icon=computer
Exec=$bin/kdlite menu local %F
DESKTOP
# Dosyalar icin: yalnizca konum secenekleri.
cat > "$menus/kdlite-files.desktop" <<DESKTOP
[Desktop Entry]
Type=Service
MimeType=application/octet-stream;
Actions=cloud;both;local;
X-KDE-Submenu=kdlite
Icon=folder-cloud

[Desktop Action cloud]
Name=Keep in cloud only
Name[tr]=Yalnızca bulutta tut
Icon=cloudstatus
Exec=$bin/kdlite menu cloud %F

[Desktop Action both]
Name=Keep in cloud and on this computer
Name[tr]=Bulutta ve bilgisayarda tut
Icon=folder-cloud
Exec=$bin/kdlite menu both %F

[Desktop Action local]
Name=Keep on this computer only
Name[tr]=Yalnızca bilgisayarda tut
Icon=computer
Exec=$bin/kdlite menu local %F
DESKTOP
chmod +x "$menus/kdlite-files.desktop"
chmod +x "$menus/kdlite.desktop"  # KDE calistirilabilir olmayan menu dosyalarini yok sayar
echo "Kuruldu: $bin/kdlite, $apps/kdlite.desktop, $menus/kdlite.desktop"

# Dolphin durum rozetleri (cmake ve KDE gelistirme dosyalari gerekir).
# Eklenti kullanici klasorune kurulur; Qt'nin orayi gormesi icin oturum
# ortamina QT_PLUGIN_PATH eklenir (bir sonraki oturum acilisinda etkin olur).
if [[ " $* " == *" --dolphin "* ]]; then
    cmake -S "$here/dolphin" -B "$here/dolphin/build" >/dev/null
    cmake --build "$here/dolphin/build" --target kdliteoverlay >/dev/null
    mkdir -p "$plugins/kf6/overlayicon" "$(dirname "$envfile")"
    cp "$here/dolphin/build/kdliteoverlay.so" "$plugins/kf6/overlayicon/"
    printf 'export QT_PLUGIN_PATH="%s${QT_PLUGIN_PATH:+:$QT_PLUGIN_PATH}"\n' "$plugins" > "$envfile"
    echo "Dolphin rozet eklentisi kuruldu: oturumu kapatip acinca etkin olur."
fi

if [[ " $* " == *" --autostart "* ]]; then
    mkdir -p "$auto"
    sed "s|^Exec=.*|Exec=$bin/kdlite tray --hidden|" "$apps/kdlite.desktop" > "$auto/kdlite.desktop"
    echo "Oturum acilisinda baslayacak: $auto/kdlite.desktop"
fi
