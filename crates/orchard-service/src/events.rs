//! Event names, task progress reporting and cancellation.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use orchard_core::Progress;
use serde::{Deserialize, Serialize};
use tokio::task::AbortHandle;

use crate::dto::{DeviceEvent, SyslogEnded, SyslogLine, TaskEvent, TaskStatus};
use crate::error::{CmdResult, CommandError};

pub const TASK_EVENT: &str = "orchard://task";
pub const DEVICES_EVENT: &str = "orchard://devices";
pub const SYSLOG_EVENT: &str = "orchard://syslog";
pub const SYSLOG_STATUS_EVENT: &str = "orchard://syslog-status";

/// Where events go: the desktop window, or the server's connected clients.
#[derive(Clone)]
pub struct Events(Arc<dyn Fn(&'static str, serde_json::Value) + Send + Sync>);

impl Events {
    pub fn new(sink: impl Fn(&'static str, serde_json::Value) + Send + Sync + 'static) -> Self {
        Self(Arc::new(sink))
    }
}

fn emit<P: Serialize>(app: &Events, event: &'static str, payload: P) {
    if let Ok(payload) = serde_json::to_value(payload) {
        (app.0)(event, payload);
    }
}

/// Emits `orchard://devices`.
pub fn emit_device(app: &Events, event: DeviceEvent) {
    emit(app, DEVICES_EVENT, event);
}

/// Emits one `orchard://syslog` line.
pub fn emit_syslog(app: &Events, line: SyslogLine) {
    emit(app, SYSLOG_EVENT, line);
}

/// Emits `orchard://syslog-status` when the device ends the stream.
pub fn emit_syslog_ended(app: &Events, ended: SyslogEnded) {
    emit(app, SYSLOG_STATUS_EVENT, ended);
}

/// A client generated task id, validated as `[A-Za-z0-9-]{1,64}` when the
/// command arguments are deserialized, so it is safe in keys and logs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct TaskId(String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TaskId {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        let valid = !s.is_empty()
            && s.len() <= 64
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if valid {
            Ok(Self(s))
        } else {
            Err("invalid task id".into())
        }
    }
}

struct Running {
    abort: AbortHandle,
    cancelled: Arc<AtomicBool>,
}

/// Running tasks keyed by task id. Managed Tauri state.
#[derive(Default)]
pub struct TaskRegistry {
    tasks: Mutex<HashMap<String, Running>>,
}

impl TaskRegistry {
    /// Cancels a running task: sets its flag (seen by blocking work) and aborts
    /// its async task. Returns false when it is unknown.
    pub fn cancel(&self, id: &str) -> bool {
        match self.lock().remove(id) {
            Some(task) => {
                task.cancelled.store(true, Ordering::SeqCst);
                task.abort.abort();
                true
            }
            None => false,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Running>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Progress sink of one task, cheap to clone into callbacks and blocking work.
#[derive(Clone)]
pub struct Reporter {
    app: Option<Events>,
    id: String,
    cancelled: Arc<AtomicBool>,
    stage: Arc<Mutex<String>>,
    note: Arc<Mutex<Option<String>>>,
}

impl Reporter {
    fn new(app: Option<Events>, id: String) -> Self {
        Self {
            app,
            id,
            cancelled: Arc::default(),
            stage: Arc::new(Mutex::new(String::from("prepare"))),
            note: Arc::default(),
        }
    }

    /// Emits a running event for a progress update.
    pub fn progress(&self, p: &Progress) {
        if let Ok(mut stage) = self.stage.lock() {
            stage.clone_from(&p.stage);
        }
        self.send(&p.stage, p.done, p.total, TaskStatus::Running, None);
    }

    /// A callback for the backend crates' `impl FnMut(Progress) + Send`.
    pub fn callback(&self) -> impl FnMut(Progress) + Send + 'static {
        let reporter = self.clone();
        move |p| reporter.progress(&p)
    }

    /// Like [`Reporter::callback`] but adds `offset` to `done` and reports
    /// `total` instead, for one step of a multi file transfer.
    pub fn offset_callback(
        &self,
        offset: u64,
        total: u64,
    ) -> impl FnMut(Progress) + Send + 'static {
        let reporter = self.clone();
        move |p| reporter.progress(&Progress::new(p.stage, offset + p.done, Some(total)))
    }

    /// Message for the final `done` event, e.g. what was skipped.
    pub fn note(&self, message: impl Into<String>) {
        if let Ok(mut note) = self.note.lock() {
            *note = Some(message.into());
        }
    }

    /// True once `cancel_task` was called, for blocking work that cannot be aborted.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// `Err(cancelled)` once the task was cancelled.
    pub fn check(&self) -> CmdResult<()> {
        if self.is_cancelled() {
            Err(CommandError::cancelled())
        } else {
            Ok(())
        }
    }

    fn finish<T>(&self, result: &CmdResult<T>) {
        let stage = self.stage.lock().map(|s| s.clone()).unwrap_or_default();
        match result {
            Ok(_) => {
                let note = self.note.lock().ok().and_then(|n| n.clone());
                self.send(&stage, 0, None, TaskStatus::Done, note)
            }
            Err(e) => self.send(&stage, 0, None, TaskStatus::Error, Some(e.message.clone())),
        }
    }

    fn send(
        &self,
        stage: &str,
        done: u64,
        total: Option<u64>,
        status: TaskStatus,
        message: Option<String>,
    ) {
        if let Some(app) = &self.app {
            emit(
                app,
                TASK_EVENT,
                TaskEvent {
                    task_id: self.id.clone(),
                    stage: stage.to_owned(),
                    done,
                    total,
                    status,
                    message,
                },
            );
        }
    }
}

/// Runs a long task on the async runtime, reporting `orchard://task` events.
///
/// The work is spawned so `cancel_task` can abort it; it then fails with kind
/// `cancelled`. A final `done` or `error` event is always emitted.
pub async fn run_task<T, F, Fut>(
    app: &Events,
    registry: &TaskRegistry,
    task_id: TaskId,
    work: F,
) -> CmdResult<T>
where
    F: FnOnce(Reporter) -> Fut,
    Fut: Future<Output = CmdResult<T>> + Send + 'static,
    T: Send + 'static,
{
    run(Some(app.clone()), registry, task_id.0, work).await
}

async fn run<T, F, Fut>(
    app: Option<Events>,
    registry: &TaskRegistry,
    task_id: String,
    work: F,
) -> CmdResult<T>
where
    F: FnOnce(Reporter) -> Fut,
    Fut: Future<Output = CmdResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let reporter = Reporter::new(app, task_id.clone());
    let handle = tokio::spawn(work(reporter.clone()));
    registry.lock().insert(
        task_id.clone(),
        Running {
            abort: handle.abort_handle(),
            cancelled: reporter.cancelled.clone(),
        },
    );
    let result = match handle.await {
        Ok(result) => result,
        Err(e) if e.is_cancelled() => Err(CommandError::cancelled()),
        Err(e) => Err(CommandError::new("unknown", format!("task failed: {e}"))),
    };
    registry.lock().remove(&task_id);
    let result = if reporter.is_cancelled() && result.is_err() {
        Err(CommandError::cancelled())
    } else {
        result
    };
    reporter.finish(&result);
    result
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn cancel_aborts_the_task() {
        let registry = Arc::new(TaskRegistry::default());
        let r = registry.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(r.cancel("t1"));
        });
        let result: CmdResult<()> = run(None, &registry, "t1".into(), |_| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(())
        })
        .await;
        canceller.await.unwrap();
        assert_eq!(result.unwrap_err().kind, "cancelled");
        assert!(!registry.cancel("t1"));
    }

    #[test]
    fn task_ids_are_validated() {
        let ok: Result<TaskId, _> = serde_json::from_value(serde_json::json!("1b4e28ba-2fa1-11d2"));
        assert!(ok.is_ok());
        for bad in ["", "../etc", "a/b", &"x".repeat(65)] {
            assert!(
                serde_json::from_value::<TaskId>(serde_json::json!(bad)).is_err(),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn result_passes_through() {
        let registry = TaskRegistry::default();
        let result = run(None, &registry, "t2".into(), |rep| async move {
            rep.progress(&Progress::new("upload", 1, Some(2)));
            Ok(7)
        })
        .await;
        assert_eq!(result.unwrap(), 7);
    }
}
