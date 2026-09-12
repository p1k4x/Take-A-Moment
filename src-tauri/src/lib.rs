use chrono::{DateTime, Local, Timelike};
use serde::{Deserialize, Serialize};
use std::{
  collections::{HashMap, HashSet},
  fs,
  path::PathBuf,
  process::Command,
  sync::{Arc, Condvar, Mutex},
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{
  image::Image,
  menu::{Menu, MenuItem, PredefinedMenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
  AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
  WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

#[cfg(target_os = "linux")]
mod video_server;

const STATUS_CHANGED: &str = "timer:status-changed";
const SETTINGS_CHANGED: &str = "settings:changed";
const BREAK_START: &str = "break:start";
const BREAK_PLAY: &str = "break:play";
const BREAK_END: &str = "break:end";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reminder {
  id: String,
  label: String,
  frequency_minutes: u32,
  duration_minutes: u32,
  message: String,
  sound_start: String,
  sound_end: String,
  enabled: bool,
  skip_on_idle: bool,
  skip_on_media: bool,
  volume: u32,
  start_time: Option<String>,
  #[serde(default)]
  end_time: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
  reminders: Vec<Reminder>,
  theme: String,
  language: String,
  idle_threshold_minutes: u32,
  pause_music_on_break: bool,
  launch_on_startup: bool,
  postpone_minutes: u32,
  cover_all_displays: bool,
  time_format: String,
  break_background: String,
  #[serde(default)]
  paused: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveBreak {
  reminder_id: String,
  reminder: Reminder,
  started_at: u64,
  ends_at: u64,
  postpone_count: u32,
  #[serde(default)]
  is_preview: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NextBreak {
  reminder_id: String,
  label: String,
  scheduled_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimerStatus {
  paused: bool,
  next_break: Option<NextBreak>,
  active_break: Option<ActiveBreak>,
  next_breaks: HashMap<String, u64>,
}

#[derive(Clone)]
struct ScheduledReminder {
  reminder_id: String,
  label: String,
  next_at: u64,
  skipped: bool,
  postpone_count: u32,
}

struct OverlaySession {
  expected: HashSet<String>,
  ready: HashSet<String>,
}

struct Inner {
  settings: AppSettings,
  first_run: bool,
  paused: bool,
  system_locked: bool,
  active_break: Option<ActiveBreak>,
  scheduled: HashMap<String, ScheduledReminder>,
  overlay: Option<OverlaySession>,
  paused_sessions: Vec<String>,
}

struct Runtime {
  app: AppHandle,
  settings_path: PathBuf,
  inner: Mutex<Inner>,
  wake: Condvar,
  #[cfg(target_os = "linux")]
  fatcat_media: Option<video_server::FatCatMedia>,
}

impl Runtime {
  fn new(app: AppHandle) -> Result<Arc<Self>, String> {
    let settings_path = legacy_settings_path(&app)?;
    let (settings, first_run) = load_settings(&app, &settings_path)?;
    let initially_paused = settings.paused;
    #[cfg(target_os = "linux")]
    let fatcat_media = match video_server::find_videos_dir(&app) {
      Some(dir) => match video_server::start(dir) {
        Ok(media) => Some(media),
        Err(e) => {
          eprintln!("[fatcat] http server failed: {e}");
          None
        }
      },
      None => {
        eprintln!("[fatcat] videos dir not found");
        None
      }
    };
    let runtime = Arc::new(Self {
      app,
      settings_path,
      inner: Mutex::new(Inner {
        settings,
        first_run,
        paused: initially_paused,
        system_locked: false,
        active_break: None,
        scheduled: HashMap::new(),
        overlay: None,
        paused_sessions: Vec::new(),
      }),
      wake: Condvar::new(),
      #[cfg(target_os = "linux")]
      fatcat_media,
    });
    runtime.reschedule_all();
    Ok(runtime)
  }

  fn start_scheduler(self: &Arc<Self>) {
    let runtime = Arc::clone(self);
    thread::spawn(move || loop {
      let due = {
        let mut inner = runtime.inner.lock().unwrap();
        loop {
          if inner.paused || inner.system_locked || inner.active_break.is_some() {
            inner = runtime.wake.wait(inner).unwrap();
            continue;
          }
          let now = now_ms();
          if let Some(entry) = inner.scheduled.values().min_by_key(|entry| entry.next_at) {
            if entry.next_at <= now {
              break Some(entry.reminder_id.clone());
            }
            let wait_ms = entry.next_at.saturating_sub(now).min(u32::MAX as u64);
            let (next_inner, _) = runtime
              .wake
              .wait_timeout(inner, Duration::from_millis(wait_ms))
              .unwrap();
            inner = next_inner;
          } else {
            inner = runtime.wake.wait(inner).unwrap();
          }
        }
      };
      if let Some(id) = due {
        runtime.on_break_due(&id);
      }
    });
  }

  fn status(&self) -> TimerStatus {
    let inner = self.inner.lock().unwrap();
    status_from_inner(&inner)
  }

  fn settings(&self) -> AppSettings {
    self.inner.lock().unwrap().settings.clone()
  }

  fn is_first_run(&self) -> bool {
    self.inner.lock().unwrap().first_run
  }

  fn save_settings(&self, mut settings: AppSettings) -> Result<(), String> {
    // The frontend doesn't track the pause state; inject the authoritative
    // in-memory value so a reminder-card save never accidentally clears it.
    settings.paused = self.inner.lock().unwrap().paused;

    self.write_settings_file(&settings)?;

    // Sync the OS autostart entry with the saved setting so the two never drift apart.
    // disable() returns ERROR_FILE_NOT_FOUND on a fresh install with no existing entry —
    // that is harmless, so we ignore the error rather than surfacing it to the UI.
    let autostart = self.app.autolaunch();
    if settings.launch_on_startup {
      autostart.enable().map_err(|e| e.to_string())?;
    } else {
      let _ = autostart.disable();
    }

    {
      let mut inner = self.inner.lock().unwrap();
      inner.settings = merge_settings(settings);
      inner.first_run = false;
      reschedule_all_locked(&mut inner);
    }
    self.emit_settings();
    self.emit_status();
    self.wake.notify_all();
    self.update_tray();
    Ok(())
  }

  fn write_settings_file(&self, settings: &AppSettings) -> Result<(), String> {
    if let Some(parent) = self.settings_path.parent() {
      fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
      &self.settings_path,
      serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
  }

  #[cfg_attr(not(windows), allow(dead_code))]
  fn lock_screen(&self) {
    {
      let mut inner = self.inner.lock().unwrap();
      inner.system_locked = true;
    }
    // A break showing over the lock screen can't be interacted with, so dismiss it.
    self.end_break();
  }

  #[cfg_attr(not(windows), allow(dead_code))]
  fn unlock_screen(&self) {
    let now = now_ms();
    {
      let mut inner = self.inner.lock().unwrap();
      inner.system_locked = false;
      if inner.paused || inner.active_break.is_some() {
        return;
      }
      // Push any overdue breaks forward so the user isn't hit the instant they return.
      let reminders = inner.settings.reminders.clone();
      for reminder in reminders.iter().filter(|r| r.enabled) {
        if inner.scheduled.get(&reminder.id).map(|e| e.next_at <= now).unwrap_or(false) {
          schedule_locked(&mut inner, reminder);
        }
      }
    }
    self.wake.notify_all();
    self.emit_status();
    self.update_tray();
  }

  fn skip_next(&self) {
    let mut inner = self.inner.lock().unwrap();
    if let Some(id) = inner
      .scheduled
      .values()
      .min_by_key(|entry| entry.next_at)
      .map(|entry| entry.reminder_id.clone())
    {
      if let Some(entry) = inner.scheduled.get_mut(&id) {
        entry.skipped = true;
      }
    }
    drop(inner);
    self.emit_status();
    self.update_tray();
  }

  fn pause(&self) {
    let settings = {
      let mut inner = self.inner.lock().unwrap();
      inner.paused = true;
      inner.settings.paused = true;
      inner.settings.clone()
    };
    let _ = self.write_settings_file(&settings);
    self.wake.notify_all();
    self.emit_status();
    self.update_tray();
  }

  fn resume(&self) {
    let settings = {
      let mut inner = self.inner.lock().unwrap();
      inner.paused = false;
      inner.settings.paused = false;
      if inner.active_break.is_none() {
        reschedule_all_locked(&mut inner);
      }
      inner.settings.clone()
    };
    let _ = self.write_settings_file(&settings);
    self.wake.notify_all();
    self.emit_status();
    self.update_tray();
  }

  fn preview(&self) {
    let id = {
      let mut inner = self.inner.lock().unwrap();
      inner.active_break = None;
      inner.overlay = None;
      inner
        .settings
        .reminders
        .iter()
        .find(|r| r.enabled)
        .map(|r| r.id.clone())
    };
    destroy_overlay_windows(&self.app);
    if let Some(id) = id {
      self.start_break(&id, 0, true);
    }
  }

  fn end_break(&self) {
    let (ended, should_lock) = {
      let mut inner = self.inner.lock().unwrap();
      let Some(active) = inner.active_break.take() else {
        return;
      };
      let sessions = std::mem::take(&mut inner.paused_sessions);
      if !sessions.is_empty() {
        resume_system_media(sessions);
      }
      let reminder = inner
        .settings
        .reminders
        .iter()
        .find(|r| r.id == active.reminder_id && r.enabled)
        .cloned();
      if !inner.paused {
        if let Some(reminder) = reminder {
          schedule_locked(&mut inner, &reminder);
        }
      }
      // Lock the PC after a real break that was on screen for ≥5 minutes.
      // This lets the full-screen overlay replace the lock screen during the
      // break, then hands back to the OS lock once the break is over.
      let should_lock = !active.is_preview
        && now_ms().saturating_sub(active.started_at) >= 5 * 60_000;
      (active, should_lock)
    };
    let _ = self.app.emit(BREAK_END, ended);
    self.emit_status();
    self.update_tray();
    self.wake.notify_all();

    let app = self.app.clone();
    thread::spawn(move || {
      thread::sleep(Duration::from_millis(2600));
      destroy_overlay_windows(&app);
      if should_lock {
        lock_pc();
      }
    });
  }

  fn postpone_break(&self) {
    let postponed = {
      let mut inner = self.inner.lock().unwrap();
      let Some(active) = inner.active_break.take() else {
        return;
      };
      let sessions = std::mem::take(&mut inner.paused_sessions);
      if !sessions.is_empty() {
        resume_system_media(sessions);
      }
      // Schedule the postponed break in the main scheduler so the status bar
      // shows the countdown instead of "No breaks scheduled", and pause/resume
      // state is respected during the postponement window.
      let delay_ms = inner.settings.postpone_minutes.max(1) as u64 * 60_000;
      inner.scheduled.insert(
        active.reminder_id.clone(),
        ScheduledReminder {
          reminder_id: active.reminder_id.clone(),
          label: active.reminder.label.clone(),
          next_at: now_ms() + delay_ms,
          skipped: false,
          postpone_count: active.postpone_count + 1,
        },
      );
      active
    };
    self.wake.notify_all();
    let _ = self.app.emit(BREAK_END, postponed);
    self.emit_status();
    self.update_tray();
    destroy_overlay_windows(&self.app);
  }

  fn overlay_ready(&self, label: String) {
    let should_play = {
      let mut inner = self.inner.lock().unwrap();
      let Some(session) = inner.overlay.as_mut() else {
        return;
      };
      session.ready.insert(label);
      session.ready.len() >= session.expected.len()
    };
    if should_play {
      self.play_overlay();
    }
  }

  fn reschedule_all(&self) {
    {
      let mut inner = self.inner.lock().unwrap();
      reschedule_all_locked(&mut inner);
    }
    self.wake.notify_all();
    self.emit_status();
    self.update_tray();
  }

  fn on_break_due(&self, id: &str) {
    let action: Option<Option<(String, u32)>> = {
      let mut inner = self.inner.lock().unwrap();
      let Some(reminder) = inner.settings.reminders.iter().find(|r| r.id == id).cloned() else {
        inner.scheduled.remove(id);
        return;
      };
      if !reminder.enabled || inner.paused || inner.active_break.is_some() {
        return;
      }
      let entry_skipped = inner.scheduled.get(id).map(|e| e.skipped).unwrap_or(false);
      let postpone_count = inner.scheduled.get(id).map(|e| e.postpone_count).unwrap_or(0);
      if entry_skipped {
        schedule_locked(&mut inner, &reminder);
        Some(None)
      } else if postpone_count > 0 && is_media_in_use() {
        // The user pressed Postpone while in a call and is still in one.
        // Re-postpone for another postpone_minutes instead of firing or
        // dropping back to the full frequency cycle.
        let delay_ms = inner.settings.postpone_minutes.max(1) as u64 * 60_000;
        inner.scheduled.insert(
          id.to_string(),
          ScheduledReminder {
            reminder_id: id.to_string(),
            label: reminder.label.clone(),
            next_at: now_ms() + delay_ms,
            skipped: false,
            postpone_count,
          },
        );
        Some(None)
      } else if reminder.skip_on_idle && is_idle(inner.settings.idle_threshold_minutes) {
        schedule_locked(&mut inner, &reminder);
        Some(None)
      } else if reminder.skip_on_media && is_media_in_use() {
        schedule_locked(&mut inner, &reminder);
        Some(None)
      } else {
        Some(Some((reminder.id.clone(), postpone_count)))
      }
    };
    match action {
      Some(Some((reminder_id, postpone_count))) => self.start_break(&reminder_id, postpone_count, false),
      Some(None) => {
        self.emit_status();
        self.update_tray();
        self.wake.notify_all();
      }
      None => {}
    }
  }

  fn start_break(&self, reminder_id: &str, postpone_count: u32, is_preview: bool) {
    let (active, should_pause_music) = {
      let mut inner = self.inner.lock().unwrap();
      let Some(reminder) = inner
        .settings
        .reminders
        .iter()
        .find(|r| r.id == reminder_id)
        .cloned()
      else {
        return;
      };
      let now = now_ms();
      let active = ActiveBreak {
        reminder_id: reminder_id.to_string(),
        ends_at: now + reminder.duration_minutes.max(1) as u64 * 60_000,
        reminder,
        started_at: now,
        postpone_count,
        is_preview,
      };
      let should_pause_music = inner.settings.pause_music_on_break;
      inner.scheduled.remove(reminder_id);
      inner.active_break = Some(active.clone());
      (active, should_pause_music)
    };
    self.create_overlay_windows(active.clone());
    self.emit_status();
    self.update_tray();

    let runtime = self.app.state::<Arc<Runtime>>().inner().clone();

    // Pause media off the critical path so it doesn't delay the overlay.
    // Only store the paused sessions if the break is still active when we finish.
    if should_pause_music {
      let music_runtime = runtime.clone();
      let break_started_at = active.started_at;
      thread::spawn(move || {
        let paused = pause_system_media();
        if !paused.is_empty() {
          let mut inner = music_runtime.inner.lock().unwrap();
          if inner.active_break.as_ref().map(|a| a.started_at == break_started_at).unwrap_or(false) {
            inner.paused_sessions = paused;
          } else {
            // Break ended before we finished pausing — undo it immediately.
            drop(inner);
            resume_system_media(paused);
          }
        }
      });
    }

    let preview_guard = runtime.clone();
    let fallback_started_at = active.started_at;
    thread::spawn(move || {
      thread::sleep(Duration::from_millis(1800));
      preview_guard.play_overlay_if_pending(fallback_started_at);
    });

    thread::spawn(move || {
      let wait = active.ends_at.saturating_sub(now_ms());
      thread::sleep(Duration::from_millis(wait));
      runtime.end_break();
    });
  }

  fn create_overlay_windows(&self, active: ActiveBreak) {
    destroy_overlay_windows(&self.app);
    let settings = self.settings();
    let monitors = if settings.cover_all_displays {
      self.app.available_monitors().unwrap_or_default()
    } else {
      self
        .app
        .primary_monitor()
        .ok()
        .flatten()
        .into_iter()
        .collect()
    };
    let mut built_labels = Vec::new();
    for (i, monitor) in monitors.iter().enumerate() {
      let label = format!("overlay-{i}");
      let pos = monitor.position();
      let size = monitor.size();
      let url = if cfg!(target_os = "linux") {
        format!("overlay/index.html?label={label}&linux=1")
      } else {
        format!("overlay/index.html?label={label}")
      };
      let window = WebviewWindowBuilder::new(&self.app, &label, WebviewUrl::App(url.into()))
      .title("Take A Moment")
      .decorations(false)
      .transparent(true)
      .always_on_top(true)
      .skip_taskbar(true)
      .resizable(false)
      .visible(false)
      .inner_size(size.width as f64, size.height as f64)
      .position(pos.x as f64, pos.y as f64)
      .build();
      if let Ok(window) = window {
        let _ = window.set_size(PhysicalSize::new(size.width, size.height));
        let _ = window.set_position(PhysicalPosition::new(pos.x, pos.y));
        built_labels.push(label);
      }
    }
    {
      let mut inner = self.inner.lock().unwrap();
      inner.overlay = Some(OverlaySession {
        expected: built_labels.iter().cloned().collect(),
        ready: HashSet::new(),
      });
    }
    let _ = self.app.emit(BREAK_START, active);
  }

  fn show_overlay_windows(&self) {
    for (_, window) in self.app.webview_windows() {
      if window.label().starts_with("overlay-") {
        let _ = window.set_fullscreen(true);
        let _ = window.show();
        let _ = window.set_focus();
      }
    }
  }

  fn play_overlay(&self) {
    {
      let mut inner = self.inner.lock().unwrap();
      inner.overlay = None;
    }
    self.show_overlay_windows();
    let _ = self.app.emit(BREAK_PLAY, ());
  }

  fn play_overlay_if_pending(&self, started_at: u64) {
    let should_play = {
      let inner = self.inner.lock().unwrap();
      inner
        .active_break
        .as_ref()
        .map(|active| active.started_at == started_at)
        .unwrap_or(false)
        && inner.overlay.is_some()
    };
    if should_play {
      self.play_overlay();
    }
  }

  fn emit_status(&self) {
    let status = self.status();
    let _ = self.app.emit(STATUS_CHANGED, status);
  }

  fn emit_settings(&self) {
    let settings = self.settings();
    let _ = self.app.emit(SETTINGS_CHANGED, settings);
  }

  fn update_tray(&self) {
    let Some(tray) = self.app.tray_by_id("main") else {
      return;
    };
    let settings = self.settings();
    let status = self.status();
    let s = tray_strings(&settings.language);
    let tooltip = if status.paused {
      format!("Take A Moment — {}", s.paused_label)
    } else if let Some(next) = &status.next_break {
      format!("{}: {}", s.next_label, next.label)
    } else {
      "Take A Moment".to_string()
    };
    let _ = tray.set_tooltip(Some(&tooltip));
    if let Ok(menu) = build_tray_menu(&self.app, &settings.language, status.paused) {
      let _ = tray.set_menu(Some(menu));
    }
  }
}

#[tauri::command]
fn get_timer_status(runtime: State<Arc<Runtime>>) -> TimerStatus {
  runtime.status()
}

#[tauri::command]
fn get_settings(runtime: State<Arc<Runtime>>) -> AppSettings {
  runtime.settings()
}

#[tauri::command]
fn save_settings(runtime: State<Arc<Runtime>>, settings: AppSettings) -> Result<(), String> {
  runtime.save_settings(settings)
}

#[tauri::command]
fn skip_next(runtime: State<Arc<Runtime>>) {
  runtime.skip_next()
}

#[tauri::command]
fn pause(runtime: State<Arc<Runtime>>) {
  runtime.pause()
}

#[tauri::command]
fn resume(runtime: State<Arc<Runtime>>) {
  runtime.resume()
}

#[tauri::command]
fn end_break(runtime: State<Arc<Runtime>>) {
  runtime.end_break()
}

#[tauri::command]
fn postpone_break(runtime: State<Arc<Runtime>>) {
  runtime.postpone_break()
}

#[tauri::command]
fn preview_break(runtime: State<Arc<Runtime>>) {
  // Sync commands run on the main thread. preview() builds overlay windows and
  // queries monitors, which must dispatch to the event loop — doing that from
  // the blocked main thread yields no monitors (so no overlay). Run it on a
  // worker thread, exactly like the scheduler does for real breaks.
  let runtime = runtime.inner().clone();
  thread::spawn(move || runtime.preview());
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
  show_settings(&app)
}

#[tauri::command]
fn quit(app: AppHandle) {
  app.exit(0);
  std::process::exit(0);
}

#[tauri::command]
fn get_version(app: AppHandle) -> String {
  app.package_info().version.to_string()
}

#[tauri::command]
fn is_first_run(runtime: State<Arc<Runtime>>) -> bool {
  runtime.is_first_run()
}

#[tauri::command]
fn overlay_ready(runtime: State<Arc<Runtime>>, label: String) {
  runtime.overlay_ready(label)
}

/// Overlay webview console does not reach the AppImage terminal. Print there.
#[tauri::command]
fn log_overlay(message: String) {
  eprintln!("{message}");
}

/// HTTP (souphttpsrc) then file:// (filesrc). Custom schemes fail in AppImage WebKit.
#[tauri::command]
fn fatcat_video_urls(
  runtime: State<Arc<Runtime>>,
  filename: String,
) -> Result<Vec<String>, String> {
  #[cfg(not(target_os = "linux"))]
  {
    let _ = runtime;
    let _ = filename;
    Err("linux only".into())
  }
  #[cfg(target_os = "linux")]
  {
    if !matches!(filename.as_str(), "neko1.webm" | "neko2.webm") {
      return Err(format!("unknown clip {filename}"));
    }
    let mut urls = Vec::new();
    if let Some(media) = &runtime.fatcat_media {
      let path = media.videos_dir.join(&filename);
      if path.is_file() {
        urls.push(format!("{}/{filename}", media.origin));
        urls.push(video_server::file_url(&path));
      }
    }
    if urls.is_empty() {
      if let Some(dir) = video_server::find_videos_dir(&runtime.app) {
        let path = dir.join(&filename);
        if path.is_file() {
          urls.push(video_server::file_url(&path));
        }
      }
    }
    if urls.is_empty() {
      return Err(format!("{filename} not found"));
    }
    Ok(urls)
  }
}

pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
      let _ = show_settings(app);
    }))
    .setup(|app| {
      app.handle().plugin(tauri_plugin_autostart::init(
        MacosLauncher::LaunchAgent,
        None,
      ))?;
      let runtime = Runtime::new(app.handle().clone())?;
      runtime.start_scheduler();
      start_session_monitor(runtime.clone());
      app.manage(runtime);
      create_tray(app.handle())?;
      app.state::<Arc<Runtime>>().update_tray();
      if app.state::<Arc<Runtime>>().is_first_run() {
        show_settings(app.handle())?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      get_timer_status,
      get_settings,
      save_settings,
      skip_next,
      pause,
      resume,
      end_break,
      postpone_break,
      preview_break,
      open_settings,
      quit,
      get_version,
      is_first_run,
      overlay_ready,
      log_overlay,
      fatcat_video_urls
    ])
    .build(tauri::generate_context!())
    .expect("error while building Take A Moment")
    .run(|app, event| match event {
      tauri::RunEvent::WindowEvent { label, event, .. } => {
        if label == "settings" {
          if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(window) = app.get_webview_window("settings") {
              let _ = window.hide();
            }
          }
        }
      }
      tauri::RunEvent::ExitRequested { api, .. } => {
        api.prevent_exit();
      }
      _ => {}
    });
}

struct TrayStrings {
  skip: &'static str,
  pause: &'static str,
  resume: &'static str,
  settings: &'static str,
  quit: &'static str,
  paused_label: &'static str,
  next_label: &'static str,
}

fn tray_strings(language: &str) -> TrayStrings {
  match language {
    "de" => TrayStrings {
      skip: "Nächste überspringen",
      pause: "Pausieren",
      resume: "Fortsetzen",
      settings: "Einstellungen",
      quit: "Beenden",
      paused_label: "Pausiert",
      next_label: "Nächste",
    },
    "fr" => TrayStrings {
      skip: "Sauter la suivante",
      pause: "Mettre en pause",
      resume: "Reprendre",
      settings: "Paramètres",
      quit: "Quitter",
      paused_label: "En pause",
      next_label: "Prochain",
    },
    "es" => TrayStrings {
      skip: "Saltar la siguiente",
      pause: "Pausar",
      resume: "Reanudar",
      settings: "Ajustes",
      quit: "Salir",
      paused_label: "Pausado",
      next_label: "Próximo",
    },
    "sv" => TrayStrings {
      skip: "Hoppa över",
      pause: "Pausa",
      resume: "Återuppta",
      settings: "Inställningar",
      quit: "Avsluta",
      paused_label: "Pausad",
      next_label: "Nästa",
    },
    "nl" => TrayStrings {
      skip: "Volgende overslaan",
      pause: "Pauzeren",
      resume: "Hervatten",
      settings: "Instellingen",
      quit: "Afsluiten",
      paused_label: "Gepauzeerd",
      next_label: "Volgende",
    },
    "da" => TrayStrings {
      skip: "Spring næste over",
      pause: "Sæt på pause",
      resume: "Genoptag",
      settings: "Indstillinger",
      quit: "Afslut",
      paused_label: "Sat på pause",
      next_label: "Næste",
    },
    _ => TrayStrings {
      skip: "Skip next break",
      pause: "Pause breaks",
      resume: "Resume breaks",
      settings: "Settings",
      quit: "Quit",
      paused_label: "Paused",
      next_label: "Next",
    },
  }
}

fn build_tray_menu(
  app: &AppHandle,
  language: &str,
  paused: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
  let s = tray_strings(language);
  let pause_text = if paused { s.resume } else { s.pause };
  let skip = MenuItem::with_id(app, "skip", s.skip, true, None::<&str>)?;
  let pause_item = MenuItem::with_id(app, "pause", pause_text, true, None::<&str>)?;
  let settings_item = MenuItem::with_id(app, "settings", s.settings, true, None::<&str>)?;
  let quit_item = MenuItem::with_id(app, "quit", s.quit, true, None::<&str>)?;
  let sep = PredefinedMenuItem::separator(app)?;
  Menu::with_items(app, &[&skip, &pause_item, &sep, &settings_item, &sep, &quit_item])
}

fn create_tray(app: &AppHandle) -> tauri::Result<()> {
  let skip = MenuItem::with_id(app, "skip", "Skip next break", true, None::<&str>)?;
  let pause = MenuItem::with_id(app, "pause", "Pause / resume breaks", true, None::<&str>)?;
  let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
  let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
  let sep = PredefinedMenuItem::separator(app)?;
  let menu = Menu::with_items(app, &[&skip, &pause, &sep, &settings, &sep, &quit])?;

  let icon = tray_icon(app)
    .or_else(|| app.default_window_icon().cloned())
    .ok_or_else(|| tauri::Error::AssetNotFound("tray icon".into()))?;

  TrayIconBuilder::with_id("main")
    .icon(icon)
    .tooltip("Take A Moment")
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, event| {
      let runtime = app.state::<Arc<Runtime>>();
      match event.id.as_ref() {
        "skip" => runtime.skip_next(),
        "pause" => {
          if runtime.status().paused {
            runtime.resume();
          } else {
            runtime.pause();
          }
        }
        "settings" => {
          let _ = show_settings(app);
        }
        "quit" => {
          app.exit(0);
          std::process::exit(0);
        }
        _ => {}
      }
    })
    .on_tray_icon_event(|tray, event| {
      if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
      } = event
      {
        let _ = show_settings(tray.app_handle());
      }
    })
    .build(app)?;
  Ok(())
}

fn show_settings(app: &AppHandle) -> Result<(), String> {
  if let Some(window) = app.get_webview_window("settings") {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    return Ok(());
  }
  WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings/index.html".into()))
    .title("Take A Moment - Settings")
    .inner_size(680.0, 760.0)
    .min_inner_size(560.0, 600.0)
    .resizable(true)
    .visible(true)
    .build()
    .map_err(|e| e.to_string())?;
  Ok(())
}

fn tray_icon(app: &AppHandle) -> Option<Image<'static>> {
  let path = app.path().resource_dir().ok()?.join("icons").join("tray.png");
  Image::from_path(path).ok()
}

