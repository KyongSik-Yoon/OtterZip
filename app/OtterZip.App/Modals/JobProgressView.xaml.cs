using System;
using System.Globalization;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Models;
using OtterZip.App.Services;

namespace OtterZip.App.Modals;

/// <summary>
/// Reusable progress surface for the v0.22 option dialogs. Owns its own
/// single-slot <see cref="JobQueue"/> and renders the same mascot /
/// progress-bar / settle-glyph experience as <c>ProgressDialog</c>, but
/// as an embeddable <see cref="UserControl"/> so a dialog can flip from
/// its option form to this view in-place.
///
/// <para>Usage: the host dialog calls <see cref="Start"/> with a work
/// delegate, then handles <see cref="CloseRequested"/> by closing its
/// window. On window-close-while-running the host gates on
/// <see cref="RequestCancelAndWaitAsync"/> for partial-output cleanup,
/// exactly like <c>ProgressDialog.OnAppWindowClosing</c>.</para>
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 UserControl isn't IDisposable; _jobQueue lives for the host window lifetime and its SemaphoreSlim is reclaimed at process teardown (same rationale as ProgressDialog/MainWindow).")]
public sealed partial class JobProgressView : UserControl
{
    // Segoe Fluent Icons code points (CheckMark / Cancel / Warning). Built
    // from numeric code points instead of string literals so the source
    // stays pure-ASCII and editor-portable (no private-use glyph bytes).
    private const int GlyphCheckMark = 0xE73E;
    private const int GlyphCancel = 0xE711;
    private const int GlyphWarning = 0xE7BA;

    private readonly ResourceLoader _strings = new();
    private readonly JobQueue _jobQueue;
    private readonly DispatcherQueue _dispatcher;
    private readonly System.Diagnostics.Stopwatch _stopwatch = new();
    private JobItem? _jobItem;
    private string _successMessage = string.Empty;
    private string? _revealPath;
    private bool _revealSelect;
    private bool _settled;
    private TaskCompletionSource<bool>? _settlementWait;

    /// <summary>Raised (on the UI thread) when the user clicks Close on a
    /// settled job, or the auto-close timer fires. The host closes its
    /// window in response.</summary>
    public event EventHandler? CloseRequested;

    /// <summary>Raised (on the UI thread) when the job reaches a terminal
    /// state, passing the settled <see cref="JobItem"/>. Hosts inspect
    /// <see cref="JobItem.State"/> to decide follow-up flow (e.g. the
    /// extract dialog loops back to its option form on wrong-password).
    /// Fires before the settled UI is rendered.</summary>
    public event EventHandler<JobItem>? Settled;

    public JobProgressView()
    {
        InitializeComponent();
        _dispatcher = DispatcherQueue.GetForCurrentThread()
                      ?? throw new InvalidOperationException("JobProgressView requires a UI dispatcher.");
        _jobQueue = new JobQueue(_dispatcher, concurrentLimit: 1);
        _jobQueue.JobSettled += OnJobSettled;
    }

    /// <summary>True once the job reached a terminal state.</summary>
    public bool IsSettled => _settled;

    /// <summary>
    /// Submit a unit of work and begin rendering progress.
    /// </summary>
    /// <param name="headerName">Display name in the header (archive/file).</param>
    /// <param name="kind">Compress or Extract — drives the settle glyph context.</param>
    /// <param name="successMessage">Localized "done" line shown on success.</param>
    /// <param name="revealPath">Path opened by the "Open folder" button.</param>
    /// <param name="revealSelect">true = highlight the file in its parent
    /// (compress archive); false = open the folder itself (extract dest).</param>
    /// <param name="work">The background work; receives the item, a cancel
    /// token, and an overall 0..1 progress sink.</param>
    public void Start(
        string headerName,
        JobKind kind,
        string successMessage,
        string revealPath,
        bool revealSelect,
        Func<JobItem, CancellationToken, System.IProgress<double>, Task> work,
        string? reservedOutputPath = null)
    {
        ArgumentNullException.ThrowIfNull(work);
        _successMessage = successMessage ?? string.Empty;
        _revealPath = revealPath;
        _revealSelect = revealSelect;
        _settled = false;
        _stopwatch.Restart();

        ResetVisualState();
        ArchiveNameText.Text = headerName;

        var item = new JobItem(kind, headerName)
        {
            ReservedOutputPath = reservedOutputPath, // queue releases on settle (APP-C1)
        };
        _jobItem = item;
        item.PropertyChanged += OnJobItemPropertyChanged;
        _jobQueue.Submit(item, (ct, progress) => work(item, ct, progress));
    }

    private void ResetVisualState()
    {
        StatusGlyph.Visibility = Visibility.Collapsed;
        OpenFolderButton.Visibility = Visibility.Collapsed;
        OverallBar.IsIndeterminate = true;
        OverallBar.Value = 0;
        ActionButton.IsEnabled = true;
        ActionButton.Content = _strings.GetString("ProgressDialog_CancelButton/Content");
        StatusText.Text = _strings.GetString("ProgressDialog_StatusReady/Text");
    }

