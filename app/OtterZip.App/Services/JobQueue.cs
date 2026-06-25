// CA1711 — "JobQueue" reads as "queue of jobs", not the System.Collections.Queue<T>
// data structure the analyzer warns about. Suppressing locally keeps the
// domain term intact.
#pragma warning disable CA1711

using System;
using System.Collections.ObjectModel;
using System.Globalization;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.UI.Dispatching;
using OtterZip.App.Models;

namespace OtterZip.App.Services;

/// <summary>
/// Background work coordinator for OtterZip. Holds a flat list of
/// <see cref="JobItem"/>s the UI can render as floating cards. The UI
/// stays passive — it subscribes to <see cref="Jobs"/> mutations and to
/// each item's <c>PropertyChanged</c>; this service decides when each
/// piece of work actually starts.
///
/// Concurrency policy:
///   - At most <see cref="ConcurrentLimit"/> jobs run at the same time
///     (default 1). Extra submissions sit in <see cref="JobState.Queued"/>
///     until a slot frees.
///   - Cancellation is per-job via <see cref="JobItem.Cts"/>.
///
/// Threading:
///   - All mutations on <see cref="Jobs"/> and <see cref="JobItem"/>
///     properties are marshalled to the captured DispatcherQueue so
///     XAML can listen via INotifyPropertyChanged without surprises.
///   - The work delegate itself runs on a thread pool task (NOT the UI
///     thread) so a slow archive doesn't freeze the window.
/// </summary>
public sealed class JobQueue : IDisposable
{
    private const int MaxSlots = 8;

    private readonly DispatcherQueue _ui;
    private readonly SemaphoreSlim _slots;
    private readonly System.Threading.Lock _limitLock = new();
    private int _currentLimit;

    public ObservableCollection<JobItem> Jobs { get; } = new();

    /// <summary>
    /// Raised when a terminal job (Done / Cancelled / Error) settles —
    /// the UI can use this for toast notifications or auto-fade timers.
    /// Fires on the dispatcher thread.
    /// </summary>
    public event EventHandler<JobItem>? JobSettled;

    public int ConcurrentLimit
    {
        get { lock (_limitLock) { return _currentLimit; } }
    }

    public JobQueue(DispatcherQueue ui, int concurrentLimit = 1)
    {
        _ui = ui;
        _currentLimit = Math.Clamp(concurrentLimit, 1, MaxSlots);
        // Pre-allocate the maximum so TrySetConcurrentLimit can raise the
        // ceiling without recreating the semaphore — Release brings the
        // active count up to the new limit without touching jobs that
        // are already holding slots.
        _slots = new SemaphoreSlim(_currentLimit, MaxSlots);
    }

    /// <summary>
    /// Adjust the active concurrency live. Raising the limit releases
    /// more slots so any queued jobs unblock on the spot; lowering
    /// requires a restart (we have no safe way to revoke a slot that's
    /// currently running work without aborting that work). Safe to call
    /// from any thread.
    /// </summary>
    public void TrySetConcurrentLimit(int newLimit)
    {
        newLimit = Math.Clamp(newLimit, 1, MaxSlots);
        lock (_limitLock)
        {
            if (newLimit <= _currentLimit)
            {
                return;
            }
            try
            {
                _slots.Release(newLimit - _currentLimit);
            }
            catch (SemaphoreFullException)
            {
                // Already at the hard ceiling; nothing more to release.
            }
            _currentLimit = newLimit;
        }
    }

    /// <summary>
    /// Submit a unit of work. Returns the JobItem so the caller can
    /// observe state (e.g. set ResultPath after success) without holding
    /// a reference to the queue. The delegate receives a cancellation
    /// token already wired to the item's X button.
    /// </summary>
    public JobItem Submit(
        JobItem item,
        Func<CancellationToken, IProgress<double>, Task> work)
    {
        ArgumentNullException.ThrowIfNull(item);
        ArgumentNullException.ThrowIfNull(work);

        item.Cts = new CancellationTokenSource();
        // Wire the dispatcher so JobItem.PropertyChanged auto-marshals
        // to the UI thread no matter where the work delegate runs.
        item.Dispatcher = _ui;

        // Add to the collection eagerly so the user immediately sees a
        // "queued" card even if all slots are busy.
        _ui.TryEnqueue(() => Jobs.Add(item));

        _ = RunAsync(item, work);
        return item;
    }

