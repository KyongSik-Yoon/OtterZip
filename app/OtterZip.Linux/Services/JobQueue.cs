// CA1711 — "JobQueue" reads as "queue of jobs", not the System.Collections
// Queue<T> data structure the analyzer warns about. Suppressing locally keeps
// the domain term intact.
#pragma warning disable CA1711

// OtterZip for Linux — background work coordinator.
//
// A port of app/OtterZip.App/Services/JobQueue.cs. The concurrency policy,
// the progress throttle, the terminal-state guards and the reservation
// release are all carried over unchanged — they encode bugs already found and
// fixed once on the Windows side, and a "clean rewrite" would reintroduce
// them. What changes:
//
//   * `Microsoft.UI.Dispatching.DispatcherQueue.TryEnqueue` → Avalonia's
//     `Dispatcher.Post`.
//   * String lookup goes through the Linux `Strings` helper.

using System;
using System.Collections.ObjectModel;
using System.Globalization;
using System.Threading;
using System.Threading.Tasks;
using Avalonia.Threading;
using OtterZip.App.Models;

namespace OtterZip.App.Services;

/// <summary>
/// Background work coordinator. Holds a flat list of <see cref="JobItem"/>s
/// the UI renders as cards. The UI stays passive — it binds to
/// <see cref="Jobs"/> and to each item's <c>PropertyChanged</c>; this service
/// decides when each piece of work actually starts.
/// </summary>
/// <remarks>
/// Concurrency: at most <see cref="ConcurrentLimit"/> jobs run at once
/// (default 1); extra submissions sit in <see cref="JobState.Queued"/> until
/// a slot frees. Cancellation is per-job.
/// <para>
/// Threading: mutations of <see cref="Jobs"/> and of item properties are
/// marshalled to the captured dispatcher, so bindings never see a
/// cross-thread notification. The work delegate itself runs on a thread-pool
/// task so a slow archive cannot freeze the window.
/// </para>
/// </remarks>
public sealed class JobQueue : IDisposable
{
    private const int MaxSlots = 8;

    /// <summary>
    /// UI marshalling cadence, in milliseconds. A fast extract fires the FFI
    /// progress callback from every rayon worker and can produce hundreds of
    /// ticks a second; 33 ms matches a typical compositor frame so the UI
    /// never shows a stale value, just at a sane rate.
    /// </summary>
    private const long ProgressThrottleMs = 33;

    private readonly Dispatcher _ui;
    private readonly SemaphoreSlim _slots;
    private readonly System.Threading.Lock _limitLock = new();
    private int _currentLimit;

    public JobQueue(Dispatcher ui, int concurrentLimit = 1)
    {
        _ui = ui;
        _currentLimit = Math.Clamp(concurrentLimit, 1, MaxSlots);
        // Pre-allocate the maximum so TrySetConcurrentLimit can raise the
        // ceiling without recreating the semaphore.
        _slots = new SemaphoreSlim(_currentLimit, MaxSlots);
    }

    public ObservableCollection<JobItem> Jobs { get; } = [];

    /// <summary>
    /// Raised when a terminal job (Done / Cancelled / Error) settles, so the
    /// UI can fire notifications or auto-fade timers. Fires on the dispatcher
    /// thread.
    /// </summary>
    public event EventHandler<JobItem>? JobSettled;

    public int ConcurrentLimit
    {
        get { lock (_limitLock) { return _currentLimit; } }
    }

    /// <summary>
    /// Adjust the active concurrency live. Raising the limit releases more
    /// slots so queued jobs unblock on the spot; lowering takes effect on
    /// restart, because there is no safe way to revoke a slot that is
    /// currently running work without aborting that work. Safe to call from
    /// any thread.
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
    /// Submit a unit of work. Returns the item so the caller can observe
    /// state without holding a reference to the queue. The delegate receives
    /// a cancellation token already wired to the card's × button.
    /// </summary>
    public JobItem Submit(
        JobItem item,
        Func<CancellationToken, IProgress<double>, Task> work)
    {
        ArgumentNullException.ThrowIfNull(item);
        ArgumentNullException.ThrowIfNull(work);

        item.Cts = new CancellationTokenSource();
        // Wire the dispatcher so JobItem.PropertyChanged auto-marshals no
        // matter where the work delegate runs.
        item.Dispatcher = _ui;

        // Add eagerly so the user immediately sees a "queued" card even when
        // every slot is busy.
        _ui.Post(() => Jobs.Add(item));

        _ = RunAsync(item, work);
        return item;
    }

    /// <summary>
    /// Surface a synchronous failure as a card already in the Error state —
    /// no slot acquisition, no work delegate. Used for failures that happen
    /// before any archive work starts (probe failure, CLI routing error, drop
    /// classification fault), which would otherwise be silently swallowed.
    /// </summary>
    public JobItem ReportError(JobKind kind, string displayName, string message)
    {
        ArgumentException.ThrowIfNullOrEmpty(displayName);
        var item = new JobItem(kind, displayName)
        {
            Dispatcher = _ui,
            State = JobState.Error,
            StatusText = string.IsNullOrEmpty(message)
                ? Strings.Get("Error_OperationFailed")
                : message,
            IsIndeterminate = false,
        };
        _ui.Post(() =>
        {
            Jobs.Add(item);
            JobSettled?.Invoke(this, item);
        });
        return item;
    }

    /// <summary>
    /// Remove a job from the visible list, cancelling first if still active.
    /// Safe to call from any thread.
    /// </summary>
    public void Remove(JobItem item)
    {
        ArgumentNullException.ThrowIfNull(item);
        if (item.State is JobState.Queued or JobState.Running)
        {
            item.RequestCancel();
        }
        _ui.Post(() =>
        {
            Jobs.Remove(item);
            item.Cts?.Dispose();
            item.Cts = null;
        });
    }

