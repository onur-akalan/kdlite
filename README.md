# kdlite

A lightweight cloud drive client for Linux and KDE Plasma.

kdlite keeps the folders you choose in sync with your cloud drive, lets you
decide file by file what takes up space on your computer, and shows the whole
drive as a normal folder. It runs quietly in the system tray.

**Supported today:** [Infomaniak kDrive](https://www.infomaniak.com/en/ksuite/kdrive).
More providers are planned.

[Türkçe](README.tr.md)

## What it does

- **Two-way sync for the folders you pick.** Add a folder from your computer,
  or bring a cloud folder to your computer. Local changes upload within
  seconds; changes made in the cloud (phone, web, another computer) arrive
  within minutes.
- **You decide where each file lives.** Cloud only, cloud and computer, or
  computer only. Right-click any file or folder to choose.
- **Your whole drive as a folder.** Browse, open, save, move and rename
  anything in `~/kDrive-bulut` (you can change the location in *Settings*),
  including folders you do not sync. Files download on first use.
- **Nothing is lost by accident.** Deleted files go to the trash, on the side
  where they are removed, never straight to oblivion. Large deletions ask for
  confirmation first. If a file changed on both sides, both versions are kept.
- **Fits into KDE.** Tray icon with status, a native settings window, Dolphin
  right-click menu and status badges on files.
- **Small.** A single Rust process using around 10 MB of memory when idle.
- **English and Turkish**, chosen from your system language.

## Install

kdlite runs on any x86_64 Linux distribution with KDE Plasma 6. It was
tested on Arch Linux, Fedora 45 and Debian 13.

**1. Install what kdlite needs** (most of it is already there on a Plasma
desktop):

| Distribution | Command |
|---|---|
| Arch, Manjaro | `sudo pacman -S fuse3 libnotify qt6-declarative kirigami qqc2-desktop-style` |
| Fedora | `sudo dnf install fuse3 libnotify qt6-qtdeclarative-devel kf6-kirigami kf6-qqc2-desktop-style` |
| Debian, Ubuntu | `sudo apt install fuse3 libnotify-bin qml-qt6 qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-dialogs qml6-module-org-kde-kirigami qml6-module-org-kde-desktop` |

**2. Download and install kdlite.** Get the latest
`kdlite-…-x86_64-linux.tar.gz` from the
[releases page](https://github.com/onur-akalan/kdlite/releases), then:

```bash
tar xzf kdlite-*-x86_64-linux.tar.gz
cd kdlite-*-x86_64-linux
./install.sh
```

No root access is needed; everything goes into your home folder. Optional
flags for `install.sh`:

| Flag | What it does |
|---|---|
| `--autostart` | Start kdlite when you log in |
| `--dolphin` | Add status badges to files in Dolphin (builds a small plugin, see below) |
| `--remove` | Uninstall; your settings are kept |

The badge plugin is built on your computer and needs a compiler and the KDE
development files:

| Distribution | Packages |
|---|---|
| Arch, Manjaro | `cmake gcc kio` |
| Fedora | `cmake gcc-c++ kf6-kio-devel` |
| Debian, Ubuntu | `cmake g++ libkf6kio-dev` |

Badges appear after you log out and back in.

### Build from source

You need Rust in addition to the packages above.

```bash
git clone https://github.com/onur-akalan/kdlite.git
cd kdlite
cargo build --release
./install.sh
```

## First run

Open **kdlite** from the application menu. A short wizard asks for an API
token, lets you pick your drive, and you are done.

For kDrive, create the token in the Infomaniak Manager under
*Developer → API tokens* with the `drive` scope. It is stored only on your
computer, in a file only you can read.

## Everyday use

- **Tray icon:** click to open the window. The menu has pause, resume and
  *Sync now*.
- **Window:** add folders, see their status, open *Settings*. *Add folder…*
  syncs a folder from your computer; *Add from cloud…* brings a cloud folder
  to your computer, where its files start as cloud-only shortcuts.
- **Dolphin:** right-click a file or folder and open the **kdlite** menu.

### Where a file is kept

| Choice | What happens |
|---|---|
| Keep in cloud only | The file leaves your computer; a shortcut stays and opens the cloud copy. |
| Keep in cloud and on this computer | The file is in both places and stays in sync. |
| Keep on this computer only | The cloud copy goes to the cloud trash; the file is no longer synced. |

The same three choices work in every synced folder, and each file and folder
shows its state as a small badge in Dolphin.

### How sync works

| What happens | Result |
|---|---|
| You change or add a file on this computer | It uploads within seconds. |
| You delete a file on this computer | The cloud copy goes to the cloud trash. |
| A file changes in the cloud | The new version is downloaded. |
| A file is added in the cloud | It appears here as a cloud-only shortcut. |
| A file is moved or renamed in the cloud | It is moved or renamed here too. |
| A file is deleted in the cloud | The copy here goes to your desktop trash. |
| A file changed on both sides | Both are kept; yours gets “conflicting copy” in its name. |

kdlite looks at the cloud every 10 minutes (changeable in *Settings*) and
when you press *Sync now*.

## How your data is protected

- kdlite never deletes anything permanently. Deleted items go to the cloud
  trash and can be restored.
- A file is removed from your computer only after an identical copy is
  confirmed in the cloud.
- If more than 20 files, or a whole folder's contents, vanish at once on
  either side, kdlite asks before removing them on the other side.
- Older versions of edited files stay in the cloud's version history.

## Settings

Everything is in the window under *Settings*. The same values live in
`~/.config/kdlite/config.toml`.

To skip files and folders during sync, list their names in
`~/.config/kdlite/ignore`, one per line, like a `.gitignore`:

```
node_modules
*.tmp
```

## Updates

kdlite checks GitHub for a new release once a day. When there is one, you
get a notification and an *Update* button appears at the top of the window;
it shows the new version, its release date and its release notes. Nothing is
installed until you press *Update*. kdlite then downloads the release,
checks its SHA-256 checksum, installs it and restarts itself once no sync is
running. Your folders and settings are kept.

To stop the checks, turn off *Tell me when a new version is out* in
*Settings*. From the command line: `kdlite update` (or `kdlite update --check`
to only look). Installs built from source are not updated; use `git pull` and
`./install.sh`.

## Command line

```bash
kdlite backup add ~/Documents   # sync a folder
kdlite backup run               # sync now
kdlite pause                    # pause syncing
kdlite update                   # install a new version
kdlite --help                   # everything else
```

## Not supported yet

- Providers other than kDrive
- Instant notice of cloud changes (they are picked up at the next check)

## Contributing

Issues and pull requests are welcome. To add a language, copy
`lang/tr.json`, translate the values and register the file in `src/i18n.rs`.

## License

MIT. See [LICENSE](LICENSE).

kdlite is an independent project and is not affiliated with Infomaniak.
