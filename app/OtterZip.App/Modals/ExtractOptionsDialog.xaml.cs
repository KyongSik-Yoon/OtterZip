using System;
using System.Globalization;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Models;
using OtterZip.App.Services;
using OtterZip.App.UserControls;
using OtterZip.Interop;

namespace OtterZip.App.Modals;

/// <summary>
/// v0.22 dedicated extract-options dialog. Routed to from the shell verb
/// "OtterZip으로 풀기...(B)" (the <c>extract-dialog</c> verb). Reuses the
/// inline <see cref="ExtractPanel"/> as its option form and an embedded
/// <see cref="JobProgressView"/> for the run, so the whole flow lives in
/// one small window instead of the MainWindow surface.
///
/// <para>Multiple selected archives are handled sequentially: option
/// form → progress → (next archive's option form) → … A wrong-password
/// settle loops back to the same archive's option form with an inline
/// error, mirroring <c>MainWindow.RunExtractPanelLoopAsync</c>.</para>
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 Window isn't IDisposable; the embedded JobProgressView's JobQueue lives for the window lifetime (same rationale as ProgressDialog).")]
public sealed partial class ExtractOptionsDialog : Window
{
    private readonly ResourceLoader _strings = new();
    private readonly InvokeRequest _request;
    private readonly DispatcherQueue _dispatcher;
    private int _index;
    private bool _running;
    private bool _closing;
    private volatile bool _wrongPassword;
    private string _currentArchive = string.Empty;

    internal ExtractOptionsDialog(InvokeRequest request)
    {
        ArgumentNullException.ThrowIfNull(request);
        InitializeComponent();
        _request = request;
        _dispatcher = DispatcherQueue.GetForCurrentThread()
                      ?? throw new InvalidOperationException("ExtractOptionsDialog requires a UI dispatcher.");

        if (Content is FrameworkElement root)
        {
            ThemeService.Apply(root, ThemeService.Load());
        }
        TrySizeWindow(460, 360);
        AppWindow.Closing += OnAppWindowClosing;

        Panel.Submitted += OnPanelSubmitted;
        Panel.Dismissed += OnPanelDismissed;
        Panel.PasswordEdited += (_, _) => Panel.ClearError();
        Progress.CloseRequested += OnProgressCloseRequested;
        Progress.Settled += OnProgressSettled;
        // Auto-fit the window to the progress view whenever its content height
        // changes (percent → multi-line done summary) so no dead space remains.
        Progress.SizeChanged += (_, _) =>
        {
            if (Progress.Visibility == Visibility.Visible) { FitHeightToContent(460); }
        };

        ConfigureForCurrentArchive();
    }

    private void ConfigureForCurrentArchive()
    {
        if (_index >= _request.Paths.Count)
        {
            Close();
            return;
        }
        _running = false;
        Progress.Visibility = Visibility.Collapsed;
        Panel.Visibility = Visibility.Visible;
        TrySizeWindow(460, 360); // restore the option-form height

        _currentArchive = _request.Paths[_index];
        bool isEncrypted = ProbeIsEncrypted(_currentArchive);
        string suggestedDest = ResolveExtractDestination(_currentArchive);
        string? prefill = PasswordResolver.ResolveExtractPanelPrefill(isEncrypted);

        Title = string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("ExtractOptionsDialog_TitleFormat/Text"),
            Path.GetFileName(_currentArchive));

