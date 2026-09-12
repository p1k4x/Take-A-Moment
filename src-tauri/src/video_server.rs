//! Loopback HTTP for Fat Cat WebMs (TMP-15).
//!
//! WebKitGTK's GStreamer player rejects custom URI schemes (`tauri://`,
//! `asset://`, `blob:`) inside the AppImage with MEDIA_ERR_SRC_NOT_SUPPORTED.
//! `http://127.0.0.1` uses souphttpsrc; `file://` uses filesrc.

use std::{
  fs::File,
  io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom, Write},
  net::{TcpListener, TcpStream},
  path::{Component, Path, PathBuf},
  thread,
  time::Duration,
};

use tauri::{AppHandle, Manager};

pub struct FatCatMedia {
  pub origin: String,
  pub videos_dir: PathBuf,
}

pub fn find_videos_dir(app: &AppHandle) -> Option<PathBuf> {
  let mut dirs = Vec::new();
  if let Ok(resource) = app.path().resource_dir() {
    dirs.push(resource.join("videos"));
    dirs.push(resource);
  }
  if let Ok(exe) = std::env::current_exe() {
    if let Some(bin) = exe.parent() {
      dirs.push(bin.join("../lib/Take A Moment/videos"));
      dirs.push(bin.join("../lib/take-a-moment/videos"));
      dirs.push(bin.join("videos"));
    }
  }
  for dir in dirs {
    let Ok(canonical) = dir.canonicalize() else {
      continue;
    };
    if canonical.join("neko2.webm").is_file() {
      return Some(canonical);
    }
  }
  None
}

pub fn file_url(path: &Path) -> String {
  let mut url = String::from("file://");
  for component in path.components() {
    match component {
      Component::RootDir => {}
      Component::Normal(part) => {
        url.push('/');
        url.push_str(&encode_path_segment(&part.to_string_lossy()));
      }
      _ => {}
    }
  }
  url
}

pub fn start(videos_dir: PathBuf) -> Result<FatCatMedia, String> {
  let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
  let port = listener.local_addr().map_err(|e| e.to_string())?.port();
  let origin = format!("http://127.0.0.1:{port}");
  let dir_for_thread = videos_dir.clone();
  thread::spawn(move || {
    for incoming in listener.incoming() {
      match incoming {
        Ok(stream) => {
          let dir = dir_for_thread.clone();
          thread::spawn(move || {
            if let Err(e) = handle_client(stream, &dir) {
              eprintln!("[fatcat] http {e}");
            }
          });
        }
        Err(e) => eprintln!("[fatcat] http accept {e}"),
      }
    }
  });
  eprintln!("[fatcat] http media {origin}/ dir={}", videos_dir.display());
  Ok(FatCatMedia { origin, videos_dir })
}

fn encode_path_segment(s: &str) -> String {
  let mut out = String::new();
  for b in s.bytes() {
    match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
      _ => out.push_str(&format!("%{b:02X}")),
    }
  }
  out
}

fn allowed_file(videos_dir: &Path, url_path: &str) -> Option<PathBuf> {
  let path_only = url_path.split(['?', '#']).next().unwrap_or(url_path);
  let name = path_only.trim_start_matches('/');
  if !matches!(name, "neko1.webm" | "neko2.webm") {
    return None;
  }
  let path = videos_dir.join(name);
  path.is_file().then_some(path)
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
  headers
    .iter()
    .find(|(k, _)| k.eq_ignore_ascii_case(name))
    .map(|(_, v)| v.as_str())
}

/// Returns (start, end inclusive, is_partial).
fn parse_range(header: Option<&str>, size: u64) -> Result<(u64, u64, bool), &'static str> {
  if size == 0 {
    // RFC 7233: a Range against a zero-length representation is unsatisfiable.
    return match header {
      Some(raw) if raw.trim().starts_with("bytes=") => Err("unsatisfiable"),
      _ => Ok((0, 0, false)),
    };
  }
  let last = size - 1;
  let Some(raw) = header else {
    return Ok((0, last, false));
  };
  let spec = raw.trim().strip_prefix("bytes=").ok_or("bad range")?;
  let spec = spec.split(',').next().unwrap_or(spec).trim();
  if let Some(start_str) = spec
    .strip_suffix('-')
    .filter(|s| !s.is_empty() && !s.contains('-'))
  {
    let start: u64 = start_str.parse().map_err(|_| "bad range")?;
    if start >= size {
      return Err("unsatisfiable");
    }
    return Ok((start, last, true));
  }
  if let Some(suffix) = spec.strip_prefix('-') {
    let len: u64 = suffix.parse().map_err(|_| "bad range")?;
    // RFC 7233: suffix-length 0 is unsatisfiable (e.g. bytes=-0).
    if len == 0 {
      return Err("unsatisfiable");
    }
    let start = size.saturating_sub(len);
    return Ok((start, last, true));
  }
  let (start_str, end_str) = spec.split_once('-').ok_or("bad range")?;
  let start: u64 = start_str.parse().map_err(|_| "bad range")?;
  if start >= size {
    return Err("unsatisfiable");
  }
  let end = if end_str.is_empty() {
    last
  } else {
    end_str.parse::<u64>().map_err(|_| "bad range")?.min(last)
  };
  if end < start {
    return Err("bad range");
  }
  Ok((start, end, true))
}