fn destroy_overlay_windows(app: &AppHandle) {
  for (_, window) in app.webview_windows() {
    if window.label().starts_with("overlay-") {
      let _ = window.close();
    }
  }
}

fn status_from_inner(inner: &Inner) -> TimerStatus {
  let mut next_break = None;
  let mut earliest = u64::MAX;
  let mut next_breaks = HashMap::new();
  for entry in inner.scheduled.values() {
    next_breaks.insert(entry.reminder_id.clone(), entry.next_at);
    if entry.next_at < earliest {
      earliest = entry.next_at;
      next_break = Some(NextBreak {
        reminder_id: entry.reminder_id.clone(),
        label: entry.label.clone(),
        scheduled_at: entry.next_at,
      });
    }
  }
  TimerStatus {
    paused: inner.paused,
    next_break,
    active_break: inner.active_break.clone(),
    next_breaks,
  }
}

fn reschedule_all_locked(inner: &mut Inner) {
  inner.scheduled.clear();
  if inner.paused || inner.active_break.is_some() {
    return;
  }
  let reminders = inner.settings.reminders.clone();
  for reminder in reminders.iter().filter(|r| r.enabled) {
    schedule_locked(inner, reminder);
  }
}

fn schedule_locked(inner: &mut Inner, reminder: &Reminder) {
  let next_at = reminder
    .start_time
    .as_deref()
    .map(|start| {
      next_occurrence_from_anchor(start, reminder.end_time.as_deref(), reminder.frequency_minutes)
    })
    .unwrap_or_else(|| now_ms() + reminder.frequency_minutes as u64 * 60_000);
  inner.scheduled.insert(
    reminder.id.clone(),
    ScheduledReminder {
      reminder_id: reminder.id.clone(),
      label: reminder.label.clone(),
      next_at,
      skipped: false,
      postpone_count: 0,
    },
  );
}

