//! Per-connection runtime state.

use super::config::{SshConnectionConfig, SshStatus};
use async_ssh2_tokio::Client;
use deepagent_terminal::TerminalReadChunk;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

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
    pub content: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PtyCommand {
    Resize { cols: u16, rows: u16 },
}

pub struct PtyState {
    pub stdin: mpsc::Sender<Vec<u8>>,
    pub commands: mpsc::UnboundedSender<PtyCommand>,
    /// Shell 输出通道的唯一消费者。轮询模式下为 `Some`；一旦 `pty_take_stdout`
    /// 将所有权移交给推送转发任务，就变为 `None`，此后轮询读取恒为空。
    pub stdout: Mutex<Option<mpsc::Receiver<Vec<u8>>>>,
    pub join: JoinHandle<()>,
    pub cols: u16,
    pub rows: u16,
    pub output_cursor: AtomicU64,
    pub history: Mutex<VecDeque<(u64, Vec<u8>)>>,
    pub history_bytes: Mutex<usize>,
}

pub struct SshSession {
    config: SshConnectionConfig,
    client: RwLock<Option<Client>>,
    pty: RwLock<Option<PtyState>>,
    status: RwLock<SshStatus>,
    last_error: RwLock<Option<String>>,
    keepalive_running: AtomicBool,
    last_keepalive_ms: AtomicU64,
}

impl SshSession {
    pub fn new(config: SshConnectionConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            client: RwLock::new(None),
            pty: RwLock::new(None),
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

    pub async fn replace_pty(&self, next: Option<PtyState>) {
        let mut guard = self.pty.write().await;
        if let Some(old) = std::mem::replace(&mut *guard, next) {
            old.join.abort();
        }
    }

    pub async fn pty_write(&self, data: Vec<u8>) -> bool {
        let pty = self.pty.read().await;
        let Some(pty) = pty.as_ref() else {
            return false;
        };
        pty.stdin.send(data).await.is_ok()
    }

    pub async fn pty_read_available(&self) -> Option<Vec<u8>> {
        let pty = self.pty.read().await;
        let pty = pty.as_ref()?;
        let mut rx = pty.stdout.lock().await;
        let Some(rx) = rx.as_mut() else {
            return Some(Vec::new());
        };
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
            if out.len() >= 64 * 1024 {
                break;
            }
        }
        Some(out)
    }

    /// 将 shell 输出接收端的所有权移交给调用方（推送转发任务）。
    /// 外层 `None` 表示未建立 PTY；内层 `None` 表示已被移交过（转发任务在运行）。
    pub async fn pty_take_stdout(&self) -> Option<Option<mpsc::Receiver<Vec<u8>>>> {
        let pty = self.pty.read().await;
        let pty = pty.as_ref()?;
        let mut stdout = pty.stdout.lock().await;
        Some(stdout.take())
    }

    pub async fn pty_read_with_cursor(&self, after_cursor: u64) -> Option<TerminalReadChunk> {
        let pty = self.pty.read().await;
        let pty = pty.as_ref()?;
        let mut rx = pty.stdout.lock().await;
        let mut history = pty.history.lock().await;
        let mut history_bytes = pty.history_bytes.lock().await;
        if let Some(rx) = rx.as_mut() {
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
        }
        let current = pty.output_cursor.load(Ordering::Acquire);
        let oldest = history.front().map(|(start, _)| *start).unwrap_or(current);
        let truncated = after_cursor < oldest && oldest > 0;
        let mut data = Vec::new();
        for (start, chunk) in history.iter() {
            let end = *start + chunk.len() as u64;
            if end > after_cursor {
                let offset = after_cursor.saturating_sub(*start) as usize;
                data.extend_from_slice(&chunk[offset.min(chunk.len())..]);
                if data.len() >= 64 * 1024 {
                    data.truncate(64 * 1024);
                    break;
                }
            }
        }
        Some(TerminalReadChunk {
            cursor: current,
            data,
            truncated,
        })
    }

    pub async fn pty_resize(&self, cols: u16, rows: u16) -> bool {
        let mut pty = self.pty.write().await;
        let Some(pty) = pty.as_mut() else {
            return false;
        };
        pty.cols = cols;
        pty.rows = rows;
        // Shell task 已结束时发送失败属预期（PTY 通道随之关闭），不改变返回语义。
        let _ = pty.commands.send(PtyCommand::Resize {
            cols: pty.cols,
            rows: pty.rows,
        });
        true
    }

    pub fn config(&self) -> &SshConnectionConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshAuthType;
    use std::collections::HashMap;

    fn test_session() -> Arc<SshSession> {
        SshSession::new(SshConnectionConfig {
            id: "conn-test".into(),
            name: "test".into(),
            host: "127.0.0.1".into(),
            port: 22,
            username: "tester".into(),
            auth_type: SshAuthType::Agent,
            key_path: None,
            password: None,
            extra_options: HashMap::new(),
            control_path: None,
            cached_status: SshStatus::Disconnected,
            cached_last_error: None,
            cached_latency_ms: None,
            cached_checked_at_ms: None,
        })
    }

    fn make_pty() -> (mpsc::Sender<Vec<u8>>, PtyState) {
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(8);
        let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(8);
        let (command_tx, _command_rx) = mpsc::unbounded_channel::<PtyCommand>();
        let state = PtyState {
            stdin: stdin_tx,
            commands: command_tx,
            stdout: Mutex::new(Some(stdout_rx)),
            join: tokio::spawn(async {}),
            cols: 80,
            rows: 24,
            output_cursor: AtomicU64::new(0),
            history: Mutex::new(VecDeque::new()),
            history_bytes: Mutex::new(0),
        };
        (stdout_tx, state)
    }

    #[tokio::test]
    async fn pty_read_available_drains_buffered_output() {
        let session = test_session();
        assert!(session.pty_read_available().await.is_none());

        let (tx, state) = make_pty();
        session.replace_pty(Some(state)).await;

        tx.send(b"abc".to_vec()).await.unwrap();
        assert_eq!(session.pty_read_available().await.unwrap(), b"abc".to_vec());
        assert_eq!(
            session.pty_read_available().await.unwrap(),
            Vec::<u8>::new()
        );
    }

    #[tokio::test]
    async fn pty_take_stdout_transfers_ownership_once() {
        let session = test_session();
        assert!(session.pty_take_stdout().await.is_none());

        let (tx, state) = make_pty();
        session.replace_pty(Some(state)).await;

        tx.send(b"hello".to_vec()).await.unwrap();

        let mut rx = session
            .pty_take_stdout()
            .await
            .expect("pty present")
            .expect("first take");
        assert!(
            session.pty_take_stdout().await.unwrap().is_none(),
            "second take"
        );

        // stdout 被移交后，轮询读取恒为空（消费者已换成推送转发任务）。
        assert_eq!(
            session.pty_read_available().await.unwrap(),
            Vec::<u8>::new()
        );

        // 数据没有丢，仍由新持有者收到。
        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        drop(tx);
        // 全部发送端关闭后，接收端收到 None。
        assert_eq!(rx.recv().await, None);
    }
}
