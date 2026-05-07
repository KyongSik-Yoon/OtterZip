//! Progress reporting. See `rust-core-api.md` §1.6.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Progress {
    pub bytes_processed: u64,
    pub bytes_total: u64,
    pub entries_processed: u32,
    pub entries_total: u32,
    pub current_entry: Option<String>,
    pub phase: ProgressPhase,
    pub elapsed: Duration,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ProgressPhase {
    Scanning = 0,
    Reading = 1,
    Writing = 2,
    Finalizing = 3,
}

/// `Send` is required so the parallel ZIP extractor (Sprint 5) can pass a
/// `&mut dyn ProgressSink` into a `Mutex` shared across rayon workers.
/// Almost all real sinks satisfy this naturally — closures capturing only
/// `Send` state, channels, atomic counters, etc.
pub trait ProgressSink: Send {
    /// Return `true` to continue, `false` to request cancellation.
    fn update(&mut self, progress: &Progress) -> bool;
}

impl<F> ProgressSink for F
where
    F: FnMut(&Progress) -> bool + Send,
{
    fn update(&mut self, progress: &Progress) -> bool {
        self(progress)
    }
}