fn parse_time_anchor_ms(now: DateTime<Local>, time: &str) -> Option<u64> {
  let parts: Vec<_> = time.split(':').collect();
  let hour = parts.first().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
  let minute = parts.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
  now
    .with_hour(hour)
    .and_then(|d| d.with_minute(minute))
    .and_then(|d| d.with_second(0))
    .and_then(|d| d.with_nanosecond(0))
    .map(|d| d.timestamp_millis() as u64)
}

// `start`/`end` anchor a daily window (e.g. "09:00".."12:00"). Occurrences step
// forward from `start` every `frequency_minutes`; once a candidate would land at
// or past `end`, scheduling rolls over to the next day's window instead of
// spilling past it. `end` is ignored if missing or not after `start`, so a
// reminder with only a start time behaves exactly as before.
fn next_occurrence_from_anchor(start: &str, end: Option<&str>, frequency_minutes: u32) -> u64 {
  let now = Local::now();
  let Some(start_ms) = parse_time_anchor_ms(now, start) else {
    return now_ms() + frequency_minutes as u64 * 60_000;
  };
  let end_ms = end
    .and_then(|e| parse_time_anchor_ms(now, e))
    .filter(|&end_ms| end_ms > start_ms);

  let now_ms = now_ms();
  let interval = frequency_minutes as u64 * 60_000;
  let candidate = if start_ms > now_ms {
    start_ms
  } else {
    start_ms + (((now_ms - start_ms) / interval) + 1) * interval
  };

  match end_ms {
    Some(end_ms) if candidate >= end_ms => start_ms + 24 * 60 * 60 * 1000,
    _ => candidate,
  }
}

