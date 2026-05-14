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
    /// Bytes processed *within the currently in-flight entry*. For
    /// the streaming compress path this ticks every ~1 MiB so the
    /// host UI can show a per-file progress bar separate from the
    /// archive-wide one — answering the "is the worker actually
    /// making progress on this huge file or is it stuck?" question
    /// the user can otherwise only resolve by attaching a profiler.
    /// Always `0` outside the streaming path and on the read side
    /// today; the field is reserved for future per-entry visibility.
    pub current_entry_bytes_processed: u64,
    /// Total bytes the currently in-flight entry's payload will
    /// occupy when it finishes. Pair with
    /// [`current_entry_bytes_processed`] to render a per-file
    /// progress fraction (clamp 0.0..=1.0). `0` when the field is
    /// unused.
    pub current_entry_bytes_total: u64,
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
