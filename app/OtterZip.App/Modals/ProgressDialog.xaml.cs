using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Threading.Tasks;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Models;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.App.Modals;

/// <summary>
/// Bandizip-style quick-compress modal. Owns its own
/// <see cref="JobQueue"/> (concurrency=1, single job) so it can run
/// completely without the MainWindow being alive.
///
/// <para>Lifecycle:</para>
/// <list type="number">
///   <item>Constructor receives the <see cref="InvokeRequest"/>, sizes
///   the window, resolves the destination + plan, and submits the
///   compress job. Activate() is called by App.OnLaunched.</item>
///   <item>Progress ticks marshal onto the dispatcher and update the
///   <see cref="ProgressBar"/> + status text.</item>
///   <item>On <see cref="JobQueue.JobSettled"/>: render the settle UI
///   (status glyph, summary line, action button flip, optional
///   auto-close timer).</item>
///   <item>If the user closes the window mid-flight, the
///   <see cref="AppWindow.Closing"/> handler cancels, asks the job to
///   stop, and waits up to 5 s for the partial-archive cleanup to run
///   before letting the window close.</item>
/// </list>
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 Window doesn't implement IDisposable. _jobQueue lives for the window lifetime; SemaphoreSlim is released when the process tears down (same rationale as MainWindow).")]
public sealed partial class ProgressDialog : Window
{
    private readonly ResourceLoader _strings = new();
    private readonly JobQueue _jobQueue;
    private readonly DispatcherQueue _dispatcher;
    private readonly InvokeRequest _request;
    private JobItem? _jobItem;
    private string? _destinationPath;
    private bool _settled;
    private bool _closeRequested;
    private TaskCompletionSource<bool>? _settlementWait;
    private readonly System.Diagnostics.Stopwatch _stopwatch = new();

    internal ProgressDialog(InvokeRequest request)
    {
        ArgumentNullException.ThrowIfNull(request);
        InitializeComponent();
        _request = request;
        _dispatcher = DispatcherQueue.GetForCurrentThread()
                      ?? throw new InvalidOperationException("ProgressDialog requires a UI dispatcher.");
        _jobQueue = new JobQueue(_dispatcher, concurrentLimit: 1);
        _jobQueue.JobSettled += OnJobSettled;

        // 500×260 dev pixels. Sized to fit mascot header + status row
        // + two-button action bar without crowding.
        TrySizeWindow(500, 260);
        _stopwatch.Start();
        AppWindow.Closing += OnAppWindowClosing;
        _ = StartCompressAsync();
    }

    private async Task StartCompressAsync()
    {
        try
        {
            // Settings_DefaultFormat is stored as the UI tag ("ZIP",
            // "7Z", "TAR.GZ"); CompressEngine takes lowercase. Unknown
            // tags fall back to ZIP via the engine's _default arm.
            string? quickFormat = _request.QuickFormat;
            string defaultTag = SettingsService.Get<string>("Settings_DefaultFormat", "ZIP");
            string fallbackFormat = string.Equals(defaultTag, "7Z", StringComparison.OrdinalIgnoreCase) ? "7z"
                : string.Equals(defaultTag, "TAR.GZ", StringComparison.OrdinalIgnoreCase) ? "tar.gz"
                : string.Equals(defaultTag, "TAR.BZ2", StringComparison.OrdinalIgnoreCase) ? "tar.bz2"
                : string.Equals(defaultTag, "TAR.XZ", StringComparison.OrdinalIgnoreCase) ? "tar.xz"
                : string.Equals(defaultTag, "TAR", StringComparison.OrdinalIgnoreCase) ? "tar"
                : string.Equals(defaultTag, "ZIPX", StringComparison.OrdinalIgnoreCase) ? "zipx"
                : string.Equals(defaultTag, "XZ", StringComparison.OrdinalIgnoreCase) ? "xz"
                : "zip";
            var plan = CompressEngine.BuildPlan(_request.Paths, fallbackFormat, quickFormat);
            _destinationPath = plan.Destination;

            string basename = Path.GetFileName(plan.Destination);
            ArchiveNameText.Text = basename;
            Title = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("ProgressDialog_TitleCompressFormat/Text"), basename);

            string? password = await PasswordResolver.ResolveCompressAsync(
                panelPassword: null,
                authReason: _strings.GetString("Auth_ReasonCompress/Text"),
                promptArchiveLabel: basename,
                prompter: PromptPasswordAsync).ConfigureAwait(true);
            if (password is null && SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false))
            {
                Close();
                return;
            }

