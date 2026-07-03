using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Threading;
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
/// v0.22 dedicated compress-options dialog. Routed to from the shell
/// verb "OtterZip으로 압축...(O)" — i.e. the plain <c>compress</c> verb
/// with no <c>QuickFormat</c>. The user picks format / level /
/// destination / password; on <b>Compress</b> the window flips in-place
/// to an embedded <see cref="JobProgressView"/> that runs the build via
/// <see cref="CompressEngine"/>. One window, no MainWindow surface.
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 Window isn't IDisposable; the embedded JobProgressView's JobQueue lives for the window lifetime (same rationale as ProgressDialog).")]
public sealed partial class CompressOptionsDialog : Window
{
    private static readonly string[] ArchiveExtensions =
    {
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst",
        ".zip", ".7z", ".tar", ".xz", ".zipx", ".gz", ".bz2", ".zst",
    };

    private readonly ResourceLoader _strings = new();
    private readonly InvokeRequest _request;
    private readonly DispatcherQueue _dispatcher;
    private bool _ready;
    private bool _running;
    private bool _closing;

    internal CompressOptionsDialog(InvokeRequest request)
    {
        ArgumentNullException.ThrowIfNull(request);
        InitializeComponent();
        _request = request;
        _dispatcher = DispatcherQueue.GetForCurrentThread()
                      ?? throw new InvalidOperationException("CompressOptionsDialog requires a UI dispatcher.");

        if (Content is FrameworkElement root)
        {
            ThemeService.Apply(root, ThemeService.Load());
        }
        Title = _strings.GetString("CompressOptionsDialog_Title/Text");
        TrySizeWindow(480, 500);
        AppWindow.Closing += OnAppWindowClosing;
        Progress.CloseRequested += (_, _) => Close();

        InitOptions();
    }

    private void InitOptions()
    {
        SummaryText.Text = string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("CompressOptionsDialog_SummaryFormat/Text"), _request.Paths.Count);

        string fmt = SettingsService.Get<string>("Settings_DefaultFormat", "ZIP");
        SelectFormat(fmt);
        LevelCombo.SelectedIndex = Math.Clamp(
            SettingsService.Get<int>("Settings_DefaultMethodIndex", 2), 0, 3);

