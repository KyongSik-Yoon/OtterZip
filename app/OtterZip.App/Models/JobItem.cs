using System;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Threading;
using Microsoft.UI.Dispatching;

namespace OtterZip.App.Models;

/// <summary>
/// One unit of background work surfaced as a floating card in the UI.
/// Holds enough state for the card to render (display name, progress
/// fraction, optional sub-text like "42% · 12.3 MB/s") and a cancel
/// hook the card's X button calls.
///
/// Threading: writes can come from any thread (the JobQueue's work
/// delegate runs on the thread pool), but XAML listeners on
/// <see cref="PropertyChanged"/> must receive notifications on the UI
/// thread or they raise RPC_E_WRONG_THREAD. <see cref="Dispatcher"/>
/// is set by the queue at submit time and used to marshal every
/// notification — callers can then assign properties from any thread
/// without thinking about it.
/// </summary>
public sealed class JobItem : INotifyPropertyChanged
{
    public Guid Id { get; } = Guid.NewGuid();
    public JobKind Kind { get; }

    /// <summary>
    /// Optional path the work produced (compress destination, extract
    /// destination). Used by "open folder" follow-up action.
    /// </summary>
    public string? ResultPath { get; set; }

    /// <summary>
    /// Optional path the work consumed. For extract jobs this is the
    /// archive being extracted — needed by the
    /// <c>Settings_DeleteArchiveAfterExtract</c> follow-up so the queue
    /// completion handler knows which file to recycle. Unused for
    /// compress (source is the input list, not a single file).
    /// </summary>
    public string? SourcePath { get; set; }

    /// <summary>
    /// Backing CancellationTokenSource for the work. The queue creates
    /// and disposes it; the card calls Cancel() through
    /// <see cref="RequestCancel"/>.
    /// </summary>
    internal CancellationTokenSource? Cts { get; set; }

    /// <summary>
    /// UI dispatcher used to marshal PropertyChanged events. Set by
    /// JobQueue when the item is submitted; before that, raises fire
    /// synchronously (no XAML listeners yet, so it's safe).
    /// </summary>
    internal DispatcherQueue? Dispatcher { get; set; }

    public JobItem(JobKind kind, string displayName)
    {
        Kind = kind;
        _displayName = displayName;
    }

    private string _displayName;
    public string DisplayName
    {
        get => _displayName;
        set => SetProp(ref _displayName, value);
    }

    private JobState _state = JobState.Queued;
    public JobState State
    {
        get => _state;
        set => SetProp(ref _state, value);
    }

    private double _progress;          // 0.0 .. 1.0
    public double Progress
    {
        get => _progress;
        set => SetProp(ref _progress, value);
    }

    private bool _isIndeterminate = true;
    /// <summary>
    /// True until the worker reports its first concrete progress fraction.
    /// Cards show an indeterminate bar in this window so the user sees
    /// activity before bytes-processed numbers start coming back.
    /// </summary>
    public bool IsIndeterminate
    {
        get => _isIndeterminate;
        set => SetProp(ref _isIndeterminate, value);
    }

    private string? _statusText;
    /// <summary>
    /// Sub-label under the file name. Free-form: "압축 중… 42%",
    /// "완료 · 8.2 MB", "오류: 권한 거부", … In the running state this
    /// is composed by <c>JobQueue.BuildProgressReporter</c> from
    /// <see cref="StatusLabel"/> + the percent fraction; in terminal
    /// states (Done / Cancelled / Error) the work delegate / queue
    /// writes a final summary directly.
    /// </summary>
    public string? StatusText
    {
        get => _statusText;
        set => SetProp(ref _statusText, value);
    }

    private string? _statusLabel;
    /// <summary>
    /// Phase label without the percent suffix — e.g. "압축 중…",
    /// "1/3 병합 중", "취소 중…". Set by work delegates when the phase
    /// changes; the JobQueue progress reporter combines this with
    /// the current percent on every tick into <see cref="StatusText"/>.
    /// Leaving it <c>null</c> falls back to "percent-only" (matches
    /// the extract path which doesn't carry phase labels today).
    /// </summary>
    public string? StatusLabel
    {
        get => _statusLabel;
        set => SetProp(ref _statusLabel, value);
    }

    public void RequestCancel() => Cts?.Cancel();

    public event PropertyChangedEventHandler? PropertyChanged;

    private void SetProp<T>(ref T field, T value, [CallerMemberName] string? propertyName = null)
    {
        if (Equals(field, value)) return;
        field = value;
        var handler = PropertyChanged;
        if (handler is null) return;

        var args = new PropertyChangedEventArgs(propertyName);
        var dispatcher = Dispatcher;
        if (dispatcher is null || dispatcher.HasThreadAccess)
        {
            handler(this, args);
        }
        else
        {
            dispatcher.TryEnqueue(() => handler(this, args));
        }
    }
}
