# kdlite

A lightweight cloud drive client for Linux and KDE Plasma.

kdlite shows your cloud drive as a normal folder, downloads files only when
you open them, and backs up the local folders you choose. It runs quietly in
the system tray.

**Supported today:** [Infomaniak kDrive](https://www.infomaniak.com/en/ksuite/kdrive).
More providers are planned.

[Türkçe](README.tr.md)

## What it does

- **Your drive as a folder.** Browse, open, save and edit files in
  `~/kDrive-bulut` (you can change the location in *Settings*). Files
  download on first use.
- **Automatic backup.** Choose folders to back up and where they go in the
  cloud. Changes upload within seconds.
- **You decide where each file lives.** Cloud only, cloud and computer, or
  computer only. Right-click any file to choose.
- **Nothing is lost by accident.** Deleted files go to the cloud trash, never
  straight to oblivion. Large deletions ask for confirmation first.
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
  *Back up now*.
- **Window:** add folders, see their status, open *Settings*. The *Cloud
  folder* section opens your drive and lets you choose cloud folders that
  should also be kept on this computer, so they work without internet.
- **Dolphin:** right-click a file or folder and open the **kdlite** menu.

### Where a file is kept

| Choice | What happens |
|---|---|
| Keep in cloud only | The file leaves your computer; a shortcut stays and opens the cloud copy. |
| Keep in cloud and on this computer | The file is in both places and stays in sync. |
| Keep on this computer only | The cloud copy goes to the cloud trash; the file is no longer backed up. |

## How your data is protected

- kdlite never deletes anything permanently. Deleted items go to the cloud
  trash and can be restored.
- A file is removed from your computer only after an identical copy is
  confirmed in the cloud.
- If more than 20 files, or a whole folder's contents, vanish at once, kdlite
  asks before touching the cloud.
- Older versions of edited files stay in the cloud's version history.

## Settings

Everything is in the window under *Settings*. The same values live in
`~/.config/kdlite/config.toml`.

To skip files and folders during backup, list their names in
`~/.config/kdlite/ignore`, one per line, like a `.gitignore`:

```
node_modules
*.tmp
```

## Command line

```bash
kdlite backup add ~/Documents   # back up a folder
kdlite backup run               # upload changes now
kdlite pause                    # pause backups
kdlite --help                   # everything else
```

## Not supported yet

- Moving and renaming items inside the cloud folder
- Two-way sync
- Providers other than kDrive

## Contributing

Issues and pull requests are welcome. To add a language, copy
`lang/tr.json`, translate the values and register the file in `src/i18n.rs`.

## License

MIT. See [LICENSE](LICENSE).

kdlite is an independent project and is not affiliated with Infomaniak.