        DestinationField.Text = ComposeDefaultDestination(CurrentFormatTag());
        ApplyFormatConstraints();
        _ready = true;
    }

    private void SelectFormat(string tag)
    {
        for (int i = 0; i < FormatCombo.Items.Count; i++)
        {
            if (FormatCombo.Items[i] is ComboBoxItem item
                && item.Tag is string t
                && string.Equals(t, tag, StringComparison.Ordinal))
            {
                FormatCombo.SelectedIndex = i;
                return;
            }
        }
        FormatCombo.SelectedIndex = 0;
    }

    private string CurrentFormatTag()
        => FormatCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag ? tag : "ZIP";

    private string ComposeDefaultDestination(string formatTag)
    {
        int methodIndex = Math.Clamp(LevelCombo.SelectedIndex, 0, 3);
        var (_, _, ext) = CompressEngine.MapFormatAndMethod(formatTag, methodIndex);
        bool useParent = SettingsService.Get<bool>("Settings_UseParentFolderName", true);
        string stem = OutputNamer.DeriveStem(_request.Paths, useParent);
        string saveLoc = SettingsService.Get<string>("Settings_SaveLocation", "same");
        string customDir = SettingsService.Get<string>("Settings_SaveLocationPath", "");
        return OutputNamer.Compose(_request.Paths, stem, ext, saveLoc, customDir);
    }

    private void OnFormatChanged(object sender, SelectionChangedEventArgs e)
    {
        // Suppress the change that fires while InitOptions seeds the
        // initial selection — the destination is composed there directly.
        if (!_ready || DestinationField is null || PasswordInput is null)
        {
            return;
        }
        string tag = CurrentFormatTag();
        int methodIndex = Math.Clamp(LevelCombo.SelectedIndex, 0, 3);
        var (_, _, ext) = CompressEngine.MapFormatAndMethod(tag, methodIndex);
        DestinationField.Text = OutputNamer.EnsureUnique(SwapArchiveExtension(DestinationField.Text, ext));
        ApplyFormatConstraints();
    }

    private void ApplyFormatConstraints()
    {
        string tag = CurrentFormatTag();
        bool supportsPassword = FormatSupportsPassword(tag);
        PasswordInput.IsEnabled = supportsPassword;
        RevealButton.IsEnabled = supportsPassword;
        if (!supportsPassword)
        {
            PasswordInput.Password = string.Empty;
        }

        // Solid is a 7z concept; split (raw .NNN byte slices) is offered
        // for 7z and ZIP only — 7-Zip / Bandizip read both natively.
        bool supportsSolid = tag is "7z";
        bool supportsSplit = tag is "7z" or "ZIP";
        SolidCheck.IsEnabled = supportsSolid;
        if (!supportsSolid)
        {
            SolidCheck.IsChecked = false;
        }
        SplitCheck.IsEnabled = supportsSplit;
        if (!supportsSplit)
        {
            SplitCheck.IsChecked = false;
        }
        UpdateSplitRowVisibility(tag);
    }

    private void UpdateSplitRowVisibility(string tag)
    {
        bool splitOn = SplitCheck.IsChecked.GetValueOrDefault() && SplitCheck.IsEnabled;
        SplitSizeRow.Visibility = splitOn ? Visibility.Visible : Visibility.Collapsed;
        // A `.001` split is a raw byte slice, not an APPNOTE spanned ZIP,
        // so some third-party tools won't auto-join it. Warn for ZIP only.
        SplitZipNote.Visibility =
            splitOn && tag is "ZIP" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnSplitToggled(object sender, RoutedEventArgs e)
    {
        if (!_ready || SplitSizeRow is null)
        {
            return;
        }
        UpdateSplitRowVisibility(CurrentFormatTag());
    }

    private static bool FormatSupportsPassword(string format) => format is "ZIP" or "7z";

    private void OnRevealToggled(object sender, RoutedEventArgs e)
    {
        PasswordInput.PasswordRevealMode = RevealButton.IsChecked.GetValueOrDefault()
            ? PasswordRevealMode.Visible
            : PasswordRevealMode.Hidden;
    }

    private async void OnBrowseClick(object sender, RoutedEventArgs e)
    {
        try
        {
            var picker = new Windows.Storage.Pickers.FolderPicker();
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);
            picker.FileTypeFilter.Add("*");
            picker.SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop;
            var folder = await picker.PickSingleFolderAsync();
            if (folder is not null)
            {
                string fileName = Path.GetFileName(DestinationField.Text);
                if (string.IsNullOrEmpty(fileName))
                {
                    var (_, _, ext) = CompressEngine.MapFormatAndMethod(
                        CurrentFormatTag(), Math.Clamp(LevelCombo.SelectedIndex, 0, 3));
                    fileName = OutputNamer.DeriveStem(
                        _request.Paths,
                        SettingsService.Get<bool>("Settings_UseParentFolderName", true)) + ext;
                }
                DestinationField.Text = Path.Combine(folder.Path, fileName);
            }
        }
        catch (Exception)
        {
            // Picker init can fail in unpackaged builds; degrade gracefully.
        }
    }

    private void OnCancelClick(object sender, RoutedEventArgs e) => Close();

    private void OnCompressClick(object sender, RoutedEventArgs e)
    {
        string raw = DestinationField.Text?.Trim() ?? string.Empty;
        if (string.IsNullOrEmpty(raw))
        {
            ShowError(_strings.GetString("CompressOptionsDialog_ErrorNoDestination/Text"));
            return;
        }

        string tag = CurrentFormatTag();
        int methodIndex = Math.Clamp(LevelCombo.SelectedIndex, 0, 3);
        var (fmt, method, ext) = CompressEngine.MapFormatAndMethod(tag, methodIndex);
        byte level = CompressEngine.MapMethodIndexToLevel(methodIndex);

        if (!TryComposeDestination(raw, ext, out string destination))
        {
            return;
        }

        string? password = FormatSupportsPassword(tag) && !string.IsNullOrEmpty(PasswordInput.Password)
            ? PasswordInput.Password
            : null;

        bool solid = SolidCheck.IsChecked.GetValueOrDefault() && SolidCheck.IsEnabled;
        ulong volumeBytes = ResolveVolumeBytes();

        var plan = new CompressEngine.CompressPlan(destination, fmt, method, level, solid, volumeBytes);
        var sources = _request.Paths;
        // In split mode the deliverable is the `.001/.002/…` set, not the
        // contiguous name — reveal / select the first segment.
        string revealTarget = volumeBytes > 0 ? destination + ".001" : destination;
        string basename = Path.GetFileName(revealTarget);

        _running = true;
        OptionsRoot.Visibility = Visibility.Collapsed;
        ActionBar.Visibility = Visibility.Collapsed;
        Progress.Visibility = Visibility.Visible;
        TrySizeWindow(500, 260);
        Title = string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("ProgressDialog_TitleCompressFormat/Text"), basename);

        Progress.Start(
            basename,
            JobKind.Compress,
            _strings.GetString("ProgressDialog_StatusSuccess/Text"),
            revealTarget,
            revealSelect: true,
            (item, ct, overall) => RunCompressWorkAsync(item, plan, sources, password, ct, overall));
    }

    /// <summary>
    /// Resolve the final, collision-free destination path and ensure its
    /// parent exists. On failure, surfaces the error inline and returns
    /// false so the caller aborts.
    /// </summary>
    private bool TryComposeDestination(string raw, string ext, out string destination)
    {
        destination = string.Empty;
        try
        {
            // Real submit point — reserve against other in-flight jobs, not
            // just the filesystem (the preview paths keep plain EnsureUnique).
            destination = OutputNamer.ReserveUnique(SwapArchiveExtension(raw, ext));
            string parent = Path.GetDirectoryName(destination) ?? string.Empty;
            if (!string.IsNullOrEmpty(parent))
            {
                Directory.CreateDirectory(parent);
            }
            return true;
        }
        catch (Exception ex)
        {
            ShowError(string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("ProgressDialog_StatusErrorFormat/Text"), ex.Message));
            return false;
        }
    }

    /// <summary>
    /// Volume size in bytes from the split controls, or 0 when split is
    /// off / unsupported. Floors at 1&#160;MB so an empty field can't
    /// request a zero-size split.
    /// </summary>
    private ulong ResolveVolumeBytes()
    {
        if (!(SplitCheck.IsChecked.GetValueOrDefault() && SplitCheck.IsEnabled))
        {
            return 0;
        }
        // NumberBox only commits typed text into Value on focus loss / Enter,
        // so "type 30 → click 압축" reads a STALE Value (live-reproduced
        // 2026-07-01: a 30 MB split silently ran with the previous 100 MB).
        // Prefer the box's current text when it parses; fall back to Value.
        double mb = SplitSizeBox.Value;
        if (double.TryParse(
                SplitSizeBox.Text,
                NumberStyles.Float | NumberStyles.AllowThousands,
                CultureInfo.CurrentCulture,
                out double typed))
        {
            mb = typed;
        }
        if (!double.IsFinite(mb) || mb < 1)
        {
            mb = 1;
        }
        return (ulong)(mb * 1024 * 1024);
    }

    private async Task RunCompressWorkAsync(
        JobItem item,
        CompressEngine.CompressPlan plan,
        IReadOnlyList<string> sources,
        string? password,
        CancellationToken ct,
        IProgress<double> overall)
    {
        var rich = new Progress<ProgressUpdate>(p =>
        {
            if (p.FractionComplete > 0)
            {
                overall.Report(Math.Clamp(p.FractionComplete, 0.0, 1.0));
            }
        });
        try
        {
            var workTimer = System.Diagnostics.Stopwatch.StartNew();
            var report = await CompressEngine.RunAsync(plan, sources, password, rich, ct).ConfigureAwait(false);
            workTimer.Stop();
            ulong originalBytes = OperationSummary.TotalInputBytes(sources);
            string summary = OperationSummary.Compress(originalBytes, report.BytesWritten, workTimer.Elapsed);
            // Verify re-opens the single archive; the split `.NNN` set has no
            // single handle to CRC-test here, so skip verify in split mode
            // (the slices are byte-faithful — see the core split round-trip).
            if (plan.VolumeSizeBytes == 0)
            {
                await CompressEngine.MaybeVerifyAsync(plan.Destination, ct, password).ConfigureAwait(false);
            }
            CompressEngine.MaybeRecycleSources(sources);
            string resultPath = plan.VolumeSizeBytes > 0 ? plan.Destination + ".001" : plan.Destination;
            _dispatcher.TryEnqueue(() =>
            {
                item.ResultPath = resultPath;
                item.Progress = 1.0;
                item.StatusText = summary;
            });
        }
        catch
        {
            // The output is unusable on ANY failure (cancel, disk full, IO
            // error) — a truncated archive left on disk looks real. Sweep the
            // base file and, in split mode, the .001..NNN segment set.
            CompressEngine.TryDeletePartialArchive(plan.Destination, plan.VolumeSizeBytes);
            throw;
        }
    }

    private void ShowError(string message)
    {
        ErrorText.Text = message;
        ErrorText.Visibility = Visibility.Visible;
    }

    private async void OnAppWindowClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (!_running || Progress.IsSettled || _closing)
        {
            return;
        }
        args.Cancel = true;
        _closing = true;
        await Progress.RequestCancelAndWaitAsync(TimeSpan.FromSeconds(5)).ConfigureAwait(true);
        Close();
    }

    private static string SwapArchiveExtension(string path, string newExt)
    {
        string dir = Path.GetDirectoryName(path) ?? string.Empty;
        string name = Path.GetFileName(path);
        foreach (string e in ArchiveExtensions)
        {
            if (name.EndsWith(e, StringComparison.OrdinalIgnoreCase))
            {
                name = name.Substring(0, name.Length - e.Length);
                break;
            }
        }
        string combined = name + newExt;
        return string.IsNullOrEmpty(dir) ? combined : Path.Combine(dir, combined);
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
