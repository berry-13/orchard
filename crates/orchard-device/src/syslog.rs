use idevice::syslog_relay::SyslogRelayClient;
use orchard_core::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::connect::DeviceConnection;
use crate::error::device_error;

/// Live device syslog. Dropping it, or calling [`SyslogStream::stop`], ends the relay.
#[derive(Debug)]
pub struct SyslogStream {
    rx: mpsc::Receiver<Result<String>>,
    task: JoinHandle<()>,
}

impl SyslogStream {
    /// Next log line. `None` once the stream ended or after an error was delivered.
    pub async fn next_line(&mut self) -> Option<Result<String>> {
        self.rx.recv().await
    }

    pub fn stop(self) {}
}

impl Drop for SyslogStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl DeviceConnection {
    /// Starts syslog_relay. Lines are buffered (up to 1024) until read.
    pub async fn syslog(&self) -> Result<SyslogStream> {
        let mut relay = self.service::<SyslogRelayClient>().await?;
        let (tx, rx) = mpsc::channel(1024);
        let task = tokio::spawn(async move {
            loop {
                let line = relay.next().await.map_err(|e| device_error("syslog", e));
                let failed = line.is_err();
                if tx.send(line).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok(SyslogStream { rx, task })
    }
}