    private async Task RunAsync(JobItem item, Func<CancellationToken, IProgress<double>, Task> work)
    {
        if (!await WaitForSlotAsync(item).ConfigureAwait(false))
        {
            return; // cancelled before starting
        }

        try
        {
            MarkRunning(item);
            var progress = BuildProgressReporter(item);
            await work(item.Cts!.Token, progress).ConfigureAwait(false);
            MarkDone(item);
        }
        catch (OperationCanceledException)
        {
            MarkCancelled(item);
        }
        catch (Exception ex)
        {
            MarkError(item, ex.Message);
            // Central choke point for every queued compress/extract failure.
            // The classifier separates our bugs (Rust panic / invalid arg)
            // from expected user errors (wrong password / corrupt / disk).
            OtterZip.Interop.OtterzipTelemetry.ReportOperationFailure(
                item.Kind == JobKind.Compress ? "compress" : "extract",
                ex,
                OperationFormatHint(item));
        }
        finally
        {
            _slots.Release();
        }
    }

    /// <summary>
    /// Best-effort archive-format hint for telemetry — the file EXTENSION
    /// only (e.g. "zip", "7z"); the path itself is never sent. Compress
    /// reads the destination, extract the source archive.
    /// </summary>
    private static string? OperationFormatHint(JobItem item)
    {
        string? path = item.Kind == JobKind.Compress
            ? (item.ResultPath ?? item.DisplayName)
            : item.SourcePath;
        if (string.IsNullOrEmpty(path))
        {
            return null;
        }
        string ext = System.IO.Path.GetExtension(path);
        return string.IsNullOrEmpty(ext) ? null : ext.TrimStart('.').ToUpperInvariant();
    }

    private async Task<bool> WaitForSlotAsync(JobItem item)
    {
        try
        {
            await _slots.WaitAsync(item.Cts!.Token).ConfigureAwait(false);
            return true;
        }
        catch (OperationCanceledException)
        {
            MarkCancelled(item);
            return false;
        }
    }

    private void MarkRunning(JobItem item) => _ui.TryEnqueue(() =>
    {
        item.State = JobState.Running;
        item.IsIndeterminate = true;
        // Seed the phase label so the first percent tick from
        // BuildProgressReporter renders as "시작 중… 0%" rather than
        // bare "0%". Work delegates flip this label to phase-specific
        // text ("압축 중…", "1/3 병합 중", …) as they reach each
        // phase.
        string starting = Localize("Job_StatusStarting");
        item.StatusLabel = starting;
        item.StatusText = starting;
    });

    /// <summary>
    /// Compose "<phase label> <percent>" so a single writer owns
    /// <see cref="JobItem.StatusText"/> in the Running state. Work
    /// delegates only mutate <see cref="JobItem.StatusLabel"/> for
    /// phase changes; this reporter is the sole StatusText author
    /// until a terminal state hands over to a final summary. Without
    /// the single-writer discipline the user sees "압축 중…" and
    /// "42%" alternating 30 Hz as the two handlers race for the same
    /// slot — the bug this code fixes.
    /// </summary>
    private static string ComposeRunningStatusText(
        JobItem item, double clamped, string percentFormat)
    {
        string percentStr = string.Format(CultureInfo.CurrentCulture,
            percentFormat, Math.Round(clamped * 100));
        string? label = item.StatusLabel;
        return string.IsNullOrEmpty(label) ? percentStr : $"{label} {percentStr}";
    }

