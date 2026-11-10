# kdlite

A lightweight cloud drive client for Linux and KDE Plasma.

kdlite mounts your cloud drive as a regular folder, downloads files only when
you open them, backs up the local folders you choose, and stays out of the way
in the system tray. The whole service runs as a single process using about
10 MB of memory.

**Supported providers:** [Infomaniak kDrive](https://www.infomaniak.com/en/ksuite/kdrive).
The provider layer is small and self-contained, and support for additional
services is planned.

[Türkçe README](README.tr.md)

## Features

- **On-demand mount.** Your drive appears under `~/kDrive-bulut`. Files are
  fetched when opened and kept in a size-limited local cache; large files
  stream and support seeking.
- **Write support.** Save, edit and create folders directly in the mounted
  drive. Edited files become new versions in the provider's version history.
- **Folder backup.** Pick any local folder and choose where it lives in the
  cloud. Changes are detected with inotify and uploaded within seconds; a
  periodic rescan acts as a safety net.
- **Per-file placement.** For every backed-up file, choose *cloud only*,
  *cloud and this computer*, or *this computer only*. Cloud-only files leave
  a shortcut in place and download when opened.
- **Safe deletion.** Nothing is ever deleted permanently. Files removed
  locally or in the mount are moved to the provider's trash, and bulk
  deletions require confirmation.
- **Desktop integration.** System tray icon with live status, a native
  Qt Quick/Kirigami window, Dolphin context-menu actions and status emblems
  on files, desktop notifications.
- **Guided setup.** A first-run wizard connects your account, lists your
  drives and writes the configuration for you.
- **Small footprint.** Written in Rust. One process, roughly 10 MB of memory,
  no measurable CPU use when idle. The window is a separate process that only
  exists while it is open.
- **Localized.** English and Turkish, following the system language.

## Requirements

- Linux with FUSE 3 (`fuse3`)
- Rust toolchain (to build)
- For the window: `qt6-declarative`, `kirigami`, `qqc2-desktop-style`
- For trash and notifications: `glib2` (`gio`), `libnotify` (`notify-send`)
- Optional, for Dolphin status emblems: `cmake`, `kio`

Package names are for Arch Linux; other distributions ship the same
components under similar names.

## Installation

```bash
git clone https://github.com/onur-akalan/kdlite.git
cd kdlite
cargo build --release
./install.sh
```

`install.sh` links the binary into `~/.local/bin`, adds an application menu
entry and installs the Dolphin context menu. Optional flags:

| Flag | Effect |
|---|---|
| `--autostart` | Start kdlite in the tray at login |
| `--dolphin` | Build and install the Dolphin status-emblem plugin |
| `--remove` | Uninstall (settings and cache are kept) |

Nothing is installed system-wide and no root access is required.

## Getting started

Launch **kdlite** from the application menu, or run:

```bash
kdlite tray
```

On first run the setup wizard asks for an API token. For kDrive, create one in
the Infomaniak Manager under *Developer → API tokens* with the `drive` scope.
The token is stored only in `~/.config/kdlite/config.toml`, readable by your
user alone.

After setup:

- **Tray icon:** left-click opens the window; the menu offers pause/resume,
  *Back up now* and a shortcut to the mounted drive.
- **Window:** add folders to back up, choose their place in the cloud, see
  status, and open *Settings*.
- **Dolphin:** right-click a file or folder → **kdlite** to back it up, sync
  it now, or choose where it is kept.

## Where files are kept

| Option | Result |
|---|---|
| Keep in cloud only | The local file is moved to the trash after its cloud copy is verified; a shortcut that opens the cloud file takes its place. |
| Keep in cloud and on this computer | The file exists in both places and stays in sync. |
| Keep on this computer only | The cloud copy is moved to the provider's trash and the file is no longer backed up. |

## Safety model

kdlite is designed so that a bug or a mistake cannot silently destroy data:

- There is no permanent-delete request anywhere in the code. Deletions go to
  the provider's trash and can be restored from there.
- A local file is only removed after an identical copy is confirmed in the
  cloud at that moment.
- If more than 20 files, or everything in a backed-up folder, disappear at
  once (an unmounted disk, an accidental `rm -r`), nothing is deleted in the
  cloud until you confirm.
- Overwritten files remain available through the provider's version history.
- Files written to the mount are staged on disk and retried until uploaded,
  including after a restart.
- Moving and renaming existing cloud items is not supported yet and is
  refused rather than emulated.

## Configuration

Settings are available in the window. They are stored in
`~/.config/kdlite/config.toml`:

| Key | Default | Meaning |
|---|---|---|
| `mountpoint` | `~/kDrive-bulut` | Where the drive is mounted |
| `writable` | `true` | Allow writing to the mounted drive |
| `sync_deletes` | `true` | Propagate local deletions to the cloud trash |
| `cache_max_gb` | `20` | Size limit of the download cache |
| `backup_root` | `Backups/<hostname>` | Default cloud location for backups |
| `rescan_secs` | `3600` | Interval of the full safety rescan |
| `language` | `auto` | Interface language (`auto`, `en`, `tr`) |

Names that should never be backed up are listed one per line in
`~/.config/kdlite/ignore`, in the spirit of `.gitignore` (`node_modules`,
`*.tmp`, `.Trash-*`). Changes apply within seconds.

## Command line

Everything the interface does is also available from the terminal:

```bash
kdlite backup add ~/Documents   # add a folder to the backup list
kdlite backup run               # scan and upload once
kdlite backup verify            # check every local file exists in the cloud
kdlite pause                    # pause / resume the running service
kdlite ls "Photos/2024"         # list a cloud folder without mounting
kdlite --help
```

## How it works

- **Mount:** a FUSE filesystem built on `fuser`. Directory listings are
  cached briefly; file contents are downloaded to `~/.cache/kdlite` on first
  read, with range requests for seeking in large files.
- **Backup:** a one-way uploader that tracks size and modification time per
  file. Large files use chunked upload sessions and are streamed from disk.
- **Service:** `kdlite tray` hosts the mount, the backup engine, the tray
  icon and a local HTTP API bound to `127.0.0.1` that the window and the
  context menu talk to.
- **Providers:** all provider-specific requests are isolated in `src/api.rs`.

## Translations

The source language is English. Translations are JSON files in `lang/`
mapping English text to the translated text (`lang/tr.json`). To add a
language, copy that file, translate the values and register it in
`src/i18n.rs`.

## Roadmap

- Additional storage providers
- Move and rename inside the mounted drive
- Two-way folder sync
- Packaging for common distributions

Contributions and issue reports are welcome.

## Disclaimer

kdlite is an independent project. It is not affiliated with, endorsed by or
supported by Infomaniak Network SA. kDrive is a product of Infomaniak
Network SA.

## License

MIT. See [LICENSE](LICENSE).