    private void OnJobItemPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (sender is not JobItem item) return;
        if (!string.Equals(e.PropertyName, nameof(JobItem.Progress), StringComparison.Ordinal)) return;
        _dispatcher.TryEnqueue(() =>
        {
            double pct = Math.Clamp(item.Progress * 100.0, 0.0, 100.0);
            OverallBar.IsIndeterminate = pct <= 0.01;
            OverallBar.Value = pct;
            if (!_settled)
            {
                StatusText.Text = string.Format(CultureInfo.CurrentCulture, "{0:0.0}%", pct);
            }
        });
    }

    private void OnJobSettled(object? sender, JobItem item)
    {
        if (_jobItem is null || !ReferenceEquals(item, _jobItem)) return;
        _settled = true;
        _stopwatch.Stop();
        _settlementWait?.TrySetResult(true);
        SoundService.MaybePlayCompletion(item);
        Settled?.Invoke(this, item);
        _dispatcher.TryEnqueue(() => RenderSettledUi(item));
    }

    private void RenderSettledUi(JobItem item)
    {
        OverallBar.IsIndeterminate = false;
        OverallBar.Value = item.Progress * 100.0;
        ApplyStatusGlyph(item);
        StatusText.Text = ComposeStatusMessage(item);
        SwitchActionToClose();
        if (item.State == JobState.Done)
        {
            OpenFolderButton.Visibility = string.IsNullOrEmpty(_revealPath)
                ? Visibility.Collapsed
                : Visibility.Visible;
            MaybeStartAutoCloseTimer();
        }
    }

    private static string GetGlyph(JobState state)
    {
        int code = state switch
        {
            JobState.Done => GlyphCheckMark,
            JobState.Cancelled => GlyphCancel,
            JobState.Error => GlyphWarning,
            _ => 0,
        };
        return code == 0 ? string.Empty : char.ConvertFromUtf32(code);
    }

    private static string GetBrushKey(JobState state) => state switch
    {
        JobState.Done => "SystemFillColorSuccessBrush",
        JobState.Cancelled => "TextFillColorSecondaryBrush",
        JobState.Error => "SystemFillColorCriticalBrush",
        _ => "TextFillColorPrimaryBrush",
    };

    private void ApplyStatusGlyph(JobItem item)
    {
        StatusGlyph.Glyph = GetGlyph(item.State);
        StatusGlyph.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources[GetBrushKey(item.State)];
        StatusGlyph.Visibility = Visibility.Visible;
    }

    private string ComposeStatusMessage(JobItem item)
    {
        switch (item.State)
        {
            case JobState.Cancelled:
                return _strings.GetString("ProgressDialog_StatusCancelled/Text");
            case JobState.Error:
                return string.Format(CultureInfo.CurrentCulture,
                    _strings.GetString("ProgressDialog_StatusErrorFormat/Text"),
                    item.StatusText ?? string.Empty);
            case JobState.Done:
                const string sep = "  ·  ";
                string detail = string.IsNullOrEmpty(item.StatusText) ? string.Empty : sep + item.StatusText;
                // Compress StatusText already carries throughput (OperationSummary);
                // append elapsed only for extract, where it doesn't.
                if (item.Kind != JobKind.Compress)
                {
                    detail += sep + FormatElapsed(_stopwatch.Elapsed);
                }
                return _successMessage + detail;
            default:
                return _successMessage;
        }
    }

    private void MaybeStartAutoCloseTimer()
    {
        if (!SettingsService.Get<bool>("Settings_QuickProgressAutoClose", false))
        {
            return;
        }
        var timer = _dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(2);
        timer.IsRepeating = false;
        timer.Tick += (_, _) =>
        {
            timer.Stop();
            CloseRequested?.Invoke(this, EventArgs.Empty);
        };
        timer.Start();
    }

    private void OnOpenFolderClick(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(_revealPath)) return;
        try
        {
            var psi = new System.Diagnostics.ProcessStartInfo
            {
                FileName = "explorer.exe",
                Arguments = _revealSelect
                    ? $"/select,\"{_revealPath}\""
                    : $"\"{_revealPath}\"",
                UseShellExecute = true,
            };
            System.Diagnostics.Process.Start(psi);
        }
        catch (Exception)
        {
            // Non-fatal — explorer failure shouldn't break the dialog.
        }
    }

    private void OnActionButtonClick(object sender, RoutedEventArgs e)
    {
        if (_settled)
        {
            CloseRequested?.Invoke(this, EventArgs.Empty);
            return;
        }
        _jobItem?.RequestCancel();
        ActionButton.IsEnabled = false;
        StatusText.Text = _strings.GetString("Job_StatusCancelling/Text");
    }

    private void SwitchActionToClose()
    {
        ActionButton.IsEnabled = true;
        ActionButton.Content = _strings.GetString("ProgressDialog_CloseButton/Content");
    }

    /// <summary>
    /// Request cancellation and wait up to <paramref name="timeout"/> for
    /// the job to settle (so partial-output cleanup in the work delegate
    /// can run). Returns true if it settled, false on timeout. Host calls
    /// this from its AppWindow.Closing handler.
    /// </summary>
    public async Task<bool> RequestCancelAndWaitAsync(TimeSpan timeout)
    {
        if (_settled || _jobItem is null)
        {
            return true;
        }
        _settlementWait = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        _jobItem.RequestCancel();
        using var cts = new CancellationTokenSource(timeout);
        using var registration = cts.Token.Register(() => _settlementWait?.TrySetResult(false));
        return await _settlementWait.Task.ConfigureAwait(true);
    }

    private string FormatElapsed(TimeSpan elapsed)
    {
        // Localized time units via .resw (hard rule §3.8 — the previous
        // hardcoded "분/초" leaked Korean into all 9 non-ko locales).
        if (elapsed.TotalMinutes >= 1)
        {
            return string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Job_ElapsedMinutesSecondsFormat/Text"),
                (int)elapsed.TotalMinutes, elapsed.Seconds);
        }
        return string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("Job_ElapsedSecondsFormat/Text"),
            elapsed.TotalSeconds);
    }
}
