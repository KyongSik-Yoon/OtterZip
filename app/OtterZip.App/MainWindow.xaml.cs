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
public sealed partial class MainWindow : Window
{
    private readonly ResourceLoader _strings;
    private CancellationTokenSource? _activeCts;

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

        // Window-wide drag/drop. Wired once in ctor — the root grid never
        // changes its lifetime so detaching is unnecessary.
        RootGrid.DragEnter += OnDragEnter;
        RootGrid.DragOver += OnDragOver;
        RootGrid.DragLeave += OnDragLeave;
        RootGrid.Drop += OnDrop;

        // Inline extract panel — completes _pendingExtractTcs when the
        // user submits or cancels. Wiring lives next to the other root-
        // level events so future view-stack additions follow the pattern.
        ExtractPanel.Submitted += OnExtractSubmitted;
        ExtractPanel.Dismissed += OnExtractDismissed;
        ExtractPanel.PasswordEdited += (_, _) => ExtractPanel.ClearError();
        // Re-fit the window when the Advanced toggle expands the
        // destination row — keeps the panel from clipping or floating
        // in dead space.
        ExtractPanel.LayoutChanged += (_, _) =>
        {
            if (_currentView == AppView.Extract)
            {
                TrySizeWindow(width: 420, height: PickExtractWindowHeight());
            }
        };

        // Phase 6+ rev 4: hook AppWindow.Closing so an in-flight job can
        // prompt the user before the window vanishes. WinUI 3 Window.Closed
        // is non-cancellable; AppWindow.Closing carries `args.Cancel`.
        WireAppWindow();
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

