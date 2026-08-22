//! A native WakaTime heartbeat client (no `wakatime-cli`): the editing events
//! the session already owns become throttled heartbeats on a dedicated worker
//! thread, POSTed in bulk over the existing rustls HTTP stack.
//!
//! Interop over invention: the API key and endpoint come from the standard
//! `$WAKATIME_HOME/.wakatime.cfg` (`[settings] api_key`, `api_url`), so an
//! existing WakaTime/Wakapi/Hackatime install works unchanged. Heartbeats
//! that fail to send queue as JSON lines in the karet cache directory and
//! flush with the next successful batch. Everything is **off** unless
//! `wakatime.enabled` is set — sending your filenames to a service is opt-in.

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::time::Duration;

use secrecy::ExposeSecret;
use secrecy::SecretString;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;

/// The write gate: a plain typing heartbeat for the same file is suppressed
/// for this long (WakaTime's standard two-minute rule).
const SAME_FILE_INTERVAL: Duration = Duration::from_secs(120);
/// Batches are flushed to the API at most this often.
const SEND_INTERVAL: Duration = Duration::from_secs(120);
/// The status-bar "today" total refreshes this often.
const STATUS_INTERVAL: Duration = Duration::from_secs(300);
/// The offline queue stops growing past this many heartbeats.
const QUEUE_CAP: usize = 1_000;

/// One editing signal from the session.
pub(crate) struct Beat {
    /// The absolute file path (the WakaTime `entity`).
    pub path: PathBuf,
    /// Display language, when known.
    pub language: Option<&'static str>,
    /// Total lines in the file.
    pub lines: usize,
    /// Whether this was a save.
    pub is_write: bool,
    /// The repository branch, when known.
    pub branch: Option<String>,
    /// The project name (workspace root directory name), when known.
    pub project: Option<String>,
}

/// The standard WakaTime configuration, read once at spawn.
#[derive(Clone, Debug)]
pub(crate) struct WakaConfig {
    /// The API key (absent = the worker never sends).
    pub api_key: Option<SecretString>,
    /// The API base, default `https://api.wakatime.com/api/v1`.
    pub api_url: String,
}

impl WakaConfig {
    /// Load `$WAKATIME_HOME/.wakatime.cfg` (falling back to `$HOME`).
    pub(crate) fn discover() -> Self {
        let home = std::env::var_os("WAKATIME_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let text = home
            .map(|dir| dir.join(".wakatime.cfg"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        Self::parse(&text)
    }

    /// Parse the `[settings]` section of a `.wakatime.cfg`.
    pub(crate) fn parse(text: &str) -> Self {
        let mut in_settings = false;
        let mut api_key = None;
        let mut api_url = "https://api.wakatime.com/api/v1".to_owned();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_settings = line == "[settings]";
                continue;
            }
            if !in_settings {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "api_key" if !value.is_empty() => {
                    api_key = Some(SecretString::from(value.to_owned()));
                },
                "api_url" if !value.is_empty() => {
                    api_url = value.trim_end_matches('/').to_owned();
                },
                _ => {},
            }
        }
        Self { api_key, api_url }
    }
}

/// The per-file throttle: `true` when a beat for `path` should be kept.
/// Saves and first-seen files always pass; a repeat of the same file within
/// [`SAME_FILE_INTERVAL`] is dropped. Pure, clock-injected.
pub(crate) fn keep_beat(
    last: Option<(&Path, Duration)>,
    path: &Path,
    is_write: bool,
    now: Duration,
) -> bool {
    if is_write {
        return true;
    }
    match last {
        Some((last_path, at)) if last_path == path => now.saturating_sub(at) >= SAME_FILE_INTERVAL,
        _ => true,
    }
}

/// Serialize one heartbeat as the API's JSON object. `time` is UNIX seconds.
pub(crate) fn heartbeat_json(beat: &Beat, time: f64) -> serde_json::Value {
    let mut object = serde_json::json!({
        "entity": beat.path.to_string_lossy(),
        "type": "file",
        "category": "coding",
        "time": time,
        "is_write": beat.is_write,
        "lines": beat.lines,
        "user_agent": concat!("karet/", env!("CARGO_PKG_VERSION"), " karet-wakatime"),
    });
    if let Some(language) = beat.language {
        object["language"] = serde_json::Value::from(language);
    }
    if let Some(branch) = &beat.branch {
        object["branch"] = serde_json::Value::from(branch.clone());
    }
    if let Some(project) = &beat.project {
        object["project"] = serde_json::Value::from(project.clone());
    }
    object
}

/// Where undelivered heartbeats wait for connectivity.
fn queue_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "karet")
        .map(|dirs| dirs.cache_dir().join("wakatime-queue.jsonl"))
}