fn read_request(stream: &TcpStream) -> Result<(String, String, Vec<(String, String)>), String> {
  stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
  let cloned = stream.try_clone().map_err(|e| e.to_string())?;
  let mut reader = BufReader::new(cloned);
  let mut request_line = String::new();
  reader
    .read_line(&mut request_line)
    .map_err(|e| e.to_string())?;
  let mut parts = request_line.split_whitespace();
  let method = parts.next().ok_or("empty request")?.to_string();
  let path = parts.next().ok_or("no path")?.to_string();
  let mut headers = Vec::new();
  loop {
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.is_empty() || line == "\n" || line == "\r\n" {
      break;
    }
    if let Some((k, v)) = line.split_once(':') {
      headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    if headers.len() > 80 {
      break;
    }
  }
  Ok((method, path, headers))
}

fn write_all(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
  stream.write_all(bytes).map_err(|e| e.to_string())
}

fn cors_and_close() -> &'static str {
  concat!(
    "Access-Control-Allow-Origin: *\r\n",
    "Access-Control-Allow-Headers: Range\r\n",
    "Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n",
    "Access-Control-Expose-Headers: Content-Length, Content-Range, Accept-Ranges\r\n",
    "Accept-Ranges: bytes\r\n",
    "Connection: close\r\n",
  )
}

fn handle_client(mut stream: TcpStream, videos_dir: &Path) -> Result<(), String> {
  let (method, path, headers) = read_request(&stream)?;
  if method == "OPTIONS" {
    write_all(
      &mut stream,
      format!("HTTP/1.1 204 No Content\r\n{}\r\n", cors_and_close()).as_bytes(),
    )?;
    return Ok(());
  }
  if method != "GET" && method != "HEAD" {
    write_all(
      &mut stream,
      format!(
        "HTTP/1.1 405 Method Not Allowed\r\n{}\r\n",
        cors_and_close()
      )
      .as_bytes(),
    )?;
    return Ok(());
  }
  let Some(file_path) = allowed_file(videos_dir, &path) else {
    write_all(
      &mut stream,
      format!("HTTP/1.1 404 Not Found\r\n{}\r\n", cors_and_close()).as_bytes(),
    )?;
    return Ok(());
  };
  let mut file = File::open(&file_path).map_err(|e| e.to_string())?;
  let size = file.metadata().map_err(|e| e.to_string())?.len();
  let (start, end, partial) = match parse_range(header_value(&headers, "range"), size) {
    Ok(v) => v,
    Err("unsatisfiable") => {
      let body = format!(
        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\n{}\r\n",
        cors_and_close()
      );
      write_all(&mut stream, body.as_bytes())?;
      return Ok(());
    }
    Err(_) => (0, size.saturating_sub(1), false),
  };
  let content_len = if size == 0 { 0 } else { end - start + 1 };
  eprintln!("[fatcat] http {method} {path} range={start}-{end}/{size}");
  let status = if partial {
    "206 Partial Content"
  } else {
    "200 OK"
  };
  let mut head = format!(
    "HTTP/1.1 {status}\r\nContent-Type: video/webm\r\nContent-Length: {content_len}\r\n{}",
    cors_and_close()
  );
  if partial && size > 0 {
    head.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
  }
  head.push_str("\r\n");
  write_all(&mut stream, head.as_bytes())?;
  if method == "HEAD" || content_len == 0 {
    return Ok(());
  }
  file
    .seek(SeekFrom::Start(start))
    .map_err(|e| e.to_string())?;
  let mut limited = file.take(content_len);
  // Client abort (seek / overlay close) is a localhost TCP reset, not WAN.
  // WebKit already notices a short body; log anything else.
  match std::io::copy(&mut limited, &mut stream) {
    Ok(_) => Ok(()),
    Err(e)
      if matches!(
        e.kind(),
        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
      ) =>
    {
      Ok(())
    }
    Err(e) => Err(e.to_string()),
  }
}