        Panel.Configure(_currentArchive, suggestedDest, isEncrypted,
            showDestination: true, prefillPassword: prefill);
    }

    private void OnPanelSubmitted(object? sender, ExtractPanelSubmitArgs args)
    {
        string dest = args.ExtractHere || string.IsNullOrWhiteSpace(args.Destination)
            ? ComputeExtractHerePath(_currentArchive)
            : args.Destination;
        bool destExistedBefore = Directory.Exists(dest);
        string? password = args.Password;
        string archive = _currentArchive;
        string basename = Path.GetFileName(archive);

        _wrongPassword = false;
        _running = true;
        Panel.Visibility = Visibility.Collapsed;
        Progress.Visibility = Visibility.Visible;
        // The progress view is far shorter than the option form — auto-fit the
        // window to it so no empty gap remains (SizeChanged re-fits on the
        // multi-line done summary). Restored to the form height on loop-back.
        FitHeightToContent(460);
        Title = string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("ExtractOptionsDialog_TitleExtractingFormat/Text"), basename);

        Progress.Start(
            basename,
            JobKind.Extract,
            _strings.GetString("ProgressDialog_StatusExtractSuccess/Text"),
            dest,
            revealSelect: false,
            (item, ct, overall) =>
                RunExtractWorkAsync(item, archive, dest, password, destExistedBefore, ct, overall));
    }

    private void OnPanelDismissed(object? sender, EventArgs e) => Close();

    private void OnProgressSettled(object? sender, JobItem item)
    {
        // Wrong password: loop back to this archive's option form with an
        // inline error instead of leaving the user on the error screen.
        if (item.State == JobState.Error && _wrongPassword)
        {
            _dispatcher.TryEnqueue(() =>
            {
                _running = false;
                Progress.Visibility = Visibility.Collapsed;
                Panel.Visibility = Visibility.Visible;
                TrySizeWindow(460, 360); // restore the option-form height
                Panel.Configure(_currentArchive,
                    ResolveExtractDestination(_currentArchive),
                    needsPassword: true, showDestination: true, prefillPassword: null);
                Panel.ShowError(_strings.GetString("Error_WrongPassword/Text"));
            });
            return;
        }
        // Done / hard error: the done summary is multi-line, and a Top-aligned
        // view clipped by the (1-line-sized) window won't fire SizeChanged — so
        // re-fit explicitly once the summary has rendered. Low priority runs
        // after JobProgressView's own settled-UI enqueue; the measure uses an
        // infinite height so the clipped state doesn't matter.
        _dispatcher.TryEnqueue(
            Microsoft.UI.Dispatching.DispatcherQueuePriority.Low,
            () => { if (Progress.Visibility == Visibility.Visible) { FitHeightToContent(460); } });

        // Advance the pointer so the progress view's Close button moves to the
        // next archive (or closes the window when none remain).
        _index++;
    }

    private void OnProgressCloseRequested(object? sender, EventArgs e)
    {
        if (_index < _request.Paths.Count)
        {
            ConfigureForCurrentArchive();
        }
        else
        {
            Close();
        }
    }

    private async Task RunExtractWorkAsync(
        JobItem item,
        string archivePath,
        string destination,
        string? password,
        bool destExistedBefore,
        CancellationToken ct,
        IProgress<double> overall)
    {
        bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
        try
        {
            using var archive = string.IsNullOrEmpty(password)
                ? Archive.Open(archivePath)
                : Archive.OpenWithPassword(archivePath, password);
            var bridge = new Progress<ProgressUpdate>(p =>
                overall.Report(Math.Clamp(p.FractionComplete, 0.0, 1.0)));
            var report = await archive
                .ExtractAllAsync(destination, ExtractDefaults.ResolveOverwrite(), bridge,
                    preserveZoneIdentifier: preserveMotw, cancellationToken: ct)
                .ConfigureAwait(false);
            _dispatcher.TryEnqueue(() =>
            {
                item.ResultPath = destination;
                item.Progress = 1.0;
                item.StatusText = string.Format(CultureInfo.CurrentCulture,
                    _strings.GetString("Main_StatusBarDoneFormat/Text"),
                    report.EntriesExtracted, FormatByteSize(report.BytesWritten));
            });
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            _wrongPassword = true;
            throw;
        }
        catch (OperationCanceledException)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
    }

    private async void OnAppWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (!_running || Progress.IsSettled || _closing)
        {
            return;
        }
        args.Cancel = true;
        _closing = true;
        try
        {
            await Progress.RequestCancelAndWaitAsync(TimeSpan.FromSeconds(5)).ConfigureAwait(true);
            Close();
        }
        catch (Exception)
        {
            // async void — an escaping COMException on a torn-down HWND
            // would leave the window stuck open. Best-effort re-close.
            try { Close(); } catch (Exception) { /* already gone */ }
        }
    }

    // ---- extract-destination helpers (mirrors MainWindow's privates so
    //      the dialog stays self-contained; behavior kept identical) ----

    private static bool ProbeIsEncrypted(string archivePath)
    {
        try
        {
            using var probe = Archive.Open(archivePath);
            return probe.IsEncrypted();
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            return true;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private static string ResolveExtractDestination(string archivePath)
    {
        string extractLoc = SettingsService.Get<string>("Settings_ExtractLocation", "same");
        string customDir = SettingsService.Get<string>("Settings_ExtractLocationPath", "");
        bool useSubfolder = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);
        bool useCustom = string.Equals(extractLoc, "custom", StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(customDir)
            && Directory.Exists(customDir);

        string baseDir = useCustom
            ? customDir
            : (Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory());
        string stem = Path.GetFileNameWithoutExtension(archivePath);
        string dest = useSubfolder ? Path.Combine(baseDir, stem) : baseDir;
        return useSubfolder ? EnsureUniqueExtractDirectory(dest) : dest;
    }

    private static string ComputeExtractHerePath(string archivePath)
        => Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory();

    private static string EnsureUniqueExtractDirectory(string path)
    {
        if (!Directory.Exists(path)) return path;
        string parent = Path.GetDirectoryName(path) ?? Directory.GetCurrentDirectory();
        string name = Path.GetFileName(path.TrimEnd(
            Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        for (int i = 1; i < 10000; i++)
        {
            string candidate = Path.Combine(parent, string.Format(
                CultureInfo.InvariantCulture, "{0} ({1})", name, i));
            if (!Directory.Exists(candidate)) return candidate;
        }
        return Path.Combine(parent, string.Format(CultureInfo.InvariantCulture,
            "{0} ({1:yyyyMMddHHmmss})", name, DateTime.Now));
    }

    private static void RollbackPartialExtract(string destination, bool destExistedBefore)
    {
        if (destExistedBefore || !Directory.Exists(destination)) return;
        try
        {
            Directory.Delete(destination, recursive: true);
        }
        catch (IOException) { /* best effort */ }
        catch (UnauthorizedAccessException) { /* best effort */ }
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

    /// <summary>
    /// Resize the window height to exactly fit the currently-visible content
    /// (the compact progress view) — removes the dead space the tall option-form
    /// height leaves below it. Width stays fixed; clamped for safety.
    /// </summary>
    private void FitHeightToContent(int widthDip)
    {
        if (Content is not FrameworkElement root) { return; }
        root.Measure(new Windows.Foundation.Size(widthDip, double.PositiveInfinity));
        int contentH = (int)Math.Ceiling(root.DesiredSize.Height);
        if (contentH <= 0) { return; }
        // + title-bar/frame allowance (~44 DIP) — enough that the action-bar
        // bottom padding survives (buttons aren't flush against the edge).
        TrySizeWindow(widthDip, Math.Clamp(contentH + 44, 150, 440));
    }

    private void TrySizeWindow(int width, int height)
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = AppWindow.GetFromWindowId(windowId);
            MainWindow.TrySetWindowIcon(appWindow);
            WindowChrome.ApplyTitleBarTheme(appWindow);

            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi / 96.0;
            appWindow.Resize(new Windows.Graphics.SizeInt32((int)(width * scale), (int)(height * scale)));

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