/// Start the worker; the session sends [`Beat`]s and the status-bar text
/// arrives as unsolicited [`Event::WakatimeStatus`] events.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<Beat> {
    let (beats_tx, beats_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-wakatime".to_owned())
        .spawn(move || run(&beats_rx, &events));
    beats_tx
}

fn run(beats: &Receiver<Beat>, events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>) {
    let config = WakaConfig::discover();
    let Some(api_key) = config.api_key.clone() else {
        // No key, nothing to do — drain silently so senders never block.
        while beats.recv().is_ok() {}
        return;
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .ok();
    let Some(client) = client else {
        while beats.recv().is_ok() {}
        return;
    };
    let started = std::time::Instant::now();
    let mut last_beat: Option<(PathBuf, Duration)> = None;
    let mut pending: Vec<serde_json::Value> = load_queue();
    let mut last_send = Duration::ZERO;
    let mut last_status = Duration::ZERO;
    loop {
        // Wake at least every 30s so sends and the status refresh keep their
        // cadence during quiet stretches.
        let beat = beats.recv_timeout(Duration::from_secs(30));
        let now = started.elapsed();
        match beat {
            Ok(beat) => {
                let keep = keep_beat(
                    last_beat.as_ref().map(|(p, at)| (p.as_path(), *at)),
                    &beat.path,
                    beat.is_write,
                    now,
                );
                if keep {
                    last_beat = Some((beat.path.clone(), now));
                    let time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    if pending.len() < QUEUE_CAP {
                        pending.push(heartbeat_json(&beat, time));
                    }
                }
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                save_queue(&pending);
                return;
            },
        }
        if !pending.is_empty() && now.saturating_sub(last_send) >= SEND_INTERVAL {
            last_send = now;
            if send_bulk(&client, &config.api_url, &api_key, &pending) {
                pending.clear();
                let _ = std::fs::remove_file(queue_path().unwrap_or_default());
            } else {
                save_queue(&pending);
            }
        }
        if now.saturating_sub(last_status) >= STATUS_INTERVAL {
            last_status = now;
            if let Some(text) = fetch_today(&client, &config.api_url, &api_key)
                && events.send((None, Event::WakatimeStatus { text })).is_err()
            {
                save_queue(&pending);
                return;
            }
        }
    }
}

/// POST one bulk batch; `true` on acceptance.
fn send_bulk(
    client: &reqwest::blocking::Client,
    api_url: &str,
    api_key: &SecretString,
    beats: &[serde_json::Value],
) -> bool {
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(api_key.expose_secret());
    client
        .post(format!("{api_url}/users/current/heartbeats.bulk"))
        .header("Authorization", format!("Basic {auth}"))
        .json(&beats)
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// GET today's coding-time text for the status bar.
fn fetch_today(
    client: &reqwest::blocking::Client,
    api_url: &str,
    api_key: &SecretString,
) -> Option<String> {
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(api_key.expose_secret());
    let body: serde_json::Value = client
        .get(format!("{api_url}/users/current/statusbar/today"))
        .header("Authorization", format!("Basic {auth}"))
        .send()
        .ok()?
        .json()
        .ok()?;
    let text = body
        .get("data")
        .and_then(|data| data.get("grand_total"))
        .and_then(|total| total.get("text"))
        .and_then(|text| text.as_str())?;
    Some(text.to_owned())
}

fn load_queue() -> Vec<serde_json::Value> {
    queue_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn save_queue(pending: &[serde_json::Value]) {
    let Some(path) = queue_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lines: Vec<String> = pending
        .iter()
        .filter_map(|beat| serde_json::to_string(beat).ok())
        .collect();
    let _ = std::fs::write(path, lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_section_parses_key_and_url() {
        let config = WakaConfig::parse(
            "[other]\napi_key = wrong\n[settings]\napi_key = waka_abc\napi_url = https://wakapi.dev/api/\n",
        );
        assert!(config.api_key.is_some());
        assert_eq!(config.api_url, "https://wakapi.dev/api");
        let empty = WakaConfig::parse("");
        assert!(empty.api_key.is_none());
        assert_eq!(empty.api_url, "https://api.wakatime.com/api/v1");
    }

    #[test]
    fn the_two_minute_gate_suppresses_only_same_file_repeats() {
        let a = Path::new("/w/a.rs");
        let b = Path::new("/w/b.rs");
        let at = Duration::from_secs(1_000);
        // First beat, other files, and saves always pass.
        assert!(keep_beat(None, a, false, at));
        assert!(keep_beat(
            Some((a, at)),
            b,
            false,
            at + Duration::from_secs(1)
        ));
        assert!(keep_beat(
            Some((a, at)),
            a,
            true,
            at + Duration::from_secs(1)
        ));
        // A same-file repeat inside two minutes is dropped, after it passes.
        assert!(!keep_beat(
            Some((a, at)),
            a,
            false,
            at + Duration::from_secs(119)
        ));
        assert!(keep_beat(
            Some((a, at)),
            a,
            false,
            at + Duration::from_secs(120)
        ));
    }

    #[test]
    fn heartbeats_serialize_the_documented_fields() {
        let beat = Beat {
            path: PathBuf::from("/w/src/main.rs"),
            language: Some("Rust"),
            lines: 42,
            is_write: true,
            branch: Some("main".to_owned()),
            project: Some("w".to_owned()),
        };
        let json = heartbeat_json(&beat, 1_700_000_000.5);
        assert_eq!(json["entity"], "/w/src/main.rs");
        assert_eq!(json["type"], "file");
        assert_eq!(json["is_write"], true);
        assert_eq!(json["lines"], 42);
        assert_eq!(json["language"], "Rust");
        assert_eq!(json["branch"], "main");
        assert_eq!(json["project"], "w");
        assert!(
            json["user_agent"]
                .as_str()
                .is_some_and(|ua| ua.starts_with("karet/"))
        );
    }
    #[test]
    fn a_secret_key_is_redacted_by_debug() {
        // `WakaConfig` derives Debug and is reachable from tracing/panic output,
        // so the key must never render.
        let config = WakaConfig::parse("[settings]\napi_key = waka_supersecret_value\n");
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("waka_supersecret_value"),
            "the key leaked into Debug output: {rendered}"
        );
    }

    #[test]
    fn the_persisted_queue_carries_heartbeats_not_credentials() {
        // The offline queue lands on disk, so it must hold only payloads.
        let beat = Beat {
            path: PathBuf::from("/repo/src/main.rs"),
            language: Some("Rust"),
            lines: 12,
            is_write: false,
            branch: Some("main".to_owned()),
            project: Some("repo".to_owned()),
        };
        let json = heartbeat_json(&beat, 1.0);
        let text = json.to_string();
        assert!(!text.contains("api_key"), "{text}");
        assert!(!text.contains("Authorization"), "{text}");
        assert!(text.contains("main.rs"), "{text}");
    }
}
