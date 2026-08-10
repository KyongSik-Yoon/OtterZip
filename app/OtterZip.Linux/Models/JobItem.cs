// OtterZip for Linux — job model.
//
// A near-verbatim port of app/OtterZip.App/Models/JobItem.cs. The only
// difference is the dispatcher: the WinUI original marshals PropertyChanged
// through `Microsoft.UI.Dispatching.DispatcherQueue`, this one through
// Avalonia's `Dispatcher`. Same properties, same names, same semantics — the
// two job cards render from the same shape.

using System;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Threading;
using Avalonia.Threading;

namespace OtterZip.App.Models;

/// <summary>
/// One unit of background work surfaced as a card in the UI. Holds enough
/// state for the card to render (display name, progress fraction, sub-text)
/// and a cancel hook the card's × button calls.
/// </summary>
/// <remarks>
/// Threading: writes can come from any thread (the queue's work delegate runs
/// on the thread pool), but bindings must be notified on the UI thread.
/// <see cref="Dispatcher"/> is set by the queue at submit time and used to
/// marshal every notification, so callers assign properties from any thread
/// without thinking about it.
/// </remarks>
public sealed class JobItem : INotifyPropertyChanged
{
    public JobItem(JobKind kind, string displayName)
    {
        Kind = kind;
        _displayName = displayName;
    }

    public Guid Id { get; } = Guid.NewGuid();

    public JobKind Kind { get; }

    /// <summary>
    /// Path the work produced (compress destination, extract destination).
    /// Used by the "open folder" follow-up action.
    /// </summary>
    public string? ResultPath { get; set; }

    /// <summary>
    /// Path the work consumed. For extract jobs this is the archive being
    /// extracted — needed by the <c>Settings_DeleteArchiveAfterExtract</c>
    /// follow-up so the completion handler knows which file to trash.
    /// </summary>
    public string? SourcePath { get; set; }

    /// <summary>
    /// Output name reserved via <c>OutputNamer.ReserveUnique</c> when this
    /// job was planned, if any. The queue releases it the moment the job
    /// settles so the in-process reservation cannot outlive the job and
    /// mis-name a later same-named job in the same session.
    /// </summary>
    public string? ReservedOutputPath { get; set; }

    /// <summary>
    /// Backing token source for the work. The queue creates and disposes it;
    /// the card cancels through <see cref="RequestCancel"/>.
    /// </summary>
    internal CancellationTokenSource? Cts { get; set; }

    /// <summary>
    /// UI dispatcher used to marshal <see cref="PropertyChanged"/>. Set by
    /// the queue at submit time; before that, notifications fire
    /// synchronously — there are no bindings attached yet, so it is safe.
    /// </summary>
    internal Dispatcher? Dispatcher { get; set; }

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
        set
        {
            if (SetProp(ref _state, value))
            {
                // Derived flags the card binds to directly. Avalonia has no
                // value-converter-free way to test an enum in a binding, and
                // three bool properties are cheaper than three converters.
                OnPropertyChanged(nameof(IsRunning));
                OnPropertyChanged(nameof(IsFinished));
                OnPropertyChanged(nameof(IsFailed));
            }
        }
    }

    /// <summary>Card shows a progress bar and a cancel button.</summary>
    public bool IsRunning => _state is JobState.Queued or JobState.Running;

    /// <summary>Card shows the "reveal in file manager" follow-up.</summary>
    public bool IsFinished => _state == JobState.Done;

    /// <summary>Card renders its status line in the error accent.</summary>
    public bool IsFailed => _state is JobState.Error or JobState.Cancelled;

    private double _progress; // 0.0 .. 1.0
    public double Progress
    {
        get => _progress;
        set
        {
            if (SetProp(ref _progress, value))
            {
                OnPropertyChanged(nameof(ProgressPercent));
            }
        }
    }

    /// <summary>0..100, for Avalonia's <c>ProgressBar</c> default range.</summary>
    public double ProgressPercent => _progress * 100.0;

    private double _currentEntryProgress; // 0.0 .. 1.0

    /// <summary>
    /// Per-entry progress fraction for the currently in-flight file,
    /// populated by the streaming compress path (ABI v9). Stays at 0 outside
    /// that path so the second bar collapses when there is no useful
    /// per-file motion to show.
    /// </summary>
    public double CurrentEntryProgress
    {
        get => _currentEntryProgress;
        set
        {
            if (SetProp(ref _currentEntryProgress, value))
            {
                OnPropertyChanged(nameof(CurrentEntryProgressPercent));
            }
        }
    }

    public double CurrentEntryProgressPercent => _currentEntryProgress * 100.0;

    private bool _currentEntryProgressVisible;

    /// <summary>
    /// Drives the show/collapse toggle for the per-entry bar, so a card on
    /// the small-file chunk path does not show a permanently-zero second bar.
    /// </summary>
    public bool CurrentEntryProgressVisible
    {
        get => _currentEntryProgressVisible;
        set => SetProp(ref _currentEntryProgressVisible, value);
    }

    private string? _currentEntryName;

    /// <summary>
    /// The entry the writer is currently streaming, shown under the
    /// per-entry bar. This is the diagnostic signal that identifies "this
    /// specific file is where compression slows down".
    /// </summary>
    public string? CurrentEntryName
    {
        get => _currentEntryName;
        set => SetProp(ref _currentEntryName, value);
    }

    private bool _isIndeterminate = true;

    /// <summary>
    /// True until the worker reports its first concrete fraction, so the
    /// user sees activity before bytes-processed numbers start coming back.
    /// </summary>
    public bool IsIndeterminate
    {
        get => _isIndeterminate;
        set => SetProp(ref _isIndeterminate, value);
    }

    private string? _statusText;

    /// <summary>
    /// Sub-label under the file name. Free-form. In the running state the
    /// queue's progress reporter composes it from <see cref="StatusLabel"/>
    /// plus the percent; in terminal states the work delegate writes a final
    /// summary directly.
    /// </summary>
    public string? StatusText
    {
        get => _statusText;
        set => SetProp(ref _statusText, value);
    }

    private string? _statusLabel;

    /// <summary>
    /// Phase label without the percent suffix. Set by work delegates when
    /// the phase changes; the queue combines it with the current percent on
    /// every tick into <see cref="StatusText"/>.
    /// </summary>
    public string? StatusLabel
    {
        get => _statusLabel;
        set => SetProp(ref _statusLabel, value);
    }

    public void RequestCancel() => Cts?.Cancel();

    public event PropertyChangedEventHandler? PropertyChanged;

    private bool SetProp<T>(ref T field, T value, [CallerMemberName] string? propertyName = null)
    {
        if (Equals(field, value))
        {
            return false;
        }
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }

    private void OnPropertyChanged(string? propertyName)
    {
        PropertyChangedEventHandler? handler = PropertyChanged;
        if (handler is null)
        {
            return;
        }
        var args = new PropertyChangedEventArgs(propertyName);
        Dispatcher? dispatcher = Dispatcher;
        if (dispatcher is null || dispatcher.CheckAccess())
        {
            handler(this, args);
        }
        else
        {
            dispatcher.Post(() => handler(this, args));
        }
    }
}