fn now_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as u64
}

fn load_settings(app: &AppHandle, path: &PathBuf) -> Result<(AppSettings, bool), String> {
  if path.exists() {
    let raw = read_text_file(path)?;
    let parsed: serde_json::Value =
      serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let settings: AppSettings = serde_json::from_value(merge_json(default_settings(), parsed))
      .map_err(|e| e.to_string())?;
    return Ok((merge_settings(settings), false));
  }

  if let Some(language) = read_install_language(app) {
    let settings = default_settings_for_language(&language);
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
      path,
      serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    return Ok((settings, false));
  }

  Ok((default_settings(), true))
}

fn legacy_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
  #[cfg(windows)]
  {
    if let Some(appdata) = std::env::var_os("APPDATA") {
      return Ok(PathBuf::from(appdata).join("take-a-moment").join("settings.json"));
    }
  }

  Ok(app
    .path()
    .app_data_dir()
    .map_err(|e| e.to_string())?
    .join("settings.json"))
}

fn read_install_language(app: &AppHandle) -> Option<String> {
  let resource_dir = app.path().resource_dir().ok()?;
  let path = resource_dir.join("install-config.json");
  let raw = read_text_file(&path).ok()?;
  let _ = fs::remove_file(path);
  serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
    .ok()?
    .get("language")?
    .as_str()
    .map(ToOwned::to_owned)
}

