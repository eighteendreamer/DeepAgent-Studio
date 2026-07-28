//! Normalized user-input envelope and per-session dispatch lease.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Prompt,
    Shell,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputAttachment {
    Text {
        id: String,
        content: String,
    },
    Image {
        id: String,
        path: PathBuf,
        media_type: String,
    },
    File {
        id: String,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Prompt,
    SlashCommand { name: String, arguments: String },
    ShellCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEnvelope {
    pub input_id: String,
    pub session_id: Option<String>,
    pub workspace: PathBuf,
    pub raw_text: String,
    pub effective_text: String,
    pub mode: InputMode,
    pub kind: InputKind,
    pub attachments: Vec<InputAttachment>,
    pub accepted_at: i64,
}

pub struct InputIngress;

impl InputIngress {
    pub fn normalize(
        session_id: Option<String>,
        workspace: impl Into<PathBuf>,
        text: impl Into<String>,
        mode: InputMode,
        attachments: Vec<InputAttachment>,
    ) -> Result<InputEnvelope> {
        let raw_text = text.into();
        let mut effective_text = raw_text.trim().to_string();
        if effective_text.is_empty() {
            return Err(CoreError::invalid("prompt must not be empty"));
        }

        for attachment in &attachments {
            if let InputAttachment::Text { id, content } = attachment {
                for marker in [format!("[Pasted text #{id}]"), format!("[Text #{id}]")] {
                    effective_text = effective_text.replace(&marker, content);
                }
            }
        }

        let kind = match mode {
            InputMode::Shell => InputKind::ShellCommand,
            _ if effective_text.starts_with('/') => {
                let command = effective_text.trim_start_matches('/');
                let (name, arguments) = command
                    .split_once(char::is_whitespace)
                    .map(|(name, args)| (name, args.trim()))
                    .unwrap_or((command, ""));
                InputKind::SlashCommand {
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                }
            }
            _ => InputKind::Prompt,
        };

        Ok(InputEnvelope {
            input_id: uuid_like_id(),
            session_id,
            workspace: workspace.into(),
            raw_text,
            effective_text,
            mode,
            kind,
            attachments,
            accepted_at: now_ms(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseDecision {
    Acquired,
    Queued { position: usize },
}

#[derive(Default)]
struct LeaseState {
    active: HashMap<String, String>,
    queued: HashMap<String, VecDeque<QueuedInput>>,
}

#[derive(Debug, Clone)]
struct QueuedInput {
    run_id: String,
    input: InputEnvelope,
}

pub struct InputLeaseRegistry {
    state: Mutex<LeaseState>,
    notify: Arc<Notify>,
}

impl Default for InputLeaseRegistry {
    fn default() -> Self {
        Self {
            state: Mutex::new(LeaseState::default()),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl InputLeaseRegistry {
    pub fn acquire(&self, session_id: &str, run_id: &str, input: InputEnvelope) -> LeaseDecision {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.active.contains_key(session_id) {
            let queue = state.queued.entry(session_id.to_string()).or_default();
            if let Some(index) = queue.iter().position(|queued| queued.run_id == run_id) {
                return LeaseDecision::Queued {
                    position: index + 1,
                };
            }
            queue.push_back(QueuedInput {
                run_id: run_id.to_string(),
                input,
            });
            LeaseDecision::Queued {
                position: queue.len(),
            }
        } else {
            state
                .active
                .insert(session_id.to_string(), run_id.to_string());
            LeaseDecision::Acquired
        }
    }

    pub fn active_run(&self, session_id: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .get(session_id)
            .cloned()
    }

    pub fn is_active_run(&self, session_id: &str, run_id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .get(session_id)
            .is_some_and(|active| active == run_id)
    }

    pub fn queued_position(&self, session_id: &str, run_id: &str) -> Option<usize> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queued
            .get(session_id)
            .and_then(|queue| {
                queue
                    .iter()
                    .position(|queued| queued.run_id == run_id)
                    .map(|index| index + 1)
            })
    }

    pub async fn wait_for_turn(
        &self,
        session_id: &str,
        run_id: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        loop {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                self.remove_queued(session_id, run_id);
                return Err(CoreError::other("input cancelled before dispatch"));
            }
            if self.is_active_run(session_id, run_id) {
                return Ok(());
            }
            if self.queued_position(session_id, run_id).is_none() {
                return Err(CoreError::other("input lease was released before dispatch"));
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
        }
    }

    pub fn release(&self, session_id: &str, run_id: &str) -> Option<InputEnvelope> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.active.get(session_id).map(String::as_str) != Some(run_id) {
            return None;
        }
        state.active.remove(session_id);
        let next = state
            .queued
            .get_mut(session_id)
            .and_then(VecDeque::pop_front);
        let next = next.map(|queued| {
            state
                .active
                .insert(session_id.to_string(), queued.run_id.clone());
            queued.input
        });
        if state.queued.get(session_id).is_some_and(VecDeque::is_empty) {
            state.queued.remove(session_id);
        }
        drop(state);
        self.notify.notify_waiters();
        next
    }

    pub fn remove_queued(&self, session_id: &str, run_id: &str) -> Option<InputEnvelope> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let queue = state.queued.get_mut(session_id)?;
        let index = queue.iter().position(|queued| queued.run_id == run_id)?;
        let queued = queue.remove(index)?;
        if queue.is_empty() {
            state.queued.remove(session_id);
        }
        drop(state);
        self.notify.notify_waiters();
        Some(queued.input)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn uuid_like_id() -> String {
    format!("input-{}-{}", now_ms(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_text_references_and_routes_slash_commands() {
        let envelope = InputIngress::normalize(
            None,
            "root",
            "/review [Pasted text #1]",
            InputMode::Prompt,
            vec![InputAttachment::Text {
                id: "1".into(),
                content: "src/main.rs".into(),
            }],
        )
        .unwrap();
        assert_eq!(envelope.effective_text, "/review src/main.rs");
        assert_eq!(
            envelope.kind,
            InputKind::SlashCommand {
                name: "review".into(),
                arguments: "src/main.rs".into()
            }
        );
    }

    #[test]
    fn queues_second_input_for_same_session() {
        let registry = InputLeaseRegistry::default();
        let input =
            InputIngress::normalize(None, "root", "one", InputMode::Prompt, vec![]).unwrap();
        assert_eq!(
            registry.acquire("s", "r1", input.clone()),
            LeaseDecision::Acquired
        );
        assert_eq!(
            registry.acquire("s", "r2", input),
            LeaseDecision::Queued { position: 1 }
        );
        assert!(registry.release("s", "wrong").is_none());
        assert!(registry.release("s", "r1").is_some());
        assert!(registry.is_active_run("s", "r2"));
    }

    #[tokio::test]
    async fn queued_input_waits_until_promoted() {
        let registry = std::sync::Arc::new(InputLeaseRegistry::default());
        let input =
            InputIngress::normalize(None, "root", "one", InputMode::Prompt, vec![]).unwrap();
        assert_eq!(
            registry.acquire("s", "r1", input.clone()),
            LeaseDecision::Acquired
        );
        assert_eq!(
            registry.acquire("s", "r2", input),
            LeaseDecision::Queued { position: 1 }
        );

        let waiter = {
            let registry = registry.clone();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let cancel_for_task = cancel.clone();
            tokio::spawn(async move {
                registry
                    .wait_for_turn("s", "r2", cancel_for_task.as_ref())
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!waiter.is_finished());
        assert!(registry.release("s", "r1").is_some());
        waiter.await.unwrap().unwrap();
        assert!(registry.is_active_run("s", "r2"));
    }
}
