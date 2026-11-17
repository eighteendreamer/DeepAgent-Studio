//! Per-connection runtime state.

use super::config::{SshConnectionConfig, SshStatus};
use super::transport::SshClient as Client;
use deepagent_terminal::TerminalReadChunk;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

#[cfg(test)]
#[path = "session_audit.rs"]
mod audit;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshPtyHandle {
    pub connection_id: String,
    pub token: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshStatusSnapshot {
    pub status: SshStatus,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
}

impl Default for SshStatusSnapshot {
    fn default() -> Self {
        Self {
            status: SshStatus::Disconnected,
            last_error: None,
            latency_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshTestResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub banner: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDirListing {
    pub connection_id: String,
    pub path: String,
    pub canonical_path: String,
    pub entries: Vec<SshDirEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshFileContent {
    pub connection_id: String,
    pub path: String,
    pub size: Option<u64>,
    pub truncated: bool,
    pub is_binary: bool,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshFileBinary {
    pub connection_id: String,
    pub path: String,
    pub size: Option<u64>,
    pub truncated: bool,
    pub data_base64: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PtyCommand {
    Resize { cols: u16, rows: u16 },
}

pub struct PtyState {
    pub output_signal: deepagent_terminal::TerminalOutputSignal,
    pub stdin: mpsc::Sender<Vec<u8>>,
    pub commands: mpsc::UnboundedSender<PtyCommand>,
    pub stdout: Mutex<mpsc::Receiver<Vec<u8>>>,
    pub join: JoinHandle<()>,
    pub channel: Option<Arc<russh::ChannelWriteHalf<russh::client::Msg>>>,
    pub cols: u16,
    pub rows: u16,
    pub output_cursor: AtomicU64,
    pub history: Mutex<VecDeque<(u64, Vec<u8>)>>,
    pub history_bytes: Mutex<usize>,
}

pub struct SshSession {
    pub(crate) host_trust: RwLock<Option<super::host_trust::HostTrust>>,
    config: SshConnectionConfig,
    client: RwLock<Option<Client>>,
    ptys: RwLock<HashMap<String, Arc<PtyState>>>,
    legacy_pty: RwLock<Option<String>>,
    status: RwLock<SshStatus>,
    last_error: RwLock<Option<String>>,
    keepalive_running: AtomicBool,
    last_keepalive_ms: AtomicU64,
}

impl SshSession {
    pub fn new(config: SshConnectionConfig) -> Arc<Self> {
        Arc::new(Self {
            host_trust: RwLock::new(None),
            config,
            client: RwLock::new(None),
            ptys: RwLock::new(HashMap::new()),
            legacy_pty: RwLock::new(None),
            status: RwLock::new(SshStatus::Disconnected),
            last_error: RwLock::new(None),
            keepalive_running: AtomicBool::new(false),
            last_keepalive_ms: AtomicU64::new(0),
        })
    }

    pub fn key(&self) -> &str {
        &self.config.id
    }

    pub async fn client(&self) -> Option<Client> {
        self.client.read().await.clone()
    }

    pub async fn set_client(&self, client: Option<Client>) {
        *self.client.write().await = client;
    }

    pub async fn status(&self) -> SshStatus {
        *self.status.read().await
    }

    pub async fn last_error(&self) -> Option<String> {
        self.last_error.read().await.clone()
    }

    pub async fn set_status(&self, status: SshStatus, error: Option<String>) {
        *self.status.write().await = status;
        *self.last_error.write().await = error;
    }

    pub fn touch_keepalive(&self) {
        self.last_keepalive_ms.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
    }

    pub fn is_keepalive_running(&self) -> bool {
        self.keepalive_running.load(Ordering::Relaxed)
    }

    pub fn set_keepalive_running(&self, running: bool) {
        self.keepalive_running.store(running, Ordering::Relaxed);
    }

    pub async fn insert_pty(&self, token: String, next: PtyState, legacy: bool) {
        self.ptys
            .write()
            .await
            .insert(token.clone(), Arc::new(next));
        if legacy {
            *self.legacy_pty.write().await = Some(token);
        }
    }

    async fn pty(&self, token: &str) -> Option<Arc<PtyState>> {
        let alias = if token == self.config.id {
            self.legacy_pty.read().await.clone()
        } else {
            None
        };
        self.ptys
            .read()
            .await
            .get(alias.as_deref().unwrap_or(token))
            .cloned()
    }

    pub(super) async fn pty_running(&self, token: &str) -> bool {
        self.pty(token)
            .await
            .is_some_and(|pty| !pty.join.is_finished())
    }

    pub(super) async fn subscribe_output(
        &self,
        token: &str,
    ) -> Option<deepagent_terminal::TerminalOutputSubscription> {
        Some(self.pty(token).await?.output_signal.subscribe())
    }

    pub async fn close_pty(&self, token: &str) -> crate::SshResult<()> {
        let Some(state) = self.pty(token).await else {
            return Ok(());
        };
        if let Some(channel) = &state.channel {
            tokio::time::timeout(std::time::Duration::from_secs(3), channel.close())
                .await
                .map_err(|_| crate::SshError::Pty("closing PTY channel timed out".into()))?
                .map_err(|error| crate::SshError::Pty(format!("close PTY channel: {error}")))?;
        }
        let mut ptys = self.ptys.write().await;
        ptys.retain(|_, value| !Arc::ptr_eq(value, &state));
        Ok(())
    }

    pub async fn close_all_ptys(&self) {
        self.ptys.write().await.clear();
        *self.legacy_pty.write().await = None;
    }

    pub async fn pty_write(&self, token: &str, data: Vec<u8>) -> bool {
        let Some(pty) = self.pty(token).await else {
            return false;
        };
        pty.stdin.send(data).await.is_ok()
    }

    pub async fn pty_read_available(&self, token: &str) -> Option<Vec<u8>> {
        let pty = self.pty(token).await?;
        let mut rx = pty.stdout.lock().await;
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
            if out.len() >= 64 * 1024 {
                break;
            }
        }
        Some(out)
    }

    pub async fn pty_read_with_cursor(
        &self,
        token: &str,
        after_cursor: u64,
    ) -> Option<TerminalReadChunk> {
        let pty = self.pty(token).await?;
        let mut rx = pty.stdout.lock().await;
        let mut history = pty.history.lock().await;
        let mut history_bytes = pty.history_bytes.lock().await;
        while let Ok(chunk) = rx.try_recv() {
            if chunk.is_empty() {
                continue;
            }
            let start = pty
                .output_cursor
                .fetch_add(chunk.len() as u64, Ordering::AcqRel);
            *history_bytes += chunk.len();
            history.push_back((start, chunk));
            while *history_bytes > 256 * 1024 {
                if let Some((_, old)) = history.pop_front() {
                    *history_bytes = history_bytes.saturating_sub(old.len());
                } else {
                    break;
                }
            }
        }
        let current = pty.output_cursor.load(Ordering::Acquire);
        let oldest = history.front().map(|(start, _)| *start).unwrap_or(current);
        let truncated = after_cursor < oldest && oldest > 0;
        let read_from = after_cursor.max(oldest);
        let mut data = Vec::new();
        let mut next_cursor = read_from;
        for (start, chunk) in history.iter() {
            let end = *start + chunk.len() as u64;
            if end <= read_from {
                continue;
            }
            let offset = read_from.saturating_sub(*start) as usize;
            let offset = offset.min(chunk.len());
            let remaining = 64 * 1024 - data.len();
            let take = remaining.min(chunk.len() - offset);
            data.extend_from_slice(&chunk[offset..offset + take]);
            next_cursor = (*start + offset as u64) + take as u64;
            if data.len() == 64 * 1024 {
                break;
            }
        }
        let eof = rx.is_closed() && rx.is_empty() && next_cursor >= current;
        Some(TerminalReadChunk {
            eof,
            cursor: if data.is_empty() {
                current
            } else {
                next_cursor
            },
            data,
            truncated,
        })
    }

    pub async fn pty_resize(&self, token: &str, cols: u16, rows: u16) -> bool {
        let Some(pty) = self.pty(token).await else {
            return false;
        };
        pty.commands.send(PtyCommand::Resize { cols, rows }).is_ok()
    }

    pub fn config(&self) -> &SshConnectionConfig {
        &self.config
    }
}

// A cancelled open or a disconnected owner must not leave a detached shell task.
// russh 0.55 dropping Channel alone does not send SSH_MSG_CHANNEL_CLOSE.
impl Drop for PtyState {
    fn drop(&mut self) {
        self.join.abort();
        if let (Some(channel), Ok(runtime)) =
            (self.channel.clone(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                let _ = channel.close().await;
            });
        }
    }
}