    private async void OnAppWindowClosing(
        Microsoft.UI.Windowing.AppWindow sender,
        Microsoft.UI.Windowing.AppWindowClosingEventArgs args)
    {
        // Already confirmed once — let the second event through.
        if (_confirmedExit) { TryCloseSubWindows(); return; }
        // No active job — nothing to confirm.
        if (_activeCts is null) { TryCloseSubWindows(); return; }
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
            // Cancel the in-flight job first, then allow the next close
            // attempt to pass through unobstructed.
            try { _activeCts?.Cancel(); }
            catch (ObjectDisposedException) { }
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
                ShowError(string.Format(CultureInfo.CurrentCulture,
                    _strings.GetString("Main_StatusBarUnknownInvokeFormat"),
                    request.Verb));
            }
        }
        catch (Exception ex)
        {
            ShowError(ex.Message);
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
        if (harvest.Paths is null) return;

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
            ShowError(ex.Message);
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
            ShowError(ex.Message);
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
        bool isEncrypted;
        string suggestedDest;
        try
        {
            isEncrypted = ProbeIsEncrypted(archivePath);
            suggestedDest = ResolveExtractDestination(archivePath);
        }
        catch (Exception ex)
        {
            ShowError(ex.Message);
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
    /// Attempt the silent (no-UI) extract path. Returns true when the
    /// caller should NOT fall through to the panel — either we succeeded,
    /// or we hit a fatal non-password error that the user already saw in
    /// the status bar. Returns false ONLY when the panel should take
    /// over (no stored password for an encrypted archive, or the stored
    /// password turned out to be wrong).
    /// </summary>
    private async Task<bool> TrySilentExtractAsync(string archivePath, string destination, bool isEncrypted)
    {
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
        catch (OperationCanceledException) { ResetStatus(); return true; }
        catch (Exception ex) { ShowError(ex.Message); return true; }
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
        ExtractPanel.Configure(archivePath, suggestedDest, isEncrypted, showDestination);

        while (true)
        {
            var args = await PromptExtractPanelAsync().ConfigureAwait(true);
            if (args is null)
            {
                SwitchView(AppView.Idle);
                ResetStatus();
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
                ExtractPanel.Configure(archivePath, dest, needsPassword: true, showDestination: showDestination);
                ExtractPanel.ShowError(_strings.GetString("Error_WrongPassword/Text"));
                // continue while-loop for retry
            }
            catch (OperationCanceledException) { SwitchView(AppView.Idle); ResetStatus(); return; }
            catch (Exception ex) { SwitchView(AppView.Idle); ShowError(ex.Message); return; }
        }
    }

    /// <summary>
    /// Window height for the ExtractPanel in its current footprint.
    /// Compact mode (password-only) shrinks well below the idle config
    /// panel; full mode needs the destination row plus the action bar
    /// to clear the status strip below.
    /// </summary>
    private int PickExtractWindowHeight()
        => PickExtractHeight(ExtractPanel.IsDestinationVisible);

    private static int PickExtractHeight(bool showDestination)
        => showDestination ? 420 : 360;

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
        _lastExtractDestination = destination;
        _lastExtractedArchive = archivePath;

        bool destExistedBefore = Directory.Exists(destination);

        await CancelInFlightAsync().ConfigureAwait(true);
        var cts = new CancellationTokenSource();
        _activeCts = cts;
        SetBusy(extracting: true, Path.GetFileName(archivePath));

        var progress = new Progress<ProgressUpdate>(p =>
            StatusProgress.Value = Math.Clamp(p.FractionComplete, 0.0, 1.0));

        try
        {
            bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
            using var archive = string.IsNullOrEmpty(password)
                ? Archive.Open(archivePath)
                : Archive.OpenWithPassword(archivePath, password);
            var report = await archive
                .ExtractAllAsync(destination, OverwritePolicy.Always, progress,
                    preserveZoneIdentifier: preserveMotw, cancellationToken: cts.Token)
                .ConfigureAwait(true);
            ShowExtractDone(report);
        }
        catch (OtterzipException ex) when (ex.IsWrongPassword)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
        catch (OperationCanceledException)
        {
            RollbackPartialExtract(destination, destExistedBefore);
            throw;
        }
        finally
        {
            if (_activeCts == cts) _activeCts = null;
            cts.Dispose();
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
        return useSubfolder ? Path.Combine(baseDir, stem) : baseDir;
    }

    /// <summary>
    /// "여기에 풀기" semantics — always sibling subfolder, ignore the
    /// Settings_ExtractLocation custom-folder rule (the user explicitly
    /// chose to extract right next to the archive).
    /// </summary>
    private static string ComputeExtractHerePath(string archivePath)
    {
        string parent = Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory();
        string stem = Path.GetFileNameWithoutExtension(archivePath);
        return Path.Combine(parent, stem);
    }

    /// <summary>
    /// Toggle the body view + resize the window to fit. ConfigPanel is
    /// 460 tall; ExtractPanel sizes between 360 (compact, password-only)
    /// and 420 (full, with destination row) depending on what the user
    /// is being asked to confirm. Callers pass the desired ExtractPanel
    /// height; idle always returns to 460.
    /// </summary>
    private void SwitchView(AppView view, int extractHeight = 420)
    {
        _currentView = view;
        bool extract = view == AppView.Extract;
        ConfigPanel.Visibility = extract ? Visibility.Collapsed : Visibility.Visible;
        ExtractPanel.Visibility = extract ? Visibility.Visible : Visibility.Collapsed;
        TrySizeWindow(width: 420, height: extract ? extractHeight : 460);
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

    private async Task ExtractManyAsync(IReadOnlyList<string> archives)
    {
        // Bulk extract: each archive → its own folder next to itself.
        // No per-archive prompts; uses sensible defaults (Always overwrite).
        await CancelInFlightAsync().ConfigureAwait(true);
        var cts = new CancellationTokenSource();
        _activeCts = cts;

        try
        {
            // Settings driving bulk extract path. Custom location collapses
            // every archive into siblings inside the chosen folder; subfolder
            // ON adds a per-archive nesting layer to keep them separated.
            string extractLoc = SettingsService.Get<string>("Settings_ExtractLocation", "same");
            string customDir = SettingsService.Get<string>("Settings_ExtractLocationPath", "");
            bool useSubfolder = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);
            bool preserveMotw = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
            bool useCustom = string.Equals(extractLoc, "custom", StringComparison.Ordinal)
                && !string.IsNullOrWhiteSpace(customDir)
                && Directory.Exists(customDir);

            ulong totalBytes = 0;
            uint totalEntries = 0;
            foreach (var archivePath in archives)
            {
                cts.Token.ThrowIfCancellationRequested();
                SetBusy(extracting: true, Path.GetFileName(archivePath));
                string baseDir = useCustom
                    ? customDir
                    : (Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory());
                string stem = Path.GetFileNameWithoutExtension(archivePath);
                string dest = useSubfolder ? Path.Combine(baseDir, stem) : baseDir;
                _lastExtractDestination = dest;

                using var archive = Archive.Open(archivePath);
                var report = await archive
                    .ExtractAllAsync(dest, OverwritePolicy.Always, progress: null,
                        preserveZoneIdentifier: preserveMotw, cancellationToken: cts.Token)
                    .ConfigureAwait(true);
                totalBytes += report.BytesWritten;
                totalEntries += report.EntriesExtracted;
            }
            ShowExtractDone(new ExtractReport
            {
                EntriesExtracted = totalEntries,
                BytesWritten = totalBytes,
            });
        }
        catch (OperationCanceledException)
        {
            ResetStatus();
        }
        catch (Exception ex)
        {
            ShowError(ex.Message);
        }
        finally
        {
            if (_activeCts == cts) _activeCts = null;
            cts.Dispose();
        }
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
    //  Compress
    // ============================================================
    private async Task CompressAsync(IReadOnlyList<string> sources)
    {
        await CancelInFlightAsync().ConfigureAwait(true);

        // Phase 6+ rev 4: resolve compress password before kicking off
        // the long-running task. Order of precedence:
        //   1. ConfigPanel.Password (user typed it on the main window)
        //   2. Settings_DefaultPassword if Settings_DefaultPasswordOnCompress is on
        //   3. Settings_AlwaysPromptPassword: prompt the user every time
        //   4. None — archive is created without encryption
        // Returns null if the user cancelled an explicit prompt.
        string? compressPassword = await ResolveCompressPasswordAsync().ConfigureAwait(true);
        if (compressPassword is null && SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false))
        {
            // Always-prompt is on but user backed out — abort the whole job.
            return;
        }

        var cts = new CancellationTokenSource();
        _activeCts = cts;

        try
        {
            // Read from SettingsService so the panel checkbox and Settings
            // dialog stay in lock-step (rev 5 sync — both write to the same
            // Settings_CompressSeparately key).
            bool separately = SettingsService.Get<bool>("Settings_CompressSeparately", false);
            if (separately && sources.Count > 1)
            {
                await CompressEachSeparatelyAsync(sources, compressPassword, cts.Token).ConfigureAwait(true);
            }
            else
            {
                var plan = PlanCompress(sources);
                SetBusyCompressing(Path.GetFileName(plan.Destination));
                var report = await RunCompressAsync(plan, sources, compressPassword, cts.Token).ConfigureAwait(true);
                await MaybeVerifyAsync(plan.Destination, cts.Token).ConfigureAwait(true);
                MaybeRecycleSources(sources);
                ShowCompressDone(plan.Destination, report.BytesWritten);
            }
        }
        catch (OperationCanceledException)
        {
            ResetStatus();
        }
        catch (Exception ex)
        {
            ShowError(ex.Message);
        }
        finally
        {
            if (_activeCts == cts) _activeCts = null;
            cts.Dispose();
        }
    }

    private async Task CompressEachSeparatelyAsync(
        IReadOnlyList<string> sources, string? password, CancellationToken ct)
    {
        // "압축 따로따로 하기" — each source becomes its own archive sibling.
        ulong totalBytes = 0;
        string? lastDest = null;
        foreach (var src in sources)
        {
            ct.ThrowIfCancellationRequested();
            var plan = PlanCompress(new[] { src });
            SetBusyCompressing(Path.GetFileName(plan.Destination));
            var report = await RunCompressAsync(plan, new[] { src }, password, ct).ConfigureAwait(true);
            await MaybeVerifyAsync(plan.Destination, ct).ConfigureAwait(true);
            MaybeRecycleSources(new[] { src });
            totalBytes += report.BytesWritten;
            lastDest = plan.Destination;
        }
        if (lastDest is not null)
        {
            ShowCompressDone(lastDest, totalBytes);
        }
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
    private async Task MaybeVerifyAsync(string archivePath, CancellationToken ct)
    {
        if (!SettingsService.Get<bool>("Settings_VerifyAfterCompress", false))
        {
            return;
        }
        StatusText.Text = string.Format(CultureInfo.CurrentCulture,
            _strings.GetString("Main_StatusBarVerifyingFormat/Text"),
            Path.GetFileName(archivePath));
        using var archive = Archive.Open(archivePath);
        var report = await archive.TestAsync(ct).ConfigureAwait(true);
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
        // source name.
        bool useParent = SettingsService.Get<bool>("Settings_UseParentFolderName", true);
        string stem = sources.Count == 1
            ? Path.GetFileNameWithoutExtension(firstSource)
            : useParent
                ? Path.GetFileName(parentDir)
                : Path.GetFileNameWithoutExtension(firstSource);
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

        return new CompressPlan(
            Destination: Path.Combine(parentDir, $"{stem}{ext}"),
            Format: fmt,
            Method: method,
            Level: level);
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
        CancellationToken ct)
    {
        // Settings toggles read once at the start of a compress job, so
        // changing them mid-flight doesn't leak into an in-progress task.
        bool excludeMeta = SettingsService.Get<bool>("Settings_ExcludeSystemMetadata", true);

        if (sources.Count == 1 && Directory.Exists(sources[0]))
        {
            return ArchiveBuilder.CreateFromDirectoryAsync(
                plan.Destination, sources[0], plan.Format, plan.Method,
                plan.Level, progress: null,
                excludeSystemMetadata: excludeMeta,
                password: password,
                cancellationToken: ct);
        }
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
    //  Status helpers
    // ============================================================
    private void ShowExtractDone(ExtractReport report)
    {
        string sizeText = FormatByteSize(report.BytesWritten);
        string fmt = _strings.GetString("Main_StatusBarDoneFormat/Text");
        StatusText.Text = string.Format(
            CultureInfo.CurrentCulture, fmt, report.EntriesExtracted, sizeText);
        StatusProgress.Value = 1.0;
        StatusProgress.Visibility = Visibility.Collapsed;

        if (SettingsService.Get<bool>("Settings_PlaySoundOnExtract", true))
        {
            Win32Helper.PlayCompletionSound();
        }
        ToastService.ShowCompletion(
            _strings.GetString("Toast_ExtractTitle/Text"),
            string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Toast_ExtractBodyFormat/Text"),
                report.EntriesExtracted, sizeText));
        // Reveal-in-Explorer for extract: ExtractDialog stashes the chosen
        // destination on _lastExtractDestination — we surface it here. The
        // dest path lives in the calling stack, not in the report, so the
        // wiring lands when ExtractAsync caches it.
        if (SettingsService.Get<bool>("Settings_RevealAfterExtract", true)
            && _lastExtractDestination is not null)
        {
            Win32Helper.RevealInExplorer(_lastExtractDestination);
        }
        // Phase 6+ rev 3: recycle the just-extracted archive.
        if (SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false)
            && _lastExtractedArchive is not null)
        {
            _ = Win32Helper.MoveToRecycleBin(_lastExtractedArchive);
        }
    }

    private string? _lastExtractDestination;
    private string? _lastExtractedArchive;

    private void ShowCompressDone(string destination, ulong bytes)
    {
        string sizeText = FormatByteSize(bytes);
        string fmtMsg = _strings.GetString("Main_StatusBarCompressDoneFormat/Text");
        StatusText.Text = string.Format(
            CultureInfo.CurrentCulture, fmtMsg, Path.GetFileName(destination), sizeText);
        StatusProgress.IsIndeterminate = false;
        StatusProgress.Visibility = Visibility.Collapsed;

        // Post-completion side effects per Settings (Phase 6+).
        if (SettingsService.Get<bool>("Settings_PlaySoundOnCompress", true))
        {
            Win32Helper.PlayCompletionSound();
        }
        if (SettingsService.Get<bool>("Settings_RevealAfterCompress", true))
        {
            Win32Helper.RevealInExplorer(destination);
        }
        ToastService.ShowCompletion(
            _strings.GetString("Toast_CompressTitle/Text"),
            string.Format(CultureInfo.CurrentCulture,
                _strings.GetString("Toast_CompressBodyFormat/Text"),
                Path.GetFileName(destination), sizeText));
        // Source deletion is handled inside CompressAsync where we still
        // know the source paths — Done is too late to recover them here.
    }

    private void SetBusyCompressing(string outputFileName)
    {
        string fmt = _strings.GetString("Main_StatusBarCompressingFormat/Text");
        StatusText.Text = string.Format(CultureInfo.CurrentCulture, fmt, outputFileName);
        StatusProgress.IsIndeterminate = true;
        StatusProgress.Visibility = Visibility.Visible;
    }

    private void SetBusy(bool extracting, string fileName)
    {
        if (extracting)
        {
            string fmt = _strings.GetString("Main_StatusBarExtractingFormat/Text");
            StatusText.Text = string.Format(CultureInfo.CurrentCulture, fmt, fileName);
            StatusProgress.Value = 0;
            StatusProgress.Visibility = Visibility.Visible;
        }
        else
        {
            StatusProgress.Visibility = Visibility.Collapsed;
        }
    }

    private void ResetStatus()
    {
        StatusText.Text = _strings.GetString("Main_StatusBarIdleText/Text");
        StatusProgress.Visibility = Visibility.Collapsed;
    }

    private void ShowError(string message)
    {
        string fmt = _strings.GetString("Main_StatusBarErrorFormat/Text");
        StatusText.Text = string.Format(CultureInfo.CurrentCulture, fmt, message);
        StatusProgress.Visibility = Visibility.Collapsed;
    }

    private async Task CancelInFlightAsync()
    {
        if (_activeCts is { } previous)
        {
            await previous.CancelAsync().ConfigureAwait(true);
        }
    }

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