fn read_text_file(path: &PathBuf) -> Result<String, String> {
  let bytes = fs::read(path).map_err(|e| e.to_string())?;
  if bytes.starts_with(&[0xff, 0xfe]) {
    let words = bytes[2..]
      .chunks_exact(2)
      .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
      .collect::<Vec<_>>();
    return String::from_utf16(&words).map_err(|e| e.to_string());
  }
  if bytes.starts_with(&[0xfe, 0xff]) {
    let words = bytes[2..]
      .chunks_exact(2)
      .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
      .collect::<Vec<_>>();
    return String::from_utf16(&words).map_err(|e| e.to_string());
  }
  String::from_utf8(bytes).map_err(|e| e.to_string())
}

fn merge_json(mut base: AppSettings, patch: serde_json::Value) -> serde_json::Value {
  let mut base = serde_json::to_value(&mut base).unwrap();
  merge_value(&mut base, patch);
  base
}

fn merge_value(base: &mut serde_json::Value, patch: serde_json::Value) {
  match (base, patch) {
    (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
      for (key, value) in patch {
        merge_value(base.entry(key).or_insert(serde_json::Value::Null), value);
      }
    }
    (base, patch) => *base = patch,
  }
}

fn merge_settings(mut settings: AppSettings) -> AppSettings {
  for reminder in &mut settings.reminders {
    if reminder.volume == 0 {
      reminder.volume = 80;
    }
  }
  settings
}

