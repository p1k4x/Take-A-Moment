<p align="center">
  <img src="assets/icons/app-full.png" width="96" alt="Take A Moment" />
</p>

<h1 align="center">Take A Moment</h1>

<p align="center">A gentle break-reminder that lives in your system tray.</p>

<p align="center">This repo is a Ubuntu 24 fork of https://github.com/Karlmit/Take-A-Moment</p>

## Features

- Fullscreen break overlay with a soft animation
- Configurable reminders — frequency, duration, message, sounds, volume
- Optional start time per reminder (e.g. first break at 09:00, then every 60 min)
- Themes: Still Garden · Soft Dusk · Morning Mist
- Languages: English · Svenska · Deutsch · Français · Español · Nederlands · Dansk
- No admin required — installs to your user folder
- Native tray/timer host — web UI only starts while Settings or a break overlay is open

## Installation

### Windows (upstream)

Download **Take A Moment Setup x.x.x.exe** from the [latest upstream release](https://github.com/Karlmit/Take-A-Moment/releases/latest) and run it.

The installer is in **Swedish** by default. The app opens settings automatically on first launch so you can change the language right away.

### Linux (this fork)

Build packages on Ubuntu 24 / Mint 22:

```
npm run package
```

Artifacts land in `release/`:

- `Take-A-Moment_x.x.x_amd64.deb`
- `Take-A-Moment_x.x.x_amd64.AppImage`

**Debian package** (uses system WebKitGTK and GStreamer):

```
sudo apt install ./release/Take-A-Moment_0.11.6_amd64.deb
```

Launch from the app menu or `take-a-moment`. Uninstall:

```
sudo apt remove take-a-moment
```

That removes the app. Settings stay in `~/.local/share/app.take-a-moment/`. Autostart, if you enabled it, is `~/.config/autostart/`. Delete those if you want a clean slate.

**AppImage** (GStreamer plugins are bundled):

```
chmod +x ./release/Take-A-Moment_0.11.6_amd64.AppImage
./release/Take-A-Moment_0.11.6_amd64.AppImage
```

Uninstall: quit from the tray, then delete the `.AppImage`. Remove the same settings/autostart paths if you do not want leftovers.

### Silent install

```
(Windows) "Take A Moment Setup 0.5.0.exe" /S
```

Silent install with a specific language:

```
(Windows) "Take A Moment Setup 0.5.0.exe" /S /language=en
```

(Windows) Supported language codes:

| Code | Language   |
|------|------------|
| `sv` | Svenska (default) |
| `en` | English    |
| `de` | Deutsch    |
| `fr` | Français   |
| `es` | Español    |
| `nl` | Nederlands |
| `da` | Dansk      |

(Windows) The `/language` flag sets the app's default language and localises the built-in reminder message. It has no effect after the first launch (settings are stored in `%APPDATA%\take-a-moment`).

(Windows) The app launches automatically after a silent install (e.g. via company portal / Intune user context). It opens the settings window on first run so the user can review their configuration.

## Usage

- **Left-click** the tray icon → open Settings
- **Right-click** the tray icon → quick actions (skip, pause, quit)
- **Preview break** button in Settings → test the overlay immediately

## Development

Windows (upstream-style):

```
git clone https://github.com/p1k4x/Take-A-Moment.git
cd Take-A-Moment
npm install
npm run dev
```

`npm run dev` is the path that has been exercised on this Ubuntu 24 fork
(tray, settings, overlay). It uses the host WebKitGTK and host GStreamer.

### Packaging (`npm run package`)

Same npm script on both OSes; `scripts/post-package.cjs` copies artifacts into
`release/` (that directory is gitignored).

**Windows** (upstream NSIS):

```
npm run package
# Output: release/Take A Moment Setup x.x.x.exe
```

**Linux** (`scripts/package.cjs` builds `.deb` then AppImage):

```
npm run package
# Output: release/Take-A-Moment_x.x.x_amd64.deb
#         release/Take-A-Moment_x.x.x_amd64.AppImage
```

The AppImage sets `bundle.linux.appimage.bundleMediaFramework` so WebKitGTK
has GStreamer plugins inside the image. Fat Cat playback in the AppImage is
[TMP-15](https://pikachurro.atlassian.net/browse/TMP-15) (done; leftover
opaque/black alpha is [TMP-16](https://pikachurro.atlassian.net/browse/TMP-16)).
The `.deb` depends on system WebKitGTK plus `gstreamer1.0-plugins-good` and
`gstreamer1.0-plugins-bad`. Install and uninstall on Ubuntu 24 is
[TMP-3](https://pikachurro.atlassian.net/browse/TMP-3) (done).

The app uses Tauri v2. The idle tray process is native Rust; Settings and the
fullscreen break overlay are Vite/React webviews created on demand.

## Linux / Ubuntu 24 / Linux Mint 22 (this fork)

Upstream is Windows-only. This repository is a fork that is adding Ubuntu 24
support (Linux Mint 22.x uses the same packages). That work is **not finished**.

Linux Mint 22.3 Cinnamon (X11) **builds and runs this tree as-is** — no Mint
fork. Overlay is transparent. Fat Cat can miss the first Preview after a cold
`npm run dev`, then show and play on a later run. See
[docs/ubuntu-24-port.md](docs/ubuntu-24-port.md).

`npm` and `cargo` are **not** on a stock Mint/Ubuntu desktop. Do not run
`sudo apt install npm` — that installs EOL Node 18. Install Node 22 via nvm,
rustup, then Tauri GTK/WebKit packages, then `npm ci`. Full order, package
list, and the errors you hit if you skip a step are in that same doc.
