using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Dialogs;
using OtterZip.App.Models;
using OtterZip.App.Services;
using OtterZip.Interop;
using Windows.ApplicationModel.DataTransfer;

namespace OtterZip.App;

/// <summary>
/// Keka-pattern main window (2026-04-30).
///
/// The whole client area is a drop target. The visible body is the
/// <see cref="UserControls.ConfigPanel"/> (compression settings); when a
/// drag enters, the <c>DropOverlay</c> Border surfaces over the panel and
/// accepts/rejects based on payload type.
///
/// Drop dispatch:
///   - Single archive file → ExtractDialog (S5)
///   - Multiple archives  → bulk extract, each into its own folder
///   - Anything else      → compress with current ConfigPanel options (S3)
///   - Mixed (archives + plain files) → reject
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 Window doesn't implement IDisposable. _jobQueue lives for the window lifetime; the SemaphoreSlim it owns is released when the process tears down.")]
public sealed partial class MainWindow : Window
{
    private readonly ResourceLoader _strings;
    private readonly JobQueue _jobQueue;

    // ============================================================
    //  View stack — Keka-style inline panels instead of modal dialogs
    // ============================================================
    private enum AppView { Idle, Extract }
    private AppView _currentView = AppView.Idle;
    // Pending extract submission lives here until OnExtractSubmitted /
    // OnExtractDismissed flips it. Lets us keep ExtractAsync
    // synchronous-looking while the user interacts with the panel.
    private TaskCompletionSource<UserControls.ExtractPanelSubmitArgs?>? _pendingExtractTcs;

    public MainWindow()
    {
        InitializeComponent();
        _strings = new ResourceLoader();
        Title = _strings.GetString("App_WindowTitle");

        // JobQueue is the home for compress/extract work. The concurrent
        // limit follows Settings_ConcurrentJobs (1-4). Default 2 reflects
        // the common case of "drop a few archives at once and want them
        // moving in parallel" — single-core / contention-sensitive users
        // can drop it to 1 in Settings.
        int concurrency = Math.Clamp(
            SettingsService.Get<int>("Settings_ConcurrentJobs", 2), 1, 4);
        _jobQueue = new JobQueue(DispatcherQueue, concurrency);

        WireConcurrentJobsLiveUpdate();
        _jobQueue.JobSettled += OnJobSettled;
        FloatLayerHost.Attach(_jobQueue);

        if (Content is FrameworkElement root)
        {
            ThemeService.Apply(root, ThemeService.Load());
        }

        // Phase 6+ rev 6: unified Mica across both windows — see ApplyBackdrop.
        ApplyBackdrop();

        // Custom title bar: hand the OS our empty Border as the drag region.
        // The OS still draws min/max/close on the right; everything to the
        // left of those is draggable, and the rest of the window is free
        // to host content without colliding with caption buttons.
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);

        // Compact Keka-style window — rev 5 tightened further now that
        // Method/advanced options moved behind an Expander.
        TrySizeWindow(width: 420, height: 460);

        // Click on the drop-hint card → reuse the file-picker flow that
        // backs the right-click "Add files…" menu.
        ConfigPanel.DropHintTapped += async (_, _) =>
            await PickFilesAndProcessAsync().ConfigureAwait(false);
        // Phase 4: gear chip in the corner strip replaces the bottom
        // status-bar settings button. Forward to the same handler so
        // both routes open the same Settings surface.
        ConfigPanel.SettingsRequested += (s, _) =>
            OnSettingsButtonClick(s ?? this, new RoutedEventArgs());

        // Window-wide drag/drop. Wired once in ctor — the root grid never
        // changes its lifetime so detaching is unnecessary.
        RootGrid.DragEnter += OnDragEnter;
        RootGrid.DragOver += OnDragOver;
        RootGrid.DragLeave += OnDragLeave;
        RootGrid.Drop += OnDrop;

        WireExtractPanel();