fn default_settings_for_language(language: &str) -> AppSettings {
  let safe = match language {
    "en" | "de" | "fr" | "es" | "sv" | "nl" | "da" => language,
    _ => "sv",
  };
  let mut settings = default_settings();
  settings.language = safe.to_string();
  let (label, message) = match safe {
    "en" => ("Take A Moment", "Take A Moment"),
    "de" => ("Einen Moment", "Nimm dir einen Moment"),
    "fr" => ("Un moment", "Prenez un moment"),
    "es" => ("Un momento", "Tómate un momento"),
    "nl" => ("Neem even een moment", "Neem even een moment"),
    "da" => ("Tag et øjeblik", "Tag et øjeblik"),
    _ => ("Stanna upp ett tag", "Stanna upp ett tag"),
  };
  if let Some(reminder) = settings.reminders.first_mut() {
    reminder.label = label.to_string();
    reminder.message = message.to_string();
  }
  settings
}

fn default_settings() -> AppSettings {
  AppSettings {
    reminders: vec![Reminder {
      id: "default-take-a-moment".into(),
      label: "Stanna upp ett tag".into(),
      frequency_minutes: 60,
      duration_minutes: 5,
      message: "Stanna upp ett tag".into(),
      sound_start: "chime".into(),
      sound_end: "soft".into(),
      enabled: true,
      skip_on_idle: true,
      skip_on_media: false,
      volume: 80,
      start_time: None,
      end_time: None,
    }],
    theme: "still-garden".into(),
    language: "sv".into(),
    idle_threshold_minutes: 5,
    pause_music_on_break: false,
    launch_on_startup: false,
    postpone_minutes: 5,
    cover_all_displays: true,
    time_format: "24h".into(),
    break_background: "default".into(),
    paused: false,
  }
}

#[cfg(windows)]
fn lock_pc() {
  let mut cmd = Command::new("rundll32.exe");
  cmd.args(["user32.dll,LockWorkStation"]);
  configure_hidden(&mut cmd);
  let _ = cmd.spawn();
}

#[cfg(not(windows))]
fn lock_pc() {
  // Attempt to lock the *current* logind session.
  // On many desktop setups `XDG_SESSION_ID` is present and matches logind's session id.
  let session_id = match std::env::var("XDG_SESSION_ID") {
    Ok(v) if !v.trim().is_empty() => v,
    _ => return,
  };

  let mut cmd = Command::new("loginctl");
  // Best-effort: don't block the break thread on failures.
  cmd.args(["lock-session", &session_id]);
  let _ = cmd.spawn();
}

#[cfg(windows)]
fn is_idle(threshold_minutes: u32) -> bool {
  use std::mem::size_of;
  use windows::Win32::{
    System::SystemInformation::GetTickCount64,
    UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
  };
  unsafe {
    let mut info = LASTINPUTINFO {
      cbSize: size_of::<LASTINPUTINFO>() as u32,
      dwTime: 0,
    };
    if GetLastInputInfo(&mut info).as_bool() {
      let idle_ms = GetTickCount64().saturating_sub(info.dwTime as u64);
      return idle_ms >= threshold_minutes as u64 * 60_000;
    }
  }
  false
}