    private Progress<double> BuildProgressReporter(JobItem item)
    {
        string percentFormat = Localize("Job_StatusRunningPercent");
        // Throttle UI marshalling to ~30 Hz. Without this gate a fast
        // extract (rayon parallel workers each firing per-N-entries
        // through the FFI callback) can produce hundreds of ticks
        // per second after the C++ exception-spam threshold the
        // dispatcher queue degrades into "WinRT.Runtime.dll
        // InvalidOperationException" floods. A 33 ms cadence matches
        // typical compositor frame time so the UI never sees stale
        // values, just at a more sane rate. Final 100 % tick is never
        // throttled — it must always reach the UI so the JobCard
        // settles cleanly. (System.Diagnostics.Stopwatch is
        // monotonic; lock-free use of ElapsedMilliseconds is safe
        // here because at-most-once duplicate ticks during a race are
        // harmless — the dispatched closure re-guards against stale
        // state anyway.)
        var sw = System.Diagnostics.Stopwatch.StartNew();
        long lastReportMs = -1000; // first tick always passes
        return new Progress<double>(p =>
        {
            long now = sw.ElapsedMilliseconds;
            bool isTerminalTick = p >= 1.0;
            if (!isTerminalTick && now - lastReportMs < 33)
            {
                return;
            }
            lastReportMs = now;
            _ui.TryEnqueue(() =>
            {
                try
                {
                    // Terminal-state guard: late progress callbacks must NOT
                    // overwrite the Done caption ("5.79 KB"), the Cancelled
                    // caption ("취소됨"), or — newly — the Error caption (the
                    // exception message). Multi-layer Progress<T> posts via
                    // the thread pool with no ordering guarantees, so we
                    // may see a "0%" tick land after MarkError has fired.
                    if (item.State is JobState.Done or JobState.Cancelled or JobState.Error)
                    {
                        return;
                    }
                    if (item.Progress >= 1.0) return;
                    double clamped = Math.Clamp(p, 0.0, 1.0);
                    if (clamped < item.Progress) return;
                    item.IsIndeterminate = false;
                    item.Progress = clamped;
                    item.StatusText = ComposeRunningStatusText(item, clamped, percentFormat);
                }
                catch (InvalidOperationException)
                {
                    // Stale XAML element access (JobCard unbound / unloaded
                    // mid-extract) and WinRT marshalling edge cases surface
                    // as InvalidOperationException. Progress reporting is a
                    // UX nicety — never let it kill the queue.
                }
            });
        });
    }

    private void MarkDone(JobItem item) => _ui.TryEnqueue(() =>
    {
        item.State = JobState.Done;
        item.Progress = 1.0;
        item.IsIndeterminate = false;
        // Leave StatusText alone — the work delegate may have set a
        // richer summary (e.g. "8.2 MB"); if not, the last running
        // update ("100%") is a fine placeholder.
        JobSettled?.Invoke(this, item);
    });

    private void MarkCancelled(JobItem item) => _ui.TryEnqueue(() =>
    {
        item.State = JobState.Cancelled;
        item.StatusText = Localize("Job_StatusCancelled");
        JobSettled?.Invoke(this, item);
    });

    private void MarkError(JobItem item, string message) => _ui.TryEnqueue(() =>
    {
        item.State = JobState.Error;
        item.StatusText = message;
        JobSettled?.Invoke(this, item);
    });

    /// <summary>
    /// Surface a synchronous failure as a ghost JobCard in the Error
    /// state — no slot acquisition, no work delegate, just a card with
    /// the message so the user sees what went wrong. Used for failures
    /// that happen before any real archive work starts (probe failure,
    /// shell-invoke routing error, drop-classification fault) — the old
    /// hidden status-bar path silently swallowed these.
    ///
    /// Returns the item so the caller can hold a reference if they
    /// want to dismiss it programmatically.
    /// </summary>
    public JobItem ReportError(JobKind kind, string displayName, string message)
    {
        ArgumentException.ThrowIfNullOrEmpty(displayName);
        var item = new JobItem(kind, displayName)
        {
            Dispatcher = _ui,
            State = JobState.Error,
            StatusText = string.IsNullOrEmpty(message) ? "Error" : message,
            IsIndeterminate = false,
        };
        _ui.TryEnqueue(() =>
        {
            Jobs.Add(item);
            JobSettled?.Invoke(this, item);
        });
        return item;
    }

    /// <summary>
    /// Remove a job from the visible list. Cancels first if still active.
    /// Safe to call from any thread.
    /// </summary>
    public void Remove(JobItem item)
    {
        ArgumentNullException.ThrowIfNull(item);
        if (item.State == JobState.Queued || item.State == JobState.Running)
        {
            item.RequestCancel();
        }
        _ui.TryEnqueue(() =>
        {
            Jobs.Remove(item);
            item.Cts?.Dispose();
            item.Cts = null;
        });
    }

    public void Dispose()
    {
        _slots.Dispose();
    }

    private static string Localize(string key)
    {
        try
        {
            var loader = new Microsoft.Windows.ApplicationModel.Resources.ResourceLoader();
            return loader.GetString(key + "/Text");
        }
        catch (Exception)
        {
            return key;
        }
    }
}