            var sources = _request.Paths;
            var item = new JobItem(JobKind.Compress, basename);
            _jobItem = item;
            item.PropertyChanged += OnJobItemPropertyChanged;
            _jobQueue.Submit(item, (ct, progress) =>
                RunWorkAsync(item, plan, sources, password, ct, progress));
        }
        catch (Exception ex)
        {
            ShowTerminalStatus(string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("ProgressDialog_StatusErrorFormat/Text"), ex.Message));
            SwitchActionToClose();
        }
    }

    private async Task RunWorkAsync(
        JobItem item,
        CompressEngine.CompressPlan plan,
        IReadOnlyList<string> sources,
        string? password,
        System.Threading.CancellationToken ct,
        IProgress<double> overallProgress)
    {
        var richProgress = new Progress<ProgressUpdate>(p =>
        {
            double frac = p.FractionComplete;
            if (frac > 0)
            {
                overallProgress.Report(Math.Clamp(frac, 0.0, 1.0));
            }
        });

        try
        {
            var report = await CompressEngine.RunAsync(plan, sources, password, richProgress, ct).ConfigureAwait(false);
            await CompressEngine.MaybeVerifyAsync(plan.Destination, ct).ConfigureAwait(false);
            CompressEngine.MaybeRecycleSources(sources);
            _dispatcher.TryEnqueue(() =>
            {
                item.ResultPath = plan.Destination;
                item.Progress = 1.0;
                item.StatusText = FormatByteSize(report.BytesWritten);
            });
        }
        catch (OperationCanceledException)
        {
            CompressEngine.TryDeletePartialArchive(plan.Destination);
            throw;
        }
    }

    private void OnJobItemPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (sender is not JobItem item) return;
        if (string.Equals(e.PropertyName, nameof(JobItem.Progress), StringComparison.Ordinal))
        {
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
    }

    private void OnJobSettled(object? sender, JobItem item)
    {
        if (_jobItem is null || !ReferenceEquals(item, _jobItem)) return;
        _settled = true;
        _stopwatch.Stop();
        _settlementWait?.TrySetResult(true);
        _dispatcher.TryEnqueue(() => RenderSettledUi(item));
    }

    // Settle-time UI mutation. Pulled out of OnJobSettled so each step
    // (glyph, status, title, auto-close timer) stays under the
    // analyzer's 60-line cap.
    private void RenderSettledUi(JobItem item)
    {
        OverallBar.IsIndeterminate = false;
        OverallBar.Value = item.Progress * 100.0;
        ApplyStatusGlyph(item);
        StatusText.Text = ComposeStatusMessage(item);
        if (!string.IsNullOrEmpty(_destinationPath))
        {
            string basename = Path.GetFileName(_destinationPath);
            Title = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("ProgressDialog_TitleDoneFormat/Text"), basename);
        }
        SwitchActionToClose();
        if (item.State == JobState.Done)
        {
            OpenFolderButton.Visibility = Visibility.Visible;
            MaybeStartAutoCloseTimer();
        }
    }

    // Segoe Fluent Icons codepoints (Done=CheckMark, Cancelled=Cancel,
    // Error=Warning) stored as escape sequences so the source file
    // stays portable across editors that mishandle private-use chars.
    private static string ApplyStatusGlyphCore_GetGlyph(JobState state) => state switch
    {
        JobState.Done => "",
        JobState.Cancelled => "",
        JobState.Error => "",
        _ => "",
    };

    private static string ApplyStatusGlyphCore_GetBrushKey(JobState state) => state switch
    {
        JobState.Done => "SystemFillColorSuccessBrush",
        JobState.Cancelled => "TextFillColorSecondaryBrush",
        JobState.Error => "SystemFillColorCriticalBrush",
        _ => "TextFillColorPrimaryBrush",
    };

    private void ApplyStatusGlyph(JobItem item)
    {
        StatusGlyph.Glyph = ApplyStatusGlyphCore_GetGlyph(item.State);
        StatusGlyph.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources[ApplyStatusGlyphCore_GetBrushKey(item.State)];
        StatusGlyph.Visibility = Visibility.Visible;
    }

    private string ComposeStatusMessage(JobItem item)
    {
        string baseMessage = item.State switch
        {
            JobState.Done => _strings.GetString("ProgressDialog_StatusSuccess/Text"),
            JobState.Cancelled => _strings.GetString("ProgressDialog_StatusCancelled/Text"),
            JobState.Error => string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("ProgressDialog_StatusErrorFormat/Text"),
                item.StatusText ?? string.Empty),
            _ => string.Empty,
        };
        if (item.State == JobState.Done && !string.IsNullOrEmpty(_destinationPath))
        {
            try
            {
                var info = new FileInfo(_destinationPath);
                string size = FormatByteSize((ulong)info.Length);
                string elapsed = FormatElapsed(_stopwatch.Elapsed);
                return $"{baseMessage}  ·  {size}  ·  {elapsed}";
            }
            catch (IOException) { /* file may have been moved/locked */ }
        }
        return baseMessage;
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
            if (!_closeRequested) Close();
        };
        timer.Start();
    }

    private void OnOpenFolderClick(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(_destinationPath)) return;
        try
        {
            // explorer.exe /select, highlights the archive in its
            // parent folder instead of navigating into it.
            var psi = new System.Diagnostics.ProcessStartInfo
            {
                FileName = "explorer.exe",
                Arguments = $"/select,\"{_destinationPath}\"",
                UseShellExecute = true,
            };
            System.Diagnostics.Process.Start(psi);
        }
        catch (Exception)
        {
            // Non-fatal — explorer failure shouldn't break the dialog.
        }
    }

    private static string FormatElapsed(TimeSpan elapsed)
    {
        if (elapsed.TotalMinutes >= 1)
        {
            return $"{(int)elapsed.TotalMinutes}분 {elapsed.Seconds}초";
        }
        return $"{elapsed.TotalSeconds:0.0}초";
    }

    private static string FormatByteSize(ulong bytes)
    {
        const ulong KB = 1024;
        const ulong MB = KB * 1024;
        const ulong GB = MB * 1024;
        if (bytes >= GB) return string.Format(CultureInfo.CurrentCulture, "{0:0.##} GB", bytes / (double)GB);
        if (bytes >= MB) return string.Format(CultureInfo.CurrentCulture, "{0:0.##} MB", bytes / (double)MB);
        if (bytes >= KB) return string.Format(CultureInfo.CurrentCulture, "{0:0.##} KB", bytes / (double)KB);
        return string.Format(CultureInfo.CurrentCulture, "{0} B", bytes);
    }

    private void SwitchActionToClose()
    {
        ActionButton.Content = _strings.GetString("ProgressDialog_CloseButton/Content");
    }

    private void ShowTerminalStatus(string message)
    {
        _dispatcher.TryEnqueue(() =>
        {
            OverallBar.IsIndeterminate = false;
            StatusText.Text = message;
            _settled = true;
        });
    }

    private void OnActionButtonClick(object sender, RoutedEventArgs e)
    {
        if (_settled)
        {
            _closeRequested = true;
            Close();
            return;
        }
        _jobItem?.RequestCancel();
        ActionButton.IsEnabled = false;
        StatusText.Text = _strings.GetString("Job_StatusCancelling/Text");
    }

    private async void OnAppWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (_settled || _closeRequested) return;
        args.Cancel = true;
        _closeRequested = true;
        _settlementWait = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        _jobItem?.RequestCancel();
        using var cts = new System.Threading.CancellationTokenSource(TimeSpan.FromSeconds(5));
        cts.Token.Register(() => _settlementWait?.TrySetResult(false));
        await _settlementWait.Task.ConfigureAwait(true);
        Close();
    }

    private async Task<string?> PromptPasswordAsync(string archiveName)
    {
        var passwordBox = new PasswordBox
        {
            PlaceholderText = _strings.GetString("PasswordDialog_Placeholder/Text"),
            Width = 280,
        };
        var dialog = new ContentDialog
        {
            Title = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("PasswordDialog_TitleFormat/Text"),
                archiveName),
            Content = passwordBox,
            PrimaryButtonText = _strings.GetString("PasswordDialog_OK/Text"),
            CloseButtonText = _strings.GetString("PasswordDialog_Cancel/Text"),
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };
        var result = await dialog.ShowAsync();
        return result == ContentDialogResult.Primary ? passwordBox.Password : null;
    }

    private void TrySizeWindow(int width, int height)
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = AppWindow.GetFromWindowId(windowId);
            MainWindow.TrySetWindowIcon(appWindow);

            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi / 96.0;
            int physW = (int)(width * scale);
            int physH = (int)(height * scale);
            appWindow.Resize(new Windows.Graphics.SizeInt32(physW, physH));

            if (appWindow.Presenter is OverlappedPresenter presenter)
            {
                presenter.IsResizable = false;
                presenter.IsMaximizable = false;
            }
        }
        catch (Exception)
        {
            // Best-effort sizing — fall back to OS default.
        }
    }

    [System.Runtime.InteropServices.DllImport("user32.dll", ExactSpelling = true)]
    private static extern uint GetDpiForWindow(IntPtr hwnd);
}