#[cfg(not(windows))]
fn is_idle(_threshold_minutes: u32) -> bool {
  // Use systemd-logind's idle hint for the current session.
  // This is robust across desktops (GNOME/KDE/etc.) and doesn't require X11.
  let session_id = match std::env::var("XDG_SESSION_ID") {
    Ok(v) if !v.trim().is_empty() => v,
    _ => return false,
  };

  let output = Command::new("loginctl")
    .args([
      "show-session",
      &session_id,
      "-p",
      "IdleHint",
      "-p",
      "IdleSinceHint",
    ])
    .output();

  let stdout = match output.ok().and_then(|o| String::from_utf8(o.stdout).ok()) {
    Some(s) => s,
    None => return false,
  };

  let mut idle_hint: Option<bool> = None;
  let mut idle_since_hint_us: Option<u64> = None;

  for line in stdout.lines() {
    let Some((k, v)) = line.split_once('=') else { continue };
    match k.trim() {
      "IdleHint" => {
        idle_hint = match v.trim().to_ascii_lowercase().as_str() {
          "yes" => Some(true),
          "no" => Some(false),
          _ => None,
        };
      }
      "IdleSinceHint" => {
        idle_since_hint_us = v.trim().parse::<u64>().ok();
      }
      _ => {}
    }
  }

  let Some(idle_hint) = idle_hint else { return false };
  if !idle_hint {
    return false;
  }
  let Some(idle_since_hint_us) = idle_since_hint_us else { return false };
  if idle_since_hint_us == 0 {
    return false;
  }

  let now_us = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_micros() as u64;
  let idle_us = now_us.saturating_sub(idle_since_hint_us);
  idle_us >= _threshold_minutes as u64 * 60 * 1_000_000
}

/// Adds CREATE_NO_WINDOW on Windows so shelling out to reg/powershell from the
/// GUI-subsystem build never flashes a console window. No-op elsewhere.
fn configure_hidden(cmd: &mut Command) {
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
  }
  let _ = cmd;
}

/// True while the camera or microphone is actively in use. Windows writes
/// LastUsedTimeStop = 0 while a device is held and a FILETIME once released, so
/// a value of exactly `0x0` under the microphone/webcam consent stores means
/// "still in use" (covers Teams, Zoom, Meet, etc.).
fn is_media_in_use() -> bool {
  if cfg!(windows) {
    for device in ["microphone", "webcam"] {
      let key = format!(
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{device}"
      );
      let mut cmd = Command::new("reg");
      cmd.args(["query", &key, "/s", "/v", "LastUsedTimeStop"]);
      configure_hidden(&mut cmd);
      let in_use = cmd
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
          s.lines().any(|line| {
            line.contains("LastUsedTimeStop")
              && line.split_whitespace().last() == Some("0x0")
          })
        })
        .unwrap_or(false);
      if in_use {
        return true;
      }
    }
    return false;
  }

  // Linux: best-effort detection.
  // We look for any process holding an open file descriptor to common camera/mic devices.
  // This is necessarily approximate, but it works across most setups without extra deps.
  //
  // Throttle checks to avoid scanning /proc too frequently.
  use std::sync::{Mutex, OnceLock};
  struct MediaCache {
    last_check_ms: u64,
    last_value: bool,
  }
  static CACHE: OnceLock<Mutex<MediaCache>> = OnceLock::new();

  let cache = CACHE.get_or_init(|| Mutex::new(MediaCache {
    last_check_ms: 0,
    last_value: false,
  }));

  let now_ms = now_ms();
  {
    let mut guard = cache.lock().unwrap();
    if guard.last_check_ms != 0 && now_ms.saturating_sub(guard.last_check_ms) < 1_500 {
      return guard.last_value;
    }

    let mut in_use = false;
    let proc_dir = match std::fs::read_dir("/proc") {
      Ok(d) => d,
      Err(_) => {
        guard.last_check_ms = now_ms;
        guard.last_value = false;
        return false;
      }
    };

    for entry in proc_dir {
      let Ok(entry) = entry else { continue };
      let pid_name = entry.file_name();
      let pid_str = match pid_name.to_str() {
        Some(s) => s,
        None => continue,
      };
      if pid_str.is_empty() || !pid_str.chars().all(|c| c.is_ascii_digit()) {
        continue;
      }
      let fd_dir = format!("/proc/{pid_str}/fd");
      let Ok(fds) = std::fs::read_dir(&fd_dir) else { continue };

      for fd in fds.flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else { continue };
        let target_str = target.to_string_lossy();

        // Camera devices are usually /dev/video*.
        if target_str.starts_with("/dev/video") {
          in_use = true;
          break;
        }

        // Microphones often go through /dev/snd/* or sound control nodes.
        if target_str.starts_with("/dev/snd/") {
          in_use = true;
          break;
        }

        // PipeWire/Pulse sockets show up as paths somewhere under /run/user/$UID.
        if target_str.contains("pipewire-0")
          || target_str.contains("/pulse/native")
        {
          in_use = true;
          break;
        }
      }
      if in_use {
        break;
      }
    }

    guard.last_check_ms = now_ms;
    guard.last_value = in_use;
    in_use
  }
}

// Block synchronously on a WinRT IAsyncOperation<T>.
// Used from background threads where blocking is acceptable.
#[cfg(windows)]
fn wait_for_async<T: windows::core::RuntimeType + 'static>(
  op: windows_future::IAsyncOperation<T>,
) -> windows::core::Result<T> {
  use windows_future::AsyncStatus;
  loop {
    if op.Status()? != AsyncStatus::Started {
      break;
    }
    thread::sleep(Duration::from_millis(10));
  }
  op.GetResults()
}

// Pause all currently-playing SMTC sessions using the native WinRT
// GlobalSystemMediaTransportControlsSessionManager API.
// Returns the SourceAppUserModelId of each session that was paused so that
// resume_system_media can target only those apps (not blindly toggle).
#[cfg(windows)]
fn pause_system_media() -> Vec<String> {
  use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
  };
  let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
    .ok()
    .and_then(|op| wait_for_async(op).ok())
  {
    Some(m) => m,
    None => return Vec::new(),
  };
  let sessions = match manager.GetSessions() {
    Ok(s) => s,
    Err(_) => return Vec::new(),
  };
  let mut paused = Vec::new();
  let count = sessions.Size().unwrap_or(0);
  for i in 0..count {
    let session = match sessions.GetAt(i) {
      Ok(s) => s,
      Err(_) => continue,
    };
    let is_playing = session
      .GetPlaybackInfo()
      .and_then(|info| info.PlaybackStatus())
      .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
      .unwrap_or(false);
    if is_playing {
      let ok = session
        .TryPauseAsync()
        .ok()
        .and_then(|op| wait_for_async(op).ok())
        .unwrap_or(false);
      if ok {
        if let Ok(id) = session.SourceAppUserModelId() {
          paused.push(id.to_string());
        }
      }
    }
  }
  paused
}

