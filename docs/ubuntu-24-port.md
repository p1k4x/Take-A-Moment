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
| Packaging | `npm run package` builds NSIS on Windows, deb+AppImage on Linux | Scripts copy both Linux artifacts to `release/`. AppImage Fat Cat playback is [TMP-15](https://pikachurro.atlassian.net/browse/TMP-15) (done). `.deb` install/uninstall on Ubuntu 24 is [TMP-3](https://pikachurro.atlassian.net/browse/TMP-3) (done). |
| Tray + overlay | Same Tauri code paths as Windows | Validated on Ubuntu 24 GNOME Wayland ([TMP-4](https://pikachurro.atlassian.net/browse/TMP-4)). Linux Mint 22.3 Cinnamon X11 **builds and launches** with that same code. First Preview after a cold `tauri dev` showed an empty transparent overlay; a later `npm run dev` showed Fat Cat playing with alpha. X11 still [TMP-12](https://pikachurro.atlassian.net/browse/TMP-12); other GPUs [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13). |

Windows behaviour is meant to stay unchanged (`#[cfg(windows)]` paths).

## What this WSL session could not finish

Development started on a Windows host inside WSL. That environment lacked Tauri GTK/WebKit/DBus **dev** packages (`glib-2.0`, `gobject-2.0`, `gdk-3.0`, `cairo`, `pango`, `dbus-1`, …). `cargo check` failed on missing `.pc` files, not on the new Rust logic.

A rustup toolchain was temporarily installed **inside the repo** (`.cargo/`, `.rustup/`) so `cargo` existed at all. Those directories are gitignored (`/.cargo/`, `/.rustup/` so `src-tauri/.cargo/config.toml` stays tracked). Do not copy them to the Ubuntu machine. Install rustup in `$HOME` there.

`npm ci` and `npm run build` succeeded. `npm run dev` / a real tray+overlay session did not.

## Continue on Ubuntu 24.04 or Linux Mint 22.x

Linux Mint 22.3 (Zena, Cinnamon) is Ubuntu 24.04 Noble underneath. The **toolchain** is the same as Ubuntu 24: Node, rustup, then Tauri GTK/WebKit packages, then `npm ci`. Cinnamon does not need a different package list or extra Rust. A Precision M2800 on Mint 22.3 Cinnamon **X11** compiled and launched this tree as-is (`npm run dev` → `target/debug/take-a-moment`). Overlay *behaviour* is not the same as the GNOME Wayland check — that is the remaining product work, not a Mint fork.

A machine with only a desktop install will fail immediately: no `npm`, no `cargo`, and `npm run dev` cannot find the Tauri CLI until `node_modules` exists.

Do the steps **in this order**. Skipping Node or Rust is what produces the errors at the bottom of this section.

### 1. Clone

```bash
git clone https://github.com/p1k4x/Take-A-Moment.git
cd Take-A-Moment
git remote add upstream https://github.com/Karlmit/Take-A-Moment.git
```

`gh repo clone p1k4x/Take-A-Moment` is the same clone. `gh` may set Karlmit as the default GitHub repo because of `upstream`; that does not change `git remote`. From inside the clone, `git remote -v` should show `origin` → p1k4x and `upstream` → Karlmit.

### 2. Node.js (do not use `apt install npm`)

`sudo apt install npm` on Mint/Ubuntu 24 pulls **Node 18** (end-of-life) plus Debian’s **npm 9** and hundreds of `node-*` packages. That satisfies the `npm: not found` hint, but it is the wrong Node for this project. Tauri v2 + Vite 5 want a current Node LTS (22 or 24).

Install [nvm](https://github.com/nvm-sh/nvm) in your home directory, then Node 22 LTS:

```bash
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.7/install.sh | bash
# close the terminal and open a new one, or:
source "$HOME/.nvm/nvm.sh"
nvm install 22
nvm alias default 22
node -v   # v22.x
npm -v
```

If you already ran `sudo apt install npm`, leave those packages for now. After nvm is sourced, `which node` should be under `~/.nvm/`, not `/usr/bin/node`. Re-run `npm ci` with that Node. Removing the distro packages later is optional: `sudo apt remove npm nodejs` (only after nvm works).

Do not run `npm audit fix` as part of setup. It rewrites `package-lock.json` and is not required to build.

### 3. Rust (cargo)

`npm run dev` shells out to `cargo metadata`. Without rustup you get `No such file or directory (os error 2)` for `cargo`.

Install rustup in `$HOME` (not inside the repo):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

The default (`stable`, proceed with 1) is enough. Do not copy WSL `.cargo/` / `.rustup/` directories from an old checkout.

### 4. System packages (Tauri v2)

Same list on Ubuntu 24.04 GNOME and Linux Mint 22.x Cinnamon:

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

Use `libayatana-appindicator3-dev` (not `libappindicator3-dev`). The legacy package conflicts with `libayatana-appindicator3-1`, which both GNOME and Cinnamon already ship. Cinnamon’s panel hosts StatusNotifier/AppIndicator; that is why the tray can work on Mint without a GNOME extension.

`gstreamer1.0-plugins-bad` silences WebKit’s WebVTT encoder warning when Fat Cat WebMs play (no subtitles are used; VP9 decode itself comes from `plugins-good`).

If `cargo check` later complains about missing `.pc` files (`glib-2.0`, `gdk-3.0`, `webkit2gtk-4.1`, `dbus-1`, …), this apt block was skipped or incomplete.

### 5. Project deps and run

`tauri` is not a system binary. It comes from `@tauri-apps/cli` in `node_modules` after install. `npm run dev` before that is `sh: 1: tauri: not found`.

```bash
npm ci
cargo check --manifest-path src-tauri/Cargo.toml
npm run dev
```

`npm ci` needs a committed `package-lock.json` and matches CI. `npm install` also works on a dirty tree.

Linux installer artifacts:

```bash
npm run package
```

Expect copies under `release/` (`Take-A-Moment_*_amd64.deb` and `.AppImage`). Mint can install the Ubuntu `.deb`. `apt remove take-a-moment` uninstalls it; user settings are left in `~/.local/share/app.take-a-moment/`.

AppImage Fat Cat is [TMP-15](https://pikachurro.atlassian.net/browse/TMP-15) (done): `bundleMediaFramework` plus loopback HTTP. Opaque/black leftover is [TMP-16](https://pikachurro.atlassian.net/browse/TMP-16). The `.deb` uses system WebKit/GStreamer (`gstreamer1.0-plugins-good` and `gstreamer1.0-plugins-bad` as package Depends). `apt install` / `apt remove take-a-moment` on Ubuntu 24 is [TMP-3](https://pikachurro.atlassian.net/browse/TMP-3) (done).

`scripts/package.cjs` sets `NO_STRIP=true` (Ubuntu 24 `.relr.dyn` strip crashes) and, when a PATH directory contains unreadable files such as SentinelOne `sentinelctl`, substitutes a symlink farm so linuxdeploy can finish.

### Errors this bootstrap is meant to prevent

| What you see | Cause | Fix |
|---|---|---|
| `Command 'npm' not found` / apt suggests `sudo apt install npm` | Node was never installed | nvm + Node 22, **not** `apt install npm` |
| `sh: 1: tauri: not found` | `npm run dev` before `npm ci` / `npm install` | `npm ci`, then `npm run dev` |
| `failed to run 'cargo metadata' … No such file or directory` | rustup / `cargo` not on `PATH` | rustup, then `source "$HOME/.cargo/env"` |
| `Package glib-2.0 was not found` / similar `.pc` errors | Tauri GTK/WebKit apt packages missing | step 4 |

### Runtime notes (after it actually launches)

[TMP-4](https://pikachurro.atlassian.net/browse/TMP-4) is tray / settings / overlay validation on Ubuntu 24 (done on a GNOME Wayland hybrid). It is not a graphics-format ticket.

#### Linux Mint 22.3 Cinnamon X11 (first run, no extra code)

Same Ubuntu 24 tree, no Mint-specific changes. After rustup + the apt list above, `cargo check` / `npm run dev` succeeded and the debug binary ran. `WEBKIT_DISABLE_DMABUF_RENDERER=1` is already a Linux-wide default, so X11 is supposed to get the same VP9-alpha path as GNOME Wayland.

Observed on Preview break (not a full TMP-12 pass):

- First Preview after the **cold** `tauri dev` (full Rust compile): fullscreen overlay came up under muffin and looked **transparent**, but **Fat Cat did not show**. Theme background felt sluggish to load (shader/theme paint lag in WebKitGTK).
- A later `npm run dev` (incremental, already-built binary): **Fat Cat shows and plays**, with a **transparent** background — same qualitative result as TMP-4 Wayland (`neko2` only; intro still skipped).

Do not treat “no cat” as the Mint baseline. Treat it as: **build/run works as-is; overlay video can miss the first Preview after a cold start, then work.** Idle/lock via cinnamon-screensaver and tray click vs right-click were not the point of this check. Still [TMP-12](https://pikachurro.atlassian.net/browse/TMP-12) / [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13), not a new distro port.

This session used distro Node **18.19.1** (`apt install npm`) plus rustup. That was enough to compile. Prefer nvm Node 22 for a clean machine anyway (step 2).

The Fat Cat overlay freeze found on GNOME Wayland is separate: the clips are 1080p VP9-**with-alpha** so the cat can sit on a transparent background. WebKitGTK’s DMA-BUF renderer cannot map that format (`_dma_fmt_to_dma_drm_fmts` / `GST_VIDEO_FORMAT_UNKNOWN`). The Linux binary sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` for all Linux sessions unless already set, plus `__NV_DISABLE_EXPLICIT_SYNC=1` when NVIDIA + Wayland. That workaround is a pragmatic default. Keeping alpha working on more than this GPU combo, and on X11 as well as Wayland, is [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13) and [TMP-12](https://pikachurro.atlassian.net/browse/TMP-12).

WebKitGTK also cannot start a **second** VP9-alpha pipeline in the same overlay (src-swap shows one stretched opaque frame, then stalls). HTML `loop` and `play()` after `ended` are ignored. On Linux the overlay therefore plays **only** `neko2`, rewinds just before the last frame so it repeats, and still uses the CSS slide-in. The ~11s `neko1` intro is skipped until [TMP-14](https://pikachurro.atlassian.net/browse/TMP-14). Intro→loop handover stays on Windows. Mint X11 reached that `neko2` loop on a later `npm run dev`; the first cold Preview did not.

## Suggested next checks (TMP)

1. [TMP-8](https://pikachurro.atlassian.net/browse/TMP-8) / [TMP-9](https://pikachurro.atlassian.net/browse/TMP-9) — install deps, `cargo check` on Linux
2. [TMP-4](https://pikachurro.atlassian.net/browse/TMP-4) — tray, settings, overlay on this Wayland hybrid (done); [TMP-12](https://pikachurro.atlassian.net/browse/TMP-12) X11 (Mint 22.3 Cinnamon: cat+alpha on a later `npm run dev`; first cold Preview was empty/sluggish — not closed); [TMP-13](https://pikachurro.atlassian.net/browse/TMP-13) other GPUs; [TMP-14](https://pikachurro.atlassian.net/browse/TMP-14) Fat Cat intro→loop
3. [TMP-11](https://pikachurro.atlassian.net/browse/TMP-11) / [TMP-5](https://pikachurro.atlassian.net/browse/TMP-5) / [TMP-10](https://pikachurro.atlassian.net/browse/TMP-10) — idle, lock/unlock, lock-after-break
4. [TMP-6](https://pikachurro.atlassian.net/browse/TMP-6) / [TMP-7](https://pikachurro.atlassian.net/browse/TMP-7) — media and camera/mic (replace the `/proc` scan if it is noisy)
5. [TMP-3](https://pikachurro.atlassian.net/browse/TMP-3) — `.deb` install/uninstall on Ubuntu 24 (done)
6. [TMP-15](https://pikachurro.atlassian.net/browse/TMP-15) — AppImage Fat Cat playback (done); leftover alpha is [TMP-16](https://pikachurro.atlassian.net/browse/TMP-16)

## After Ubuntu 24 is stable (long-term)

Do not expand [TMP-1](https://pikachurro.atlassian.net/browse/TMP-1) for this. File a new epic only once the Ubuntu 24 port is actually stable.

Linux stays a **tray host**, same as Windows: idle process, Settings and overlay on demand. There is no window-only mode for WSL or for sessions without a panel. WSL can build and edit; it cannot run the product (no StatusNotifier/AppIndicator host). Optional later hardening is “log and keep going if tray creation fails,” not a second UX.

Intended follow-on targets, in order:

1. **Linux Mint (Cinnamon)** — X11 by default, native panel tray. Mint 22.3 already **compiled and launched** the Ubuntu 24 tree with no extra code. Fat Cat + transparent background worked on a later `npm run dev`; first cold Preview was empty. Remaining: first-preview reliability, tray click vs menu, `loginctl` idle/lock via cinnamon-screensaver. Mint can use the Ubuntu `.deb`.
2. **Fedora Cinnamon** — same desktop, different distro. Re-check packages, AppImage (no `.deb`), and idle/lock/overlay again.

When that epic is opened, split tickets from what Ubuntu 24 already proved. Do not re-spec idle, lock, or overlay as unknown work.
