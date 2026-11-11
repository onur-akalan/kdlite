#!/usr/bin/env bash
# Installs kdlite for the current user: the command into ~/.local/bin, the
# application menu entry into ~/.local/share/applications. No root needed.
#   --autostart   start the service in the tray when you log in
#   --dolphin     build and install the Dolphin status badge plugin
#   --remove      uninstall (settings and cache are kept)
# Works from a release archive (prebuilt ./kdlite next to this script) and
# from a source checkout (builds with cargo).
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
    echo "Removed. Settings and cache are kept: ~/.config/kdlite, ~/.cache/kdlite"
    exit 0
fi

mkdir -p "$bin" "$apps"
if [[ -f "$here/kdlite" && -x "$here/kdlite" ]]; then
    # Release archive: copy the prebuilt program, so the extracted folder
    # can be deleted afterwards.
    rm -f "$bin/kdlite"
    install -m 755 "$here/kdlite" "$bin/kdlite"
else
    [[ -x "$here/target/release/kdlite" ]] || (cd "$here" && cargo build --release)
    ln -sf "$here/target/release/kdlite" "$bin/kdlite"
fi
rm -f "$bin/kdlite-tray"  # old Python tray script
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
# Dolphin right-click menu for folders.
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
# For files: only the "where to keep it" choices.
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
chmod +x "$menus/kdlite.desktop"  # KDE ignores menu files that are not executable
echo "Installed: $bin/kdlite, $apps/kdlite.desktop, $menus/kdlite.desktop"

# Things kdlite needs at run time; only a warning, the install is complete.
missing=()
command -v fusermount3 >/dev/null || missing+=("fusermount3 (package: fuse3)")
command -v notify-send >/dev/null || missing+=("notify-send (package: libnotify, on Debian/Ubuntu libnotify-bin)")
if ! command -v qml6 >/dev/null && ! command -v qml-qt6 >/dev/null \
    && [[ ! -x /usr/lib/qt6/bin/qml && ! -x /usr/lib64/qt6/bin/qml ]]; then
    missing+=("qml6 / qml-qt6 (Qt 6 Declarative tools; see README for your distribution)")
fi
if (( ${#missing[@]} )); then
    echo "Not found on this system, kdlite needs them:"
    printf '  - %s\n' "${missing[@]}"
fi
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not in your PATH; the menu entry works, the 'kdlite' command needs it." ;; esac

# Dolphin status badges (needs cmake and the KDE development files).
# The plugin goes into the user's folder; QT_PLUGIN_PATH is added to the
# session environment so Qt finds it (takes effect at the next login).
if [[ " $* " == *" --dolphin "* ]]; then
    cmake -S "$here/dolphin" -B "$here/dolphin/build" >/dev/null
    cmake --build "$here/dolphin/build" --target kdliteoverlay >/dev/null
    mkdir -p "$plugins/kf6/overlayicon" "$(dirname "$envfile")"
    cp "$here/dolphin/build/kdliteoverlay.so" "$plugins/kf6/overlayicon/"
    printf 'export QT_PLUGIN_PATH="%s${QT_PLUGIN_PATH:+:$QT_PLUGIN_PATH}"\n' "$plugins" > "$envfile"
    echo "Dolphin badge plugin installed: log out and back in to see the badges."
fi

if [[ " $* " == *" --autostart "* ]]; then
    mkdir -p "$auto"
    sed "s|^Exec=.*|Exec=$bin/kdlite tray --hidden|" "$apps/kdlite.desktop" > "$auto/kdlite.desktop"
    echo "kdlite will start when you log in: $auto/kdlite.desktop"
fi