#[cfg(not(windows))]
fn pause_system_media() -> Vec<String> {
  // Linux: use MPRIS via `playerctl`.
  // If playerctl isn't available, we can't pause/resume and will no-op.
  let list_output = match Command::new("playerctl").args(["-l"]).output() {
    Ok(o) => o,
    Err(_) => return Vec::new(),
  };
  let players = match String::from_utf8(list_output.stdout) {
    Ok(s) => s,
    Err(_) => return Vec::new(),
  };
  let players: Vec<String> = players
    .lines()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .map(|s| s.to_string())
    .collect();

  if players.is_empty() {
    return Vec::new();
  }

  let mut paused = Vec::new();
  for player in players {
    let status_out = Command::new("playerctl")
      .args(["-p", &player, "status"])
      .output()
      .ok();
    let Some(status_out) = status_out else { continue };
    let status = String::from_utf8(status_out.stdout).ok().unwrap_or_default();
    if status.trim().eq_ignore_ascii_case("Playing") {
      let _ = Command::new("playerctl")
        .args(["-p", &player, "pause"])
        .output();
      paused.push(player);
    }
  }
  paused
}

// Resume only the sessions that pause_system_media paused.
#[cfg(windows)]
fn resume_system_media(sessions_to_resume: Vec<String>) {
  if sessions_to_resume.is_empty() {
    return;
  }
  thread::spawn(move || {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
      .ok()
      .and_then(|op| wait_for_async(op).ok())
    {
      Some(m) => m,
      None => return,
    };
    let sessions = match manager.GetSessions() {
      Ok(s) => s,
      Err(_) => return,
    };
    let count = sessions.Size().unwrap_or(0);
    for i in 0..count {
      let session = match sessions.GetAt(i) {
        Ok(s) => s,
        Err(_) => continue,
      };
      let id = match session.SourceAppUserModelId() {
        Ok(id) => id.to_string(),
        Err(_) => continue,
      };
      if sessions_to_resume.contains(&id) {
        if let Ok(play_op) = session.TryPlayAsync() {
          let _ = wait_for_async(play_op);
        }
      }
    }
  });
}

#[cfg(not(windows))]
fn resume_system_media(sessions_to_resume: Vec<String>) {
  if sessions_to_resume.is_empty() {
    return;
  }

  thread::spawn(move || {
    for player in sessions_to_resume {
      let _ = Command::new("playerctl")
        .args(["-p", &player, "play"])
        .output();
    }
  });
}

// ─── Lock-screen guard (Windows session lock/unlock) ─────────────────────────
// Breaks should not fire over the lock screen (the user can't dismiss them) and
// overdue breaks must be pushed forward when the user returns. We listen for
// WM_WTSSESSION_CHANGE on a hidden message-only window.

#[cfg(windows)]
static SESSION_RUNTIME: Mutex<Option<Arc<Runtime>>> = Mutex::new(None);

#[cfg(windows)]
unsafe extern "system" fn session_wnd_proc(
  hwnd: windows::Win32::Foundation::HWND,
  msg: u32,
  wparam: windows::Win32::Foundation::WPARAM,
  lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
  use windows::Win32::Foundation::LRESULT;
  use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_WTSSESSION_CHANGE};

  // WTS_SESSION_LOCK / WTS_SESSION_UNLOCK (wtsapi32.h) — not surfaced by the crate.
  const WTS_SESSION_LOCK: u32 = 0x7;
  const WTS_SESSION_UNLOCK: u32 = 0x8;

  if msg == WM_WTSSESSION_CHANGE {
    let event = wparam.0 as u32;
    if event == WTS_SESSION_LOCK || event == WTS_SESSION_UNLOCK {
      if let Some(runtime) = SESSION_RUNTIME.lock().unwrap().clone() {
        if event == WTS_SESSION_LOCK {
          runtime.lock_screen();
        } else {
          runtime.unlock_screen();
        }
      }
    }
    return LRESULT(0);
  }
  DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(windows)]
fn start_session_monitor(runtime: Arc<Runtime>) {
  *SESSION_RUNTIME.lock().unwrap() = Some(runtime);
  thread::spawn(|| unsafe {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::RemoteDesktop::{
      WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
      CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage,
      HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    let class_name: Vec<u16> = "TamSessionMonitor\0".encode_utf16().collect();
    let hinstance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
      .map(|m| HINSTANCE(m.0))
      .unwrap_or_default();

    let wc = WNDCLASSW {
      lpfnWndProc: Some(session_wnd_proc),
      hInstance: hinstance,
      lpszClassName: PCWSTR(class_name.as_ptr()),
      ..Default::default()
    };
    RegisterClassW(&wc);

    let Ok(hwnd) = CreateWindowExW(
      WINDOW_EX_STYLE(0),
      PCWSTR(class_name.as_ptr()),
      PCWSTR(class_name.as_ptr()),
      WINDOW_STYLE(0),
      0,
      0,
      0,
      0,
      Some(HWND_MESSAGE),
      None,
      Some(hinstance),
      None,
    ) else {
      return;
    };

    if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_err() {
      return;
    }

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, Some(hwnd), 0, 0).0 > 0 {
      let _ = TranslateMessage(&msg);
      DispatchMessageW(&msg);
    }
  });
}

#[cfg(not(windows))]
fn start_session_monitor(runtime: Arc<Runtime>) {
  // Linux: best-effort polling based on logind's `LockedHint`.
  // Polling keeps the implementation dependency-free and works across desktops.
  thread::spawn(move || {
    let session_id = match std::env::var("XDG_SESSION_ID") {
      Ok(v) if !v.trim().is_empty() => v,
      _ => return,
    };

    let mut last_locked: Option<bool> = None;

    loop {
      let locked = Command::new("loginctl")
        .args(["show-session", &session_id, "-p", "LockedHint"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|stdout| {
          stdout
            .lines()
            .find_map(|line| line.split_once('=').map(|(_, v)| v.trim().to_string()))
        })
        .and_then(|v| match v.to_ascii_lowercase().as_str() {
          "yes" => Some(true),
          "no" => Some(false),
          _ => None,
        });

      if let Some(locked) = locked {
        if last_locked.map(|prev| prev != locked).unwrap_or(true) {
          if locked {
            runtime.lock_screen();
          } else {
            runtime.unlock_screen();
          }
          last_locked = Some(locked);
        }
      }

      thread::sleep(Duration::from_secs(2));
    }
  });
}
