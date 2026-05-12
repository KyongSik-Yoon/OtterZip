using System;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Threading;

namespace OtterZip.App.Models;

/// <summary>
/// One unit of background work surfaced as a floating card in the UI.
/// Holds enough state for the card to render (display name, progress
/// fraction, optional sub-text like "42% · 12.3 MB/s") and a cancel
/// hook the card's X button calls.
///
/// Designed for hand-written code-behind binding — UI controls subscribe
/// to PropertyChanged and refresh whichever element changed. This keeps
/// us off x:Bind generated code paths that get hairy under AOT.
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
    /// Backing CancellationTokenSource for the work. The queue creates
    /// and disposes it; the card calls Cancel() through
    /// <see cref="RequestCancel"/>.
    /// </summary>
    internal CancellationTokenSource? Cts { get; set; }

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
    /// Sub-label under the file name. Free-form: "42% · 12.3 MB/s",
    /// "대기 중", "완료 · 8.2 MB", "오류: 권한 거부", …
    /// </summary>
    public string? StatusText
    {
        get => _statusText;
        set => SetProp(ref _statusText, value);
    }

    public void RequestCancel() => Cts?.Cancel();

    public event PropertyChangedEventHandler? PropertyChanged;

    private void SetProp<T>(ref T field, T value, [CallerMemberName] string? propertyName = null)
    {
        if (Equals(field, value)) return;
        field = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}
