//! Live syslog streaming as `orchard://syslog` events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::events::Events;
use tokio::task::JoinHandle;

use crate::dto::{SyslogEnded, SyslogLevel, SyslogLine};
use crate::error::CmdResult;
use crate::events::{emit_syslog, emit_syslog_ended};
use crate::state::DeviceState;

/// The running syslog stream, at most one. Managed Tauri state.
///
/// Every start and stop bumps `generation`; a start only installs its stream
/// when no newer start or stop happened while it was connecting.
#[derive(Default)]
pub struct SyslogState {
    generation: AtomicU64,
    stream: Mutex<Option<JoinHandle<()>>>,
}

impl SyslogState {
    /// Stops the running stream and returns the new generation.
    pub fn bump(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let old = self.slot().take();
        if let Some(old) = old {
            old.abort();
        }
        generation
    }

    /// Starts `spawn()` as the running stream if `generation` is still current.
    /// Returns false (spawning nothing) when it was superseded.
    pub fn install(&self, generation: u64, spawn: impl FnOnce() -> JoinHandle<()>) -> bool {
        let mut slot = self.slot();
        if self.generation.load(Ordering::SeqCst) != generation {
            return false;
        }
        *slot = Some(spawn());
        true
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.stream.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn level(tag: &str) -> SyslogLevel {
    match tag.to_ascii_lowercase().as_str() {
        "notice" => SyslogLevel::Notice,
        "info" => SyslogLevel::Info,
        "debug" => SyslogLevel::Debug,
        "error" => SyslogLevel::Error,
        "fault" | "critical" | "alert" | "emergency" => SyslogLevel::Fault,
        _ => SyslogLevel::Default,
    }
}

/// Parses `Oct 19 10:20:47 iPhone process(Lib)[123] <Notice>: message`.
/// Lines in another shape (continuations) keep the whole text as the message.
pub fn parse_syslog(udid: &str, line: &str) -> SyslogLine {
    let raw = SyslogLine {
        udid: udid.to_owned(),
        timestamp: String::new(),
        process: String::new(),
        pid: None,
        level: SyslogLevel::Default,
        message: line.to_owned(),
    };
    let Some(timestamp) = line.get(..15) else {
        return raw;
    };
    let mut rest = line[15..].trim_start().splitn(3, ' ');
    let (Some(_device), Some(proc_part), Some(tail)) = (rest.next(), rest.next(), rest.next())
    else {
        return raw;
    };
    let Some(open) = proc_part.rfind('[') else {
        return raw;
    };
    let pid = proc_part[open + 1..].trim_end_matches(']').parse().ok();
    let process = proc_part[..open]
        .split('(')
        .next()
        .unwrap_or_default()
        .to_owned();
    let (level_tag, message) = match tail.strip_prefix('<').and_then(|t| t.split_once(">:")) {
        Some((tag, msg)) => (tag, msg.trim_start()),
        None => ("", tail),
    };
    SyslogLine {
        udid: udid.to_owned(),
        timestamp: timestamp.to_owned(),
        process,
        pid,
        level: level(level_tag),
        message: message.to_owned(),
    }
}

/// Starts streaming a device's syslog, replacing any previous stream. Lines
/// are emitted until `stop_syslog` or the next `start_syslog`; if the device
/// ends the stream, `orchard://syslog-status` reports it.
pub async fn start_syslog(
    app: Events,
    state: &SyslogState,
    devices: &Arc<DeviceState>,
    udid: String,
) -> CmdResult<()> {
    let generation = state.bump();
    let mut stream = devices.connection(&udid).await?.syslog().await?;
    state.install(generation, move || {
        tokio::spawn(async move {
            let message = loop {
                match stream.next_line().await {
                    Some(Ok(line)) => {
                        if !line.trim().is_empty() {
                            emit_syslog(&app, parse_syslog(&udid, &line));
                        }
                    }
                    Some(Err(e)) => break Some(e.to_string()),
                    None => break None,
                }
            };
            emit_syslog_ended(&app, SyslogEnded { udid, message });
        })
    });
    Ok(())
}

pub async fn stop_syslog(state: &SyslogState) -> CmdResult<()> {
    state.bump();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_line() {
        let l = parse_syslog(
            "u",
            "Oct 19 10:20:47 iPhone SpringBoard(FrontBoard)[57] <Notice>: Scene: did update",
        );
        assert_eq!(l.timestamp, "Oct 19 10:20:47");
        assert_eq!(l.process, "SpringBoard");
        assert_eq!(l.pid, Some(57));
        assert_eq!(l.level, SyslogLevel::Notice);
        assert_eq!(l.message, "Scene: did update");
    }

    #[tokio::test]
    async fn stop_during_connect_wins() {
        let state = SyslogState::default();
        let started = state.bump();
        state.bump(); // stop_syslog while the start was still connecting
        assert!(!state.install(started, || tokio::spawn(async {})));
        let current = state.bump();
        assert!(state.install(current, || tokio::spawn(async {})));
    }

    #[test]
    fn keeps_odd_lines_as_message() {
        let l = parse_syslog("u", "  continuation of a message");
        assert_eq!(l.message, "  continuation of a message");
        assert_eq!(l.process, "");
    }
}
