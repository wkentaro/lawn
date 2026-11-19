use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::hc::Core;

pub(super) const HOLD_LIMIT: Duration = Duration::from_secs(30);
pub(super) const POLL: Duration = Duration::from_millis(200);
const ATTEMPTS: usize = 8;

pub(crate) struct Pace {
    pub(super) spacing: Duration,
    pub(super) elapsed: Box<dyn Fn() -> Duration + Send>,
    pub(super) wait: Box<dyn FnMut(Duration) -> bool + Send>,
}

impl Pace {
    pub(crate) fn new(spacing: Duration, stop: std::sync::mpsc::Receiver<()>) -> Self {
        let start = Instant::now();
        Self {
            spacing,
            elapsed: Box::new(move || start.elapsed()),
            wait: Box::new(move |wait| {
                matches!(
                    stop.recv_timeout(wait),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                )
            }),
        }
    }
}

pub(crate) fn drain<T>(
    queue: std::sync::mpsc::Receiver<T>,
    mut pace: Pace,
    mut each: impl FnMut(T, &mut Pace),
) {
    for request in queue {
        each(request, &mut pace);
    }
}

pub(super) trait Board {
    fn load(&self) -> Result<(Value, u64), String>;
    fn save(&self, value: &Value, version: u64) -> Result<bool, String>;
}

pub(super) struct CoreBoard<'a>(pub(super) &'a Core);

fn board_name() -> String {
    format!("{}-resume-pace", crate::config::app_dir_name())
}

impl Board for CoreBoard<'_> {
    fn load(&self) -> Result<(Value, u64), String> {
        let document = self
            .0
            .doc_get(&board_name(), None)
            .map_err(|error| error.to_string())?;
        Ok((document.value, document.version))
    }

    fn save(&self, value: &Value, version: u64) -> Result<bool, String> {
        match self.0.doc_set(&board_name(), None, value, version) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == Some("conflict") => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn holder_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        "{}-{}-{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn entry(document: &Value, session: &str) -> Value {
    document.get(session).cloned().unwrap_or(Value::Null)
}

fn with_entry(document: Value, session: &str, entry: Value) -> Value {
    let mut document = if document.is_object() {
        document
    } else {
        json!({})
    };
    document[session] = entry;
    document
}

fn wait_before_taking(entry: &Value, unchanged_for: Duration, spacing: Duration) -> Duration {
    if entry.is_null() {
        Duration::ZERO
    } else if entry.get("holder").is_some() {
        HOLD_LIMIT.saturating_sub(unchanged_for).min(POLL)
    } else {
        spacing.saturating_sub(unchanged_for)
    }
}

fn take_turn(board: &impl Board, session: &str, pace: &mut Pace) -> Result<Option<String>, String> {
    let mut seen: Option<(Value, Duration)> = None;
    loop {
        let (document, version) = board.load()?;
        let current = entry(&document, session);
        let now = (pace.elapsed)();
        let since = match &seen {
            Some((entry, at)) if *entry == current => now.saturating_sub(*at),
            _ => {
                seen = Some((current.clone(), now));
                Duration::ZERO
            }
        };
        let wait = wait_before_taking(&current, since, pace.spacing);
        if !(pace.wait)(wait) {
            return Ok(None);
        }
        if !wait.is_zero() {
            continue;
        }
        let id = holder_id();
        let sent = current.get("sent").and_then(Value::as_u64).unwrap_or(0);
        let taken = with_entry(document, session, json!({"sent": sent, "holder": id}));
        if board.save(&taken, version)? {
            return Ok(Some(id));
        }
    }
}

pub(super) fn with_turn(
    board: &impl Board,
    session: &str,
    pace: &mut Pace,
    send: impl FnOnce() -> Result<bool, String>,
) -> Result<(), String> {
    let Some(holder) = take_turn(board, session, pace)? else {
        return Ok(());
    };
    let sent = if (pace.wait)(Duration::ZERO) {
        send()
    } else {
        Ok(false)
    };
    let ended = end_turn(board, session, &holder, sent == Ok(true));
    sent?;
    ended
}

fn end_turn(
    board: &impl Board,
    session: &str,
    holder: &str,
    attempted: bool,
) -> Result<(), String> {
    for _ in 0..ATTEMPTS {
        let (document, version) = board.load()?;
        let current = entry(&document, session);
        let mine = current["holder"].as_str() == Some(holder);
        if !mine && !attempted {
            return Ok(());
        }
        let sent = current.get("sent").and_then(Value::as_u64).unwrap_or(0) + u64::from(attempted);
        let mut ended = json!({"sent": sent});
        if !mine {
            if let Some(other) = current.get("holder") {
                ended["holder"] = other.clone();
            }
        }
        if board.save(&with_entry(document, session, ended), version)? {
            return Ok(());
        }
    }
    Err(format!("{} kept changing", board_name()))
}

#[cfg(test)]
mod tests;
