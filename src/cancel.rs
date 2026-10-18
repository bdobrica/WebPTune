//! Cooperative cancellation. Library callers opt in; only the CLI installs signals.

use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A permanent cancellation request shared by clones; checks never reset it.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

/// Typed failure returned when a cancellation checkpoint is reached.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation cancelled; completed outputs were retained")
    }
}

impl std::error::Error for Cancelled {}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub fn check(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn check_io(&self) -> std::io::Result<()> {
        if self.is_cancelled() {
            Err(std::io::Error::other(Cancelled))
        } else {
            Ok(())
        }
    }
}