    public void Dispose() => _slots.Dispose();

    private async Task RunAsync(JobItem item, Func<CancellationToken, IProgress<double>, Task> work)
    {
        if (!await WaitForSlotAsync(item).ConfigureAwait(false))
        {
            return; // cancelled before starting
        }
        try
        {
            MarkRunning(item);
            Progress<double> progress = BuildProgressReporter(item);
            await work(item.Cts!.Token, progress).ConfigureAwait(false);
            MarkDone(item);
        }
        catch (OperationCanceledException)
        {
            MarkCancelled(item);
        }
        catch (Exception ex)
        {
            // Localized headline for known failure shapes; raw message only
            // for unexpected managed-side errors.
            MarkError(item, ErrorMessages.Localize(ex));
            // Central choke point for every queued compress/extract failure.
            // The classifier separates our bugs (panic / invalid arg) from
            // expected user errors (wrong password / corrupt / disk full).
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
    /// only; the path itself is never sent. Compress reads the destination,
    /// extract the source archive.
    /// </summary>
    private static string? OperationFormatHint(JobItem item)
    {
        string? path = item.Kind == JobKind.Compress
            ? item.ResultPath ?? item.DisplayName
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

    private void MarkRunning(JobItem item) => _ui.Post(() =>
    {
        item.State = JobState.Running;
        item.IsIndeterminate = true;
        // Seed the phase label so the first percent tick renders as
        // "Starting… 0%" rather than a bare "0%". Work delegates flip this
        // label to phase-specific text as they reach each phase.
        string starting = Strings.Get("Job_StatusStarting");
        item.StatusLabel = starting;
        item.StatusText = starting;
    });

    /// <summary>
    /// Compose "&lt;phase label&gt; &lt;percent&gt;" so a single writer owns
    /// <see cref="JobItem.StatusText"/> in the Running state. Work delegates
    /// only mutate <see cref="JobItem.StatusLabel"/>; this reporter is the
    /// sole StatusText author until a terminal state hands over to a final
    /// summary. Without that discipline the user sees the phase label and the
    /// percent alternating at 30 Hz as two writers race for the same slot.
    /// </summary>
    private static string ComposeRunningStatusText(
        JobItem item, double clamped, string percentFormat)
    {
        string percentStr = string.Format(
            CultureInfo.CurrentCulture, percentFormat, Math.Round(clamped * 100));
        string? label = item.StatusLabel;
        return string.IsNullOrEmpty(label) ? percentStr : $"{label} {percentStr}";
    }

    private Progress<double> BuildProgressReporter(JobItem item)
    {
        string percentFormat = Strings.Get("Job_StatusRunningPercent");
        // Stopwatch is monotonic; lock-free use of ElapsedMilliseconds is
        // safe here because an at-most-once duplicate tick during a race is
        // harmless — the posted closure re-guards against stale state anyway.
        var sw = System.Diagnostics.Stopwatch.StartNew();
        long lastReportMs = -1000; // first tick always passes
        return new Progress<double>(p =>
        {
            long now = sw.ElapsedMilliseconds;
            bool isTerminalTick = p >= 1.0;
            if (!isTerminalTick && now - lastReportMs < ProgressThrottleMs)
            {
                return;
            }
            lastReportMs = now;
            _ui.Post(() =>
            {
                // Terminal-state guard: a late progress callback must not
                // overwrite the Done caption, the Cancelled caption, or the
                // Error message. Progress<T> posts through the thread pool
                // with no ordering guarantee, so a "0%" tick can land after
                // MarkError has already fired.
                if (item.State is JobState.Done or JobState.Cancelled or JobState.Error)
                {
                    return;
                }
                if (item.Progress >= 1.0)
                {
                    return;
                }
                double clamped = Math.Clamp(p, 0.0, 1.0);
                if (clamped < item.Progress)
                {
                    return;
                }
                item.IsIndeterminate = false;
                item.Progress = clamped;
                item.StatusText = ComposeRunningStatusText(item, clamped, percentFormat);
            });
        });
    }

    /// <summary>
    /// Release any output-name reservation the moment a job settles, in every
    /// terminal state (including cancelled-while-queued). Without this the
    /// in-process reservation outlives the job, a later same-named job gets a
    /// spurious "(1)" suffix in the same session, and the reservation set
    /// grows unbounded. Idempotent.
    /// </summary>
    private static void ReleaseReservation(JobItem item)
    {
        if (!string.IsNullOrEmpty(item.ReservedOutputPath))
        {
            OutputNamer.Release(item.ReservedOutputPath);
        }
    }

    private void MarkDone(JobItem item) => _ui.Post(() =>
    {
        item.State = JobState.Done;
        item.Progress = 1.0;
        item.IsIndeterminate = false;
        // Leave StatusText alone — the work delegate may have set a richer
        // summary; if not, the last running update is a fine placeholder.
        ReleaseReservation(item);
        JobSettled?.Invoke(this, item);
    });

    private void MarkCancelled(JobItem item) => _ui.Post(() =>
    {
        item.State = JobState.Cancelled;
        item.StatusText = Strings.Get("Job_StatusCancelled");
        ReleaseReservation(item);
        JobSettled?.Invoke(this, item);
    });

    private void MarkError(JobItem item, string message) => _ui.Post(() =>
    {
        item.State = JobState.Error;
        item.StatusText = message;
        ReleaseReservation(item);
        JobSettled?.Invoke(this, item);
    });
}