        // Phase 6+ rev 4: hook AppWindow.Closing so an in-flight job can
        // prompt the user before the window vanishes. WinUI 3 Window.Closed
        // is non-cancellable; AppWindow.Closing carries `args.Cancel`.
        WireAppWindow();
    }

    /// <summary>
    /// Live-update hook for Settings_ConcurrentJobs. When the user bumps
    /// the setting in Settings UI, raise the JobQueue's limit on the
    /// spot so already-queued cards start moving without an app restart.
    /// Lowering still needs a restart — we can't yank a slot from work
    /// already running.
    /// </summary>
    private void WireConcurrentJobsLiveUpdate()
    {
        SettingsService.Changed += (_, args) =>
        {
            if (!string.Equals(args.Key, "Settings_ConcurrentJobs", StringComparison.Ordinal))
            {
                return;
            }
            int n = Math.Clamp(
                SettingsService.Get<int>("Settings_ConcurrentJobs", 2), 1, 4);
            _jobQueue.TrySetConcurrentLimit(n);
        };
    }

    /// <summary>
    /// Inline extract panel event wiring. Completes <c>_pendingExtractTcs</c>
    /// when the user submits or cancels; LayoutChanged re-fits the window
    /// after the "다른 폴더로…" toggle expands the destination row.
    /// </summary>
    private void WireExtractPanel()
    {
        ExtractPanel.Submitted += OnExtractSubmitted;
        ExtractPanel.Dismissed += OnExtractDismissed;
        ExtractPanel.PasswordEdited += (_, _) => ExtractPanel.ClearError();
        // ExtractPanel.LayoutChanged used to resize the host window
        // when the destination row toggled. The popup-card refactor
        // keeps the window fixed and lets the card grow/shrink in
        // place, so the handler is no-op now — left unsubscribed.
    }

    /// <summary>
    /// Bind to the underlying AppWindow for both the Closing-confirm hook
    /// and the otter mascot icon. Best-effort: if either step fails the
    /// window still works, just without the close prompt or custom icon.
    /// </summary>
    private void WireAppWindow()
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(windowId);
            appWindow.Closing += OnAppWindowClosing;
            TrySetWindowIcon(appWindow);
        }
        catch (Exception)
        {
            // Best-effort — closing-confirm degrades to "always allow close".
        }
    }

    /// <summary>
    /// Apply the otter mascot to the window. AppWindow.SetIcon resolves
    /// relative paths against the *working directory*, not the EXE — which
    /// burns us when VS launches the unpackaged build with a different CWD.
    /// We resolve against AppContext.BaseDirectory and pre-check existence
    /// so the call site stays predictable; the all-Exception catch keeps a
    /// future WindowsAppSDK regression from crashing the app over a purely
    /// cosmetic asset.
    /// </summary>
    internal static void TrySetWindowIcon(Microsoft.UI.Windowing.AppWindow appWindow)
    {
        try
        {
            string iconPath = System.IO.Path.Combine(
                AppContext.BaseDirectory, "Assets", "AppIcon.ico");
            if (System.IO.File.Exists(iconPath))
            {
                appWindow.SetIcon(iconPath);
            }
        }
        catch (Exception)
        {
            // Cosmetic asset — falls back to the EXE-embedded ApplicationIcon.
        }
    }

    /// <summary>
    /// Phase 6+ rev 6: unified Mica backdrop across MainWindow + SettingsWindow.
    /// Earlier rev used DesktopAcrylicBackdrop here while Settings used Mica,
    /// which produced a visible tone mismatch (main = nearly black due to
    /// wallpaper bleed-through, Settings = navy-grey from Mica's tonal palette).
    /// WinUI 3 design guidance: long-lived windows use Mica; Acrylic is reserved
    /// for transient surfaces (flyouts, command bars).
    /// Kind=Base is the standard for primary windows. SettingsWindow uses
    /// BaseAlt because it hosts a NavigationView pane — both Kinds derive from
    /// the same wallpaper palette so the perceived tone matches.
    /// Falls back automatically to a solid color on Windows builds without
    /// composition support — no try/catch needed at this layer.
    /// </summary>
    private void ApplyBackdrop()
    {
        SystemBackdrop = new Microsoft.UI.Xaml.Media.MicaBackdrop
        {
            Kind = Microsoft.UI.Composition.SystemBackdrops.MicaKind.Base,
        };
    }

    private bool _confirmedExit;

    /// <summary>
    /// True when the JobQueue has at least one running or queued card —
    /// used by OnAppWindowClosing to decide whether to prompt.
    /// </summary>
    private bool HasActiveQueueJobs()
    {
        foreach (var job in _jobQueue.Jobs)
        {
            if (job.State == JobState.Running || job.State == JobState.Queued)
            {
                return true;
            }
        }
        return false;
    }

    private void CancelAllQueueJobs()
    {
        foreach (var job in _jobQueue.Jobs)
        {
            if (job.State == JobState.Running || job.State == JobState.Queued)
            {
                job.RequestCancel();
            }
        }
    }

    /// <summary>
    /// Hook for JobQueue's JobSettled event — fires on the dispatcher
    /// thread after any terminal transition. Wired in the constructor.
    ///
    /// Successful jobs trigger three opt-in follow-ups, gated by Settings:
    ///   * Toast notification (<c>Settings_ShowToast</c>, default ON)
    ///   * Reveal-in-Explorer (<c>Settings_RevealAfter*</c>, default ON)
    ///   * Recycle source archive after extract
    ///     (<c>Settings_DeleteArchiveAfterExtract</c>, default OFF)
    /// All three live in this single hook so the queue's work delegates
    /// stay focused on the actual archive op — side effects only run
    /// once per Done transition, never on Error/Cancelled.
    /// </summary>
    private void OnJobSettled(object? sender, JobItem item)
    {
        if (item.State != JobState.Done || string.IsNullOrEmpty(item.ResultPath))
        {
            return;
        }
        TryToastCompletion(item);
        TryRevealOnCompletion(item);
        TryRecycleSourceArchive(item);
    }

    private void TryToastCompletion(JobItem item)
    {
        try
        {
            string fmt = item.Kind == JobKind.Compress
                ? _strings.GetString("Main_StatusBarCompressDoneFormat/Text")
                : _strings.GetString("Main_StatusBarDoneFormat/Text");
            string body = string.Format(CultureInfo.CurrentCulture, fmt,
                Path.GetFileName(item.ResultPath!),
                item.StatusText ?? string.Empty);
            ToastService.ShowCompletion(item.DisplayName, body);
        }
        catch
        {
            // Toasts are decoration — never let them break the run.
        }
    }

    /// <summary>
    /// Show the result in Explorer when the corresponding "reveal after"
    /// setting is on. Compress reveals the archive file itself (selected
    /// in its parent folder); extract reveals the destination folder.
    /// </summary>
    private static void TryRevealOnCompletion(JobItem item)
    {
        string key = item.Kind == JobKind.Compress
            ? "Settings_RevealAfterCompress"
            : "Settings_RevealAfterExtract";
        if (!SettingsService.Get<bool>(key, true)) return;
        try
        {
            Win32Helper.RevealInExplorer(item.ResultPath!);
        }
        catch
        {
            // Reveal is a convenience — never let it break the queue.
        }
    }

    /// <summary>
    /// Move the source archive to the Recycle Bin after a successful
    /// extract when <c>Settings_DeleteArchiveAfterExtract</c> is on. The
    /// source path is captured at submit time on <see cref="JobItem.SourcePath"/>
    /// — relying on local state in the work delegate would race the
    /// completion handler. Compress jobs have no single source archive
    /// here (their sources are handled inline via
    /// <c>MaybeRecycleSources</c>), so this branch is extract-only.
    /// </summary>
    private static void TryRecycleSourceArchive(JobItem item)
    {
        if (item.Kind != JobKind.Extract) return;
        if (string.IsNullOrEmpty(item.SourcePath)) return;
        if (!SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false)) return;
        try
        {
            Win32Helper.MoveToRecycleBin(item.SourcePath);
        }
        catch
        {
            // Recycle is best-effort; file locks / permissions stay out
            // of the user's face — the extract itself already succeeded.
        }
    }

    private async void OnAppWindowClosing(
        Microsoft.UI.Windowing.AppWindow sender,
        Microsoft.UI.Windowing.AppWindowClosingEventArgs args)
    {
        // Already confirmed once — let the second event through.
        if (_confirmedExit) { TryCloseSubWindows(); return; }
        // No active job — nothing to confirm.
        if (!HasActiveQueueJobs()) { TryCloseSubWindows(); return; }
        // User opted out — let it close.
        if (!SettingsService.Get<bool>("Settings_ConfirmExitWhileBusy", true))
        {
            TryCloseSubWindows();
            return;
        }

        // Cancel the OS close until the user confirms. We re-raise Close()
        // ourselves once they say yes; otherwise we leave the window open.
        args.Cancel = true;
        bool quit = await PromptExitWhileBusyAsync().ConfigureAwait(true);
        if (quit)
        {
            // Cancel any in-flight queue card so the OS hand-off below
            // doesn't strand work mid-flight.
            CancelAllQueueJobs();
            _confirmedExit = true;
            TryCloseSubWindows();
            try { this.Close(); }
            catch (InvalidOperationException) { /* already torn down */ }
            catch (System.Runtime.InteropServices.COMException) { /* HWND gone */ }
        }
    }

    /// <summary>
    /// Best-effort close of any auxiliary windows MainWindow owns. Called
    /// from the AppWindow.Closing handler so child windows tear down while
    /// MainWindow.Content is still alive — their Closed handlers can then
    /// safely touch our Content (theme re-apply etc.).
    /// </summary>
    private void TryCloseSubWindows()
    {
        try { _settingsWindow?.Close(); }
        catch (InvalidOperationException) { /* already closed */ }
        catch (System.Runtime.InteropServices.COMException) { /* HWND gone */ }
    }

    private async Task<bool> PromptExitWhileBusyAsync()
    {
        var dialog = new ContentDialog
        {
            Title = _strings.GetString("ExitConfirm_Title/Text"),
            Content = _strings.GetString("ExitConfirm_Body/Text"),
            PrimaryButtonText = _strings.GetString("ExitConfirm_Quit/Text"),
            CloseButtonText = _strings.GetString("ExitConfirm_Stay/Text"),
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = Content.XamlRoot,
        };
        var result = await dialog.ShowAsync();
        return result == ContentDialogResult.Primary;
    }

    private void TrySizeWindow(int width, int height)
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(windowId);
            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi / 96.0;
            int physW = (int)(width * scale);
            int physH = (int)(height * scale);
            appWindow.Resize(new Windows.Graphics.SizeInt32(physW, physH));

            // Phase 6+ rev 5: lock the window to the chosen compact size.
            // OtterZip is a launcher / config surface, not a workspace —
            // resizing or maximizing actively hurts the layout. We keep
            // the minimize button so users can stash it out of the way.
            if (appWindow.Presenter is Microsoft.UI.Windowing.OverlappedPresenter presenter)
            {
                presenter.IsResizable = false;
                presenter.IsMaximizable = false;
            }
        }
        catch (Exception)
        {
            // Best-effort — fall back to OS-default sizing.
        }
    }

    [System.Runtime.InteropServices.DllImport("user32.dll", ExactSpelling = true)]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    // ============================================================
    //  Shell extension entry (--invoke verb)
    // ============================================================
    internal void PreloadInvoke(InvokeRequest request)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            _ = DispatchInvokeAsync(request);
        });
    }

    private async Task DispatchInvokeAsync(InvokeRequest request)
    {
        try
        {
            if (string.Equals(request.Verb, "extract-here", StringComparison.OrdinalIgnoreCase)
                || string.Equals(request.Verb, "extract", StringComparison.OrdinalIgnoreCase))
            {
                if (request.Paths.Count > 0)
                {
                    await ExtractAsync(request.Paths[0]).ConfigureAwait(true);
                }
            }
            else if (string.Equals(request.Verb, "compress", StringComparison.OrdinalIgnoreCase))
            {
                await CompressAsync(request.Paths).ConfigureAwait(true);
            }
            else
            {
                string msg = string.Format(CultureInfo.CurrentCulture,
                    _strings.GetString("Main_StatusBarUnknownInvokeFormat"),
                    request.Verb);
                _jobQueue.ReportError(JobKind.Compress, "OtterZip", msg);
            }
        }
        catch (Exception ex)
        {
            _jobQueue.ReportError(JobKind.Compress, "OtterZip", ex.Message);
        }
    }

    // ============================================================
    //  Window-wide drag & drop
    // ============================================================
    private DropClassification _currentDrop = DropClassification.None;

    private async void OnDragEnter(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems))
        {
            e.AcceptedOperation = DataPackageOperation.None;
            ShowOverlayReject(_strings.GetString("DropOverlay_MixedRejectText/Text"));
            return;
        }

        // Peeking the storage items requires an async call; defer with a
        // deferral so the framework keeps the drag operation alive while
        // we classify the payload.
        var deferral = e.GetDeferral();
        try
        {
            var items = await e.DataView.GetStorageItemsAsync();
            var paths = items
                .Where(static i => i is not null)
                .Select(static i => i.Path)
                .Where(static p => !string.IsNullOrEmpty(p))
                .ToList();

            _currentDrop = ClassifyDrop(paths);
            switch (_currentDrop)
            {
                case DropClassification.CompressBatch:
                    e.AcceptedOperation = DataPackageOperation.Copy;
                    ShowOverlayAccept(FormatCompressSubtitle(paths.Count));
                    break;
                case DropClassification.ExtractSingle:
                    e.AcceptedOperation = DataPackageOperation.Copy;
                    ShowOverlayAccept(string.Format(CultureInfo.CurrentCulture,
                        _strings.GetString("DropOverlay_WillExtractFormat"),
                        Path.GetFileName(paths[0])));
                    break;
                case DropClassification.ExtractMultiple:
                    e.AcceptedOperation = DataPackageOperation.Copy;
                    ShowOverlayAccept(string.Format(CultureInfo.CurrentCulture,
                        _strings.GetString("DropOverlay_WillExtractMultipleFormat"),
                        paths.Count));
                    break;
                case DropClassification.Mixed:
                    e.AcceptedOperation = DataPackageOperation.None;
                    ShowOverlayReject(_strings.GetString("DropOverlay_MixedRejectText/Text"));
                    break;
                default:
                    e.AcceptedOperation = DataPackageOperation.None;
                    ShowOverlayReject(_strings.GetString("DropOverlay_EmptyReject/Text"));
                    break;
            }
        }
        finally
        {
            deferral.Complete();
        }
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        // Re-affirm the operation kind so Windows doesn't downgrade it on
        // long hovers. Cheap; based on the cached classification.
        e.AcceptedOperation = _currentDrop is DropClassification.CompressBatch
            or DropClassification.ExtractSingle
            or DropClassification.ExtractMultiple
            ? DataPackageOperation.Copy
            : DataPackageOperation.None;
    }

    private void OnDragLeave(object sender, DragEventArgs e)
    {
        HideOverlay();
        _currentDrop = DropClassification.None;
    }

    private async void OnDrop(object sender, DragEventArgs e)
    {
        // Two-phase drop handler. See HarvestDropAsync for the why.
        var harvest = await HarvestDropAsync(e).ConfigureAwait(true);
        if (harvest.Paths is null)
        {
            DebugLog.Info("OnDrop: harvest returned null paths");
            return;
        }
        DebugLog.Info("OnDrop: classification=" + harvest.Classification + ", pathCount=" + harvest.Paths.Count + ", forceDialog=" + harvest.ForceDialog);

        try
        {
            switch (harvest.Classification)
            {
                case DropClassification.ExtractSingle:
                    // ForceDialog flips Keka-style silent extract to the
                    // inline panel for this one drop only — set when the
                    // user held Ctrl or Alt during release.
                    await ExtractAsync(harvest.Paths[0], forceDialog: harvest.ForceDialog).ConfigureAwait(true);
                    break;
                case DropClassification.ExtractMultiple:
                    await ExtractManyAsync(harvest.Paths).ConfigureAwait(true);
                    break;
                case DropClassification.CompressBatch:
                    await CompressAsync(harvest.Paths).ConfigureAwait(true);
                    break;
                // Mixed / Empty already rejected by overlay; no-op here.
            }
        }
        catch (Exception ex)
        {
            _jobQueue.ReportError(JobKind.Compress, "OtterZip", ex.Message);
        }
    }

    private readonly record struct DropHarvest(
        IReadOnlyList<string>? Paths,
        DropClassification Classification,
        bool ForceDialog);

    /// <summary>
    /// Phase 1 of the drop pipeline: gather paths inside the OLE deferral
    /// and complete the deferral BEFORE returning. Caller (OnDrop) then
    /// runs the long-lived dialog work outside the drop event, which lets
    /// Windows release its drop animation immediately. Without this split
    /// the blue "Copy/복사" cursor with arrow stays pinned on top of the
    /// Extract dialog until the user clicks through.
    /// </summary>
    private async Task<DropHarvest> HarvestDropAsync(DragEventArgs e)
    {
        // Capture modifier state up-front: WinUI's DragDropModifiers
        // reflects keys held at the moment of the drop (NOT current
        // keyboard state). Ctrl OR Alt routes the drop through the
        // inline panel even when Settings_AskBeforeExtract is off.
        bool forceDialog = (e.Modifiers
            & (Windows.ApplicationModel.DataTransfer.DragDrop.DragDropModifiers.Control
               | Windows.ApplicationModel.DataTransfer.DragDrop.DragDropModifiers.Alt))
            != Windows.ApplicationModel.DataTransfer.DragDrop.DragDropModifiers.None;

        var deferral = e.GetDeferral();
        try
        {
            HideOverlay();
            if (!e.DataView.Contains(StandardDataFormats.StorageItems))
            {
                return default;
            }
            var items = await e.DataView.GetStorageItemsAsync();
            var paths = items
                .Where(static i => i is not null)
                .Select(static i => i.Path)
                .Where(static p => !string.IsNullOrEmpty(p))
                .ToList();
            var classification = ClassifyDrop(paths);
            _currentDrop = DropClassification.None;
            return new DropHarvest(paths, classification, forceDialog);
        }
        catch (Exception ex)
        {
            _jobQueue.ReportError(JobKind.Compress, "OtterZip", ex.Message);
            return default;
        }
        finally
        {
            deferral.Complete();
        }
    }

    // ============================================================
    //  Drop classification
    // ============================================================
    private enum DropClassification { None, CompressBatch, ExtractSingle, ExtractMultiple, Mixed }

    private static DropClassification ClassifyDrop(IReadOnlyList<string> paths)
    {
        if (paths.Count == 0) return DropClassification.None;

        int archives = 0;
        int plains = 0;
        foreach (var p in paths)
        {
            if (Directory.Exists(p))
            {
                plains++;
            }
            else if (IsKnownArchive(p))
            {
                archives++;
            }
            else
            {
                plains++;
            }
        }

        // Phase 6+ rev 4: Settings_DefaultAction overrides the auto path
        // when the user has explicitly chosen "always compress" or
        // "always extract". "Mixed" still rejects in either mode — there's
        // no sensible single-action interpretation of a mixed payload.
        string action = SettingsService.Get<string>("Settings_DefaultAction", "auto");
        if (archives > 0 && plains > 0) return DropClassification.Mixed;

        if (string.Equals(action, "compress", StringComparison.Ordinal))
        {
            // Force compress even if the payload is an archive — user knows
            // they want to nest the archive into another archive.
            return DropClassification.CompressBatch;
        }
        if (string.Equals(action, "extract", StringComparison.Ordinal))
        {
            // Force extract — only meaningful when there are archives.
            // Plain-files-only with this setting becomes a no-op (None).
            if (archives == 1) return DropClassification.ExtractSingle;
            if (archives > 1) return DropClassification.ExtractMultiple;
            return DropClassification.None;
        }

        // Auto (default).
        if (archives == 1) return DropClassification.ExtractSingle;
        if (archives > 1) return DropClassification.ExtractMultiple;
        return DropClassification.CompressBatch;
    }

    private static bool IsKnownArchive(string path)
    {
        try
        {
            var fmt = Archive.DetectFormat(path);
            return fmt switch
            {
                ArchiveFormat.Zip => true,
                ArchiveFormat.SevenZ => true,
                ArchiveFormat.Rar => true,
                ArchiveFormat.Tar => true,
                ArchiveFormat.TarGz => true,
                ArchiveFormat.TarBz2 => true,
                ArchiveFormat.TarXz => true,
                // Phase 7+ option Y -- mainstream cover extension.
                // Single-stream + tar wrappers + ZIP variants + ISO /
                // CAB / MSI / DEB. Drop-zone classification needs to
                // recognise every read-side ArchiveFormat the
                // dispatcher accepts; otherwise the user's drag of a
                // perfectly valid `.zst` / `.cab` / etc. would route
                // to the compress flow instead of extract.
                ArchiveFormat.Bzip2 => true,
                ArchiveFormat.Xz => true,
                ArchiveFormat.Lzma => true,
                ArchiveFormat.Zstd => true,
                ArchiveFormat.TarZst => true,
                ArchiveFormat.Lz4 => true,
                ArchiveFormat.TarLz4 => true,
                ArchiveFormat.Zipx => true,
                ArchiveFormat.Iso => true,
                ArchiveFormat.Cab => true,
                ArchiveFormat.Msi => true,
                ArchiveFormat.Deb => true,
                _ => false,
            };
        }
        catch
        {
            return false;
        }
    }

    private string FormatCompressSubtitle(int itemCount)
    {
        string fmt = _strings.GetString("DropOverlay_WillCompressFormat");
        return string.Format(CultureInfo.CurrentCulture, fmt, itemCount, ConfigPanel.SelectedFormat);
    }

    // ============================================================
    //  Drop overlay visibility helpers
    // ============================================================
    private void ShowOverlayAccept(string subtitle)
    {
        DropOverlay.Visibility = Visibility.Visible;
        DropOverlay.Background = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["DropOverlayBackgroundBrush"];
        DropOverlayCard.BorderBrush = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["OtterzipBrandBrush"];
        DropOverlayIcon.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["OtterzipBrandBrush"];
        DropOverlayTitle.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["OtterzipBrandBrush"];
        DropOverlaySubtitle.Text = subtitle;
    }

    private void ShowOverlayReject(string subtitle)
    {
        DropOverlay.Visibility = Visibility.Visible;
        DropOverlay.Background = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["DropOverlayRejectBackgroundBrush"];
        DropOverlayCard.BorderBrush = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["SystemFillColorCriticalBrush"];
        DropOverlayIcon.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["SystemFillColorCriticalBrush"];
        DropOverlayTitle.Foreground = (Microsoft.UI.Xaml.Media.Brush)
            Application.Current.Resources["SystemFillColorCriticalBrush"];
        DropOverlaySubtitle.Text = subtitle;
    }

    private void HideOverlay()
    {
        DropOverlay.Visibility = Visibility.Collapsed;
    }

    // ============================================================
    //  Right-click menu
    // ============================================================
    private async void OnContextAddFiles(object sender, RoutedEventArgs e)
        => await PickFilesAndProcessAsync().ConfigureAwait(false);

    private async void OnContextAddFolder(object sender, RoutedEventArgs e)
        => await PickFolderAndProcessAsync().ConfigureAwait(false);

    private async void OnContextOpenSettings(object sender, RoutedEventArgs e)
        => await ShowSettingsAsync().ConfigureAwait(false);

    private async void OnSettingsButtonClick(object sender, RoutedEventArgs e)
        => await ShowSettingsAsync().ConfigureAwait(false);

    private async Task PickFilesAndProcessAsync()
    {
        var picker = new Windows.Storage.Pickers.FileOpenPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add("*");
        picker.SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop;
        var files = await picker.PickMultipleFilesAsync();
        if (files is null || files.Count == 0) return;
        var paths = files.Select(f => f.Path).ToList();
        await ForwardPathsAsync(paths).ConfigureAwait(true);
    }

    private async Task PickFolderAndProcessAsync()
    {
        var picker = new Windows.Storage.Pickers.FolderPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add("*");
        picker.SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop;
        var folder = await picker.PickSingleFolderAsync();
        if (folder is null) return;
        await ForwardPathsAsync(new[] { folder.Path }).ConfigureAwait(true);
    }

    private async Task ForwardPathsAsync(IReadOnlyList<string> paths)
    {
        switch (ClassifyDrop(paths))
        {
            case DropClassification.ExtractSingle:
                await ExtractAsync(paths[0]).ConfigureAwait(false);
                break;
            case DropClassification.ExtractMultiple:
                await ExtractManyAsync(paths).ConfigureAwait(false);
                break;
            case DropClassification.CompressBatch:
                await CompressAsync(paths).ConfigureAwait(false);
                break;
            // Mixed / None: silently ignore — picker UI doesn't usually permit this.
        }
    }

    private SettingsWindow? _settingsWindow;

    private Task ShowSettingsAsync()
    {
        // Phase 6+ rev 2: standalone window instead of in-process dialog.
        // We track the instance so re-clicking the gear focuses the
        // existing window rather than opening a duplicate.
        if (_settingsWindow is null)
        {
            _settingsWindow = new SettingsWindow();
            _settingsWindow.Closed += (_, _) =>
            {
                // Re-apply the theme once Settings closes — the user may
                // have flipped it inside General.
                //
                // Guard: if MainWindow itself was closed first (Settings
                // left open as a child surface), the WinUI runtime tears
                // SettingsWindow down too — its Closed handler then fires
                // while `this.Content` is already invalidated, throwing
                // "The WinUI Desktop Window object has already been closed".
                // We swallow that case so app shutdown stays clean.
                try
                {
                    if (Content is FrameworkElement root)
                    {
                        ThemeService.Apply(root, ThemeService.Load());
                    }
                }
                catch (InvalidOperationException)
                {
                    // MainWindow already torn down — nothing to re-theme.
                }
                catch (System.Runtime.InteropServices.COMException)
                {
                    // Underlying HWND gone (same root cause as above on
                    // some Windows builds). Safe to ignore.
                }
                _settingsWindow = null;
            };
            _settingsWindow.Activate();
        }
        else
        {
            _settingsWindow.Activate();
        }
        return Task.CompletedTask;
    }

    // ============================================================
    //  Extract (single) — Keka-style inline panel flow
    //
    // Decision tree on each single-archive extract request:
    //   1. Probe IsEncrypted (cheap, opens central dir only).
    //   2. If !forceDialog && !Settings_AskBeforeExtract:
    //        try silent extract with the stored default password (if any).
    //        On success — done. On WrongPassword — fall through to panel.
    //   3. Otherwise: open ExtractPanel inline, loop until success/cancel.
    //
    // forceDialog is set by OnDrop when the user held Ctrl or Alt during
    // the drop — that's the one-shot "actually I want to pick the
    // destination this time" override.
    // ============================================================
    private async Task ExtractAsync(string archivePath, bool forceDialog = false)
    {
        DebugLog.Info("ExtractAsync begin: " + archivePath + " forceDialog=" + forceDialog);
        // Probe for split-archive layout before touching any archive
        // reader — the Rust core's `open()` on a partial volume returns
        // a "Could not find EOCD" / mid-stream error that confuses
        // users. We classify and either route the multi-part set
        // through a dedicated handler or surface a friendly "v1.1
        // 예정" card for spanning forms we don't read yet.
        if (TryHandleSplitArchive(archivePath))
        {
            DebugLog.Info("ExtractAsync: handled as split archive, returning");
            return;
        }

        bool isEncrypted;
        string suggestedDest;
        try
        {
            var probeStart = System.Diagnostics.Stopwatch.StartNew();
            isEncrypted = ProbeIsEncrypted(archivePath);
            DebugLog.Info("ExtractAsync: ProbeIsEncrypted took " + probeStart.ElapsedMilliseconds + "ms (encrypted=" + isEncrypted + ")");
            suggestedDest = ResolveExtractDestination(archivePath);
        }
        catch (Exception ex)
        {
            DebugLog.Info("ExtractAsync: probe failed: " + ex.Message);
            _jobQueue.ReportError(JobKind.Extract, Path.GetFileName(archivePath), ex.Message);
            return;
        }

        bool askSetting = SettingsService.Get<bool>("Settings_AskBeforeExtract", false);
        if (!forceDialog && !askSetting
            && await TrySilentExtractAsync(archivePath, suggestedDest, isEncrypted).ConfigureAwait(true))
        {
            return;
        }

        await RunExtractPanelLoopAsync(archivePath, suggestedDest, isEncrypted, forceDialog).ConfigureAwait(true);
    }

    /// <summary>
    /// Branch the extract flow based on multi-volume layout. Returns
    /// <c>true</c> when the caller should NOT continue with the normal
    /// single-volume open path.
    ///
    ///   * <see cref="SplitKind.Spanned"/> — surface a localized
    ///     "not supported" JobCard and return.
    ///   * <see cref="SplitKind.RawByteSplit"/> — enqueue a dedicated
    ///     concat-then-extract job that handles cleanup of the temp
    ///     file. The user sees a single card with combined progress.
    /// </summary>
    private bool TryHandleSplitArchive(string archivePath)
    {
        var split = SplitArchiveDetector.Probe(archivePath);
        switch (split.Kind)
        {
            case SplitKind.Spanned:
                EnqueueSpannedExtractJob(split);
                return true;
            case SplitKind.RawByteSplit:
                EnqueueRawByteSplitExtractJob(split);
                return true;
            default:
                return false;
        }
    }

    /// <summary>
    /// Attempt the silent (no-UI) extract path. Returns true when the
    /// caller should NOT fall through to the panel — either we succeeded,
    /// or we hit a fatal non-password error that the user already saw in
    /// the status bar. Returns false ONLY when the panel should take
    /// over (no stored password for an encrypted archive, or the stored
    /// password turned out to be wrong).
    /// </summary>
    private async Task<bool> TrySilentExtractAsync(string archivePath, string destination, bool isEncrypted)
    {
        DebugLog.Info("TrySilentExtractAsync: archivePath=" + archivePath + ", dest=" + destination + ", isEncrypted=" + isEncrypted);
        string? silentPw = null;
        if (isEncrypted)
        {
            silentPw = await ResolveStoredDefaultPasswordAsync(
                "Settings_DefaultPasswordOnExtract",
                _strings.GetString("Auth_ReasonExtract/Text")).ConfigureAwait(true);
            if (string.IsNullOrEmpty(silentPw))
            {
                return false; // need user input
            }
        }

        try
        {
            await PerformExtractAsync(archivePath, destination, silentPw).ConfigureAwait(true);
            return true;
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            return false; // stored password didn't work — surface panel
        }
        catch (OperationCanceledException) { return true; }
        catch (Exception ex)
        {
            // Silent path fatal — surface as a JobCard so the user actually
            // sees the failure (the queue's own error path runs only when
            // the work delegate inside PerformExtractAsync was reached;
            // some failures throw before that).
            _jobQueue.ReportError(JobKind.Extract, Path.GetFileName(archivePath), ex.Message);
            return true;
        }
    }

    /// <summary>
    /// Shows the inline ExtractPanel and loops on it until the user
    /// either submits a working extract or cancels. Wrong-password
    /// retries stay inside the same panel — no second dialog ever opens.
    ///
    /// <paramref name="forceDialog"/> doubles as <c>showDestination</c>:
    /// when the user held Ctrl/Alt during the drop they explicitly want
    /// to pick the destination, otherwise the panel starts in compact
    /// mode (password-only) and exposes an Advanced link to reveal the
    /// destination row on demand.
    /// </summary>
    private async Task RunExtractPanelLoopAsync(string archivePath, string suggestedDest, bool isEncrypted, bool forceDialog)
    {
        bool showDestination = forceDialog;
        SwitchView(AppView.Extract, extractHeight: PickExtractHeight(showDestination));
        // Pre-fill the password field with the stored default IF the
        // silent-extract path didn't already try it (i.e.
        // Settings_DefaultPasswordOnExtract is off). When the toggle is
        // on, any stored value was already attempted upstream — pre-
        // filling it again would just suggest a known-wrong value.
        // The Hello gate, if enabled, applies to actual auto-use; we
        // don't bypass it by pre-filling so we skip the convenience in
        // that case too.
        string? prefill = ResolveExtractPanelPrefill(isEncrypted);
        ExtractPanel.Configure(archivePath, suggestedDest, isEncrypted, showDestination, prefill);

        while (true)
        {
            var args = await PromptExtractPanelAsync().ConfigureAwait(true);
            if (args is null)
            {
                SwitchView(AppView.Idle);
                return;
            }

            string dest = args.ExtractHere
                ? ComputeExtractHerePath(archivePath)
                : args.Destination;

            try
            {
                SwitchView(AppView.Idle);
                await PerformExtractAsync(archivePath, dest, args.Password).ConfigureAwait(true);
                return;
            }
            catch (OtterzipException ex) when (ex.IsWrongPassword)
            {
                // Preserve the user's destination-visibility choice on
                // retry — if they expanded "Advanced" before submitting,
                // keep it expanded; otherwise stay compact.
                showDestination = showDestination || ExtractPanel.IsDestinationVisible;
                SwitchView(AppView.Extract, extractHeight: PickExtractHeight(showDestination));
                // Wrong-password retry: clear the field so the user
                // doesn't see the same wrong value they just submitted.
                ExtractPanel.Configure(archivePath, dest, needsPassword: true, showDestination: showDestination, prefillPassword: null);
                ExtractPanel.ShowError(_strings.GetString("Error_WrongPassword/Text"));
                // continue while-loop for retry
            }
            catch (OperationCanceledException) { SwitchView(AppView.Idle); return; }
            catch (Exception ex)
            {
                SwitchView(AppView.Idle);
                _jobQueue.ReportError(JobKind.Extract, Path.GetFileName(archivePath), ex.Message);
                return;
            }
        }
    }

    /// <summary>
    /// Window height for the ExtractPanel in its current footprint.
    /// The outer StackPanel centers its content vertically, so the
    /// destination row can be revealed within the same window
    /// footprint as the compact (password-only) mode — no resize
    /// needed when "다른 폴더로…" expands.
    /// </summary>
    private int PickExtractWindowHeight()
        => PickExtractHeight(ExtractPanel.IsDestinationVisible);

    private static int PickExtractHeight(bool showDestination)
        => 360;

    /// <summary>
    /// Run one extract attempt — opens the archive, drives ExtractAllAsync
    /// with progress + cancel token plumbed through. Caller decides what
    /// to do with WrongPassword exceptions (silent path bails; panel loop
    /// re-prompts in the same panel).
    ///
    /// Rolls back partial output on WrongPassword: the Rust core opens
    /// the archive's central directory before verifying credentials, so
    /// the first entry's output file gets created and starts streaming
    /// before decryption rejects the password. Result without rollback
    /// is a fresh destination folder containing a 0-byte stub file. We
    /// remove the destination ONLY when this run created it; existing
    /// directories stay untouched (might hold unrelated user data).
    /// </summary>
    private async Task PerformExtractAsync(string archivePath, string destination, string? password)
    {
        bool destExistedBefore = Directory.Exists(destination);
        bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);

        // Phase 2: the actual archive work lives inside a JobQueue card.
        // The caller (RunExtractPanelLoopAsync or silent path) still needs
        // to know whether the run succeeded or hit WrongPassword, so we
        // bridge with a TaskCompletionSource — the work delegate fulfills
        // it before re-throwing for the queue's own state machine.
        var item = new JobItem(JobKind.Extract, Path.GetFileName(archivePath))
        {
            SourcePath = archivePath, // for Settings_DeleteArchiveAfterExtract
        };
        var done = new TaskCompletionSource<ExtractReport>(TaskCreationOptions.RunContinuationsAsynchronously);

        _jobQueue.Submit(item, (ct, progress) =>
            RunInlineExtractWorkAsync(item, archivePath, destination, password,
                preserveMotw, destExistedBefore, done, ct, progress));

        await done.Task.ConfigureAwait(true);
    }

    /// <summary>
    /// Inline-extract work delegate body. Pulled out of PerformExtractAsync
    /// so the public method stays under the analyzer's 60-line cap; the
    /// flow is unchanged.
    /// </summary>
    private async Task RunInlineExtractWorkAsync(
        JobItem item, string archivePath, string destination, string? password,
        bool preserveMotw, bool destExistedBefore,
        TaskCompletionSource<ExtractReport> done,
        CancellationToken ct, IProgress<double> progress)
    {
        try
        {
            DebugLog.Info("RunInlineExtractWorkAsync: starting Archive.Open: " + archivePath);
            var openStart = System.Diagnostics.Stopwatch.StartNew();
            using var archive = string.IsNullOrEmpty(password)
                ? Archive.Open(archivePath)
                : Archive.OpenWithPassword(archivePath, password);
            DebugLog.Info("RunInlineExtractWorkAsync: Archive.Open done in " + openStart.ElapsedMilliseconds + "ms");
            var progressBridge = new Progress<ProgressUpdate>(p =>
                progress.Report(Math.Clamp(p.FractionComplete, 0.0, 1.0)));
            DebugLog.Info("RunInlineExtractWorkAsync: calling ExtractAllAsync, destination=" + destination);
            var extractStart = System.Diagnostics.Stopwatch.StartNew();
            var report = await archive
                .ExtractAllAsync(destination, OverwritePolicy.Always, progressBridge,
                    preserveZoneIdentifier: preserveMotw, cancellationToken: ct)
                .ConfigureAwait(false);
            DebugLog.Info("RunInlineExtractWorkAsync: ExtractAllAsync done in " + extractStart.ElapsedMilliseconds + "ms (entries=" + report.EntriesExtracted + ", bytes=" + report.BytesWritten + ")");
            var flattenStart = System.Diagnostics.Stopwatch.StartNew();
            TryFlattenRedundantWrapper(destination, destExistedBefore);
            DebugLog.Info("RunInlineExtractWorkAsync: TryFlattenRedundantWrapper done in " + flattenStart.ElapsedMilliseconds + "ms");

            string doneText = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Main_StatusBarDoneFormat/Text"),
                report.EntriesExtracted,
                FormatByteSize(report.BytesWritten));
            var dispatchStart = System.Diagnostics.Stopwatch.StartNew();
            await DispatchUiAndWaitAsync(() =>
            {
                item.ResultPath = destination;
                item.Progress = 1.0;     // tripwire for JobQueue's monotonic guard
                item.StatusText = doneText;
            }).ConfigureAwait(false);
            DebugLog.Info("RunInlineExtractWorkAsync: UI dispatch done in " + dispatchStart.ElapsedMilliseconds + "ms");
            done.TrySetResult(report);
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            done.TrySetException(ex);
            throw;
        }
        catch (OperationCanceledException)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            done.TrySetCanceled(ct);
            throw;
        }
        catch (Exception ex)
        {
            done.TrySetException(ex);
            throw;
        }
    }

    /// <summary>
    /// Best-effort cleanup of a destination folder that this extract run
    /// created but failed to populate. Skips the cleanup if the folder
    /// pre-existed (it might contain unrelated user data we shouldn't
    /// touch) or if the deletion itself fails (file lock, permission).
    /// </summary>
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

    /// <summary>
    /// Collapse the "wrapper folder of the same name" case after a
    /// successful extract. If we created <paramref name="destination"/>
    /// (i.e. <paramref name="destExistedBefore"/> is false) and the
    /// extract dumped everything into a single inner folder whose name
    /// matches <paramref name="destination"/>, hoist that inner folder's
    /// children up one level and remove the redundant inner folder.
    ///
    /// Example — user drops <c>TEST.zip</c> whose root is also
    /// <c>TEST/</c>:
    ///   Before: <c>parent/TEST/TEST/file1, parent/TEST/TEST/file2 …</c>
    ///   After:  <c>parent/TEST/file1, parent/TEST/file2 …</c>
    ///
    /// We deliberately stay conservative: we only flatten when the inner
    /// folder name matches the wrapper exactly. Archives whose root has
    /// a different meaningful name (e.g. <c>photo.zip</c> containing
    /// <c>Photos2024/</c>) are left alone so we don't lose information
    /// the archive author chose to encode.
    /// </summary>
    private static void TryFlattenRedundantWrapper(string destination, bool destExistedBefore)
    {
        if (destExistedBefore) return;            // user / existing folder — don't touch
        if (!Directory.Exists(destination)) return;
        try
        {
            var entries = Directory.GetFileSystemEntries(destination);
            if (entries.Length != 1) return;       // either empty or multi-root — leave as-is
            string inner = entries[0];
            if (!Directory.Exists(inner)) return;  // single file, not a folder — leave as-is
            if (!string.Equals(
                    Path.GetFileName(inner),
                    Path.GetFileName(destination),
                    StringComparison.OrdinalIgnoreCase))
            {
                return; // names differ — preserve the archive author's intent
            }

            // Two-step rename to dodge "inner shares parent's name" path
            // collisions: move inner out to a scratch directory next to
            // destination, hoist its children into destination, then
            // delete scratch. The scratch path is a sibling (not a child)
            // of destination so EnsureUniqueExtractDirectory's parent
            // ownership stays clean.
            string parent = Path.GetDirectoryName(destination) ?? destination;
            string scratch = Path.Combine(
                parent,
                ".otterzip-flatten-" + Guid.NewGuid().ToString("N"));
            Directory.Move(inner, scratch);
            foreach (var subPath in Directory.GetFileSystemEntries(scratch))
            {
                string name = Path.GetFileName(subPath);
                string target = Path.Combine(destination, name);
                if (Directory.Exists(subPath))
                {
                    Directory.Move(subPath, target);
                }
                else
                {
                    File.Move(subPath, target);
                }
            }
            Directory.Delete(scratch);
        }
        catch (IOException) { /* best effort — files already on disk */ }
        catch (UnauthorizedAccessException) { /* ditto */ }
    }

    // ============================================================
    //  Extract helpers (probe / dest resolution / view stack)
    // ============================================================

    /// <summary>
    /// Cheap up-front check — opens the archive to read its central
    /// directory and asks the core "do you need a password?". Header-
    /// encrypted formats (7z, RAR) where Open itself throws
    /// WrongPassword report as encrypted too.
    /// </summary>
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
            return false; // let downstream surface the real error
        }
    }

    /// <summary>
    /// Mirrors the destination logic ExtractManyAsync uses, kept in one
    /// helper so the silent path and the panel pre-fill always agree on
    /// where extracts land by default.
    /// </summary>
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
        // Only auto-rename the subfolder case; "extract directly into
        // baseDir" doesn't make sense to suffix.
        return useSubfolder ? EnsureUniqueExtractDirectory(dest) : dest;
    }

    /// <summary>
    /// "여기에 풀기" semantics — standard 7-Zip / WinRAR / Keka
    /// convention: drop every entry directly into the archive's parent
    /// folder, no subfolder. This is what makes the button meaningfully
    /// different from "추출" (which honours Settings_AlwaysExtractToSubfolder
    /// and Settings_ExtractLocation). No uniqueness suffix here either —
    /// the destination IS the user's chosen folder; files merging with
    /// existing siblings is the intended behaviour.
    /// </summary>
    private static string ComputeExtractHerePath(string archivePath)
    {
        return Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory();
    }

    /// <summary>
    /// Mirror of <see cref="EnsureUniqueDestination"/> for extract
    /// targets — when "Downloads/foo" already exists, return
    /// "Downloads/foo (1)" / "(2)" / … so we never silently overwrite a
    /// folder full of someone's prior extract. Only applied to the auto-
    /// computed destination paths; user-typed targets are honoured as-is.
    /// </summary>
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

    /// <summary>
    /// Toggle the body view + resize the window to fit. ConfigPanel is
    /// 460 tall; ExtractPanel stays at a single 360 height for both
    /// compact and full modes — the destination row reveals into the
    /// existing slack instead of triggering a resize.
    /// </summary>
    private void SwitchView(AppView view, int extractHeight = 360)
    {
        _ = extractHeight;   // legacy param — window no longer resizes for Extract
        _currentView = view;
        bool extract = view == AppView.Extract;
        // ConfigPanel stays visible underneath the extract overlay so
        // the canvas / cards remain perceptible behind the backdrop.
        ConfigPanel.Visibility = Visibility.Visible;
        // ExtractOverlay carries the dim backdrop + the floating card.
        ExtractOverlay.Visibility = extract ? Visibility.Visible : Visibility.Collapsed;
        // The window stays at its idle size — no shrinking, no growing.
    }

    /// <summary>
    /// Awaits the next user action on the inline ExtractPanel. Returns
    /// the submitted args, or null when the user cancelled. Caller is
    /// responsible for SwitchView before/after.
    /// </summary>
    private Task<UserControls.ExtractPanelSubmitArgs?> PromptExtractPanelAsync()
    {
        _pendingExtractTcs = new TaskCompletionSource<UserControls.ExtractPanelSubmitArgs?>();
        return _pendingExtractTcs.Task;
    }

    private void OnExtractSubmitted(object? sender, UserControls.ExtractPanelSubmitArgs e)
    {
        _pendingExtractTcs?.TrySetResult(e);
        _pendingExtractTcs = null;
    }

    private void OnExtractDismissed(object? sender, EventArgs e)
    {
        _pendingExtractTcs?.TrySetResult(null);
        _pendingExtractTcs = null;
    }

    private Task ExtractManyAsync(IReadOnlyList<string> archives)
    {
        // Bulk extract: each archive becomes its own JobQueue card so the
        // user sees per-archive progress and per-archive cancel. No batch
        // password prompt — the queue UI handles errors per-card.
        string extractLoc = SettingsService.Get<string>("Settings_ExtractLocation", "same");
        string customDir = SettingsService.Get<string>("Settings_ExtractLocationPath", "");
        bool useSubfolder = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);
        bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
        bool useCustom = string.Equals(extractLoc, "custom", StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(customDir)
            && Directory.Exists(customDir);

        foreach (var archivePath in archives)
        {
            // Split-archive detection runs per-path so a mixed drop of
            // single-volume + spanned + raw-split archives each lands
            // on the right route.
            if (TryHandleSplitArchive(archivePath))
            {
                continue;
            }
            string baseDir = useCustom
                ? customDir
                : (Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory());
            string stem = Path.GetFileNameWithoutExtension(archivePath);
            string dest = useSubfolder
                ? EnsureUniqueExtractDirectory(Path.Combine(baseDir, stem))
                : baseDir;
            EnqueueBulkExtractJob(archivePath, dest, preserveMotw);
        }
        return Task.CompletedTask;
    }

    /// <summary>
    /// Submit one archive's worth of extract work as a JobQueue card.
    /// Used by the bulk-extract path where there's no password prompt
    /// in the flow (archives that need a password surface as Error on
    /// their card, and the user can retry by single-dropping).
    /// </summary>
    private void EnqueueBulkExtractJob(string archivePath, string destination, bool preserveMotw)
    {
        var item = new JobItem(JobKind.Extract, Path.GetFileName(archivePath))
        {
            SourcePath = archivePath, // for Settings_DeleteArchiveAfterExtract
        };
        bool destExistedBefore = Directory.Exists(destination);

        _jobQueue.Submit(item, async (ct, progress) =>
        {
            try
            {
                using var archive = Archive.Open(archivePath);
                var progressBridge = new Progress<ProgressUpdate>(p =>
                    progress.Report(Math.Clamp(p.FractionComplete, 0.0, 1.0)));
                var report = await archive
                    .ExtractAllAsync(destination, OverwritePolicy.Always, progressBridge,
                        preserveZoneIdentifier: preserveMotw, cancellationToken: ct)
                    .ConfigureAwait(false);
                TryFlattenRedundantWrapper(destination, destExistedBefore);
                string doneText = string.Format(CultureInfo.CurrentCulture,
                    _strings.GetString("Main_StatusBarDoneFormat/Text"),
                    report.EntriesExtracted,
                    FormatByteSize(report.BytesWritten));
                await DispatchUiAndWaitAsync(() =>
                {
                    item.ResultPath = destination;
                    item.Progress = 1.0;
                    item.StatusText = doneText;
                }).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                RollbackPartialExtract(destination, destExistedBefore);
                throw;
            }
            catch (OtterzipException ex) when (ex.IsWrongPassword)
            {
                RollbackPartialExtract(destination, destExistedBefore);
                throw;
            }
        });
    }

    // ============================================================
    //  Raw byte-split (.zip.001..NNN / .7z.001..NNN) extract
    //
    // Tools like WinRAR's "split to volumes" without container
    // spanning emit a contiguous numbered sequence whose bytes
    // recombine into the original single-volume archive. v1.0 of
    // OtterZip cannot read these volumes natively (the Rust `zip`
    // crate operates on a single file handle), so we recombine
    // into a temp file up-front and then route the result through
    // the regular extract flow. The temp file is removed on every
    // exit path — success, cancel, or error.
    //
    // Encryption is not surfaced through an interactive password
    // panel on this route; if the archive turns out to be
    // encrypted, the silent-extract path tries Settings_
    // DefaultPasswordOnExtract (when ON), otherwise the job lands
    // on the Error card with a wrong-password message. Interactive
    // password prompts for raw byte-split sets are deferred to v1.1
    // — by then the native multi-volume reader should subsume this
    // helper entirely.
    // ============================================================
    // ============================================================
    //  Spanned ZIP / 7z extract (.z01..zN + .zip / .7z.001..NNN)
    //
    // v1.0 (ABI v8): supported for ZIP via Archive.OpenMulti — the
    // native side stitches all volumes into a virtual byte stream so
    // the standard extract pipeline can walk them as one archive. 7z
    // container spanning still falls through with UnsupportedFormat,
    // which we catch here and surface as the localized "v1.1 예정"
    // card.
    // ============================================================
    private void EnqueueSpannedExtractJob(SplitArchiveDetector.DetectionResult split)
    {
        var item = new JobItem(JobKind.Extract, Path.GetFileName(split.EntryPath))
        {
            SourcePath = split.EntryPath,
        };
        string destination = ResolveExtractDestination(split.EntryPath);
        bool destExistedBefore = Directory.Exists(destination);
        bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);

        _jobQueue.Submit(item, (ct, progress) =>
            RunSpannedExtractWorkAsync(
                item, split, destination, destExistedBefore, preserveMotw, ct, progress));
    }

    /// <summary>
    /// Work delegate for the spanned (container-aware) extract job.
    /// Opens every volume via <see cref="Archive.OpenMulti"/> and runs
    /// the standard extract pipeline. If the native side reports
    /// UnsupportedFormat (e.g. 7z spanning — not yet implemented), the
    /// job surfaces a localized "v1.1 예정" message instead of leaking
    /// the raw native error.
    /// </summary>
    private async Task RunSpannedExtractWorkAsync(
        JobItem item, SplitArchiveDetector.DetectionResult split,
        string destination, bool destExistedBefore, bool preserveMotw,
        CancellationToken ct, IProgress<double> progress)
    {
        try
        {
            using var archive = Archive.OpenMulti(split.Volumes);
            var progressBridge = new Progress<ProgressUpdate>(p =>
                progress.Report(Math.Clamp(p.FractionComplete, 0.0, 1.0)));
            var report = await archive
                .ExtractAllAsync(destination, OverwritePolicy.Always, progressBridge,
                    preserveZoneIdentifier: preserveMotw, cancellationToken: ct)
                .ConfigureAwait(false);
            TryFlattenRedundantWrapper(destination, destExistedBefore);

            string doneText = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Main_StatusBarDoneFormat/Text"),
                report.EntriesExtracted,
                FormatByteSize(report.BytesWritten));
            await DispatchUiAndWaitAsync(() =>
            {
                item.ResultPath = destination;
                item.Progress = 1.0;
                item.StatusText = doneText;
            }).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
        catch (OtterzipException ex)
            when (ex.Message.Contains("UnsupportedFormat", StringComparison.OrdinalIgnoreCase)
               || ex.Message.Contains("not supported", StringComparison.OrdinalIgnoreCase))
        {
            // 7z spanning lands here. Convert to the user-facing
            // "v1.1 예정" card so the message is intelligible regardless
            // of locale or upstream wording.
            RollbackPartialExtract(destination, destExistedBefore);
            string msg = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Split_NotSupportedFormat/Text"),
                split.Volumes.Count);
            throw new OtterzipException(-1, msg);
        }
    }

    private void EnqueueRawByteSplitExtractJob(SplitArchiveDetector.DetectionResult split)
    {
        var item = new JobItem(JobKind.Extract, Path.GetFileName(split.EntryPath))
        {
            // Reveal-on-completion shows the file the user actually
            // dropped, not the temp recombined archive.
            SourcePath = split.EntryPath,
        };

        string tempPath = Path.Combine(
            Path.GetTempPath(),
            "otterzip-join-" + Guid.NewGuid().ToString("N") + Path.GetExtension(split.Stem));
        string destination = ResolveExtractDestination(split.EntryPath);
        bool destExistedBefore = Directory.Exists(destination);
        bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);

        _jobQueue.Submit(item, (ct, progress) =>
            RunRawByteSplitExtractWorkAsync(
                item, split, tempPath, destination, destExistedBefore, preserveMotw, ct, progress));
    }

    /// <summary>
    /// Work delegate for the raw byte-split job. Two phases share one
    /// progress bar: concat-to-temp (40%) then real extract (60%). The
    /// temp archive is always removed in <c>finally</c> so a cancel or
    /// crash never leaves a multi-GB stub in %TEMP%.
    /// </summary>
    private async Task RunRawByteSplitExtractWorkAsync(
        JobItem item, SplitArchiveDetector.DetectionResult split,
        string tempPath, string destination, bool destExistedBefore, bool preserveMotw,
        CancellationToken ct, IProgress<double> progress)
    {
        string joiningFormat = _strings.GetString("Split_StatusJoining/Text");
        try
        {
            await ConcatVolumesAsync(
                split.Volumes, tempPath, item, joiningFormat,
                fraction => progress.Report(fraction * 0.4), ct).ConfigureAwait(false);

            using var archive = Archive.Open(tempPath);
            var progressBridge = new Progress<ProgressUpdate>(p =>
                progress.Report(0.4 + Math.Clamp(p.FractionComplete, 0.0, 1.0) * 0.6));
            var report = await archive
                .ExtractAllAsync(destination, OverwritePolicy.Always, progressBridge,
                    preserveZoneIdentifier: preserveMotw, cancellationToken: ct)
                .ConfigureAwait(false);
            TryFlattenRedundantWrapper(destination, destExistedBefore);

            string doneText = string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Main_StatusBarDoneFormat/Text"),
                report.EntriesExtracted,
                FormatByteSize(report.BytesWritten));
            await DispatchUiAndWaitAsync(() =>
            {
                item.ResultPath = destination;
                item.Progress = 1.0;
                item.StatusText = doneText;
            }).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
        finally
        {
            TryDeleteTempJoinedArchive(tempPath);
        }
    }

    /// <summary>
    /// Streams every volume in <paramref name="volumes"/> into a single
    /// temp file at <paramref name="destPath"/>. The status caption is
    /// updated as each volume rolls over; the progress fraction is
    /// reported continuously so the JobCard's bar moves smoothly across
    /// volume boundaries.
    /// </summary>
    private async Task ConcatVolumesAsync(
        IReadOnlyList<string> volumes, string destPath, JobItem item,
        string joiningFormat, Action<double> reportFraction, CancellationToken ct)
    {
        long total = 0;
        for (int i = 0; i < volumes.Count; i++)
        {
            total += new FileInfo(volumes[i]).Length;
        }
        if (total <= 0) total = 1; // avoid /0 on empty edge case

        long bytesDone = 0;
        const int bufferSize = 1 << 20; // 1 MiB — matches the Rust progress tick.
        byte[] buffer = new byte[bufferSize];

        using (var outStream = new FileStream(destPath, FileMode.CreateNew, FileAccess.Write,
            FileShare.None, bufferSize, FileOptions.SequentialScan))
        {
            for (int i = 0; i < volumes.Count; i++)
            {
                ct.ThrowIfCancellationRequested();
                string caption = string.Format(CultureInfo.CurrentCulture,
                    joiningFormat, i + 1, volumes.Count);
                // Phase label only — JobQueue's progress reporter
                // glues the percent suffix on. Writing to StatusText
                // directly here used to race with that reporter and
                // produced a 30 Hz "1/3 병합 중" ↔ "42%" flicker.
                DispatcherQueue.TryEnqueue(() => item.StatusLabel = caption);

                using var inStream = new FileStream(volumes[i], FileMode.Open, FileAccess.Read,
                    FileShare.Read, bufferSize, FileOptions.SequentialScan);
                int read;
                while ((read = await inStream.ReadAsync(buffer.AsMemory(0, bufferSize), ct).ConfigureAwait(false)) > 0)
                {
                    await outStream.WriteAsync(buffer.AsMemory(0, read), ct).ConfigureAwait(false);
                    bytesDone += read;
                    reportFraction((double)bytesDone / total);
                }
            }
            await outStream.FlushAsync(ct).ConfigureAwait(false);
        }
    }

    private static void TryDeleteTempJoinedArchive(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch (IOException) { /* file lock — leave for the OS temp cleanup */ }
        catch (UnauthorizedAccessException) { /* ditto */ }
    }

    // ExtractWithPasswordRetryAsync was retired with the inline-panel
    // refactor — PerformExtractAsync + RunExtractPanelLoopAsync subsume
    // its responsibilities (run extract / catch WrongPassword / re-prompt
    // in the same panel rather than a second modal).
    //
    // PromptPasswordAsync stays — Compress flow still uses it for the
    // Settings_AlwaysPromptPassword case. A follow-up sprint can move
    // that to an inline panel too.

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

    // ============================================================
    //  Compress — Phase 1: routed through JobQueue
    //
    // Each request resolves its password / plan up front, then drops one
    // or more JobItem entries on the queue. The queue owns concurrency,
    // cancellation, and progress; this method returns as soon as the
    // submissions are recorded so the caller (drag/drop handler) is
    // never blocked by long-running work.
    //
    // When "따로따로 압축하기" (Settings_CompressSeparately) is on with
    // N>1 sources, each source becomes its own card in the queue. This
    // also gives the user per-archive cancel / open-folder UX.
    // ============================================================
    private async Task CompressAsync(IReadOnlyList<string> sources)
    {
        string? compressPassword = await ResolveCompressPasswordAsync().ConfigureAwait(true);
        if (compressPassword is null && SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false))
        {
            // Always-prompt is on but user backed out — abort the whole job.
            return;
        }

        bool separately = SettingsService.Get<bool>("Settings_CompressSeparately", false);
        if (separately && sources.Count > 1)
        {
            foreach (var src in sources)
            {
                EnqueueCompressJob(new[] { src }, compressPassword);
            }
        }
        else
        {
            EnqueueCompressJob(sources, compressPassword);
        }
    }

    /// <summary>
    /// Build the compress plan + create a JobItem + submit it to the
    /// queue. The work delegate captures everything the actual
    /// compress run needs; the queue handles state transitions and
    /// surfaces them to the floating card.
    /// </summary>
    private void EnqueueCompressJob(IReadOnlyList<string> sources, string? password)
    {
        var plan = PlanCompress(sources);
        var item = new JobItem(JobKind.Compress, Path.GetFileName(plan.Destination));
        _jobQueue.Submit(item, (ct, progress) =>
            RunCompressWorkAsync(item, plan, sources, password, ct, progress));
    }

    /// <summary>
    /// Compress work delegate body. Extracted from EnqueueCompressJob so
    /// the public method stays under the analyzer's 60-line cap.
    /// </summary>
    private async Task RunCompressWorkAsync(
        JobItem item, CompressPlan plan, IReadOnlyList<string> sources,
        string? password, CancellationToken ct, IProgress<double> progress)
    {
        // ABI v7: the native side reports byte/entry counts per file.
        // Convert that to a 0..1 fraction for the JobCard's progress
        // bar and swap the "Starting…" caption for "Compressing…" once
        // the first Writing tick lands.
        string compressingText = _strings.GetString("Job_StatusCompressing/Text");
        var richProgress = new Progress<ProgressUpdate>(p =>
        {
            double frac = p.FractionComplete;
            if (frac > 0)
            {
                progress.Report(Math.Clamp(frac, 0.0, 1.0));
            }
            if (p.Phase == ProgressPhase.Writing)
            {
                // Phase label only — JobQueue's progress reporter
                // glues the percent suffix on. This used to write
                // StatusText directly and produced a 30 Hz
                // "압축 중…" ↔ "42%" flicker against the reporter's
                // own StatusText writes.
                DispatcherQueue.TryEnqueue(() => item.StatusLabel = compressingText);
            }
        });

        // Cancel feedback: caption flips to "취소 중…" the moment the
        // user clicks X. Native side observes the CT on the next entry
        // boundary and throws OperationCanceledException; the catch
        // below cleans up the partial archive.
        string cancellingText = _strings.GetString("Job_StatusCancelling/Text");
        using var cancellingReg = ct.Register(() =>
            // Phase label only. After this fires no further percent
            // ticks arrive (the work delegate races against cancel),
            // so the user sees the last "압축 중… 42%" composed line
            // for a moment until MarkCancelled flips StatusText to
            // the final "취소됨" terminal message.
            DispatcherQueue.TryEnqueue(() => item.StatusLabel = cancellingText));

        try
        {
            var report = await RunCompressAsync(plan, sources, password, richProgress, ct).ConfigureAwait(false);
            await MaybeVerifyAsync(plan.Destination, ct).ConfigureAwait(false);
            MaybeRecycleSources(sources);
            await DispatchUiAndWaitAsync(() =>
            {
                item.ResultPath = plan.Destination;
                item.Progress = 1.0;
                item.StatusText = FormatByteSize(report.BytesWritten);
            }).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            // Native side aborts at the next entry boundary; whatever's
            // been written so far is incomplete, so delete the file.
            TryDeletePartialArchive(plan.Destination);
            throw;
        }
    }

    /// <summary>
    /// Best-effort cleanup for an archive that was completed natively but
    /// the user had already cancelled. Silently swallows IO errors — the
    /// cancellation path runs as a finalizer and shouldn't crash on file
    /// locks or permission issues.
    /// </summary>
    private static void TryDeletePartialArchive(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    /// <summary>
    /// Marshal <paramref name="action"/> onto the window's DispatcherQueue
    /// and await its completion. Used by JobQueue work delegates to apply
    /// final-state updates before the queue transitions the job to Done.
    /// </summary>
    private Task DispatchUiAndWaitAsync(Action action)
    {
        var tcs = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        bool ok = DispatcherQueue.TryEnqueue(() =>
        {
            try { action(); }
            finally { tcs.TrySetResult(); }
        });
        if (!ok)
        {
            // Dispatcher refused (shutting down?) — fall back to inline so
            // the work delegate doesn't hang the queue's RunAsync.
            try { action(); }
            finally { tcs.TrySetResult(); }
        }
        return tcs.Task;
    }

    /// <summary>
    /// Decide which password (if any) the upcoming compress job should use.
    /// Precedence: ConfigPanel input → DefaultPassword setting → optional
    /// always-prompt dialog → none. Returns null when no password applies
    /// or when the user cancelled the always-prompt.
    /// </summary>
    private async Task<string?> ResolveCompressPasswordAsync()
    {
        // 1. The main window's ConfigPanel password takes precedence —
        //    it represents what the user just typed on the surface they
        //    drove the compress from.
        string fromPanel = ConfigPanel.Password ?? string.Empty;
        if (!string.IsNullOrEmpty(fromPanel))
        {
            return fromPanel;
        }

        // 2. Settings: stored default applied automatically (via vault
        //    + optional Hello gate).
        string? stored = await ResolveStoredDefaultPasswordAsync(
            "Settings_DefaultPasswordOnCompress",
            _strings.GetString("Auth_ReasonCompress/Text")).ConfigureAwait(true);
        if (!string.IsNullOrEmpty(stored))
        {
            return stored;
        }

        // 3. Settings: prompt every time. Returns null if user cancels —
        //    caller distinguishes cancel from "no password" by checking
        //    the Settings_AlwaysPromptPassword toggle.
        if (SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false))
        {
            return await PromptPasswordAsync(_strings.GetString("PasswordDialog_PlaceholderArchiveLabel/Text"))
                .ConfigureAwait(true);
        }

        return null;
    }

    /// <summary>
    /// PR-7C: shared "fetch stored default password if the per-flow
    /// auto-apply toggle is on" helper. Reads the credential out of
    /// <see cref="CredentialStore"/>; when
    /// <c>Settings_AuthBeforeUseDefaultPassword</c> is on, gates the
    /// release of that credential behind a Windows Hello prompt. Returns
    /// null when the toggle is off, no credential exists, or the user
    /// fails the Hello check.
    /// </summary>
    /// <summary>
    /// Pre-fill source for the inline ExtractPanel password field.
    /// Returns the stored default password ONLY when the silent-extract
    /// path didn't already try it — otherwise the panel would suggest a
    /// known-wrong value. Honours the Hello-gate by leaving the field
    /// empty when the user opted to require auth before each use.
    /// </summary>
    private static string? ResolveExtractPanelPrefill(bool isEncrypted)
    {
        if (!isEncrypted) return null;
        // If auto-try is on the silent path already consumed (and
        // rejected) the stored default — pre-filling is pointless.
        if (SettingsService.Get<bool>("Settings_DefaultPasswordOnExtract", false))
        {
            return null;
        }
        // Hello gate: protect cleartext display by skipping pre-fill.
        if (SettingsService.Get<bool>("Settings_AuthBeforeUseDefaultPassword", false))
        {
            return null;
        }
        string stored = CredentialStore.Get();
        return string.IsNullOrEmpty(stored) ? null : stored;
    }

    private static async Task<string?> ResolveStoredDefaultPasswordAsync(string toggleKey, string authReason)
    {
        if (!SettingsService.Get<bool>(toggleKey, false))
        {
            return null;
        }
        string stored = CredentialStore.Get();
        if (string.IsNullOrEmpty(stored))
        {
            return null;
        }
        if (SettingsService.Get<bool>("Settings_AuthBeforeUseDefaultPassword", false))
        {
            bool ok = await HelloService.RequestVerificationAsync(authReason).ConfigureAwait(true);
            if (!ok)
            {
                return null;
            }
        }
        return stored;
    }

    /// <summary>
    /// Honor the <c>Settings_DeleteSourceAfterCompress</c> toggle: every
    /// source path that survived the compress moves to the Recycle Bin.
    /// Runs only after a successful compress + verify (caller-gated) so a
    /// failure path can't inadvertently destroy the user's input.
    /// </summary>
    private static void MaybeRecycleSources(IReadOnlyList<string> sources)
    {
        if (!SettingsService.Get<bool>("Settings_DeleteSourceAfterCompress", false))
        {
            return;
        }
        foreach (var src in sources)
        {
            // Best-effort — a failure here is logged elsewhere and never
            // aborts the user-visible "compress done" message.
            _ = Win32Helper.MoveToRecycleBin(src);
        }
    }

    /// <summary>
    /// Honor the <c>Settings_VerifyAfterCompress</c> toggle: re-open the
    /// just-written archive and CRC32-verify every entry. Throws when any
    /// entry is corrupted — the caller's outer try/catch surfaces the
    /// failure as a normal error toast.
    /// </summary>
    private static async Task MaybeVerifyAsync(string archivePath, CancellationToken ct)
    {
        if (!SettingsService.Get<bool>("Settings_VerifyAfterCompress", false))
        {
            return;
        }
        // NOTE: this runs inside JobQueue.Submit's work delegate, which
        // executes on a thread-pool task. Touching StatusText (or any
        // XAML element) from here would raise RPC_E_WRONG_THREAD and the
        // queue would mark the job as Error even though the archive was
        // written successfully. The verifying-state caption is shown on
        // the JobCard via StatusText updates from the queue itself; this
        // helper just gates on the setting and runs the actual CRC scan.
        using var archive = Archive.Open(archivePath);
        var report = await archive.TestAsync(ct).ConfigureAwait(false);
        if (!report.IsHealthy)
        {
            throw new InvalidDataException(
                $"Verification failed: {report.EntriesCorrupted}/{report.EntriesTested} entries corrupted");
        }
    }

    private CompressPlan PlanCompress(IReadOnlyList<string> sources)
    {
        string firstSource = sources[0];
        string parentDir = Path.GetDirectoryName(firstSource) ?? Directory.GetCurrentDirectory();

        // Phase 6+ rev 3: Settings_UseParentFolderName decides the stem for
        // multi-file compress. Default ON — matches Keka's "use parent folder
        // name when compressing multiple files". OFF falls back to first
        // source name. For folder sources we use the full folder name —
        // GetFileNameWithoutExtension would treat a dotted folder like
        // "Maru.App_1.0.1.0_x64_Test" as having a ".0_x64_Test" extension
        // and strip it, leaving a wrong stem.
        bool useParent = SettingsService.Get<bool>("Settings_UseParentFolderName", true);
        string stem = sources.Count == 1
            ? SourceStem(firstSource)
            : useParent
                ? Path.GetFileName(parentDir)
                : SourceStem(firstSource);
        if (string.IsNullOrWhiteSpace(stem))
        {
            stem = "archive";
        }

        // PR-7B: filename template overrides the default stem when set.
        // Tokens: {name}/{date}/{time}/{count}/{parent}. Empty template
        // means "use the rule above unchanged".
        string template = SettingsService.Get<string>("Settings_FilenameTemplate", "");
        if (!string.IsNullOrWhiteSpace(template))
        {
            stem = ApplyFilenameTemplate(template, stem, parentDir, sources.Count);
        }

        // Settings_SaveLocation: "same" (sibling of source) or "custom"
        // (configured folder). Custom path empty → graceful fall back.
        string saveLoc = SettingsService.Get<string>("Settings_SaveLocation", "same");
        if (string.Equals(saveLoc, "custom", StringComparison.Ordinal))
        {
            string customDir = SettingsService.Get<string>("Settings_SaveLocationPath", "");
            if (!string.IsNullOrWhiteSpace(customDir) && Directory.Exists(customDir))
            {
                parentDir = customDir;
            }
        }

        // Phase 6+ rev 5: method index lives in Settings (Compression
        // tab) since the slider was moved off the main panel.
        int methodIndex = Math.Clamp(
            SettingsService.Get<int>("Settings_DefaultMethodIndex", 2), 0, 3);
        var (fmt, method, ext) = MapFormatAndMethod(ConfigPanel.SelectedFormat, methodIndex);
        byte level = MapMethodIndexToLevel(methodIndex);

        string destination = EnsureUniqueDestination(Path.Combine(parentDir, $"{stem}{ext}"));
        return new CompressPlan(
            Destination: destination,
            Format: fmt,
            Method: method,
            Level: level);
    }

    /// <summary>
    /// Avoid silently overwriting an existing archive. Mirrors Windows
    /// Explorer's "Copy" behaviour: if "foo.zip" exists, return
    /// "foo (1).zip"; if that exists too, "foo (2).zip"; and so on.
    /// Handles dotted extensions like ".tar.gz" / ".tar.bz2" / ".tar.xz"
    /// as a unit so the suffix lands between the stem and the whole
    /// extension, not between ".tar" and ".gz".
    ///
    /// Race note: ConcurrentLimit=1 makes the File.Exists / write pair
    /// sequential per process. With a higher limit two parallel jobs
    /// could pick the same unused index — fine for now, revisit if the
    /// concurrency option is raised by default.
    /// </summary>
    private static string EnsureUniqueDestination(string path)
    {
        if (!File.Exists(path)) return path;
        string dir = Path.GetDirectoryName(path) ?? Directory.GetCurrentDirectory();
        string nameOnly = Path.GetFileNameWithoutExtension(path);
        string ext = Path.GetExtension(path);
        // Stitch back compound .tar.* extensions so the numeric suffix
        // doesn't split them.
        if (nameOnly.EndsWith(".tar", StringComparison.OrdinalIgnoreCase)
            && (string.Equals(ext, ".gz", StringComparison.OrdinalIgnoreCase)
                || string.Equals(ext, ".bz2", StringComparison.OrdinalIgnoreCase)
                || string.Equals(ext, ".xz", StringComparison.OrdinalIgnoreCase)))
        {
            nameOnly = Path.GetFileNameWithoutExtension(nameOnly);
            ext = ".tar" + ext;
        }
        for (int i = 1; i < 10000; i++)
        {
            string candidate = Path.Combine(dir, string.Format(
                CultureInfo.InvariantCulture, "{0} ({1}){2}", nameOnly, i, ext));
            if (!File.Exists(candidate)) return candidate;
        }
        // Pathological fallback — millions of duplicates. Stamp with a
        // timestamp rather than blowing the loop.
        return Path.Combine(dir, string.Format(CultureInfo.InvariantCulture,
            "{0} ({1:yyyyMMddHHmmss}){2}", nameOnly, DateTime.Now, ext));
    }

    /// <summary>
    /// Maps ConfigPanel.SelectedFormat + 4-step MethodIndex onto the
    /// (ArchiveFormat, CompressionMethod, file extension) triple the
    /// native ArchiveBuilder takes.
    ///
    /// MethodIndex 0 (Store) selects the Stored / Copy method regardless
    /// of format; otherwise the format's natural compression method is
    /// used and Level encodes Fast / Normal / Best.
    /// </summary>
    private static (ArchiveFormat fmt, CompressionMethod method, string ext) MapFormatAndMethod(
        string formatTag, int methodIndex)
    {
        bool store = methodIndex == 0;
        return formatTag switch
        {
            "7z" => (ArchiveFormat.SevenZ,
                     store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                     ".7z"),
            "tar" => (ArchiveFormat.Tar, CompressionMethod.Store, ".tar"),
            "tar.gz" => (ArchiveFormat.TarGz,
                         store ? CompressionMethod.Store : CompressionMethod.Deflate,
                         ".tar.gz"),
            "tar.bz2" => (ArchiveFormat.TarBz2,
                          store ? CompressionMethod.Store : CompressionMethod.Bzip2,
                          ".tar.bz2"),
            "tar.xz" => (ArchiveFormat.TarXz,
                         store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                         ".tar.xz"),
            // PR-F7 -- single-stream .xz writer (Phase 7+ option Y).
            // Method choice is informational here; the XZ writer
            // always uses LZMA2 internally.
            "xz" => (ArchiveFormat.Xz,
                     store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                     ".xz"),
            // PR-F7 -- ZIPX writer with Bzip2 method (the de-facto
            // baseline; LZMA write is not supported by zip 2.x).
            "zipx" => (ArchiveFormat.Zipx,
                       store ? CompressionMethod.Store : CompressionMethod.Bzip2,
                       ".zipx"),
            _ => (ArchiveFormat.Zip,
                  store ? CompressionMethod.Store : CompressionMethod.Deflate,
                  ".zip"),
        };
    }

    /// <summary>4-step UI → 1..9 backend level. Store(0) is irrelevant when method is Stored.</summary>
    private static byte MapMethodIndexToLevel(int methodIndex) => methodIndex switch
    {
        0 => 1,
        1 => 1,
        2 => 5,
        3 => 9,
        _ => 5,
    };

    private static Task<ArchiveBuildReport> RunCompressAsync(
        CompressPlan plan,
        IReadOnlyList<string> sources,
        string? password,
        IProgress<ProgressUpdate>? progress,
        CancellationToken ct)
    {
        // Settings toggles read once at the start of a compress job, so
        // changing them mid-flight doesn't leak into an in-progress task.
        bool excludeMeta = SettingsService.Get<bool>("Settings_ExcludeSystemMetadata", true);

        if (sources.Count == 1 && Directory.Exists(sources[0]))
        {
            // ABI v7 path: native reports bytes_processed / entries_processed
            // per file and honours mid-write cancellation.
            return ArchiveBuilder.CreateFromDirectoryAsync(
                plan.Destination, sources[0], plan.Format, plan.Method,
                plan.Level, progress: progress,
                excludeSystemMetadata: excludeMeta,
                password: password,
                cancellationToken: ct);
        }
        // CompressMixedSources doesn't currently expose a progress hook
        // through the per-entry ArchiveBuilder API — the card stays
        // indeterminate for the mixed-file path until that's plumbed.
        return Task.Run(() => CompressMixedSources(plan, sources, excludeMeta, password, ct), ct);
    }

    private static ArchiveBuildReport CompressMixedSources(
        CompressPlan plan,
        IReadOnlyList<string> sources,
        bool excludeSystemMetadata,
        string? password,
        CancellationToken ct)
    {
        using var builder = ArchiveBuilder.Create(
            plan.Destination, plan.Format, plan.Method, plan.Level,
            solid: false,
            excludeSystemMetadata: excludeSystemMetadata,
            password: password);
        foreach (string src in sources)
        {
            ct.ThrowIfCancellationRequested();
            if (Directory.Exists(src))
            {
                builder.AddDirectory(src, Path.GetFileName(src));
            }
            else
            {
                builder.AddFile(src, Path.GetFileName(src));
            }
        }
        builder.Commit();
        ulong size = 0;
        try { size = (ulong)new FileInfo(plan.Destination).Length; }
        catch (IOException) { }
        return new ArchiveBuildReport { BytesWritten = size };
    }

    private sealed record CompressPlan(string Destination, ArchiveFormat Format, CompressionMethod Method, byte Level);

    /// <summary>
    /// Pick the archive name stem from a single source path. Directories
    /// keep their full name (dots are part of the folder name, not an
    /// extension); files strip the extension as usual.
    /// </summary>
    private static string SourceStem(string source)
    {
        if (Directory.Exists(source))
        {
            string trimmed = source.TrimEnd(
                Path.DirectorySeparatorChar,
                Path.AltDirectorySeparatorChar);
            return Path.GetFileName(trimmed);
        }
        return Path.GetFileNameWithoutExtension(source);
    }

    /// <summary>
    /// PR-7B: substitute filename template tokens. Sanitises the result
    /// against Windows-illegal characters so the user can't accidentally
    /// produce a path that File.Create rejects.
    /// </summary>
    private static string ApplyFilenameTemplate(string template, string name, string parentDir, int count)
    {
        var now = DateTime.Now;
        string parent = string.IsNullOrEmpty(parentDir) ? "" : Path.GetFileName(parentDir);
        string result = template
            .Replace("{name}", name, StringComparison.Ordinal)
            .Replace("{date}", now.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{time}", now.ToString("HHmm", CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{count}", count.ToString(CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{parent}", parent, StringComparison.Ordinal);

        // Strip Windows-illegal filename chars after substitution so a
        // {parent} containing a colon (impossible normally) or a manually
        // typed `*` doesn't break Path.Combine.
        foreach (char invalid in Path.GetInvalidFileNameChars())
        {
            result = result.Replace(invalid.ToString(), "", StringComparison.Ordinal);
        }
        return string.IsNullOrWhiteSpace(result) ? name : result;
    }

    // ============================================================
    //  Formatting helpers
    // ============================================================
    private static string FormatByteSize(ulong bytes)
    {
        const ulong KB = 1024;
        const ulong MB = KB * 1024;
        const ulong GB = MB * 1024;
        if (bytes >= GB)
        {
            return string.Format(CultureInfo.CurrentCulture, "{0:0.##} GB", bytes / (double)GB);
        }
        if (bytes >= MB)
        {
            return string.Format(CultureInfo.CurrentCulture, "{0:0.##} MB", bytes / (double)MB);
        }
        if (bytes >= KB)
        {
            return string.Format(CultureInfo.CurrentCulture, "{0:0.##} KB", bytes / (double)KB);
        }
        return string.Format(CultureInfo.CurrentCulture, "{0} B", bytes);
    }
}
