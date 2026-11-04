//! Cooperative checkpoints for the existing authority decoder. No alternate
//! parsing/recovery path and no independent cancellation signal lives here.
use super::*;
use std::cell::Cell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadStop {
    Cancelled,
    Budget,
}

pub(crate) struct ReadControl<'a> {
    cancelled: &'a (dyn Fn() -> bool + Sync),
    deadline: Option<Instant>,
    source_limit: u64,
    decoded_limit: u64,
    source_bytes: Cell<u64>,
    decoded_bytes: Cell<u64>,
    stopped: Cell<Option<ReadStop>>,
}

impl ReadControl<'static> {
    pub(crate) fn unbounded() -> Self {
        Self::new(&|| false, None, u64::MAX, u64::MAX)
    }
}

impl<'a> ReadControl<'a> {
    pub(crate) fn new(
        cancelled: &'a (dyn Fn() -> bool + Sync),
        deadline: Option<Instant>,
        source_limit: u64,
        decoded_limit: u64,
    ) -> Self {
        Self {
            cancelled,
            deadline,
            source_limit,
            decoded_limit,
            source_bytes: Cell::new(0),
            decoded_bytes: Cell::new(0),
            stopped: Cell::new(None),
        }
    }

    pub(crate) fn stopped(&self) -> Option<ReadStop> {
        self.stopped.get()
    }

    #[cfg(test)]
    pub(crate) fn progress(&self) -> (u64, u64) {
        (self.source_bytes.get(), self.decoded_bytes.get())
    }

    fn stop(&self, reason: ReadStop) -> CoreError {
        self.stopped.set(Some(reason));
        CoreError::other(match reason {
            ReadStop::Cancelled => "session_search_cancelled",
            ReadStop::Budget => "session_search_maintenance_deferred",
        })
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        if (self.cancelled)() {
            return Err(self.stop(ReadStop::Cancelled));
        }
        if let Some(reason) = self.stopped.get() {
            return Err(self.stop(reason));
        }
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(self.stop(ReadStop::Budget));
        }
        Ok(())
    }

    pub(crate) fn source_size(&self, size: u64) -> Result<()> {
        self.checkpoint()?;
        if size > self.source_limit {
            return Err(self.stop(ReadStop::Budget));
        }
        Ok(())
    }

    pub(crate) fn source_bytes(&self, bytes: usize) -> Result<()> {
        self.charge(&self.source_bytes, bytes)
    }

    pub(crate) fn decoded_bytes(&self, bytes: usize) -> Result<()> {
        self.charge(&self.decoded_bytes, bytes)
    }

    pub(crate) fn record_size(&self, size: u64) -> Result<()> {
        self.checkpoint()?;
        if size > self.decoded_limit {
            return Err(self.stop(ReadStop::Budget));
        }
        Ok(())
    }

    fn charge(&self, used: &Cell<u64>, bytes: usize) -> Result<()> {
        self.checkpoint()?;
        let total = used.get().checked_add(bytes as u64);
        match total {
            Some(total) => {
                used.set(total);
                Ok(())
            }
            _ => Err(self.stop(ReadStop::Budget)),
        }
    }
}
