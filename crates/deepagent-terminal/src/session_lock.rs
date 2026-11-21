use crate::{TerminalError, TerminalResult};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct SessionLock {
    salt: [u8; 16],
    digest: [u8; 32],
    hide_output: bool,
    failures: u8,
    retry_after: Instant,
}

#[derive(Default)]
pub(crate) struct SessionLocks(Mutex<HashMap<String, SessionLock>>);

fn digest(salt: &[u8], password: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"deepagent-terminal-lock-v1\0");
    hash.update(salt);
    hash.update(password.as_bytes());
    hash.finalize().into()
}

impl SessionLocks {
    pub(crate) fn ensure_unlocked(&self, id: &str) -> TerminalResult<()> {
        if self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
        {
            Err(TerminalError::Locked)
        } else {
            Ok(())
        }
    }

    pub(crate) fn status(&self, id: &str) -> Option<bool> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|s| s.hide_output)
    }

    pub(crate) fn lock(&self, id: &str, password: &str, hide_output: bool) -> TerminalResult<()> {
        if password.is_empty() || password.len() > 1024 {
            return Err(TerminalError::Backend(
                "lock password must contain 1..1024 bytes".into(),
            ));
        }
        let mut locks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if locks.contains_key(id) {
            return Err(TerminalError::Locked);
        }
        let salt = *uuid::Uuid::new_v4().as_bytes();
        locks.insert(
            id.into(),
            SessionLock {
                salt,
                digest: digest(&salt, password),
                hide_output,
                failures: 0,
                retry_after: Instant::now(),
            },
        );
        Ok(())
    }

    pub(crate) fn unlock(&self, id: &str, password: &str) -> TerminalResult<()> {
        let mut locks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lock) = locks.get_mut(id) else {
            return Ok(());
        };
        if password.len() > 1024 || Instant::now() < lock.retry_after {
            return Err(TerminalError::Locked);
        }
        let supplied = digest(&lock.salt, password);
        let difference = supplied
            .iter()
            .zip(lock.digest)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b));
        if difference != 0 {
            lock.failures = lock.failures.saturating_add(1);
            if lock.failures >= 5 {
                lock.retry_after = Instant::now() + Duration::from_secs(2);
            }
            return Err(TerminalError::Locked);
        }
        locks.remove(id);
        Ok(())
    }

    pub(crate) fn remove(&self, id: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}
