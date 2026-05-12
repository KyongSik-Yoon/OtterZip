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
        }
        finally
        {
            _slots.Release();
        }
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
        item.StatusText = Localize("Job_StatusStarting");
    });

    private Progress<double> BuildProgressReporter(JobItem item)
    {
        string percentFormat = Localize("Job_StatusRunningPercent");
        return new Progress<double>(p => _ui.TryEnqueue(() =>
        {
            item.IsIndeterminate = false;
            item.Progress = Math.Clamp(p, 0.0, 1.0);
            item.StatusText = string.Format(CultureInfo.CurrentCulture,
                percentFormat, Math.Round(p * 100));
        }));
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
