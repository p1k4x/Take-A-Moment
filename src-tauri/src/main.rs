#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
  // When deployed via Intune the installer runs as SYSTEM, and the post-install
  // ExecShell can inherit that context. WebView2 then tries to write its data
  // directory to the SYSTEM account profile and shows an unwritable-path error.
  // Exit silently instead of attempting to start a GUI as SYSTEM.
  #[cfg(windows)]
  if std::env::var("USERNAME")
    .map(|u| u.eq_ignore_ascii_case("SYSTEM"))
    .unwrap_or(false)
  {
    return;
  }

  #[cfg(target_os = "linux")]
  apply_linux_webkit_workarounds();

  take_a_moment_lib::run()
}

/// Fat Cat plays 1080p VP9-with-alpha WebMs. WebKitGTK's DMA-BUF renderer cannot
/// map that pixel format (GStreamer asserts `fmt != GST_VIDEO_FORMAT_UNKNOWN`),
/// which froze the overlay on Ubuntu 24 Wayland (TMP-4, Intel+NVIDIA hybrid).
///
/// `WEBKIT_DISABLE_DMABUF_RENDERER` is a Linux-wide pragmatic default so X11 and
/// other GPUs get the same freeze fix; `__NV_DISABLE_EXPLICIT_SYNC` is only the
/// NVIDIA+Wayland explicit-sync quirk. Long-term the overlay must work on more
/// than this machine — Intel-only, AMD, discrete NVIDIA, Wayland *and* X11 —
/// and we should narrow this if DMA-BUF is viable. See TMP-12 and TMP-13.
/// tauri-apps/tauri#9394
#[cfg(target_os = "linux")]
fn apply_linux_webkit_workarounds() {
  set_env_if_unset("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
  if std::path::Path::new("/sys/module/nvidia").exists()
    && std::env::var_os("WAYLAND_DISPLAY").is_some()
  {
    set_env_if_unset("__NV_DISABLE_EXPLICIT_SYNC", "1");
  }
}

#[cfg(target_os = "linux")]
fn set_env_if_unset(key: &str, value: &str) {
  if std::env::var_os(key).is_none() {
    std::env::set_var(key, value);
  }
}
