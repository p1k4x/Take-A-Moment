# Ubuntu 24 port journey

This fork (`p1k4x/Take-A-Moment`) is an in-progress port of [Karlmit/Take-A-Moment](https://github.com/Karlmit/Take-A-Moment) from Windows-only to Ubuntu 24.

The original app is a Tauri v2 tray host (Rust) with React/Vite webviews for Settings and the fullscreen break overlay. Most of the UI is already cross-platform. The Windows-only parts are OS integration and NSIS packaging.

Work is tracked in Jira project **TMP** ([epic TMP-1](https://pikachurro.atlassian.net/browse/TMP-1)).

## Why a fork

The upstream repo is Windows-only (NSIS installer, Win32/WinRT APIs). A fork lets this Linux work live independently. If the upstream owner wants it later, it can still be offered as a PR from `origin` against `upstream`.

Remotes in this clone:

```
origin    https://github.com/p1k4x/Take-A-Moment.git
upstream  https://github.com/Karlmit/Take-A-Moment.git
```

Push Linux work to `origin`. Do not push to `upstream` unless opening a contribution back to Karlmit.

## What landed in the first commit

Commit `2543b02` is a starting slice, not a finished Linux product.

| Area | Linux approach | Status |
|---|---|---|
| Windows crate deps | Already gated with `cfg(windows)` in `Cargo.toml` | OK |
| Idle detection | `loginctl show-session $XDG_SESSION_ID` (`IdleHint`, `IdleSinceHint`) | Implemented, untested on Ubuntu 24 |
| Session lock/unlock | Poll `LockedHint` every 2s | Implemented, untested |
| Lock PC after long break | `loginctl lock-session` | Implemented, untested |
| Pause/resume media | `playerctl` (MPRIS) | Implemented, untested; no-ops if missing |
| Camera/mic in use | Scan `/proc/*/fd` for `/dev/video*`, `/dev/snd/`, PipeWire/Pulse sockets | Rough; likely false positives |
| Packaging | `npm run package` builds NSIS on Windows, deb+AppImage on Linux | Configured, not produced in WSL |
| Tray + overlay | Same Tauri code paths as Windows | Frontend `npm run build` works; full `tauri dev` not run on Ubuntu 24 yet |

Windows behaviour is meant to stay unchanged (`#[cfg(windows)]` paths).

## What this WSL session could not finish

Development started on a Windows host inside WSL. That environment lacked Tauri GTK/WebKit/DBus **dev** packages (`glib-2.0`, `gobject-2.0`, `gdk-3.0`, `cairo`, `pango`, `dbus-1`, …). `cargo check` failed on missing `.pc` files, not on the new Rust logic.

A rustup toolchain was temporarily installed **inside the repo** (`.cargo/`, `.rustup/`) so `cargo` existed at all. Those directories are gitignored (`/.cargo/`, `/.rustup/` so `src-tauri/.cargo/config.toml` stays tracked). Do not copy them to the Ubuntu machine. Install rustup in `$HOME` there.

`npm ci` and `npm run build` succeeded. `npm run dev` / a real tray+overlay session did not.

## Continue on a native Ubuntu 24 host

```bash
git clone https://github.com/p1k4x/Take-A-Moment.git
cd Take-A-Moment
git remote add upstream https://github.com/Karlmit/Take-A-Moment.git
```

System packages (Tauri v2 on Ubuntu 24):

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libdbus-1-dev \
  pkg-config \
  build-essential \
  curl \
  wget \
  file \
  libssl-dev \
  libxdo-dev \
  playerctl \
  gstreamer1.0-plugins-bad
```

On Ubuntu 24, use `libayatana-appindicator3-dev` (not `libappindicator3-dev`). The legacy package conflicts with `libayatana-appindicator3-1`, which GNOME already ships.

`gstreamer1.0-plugins-bad` silences WebKit’s WebVTT encoder warning when Fat Cat WebMs play (no subtitles are used; VP9 decode itself comes from `plugins-good`).

The Fat Cat clips are 1080p VP9-with-alpha. WebKitGTK’s DMA-BUF renderer cannot map that format and can freeze the overlay (`_dma_fmt_to_dma_drm_fmts` / `GST_VIDEO_FORMAT_UNKNOWN`). [TMP-4](https://pikachurro.atlassian.net/browse/TMP-4) found this on an Intel+NVIDIA hybrid under GNOME Wayland. The Linux binary sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` for all Linux sessions (Wayland and X11) unless already set, plus `__NV_DISABLE_EXPLICIT_SYNC=1` when NVIDIA + Wayland. That is a pragmatic default, not the target: overlay must stay usable beyond this GPU combo, on Wayland **and** X11 ([TMP-12](https://pikachurro.atlassian.net/browse/TMP-12), [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13)).

Rust in your home directory:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Then:

```bash
npm ci
cargo check --manifest-path src-tauri/Cargo.toml
npm run dev
```

Linux installer artifacts:

```bash
npm run package
```

Expect bundles under `src-tauri/target/release/bundle/` (`deb` and `appimage`).

## Suggested next checks (TMP)

1. [TMP-8](https://pikachurro.atlassian.net/browse/TMP-8) / [TMP-9](https://pikachurro.atlassian.net/browse/TMP-9) — install deps, `cargo check` on Linux
2. [TMP-4](https://pikachurro.atlassian.net/browse/TMP-4) — tray, settings, overlay on this Wayland hybrid (done); [TMP-12](https://pikachurro.atlassian.net/browse/TMP-12) X11; [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13) other GPUs
3. [TMP-11](https://pikachurro.atlassian.net/browse/TMP-11) / [TMP-5](https://pikachurro.atlassian.net/browse/TMP-5) / [TMP-10](https://pikachurro.atlassian.net/browse/TMP-10) — idle, lock/unlock, lock-after-break
4. [TMP-6](https://pikachurro.atlassian.net/browse/TMP-6) / [TMP-7](https://pikachurro.atlassian.net/browse/TMP-7) — media and camera/mic (replace the `/proc` scan if it is noisy)
5. [TMP-3](https://pikachurro.atlassian.net/browse/TMP-3) — install/uninstall a `.deb` on Ubuntu 24
