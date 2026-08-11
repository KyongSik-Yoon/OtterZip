// OtterZip for Linux — main window.
//
// The Linux counterpart of app/OtterZip.App/MainWindow.xaml.cs, reduced to
// what the platform actually needs. Two entry paths converge here:
//
//   * A plain launch, where the user drops or picks files. Archives queue for
//     extraction, everything else queues for compression — one rule, applied
//     to both the drop target and a bare argument list.
//   * A context-menu verb (`--invoke extract-here --files …`), routed by the
//     file-manager actions DesktopIntegration installs. Those launches run
//     the job immediately and close when it settles, so a right-click never
//     leaves a window behind.
//
// All archive work goes through the same JobQueue + CompressEngine the
// Windows build uses; nothing in this file knows how an archive is made.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using Avalonia.Threading;
using OtterZip.App.Models;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.Linux.Views;

public partial class MainWindow : Window
{
    /// <summary>
    /// Archive extensions that make a dropped file "something to extract"
    /// rather than "something to compress". Extension-based rather than
    /// content-based on purpose: this runs on the UI thread for every item
    /// of a drag-hover, and sniffing magic bytes off a network mount would
    /// stall the drag. The real format decision is the core's, at open time.
    /// </summary>
    private static readonly string[] ArchiveExtensions =
    [
        ".zip", ".zipx", ".7z", ".rar", ".tar", ".gz", ".tgz", ".bz2", ".tbz",
        ".tbz2", ".xz", ".txz", ".lzma", ".tlz", ".zst", ".tzst", ".lz4",
        ".iso", ".img", ".cab", ".msi", ".deb", ".jar", ".war", ".ear",
        ".apk", ".aab", ".ipa", ".xpi", ".crx", ".appx", ".msix",
    ];

    /// <summary>
    /// Formats offered in the compress bar, in the order they appear. These
    /// are CompressEngine format tags, not extensions — the engine maps each
    /// to a container + method + extension.
    /// </summary>
    private static readonly string[] FormatTags = ["zip", "7z", "tar.gz", "tar"];

    private readonly JobQueue _queue;
    private readonly List<string> _pending = [];
    private readonly InvokeRequest? _invoke;

    /// <summary>
    /// Number of jobs a headless verb launch is still waiting on. When it
    /// reaches zero the window closes itself — a context-menu action should
    /// not leave a window behind. Zero for a plain launch, which keeps the
    /// window open forever.
    /// </summary>
    private int _headlessOutstanding;

    public MainWindow() : this(null)
    {
    }

    public MainWindow(InvokeRequest? invoke)
    {
        InitializeComponent();

        _invoke = invoke;
        _queue = new JobQueue(
            Dispatcher.UIThread,
            SettingsService.Get<int>("Settings_ConcurrentJobs", 1));
        _queue.JobSettled += OnJobSettled;
        JobList.ItemsSource = _queue.Jobs;

        ApplyStrings();
        SetUpDragAndDrop();

        Opened += OnOpened;
        Closed += (_, _) => _queue.Dispose();
    }

    private void OnOpened(object? sender, EventArgs e)
    {
        if (App.StartupError is { Length: > 0 } err)
        {
            // The native library is missing or ABI-mismatched. Every action
            // would throw, so say so once, plainly, instead of failing per
            // click.
            _ = _queue.ReportError(JobKind.Extract, "OtterZip", err);
            SetStatus(err);
            return;
        }
        if (_invoke is not null)
        {
            _ = DispatchInvokeAsync(_invoke);
        }
    }

    // ---------------------------------------------------------------- UI text

    private void ApplyStrings()
    {
        Title = Strings.Get("App_WindowTitle");
        SettingsButton.Content = Strings.Get("Main_SettingsButton.AutomationProperties.Name");
        DropTitle.Text = Strings.Get("ConfigPanel_DropHintTitle");
        DropSubtitle.Text = Strings.Get("ConfigPanel_DropHintSubtitle");
        AddFilesButton.Content = Strings.Get("Linux_AddFiles");
        AddFolderButton.Content = Strings.Get("Linux_AddFolder");
        CompressButton.Content = Strings.Get("CompressOptionsDialog_CompressButton");
        ClearButton.Content = Strings.Get("ExtractDialog_CancelButton");
        SetStatus(Strings.Get("Main_StatusBarIdleText"));

        FormatCombo.ItemsSource = FormatTags;
        string preferred = SettingsService.Get<string>("Settings_DefaultFormat", "zip");
        FormatCombo.SelectedIndex = Math.Max(0, Array.IndexOf(FormatTags, preferred));
    }

    private void SetStatus(string text) => StatusBar.Text = text;

    // ------------------------------------------------------------ drag & drop

    private void SetUpDragAndDrop()
    {
        DragDrop.SetAllowDrop(this, true);
        // External file drops reach the app on Linux/X11 as of Avalonia 12.1
        // (XDND drop target, AvaloniaUI/Avalonia#20926). handledEventsToo lets
        // the window still see a drop the job list marked handled on the way
        // up. See ArchiveWindow.SetUpDragAndDrop for the same reasoning.
        AddHandler(DragDrop.DragEnterEvent, OnDragEnter, RoutingStrategies.Bubble, handledEventsToo: true);
        AddHandler(DragDrop.DragOverEvent, OnDragOver, RoutingStrategies.Bubble, handledEventsToo: true);
        AddHandler(DragDrop.DragLeaveEvent, OnDragLeave, RoutingStrategies.Bubble, handledEventsToo: true);
        AddHandler(DragDrop.DropEvent, OnDrop, RoutingStrategies.Bubble, handledEventsToo: true);
    }

    private void OnDragEnter(object? sender, DragEventArgs e) => OnDragOver(sender, e);

    private void OnDragOver(object? sender, DragEventArgs e)
    {
        bool hasFiles = e.DataTransfer.Contains(DataFormat.File);
        e.DragEffects = hasFiles ? DragDropEffects.Copy : DragDropEffects.None;
        DropZone.Classes.Set("active", hasFiles);
        e.Handled = true;
    }

    private void OnDragLeave(object? sender, RoutedEventArgs e) =>
        DropZone.Classes.Set("active", false);

    private void OnDrop(object? sender, DragEventArgs e)
    {
        DropZone.Classes.Set("active", false);
        e.Handled = true;

        List<string> paths = DropData.LocalPaths(e.DataTransfer);
        if (paths.Count == 0)
        {
            return;
        }
        Accept(paths);
    }

    /// <summary>
    /// Classify a batch of paths and act: archives extract immediately,
    /// everything else accumulates as a compress selection so the user can
    /// pick a format before committing.
    /// </summary>
    private void Accept(IReadOnlyList<string> paths)
    {
        var archives = new List<string>();
        var others = new List<string>();
        foreach (string p in paths)
        {
            if (File.Exists(p) && IsArchive(p))
            {
                archives.Add(p);
            }
            else if (File.Exists(p) || Directory.Exists(p))
            {
                others.Add(p);
            }
        }

        // A single archive on its own means "show me what's inside" — the
        // thing a double-click or a lone drop most naturally asks for, and
        // what every other archive tool does. It opens the contents window,
        // from which the user can extract or add files. Directly extracting a
        // dropped archive (the old behaviour) surprised people who only wanted
        // to look; the explicit extract-here / extract-smart context-menu
        // verbs still extract without opening a window.
        if (archives.Count == 1 && others.Count == 0)
        {
            OpenArchiveWindow(archives[0]);
            return;
        }

        foreach (string archive in archives)
        {
            QueueExtract(archive, ShellExtractMode.Smart);
        }
        if (others.Count > 0)
        {
            _pending.AddRange(others);
            RefreshPendingBar();
        }
    }

    /// <summary>
    /// Open the archive contents window for <paramref name="path"/>. A verb
    /// launch (double-click) counts it as a job to wait for, so the process
    /// does not exit out from under a window the user is still looking at.
    /// </summary>
    private void OpenArchiveWindow(string path)
    {
        var window = new ArchiveWindow(path);
        // A double-click launch would otherwise close the moment its (zero)
        // jobs settle. Keep the process alive until the contents window closes.
        if (_invoke is not null && !string.Equals(_invoke.Verb, "open", StringComparison.Ordinal))
        {
            Interlocked.Increment(ref _headlessOutstanding);
        }
        window.Closed += (_, _) => OnArchiveWindowClosed();
        window.Show();
    }

    private void OnArchiveWindowClosed()
    {
        // Mirror the headless-job accounting so a context-menu "open" launch
        // exits when its window closes, and a plain launch stays open.
        if (_invoke is null || string.Equals(_invoke.Verb, "open", StringComparison.Ordinal))
        {
            return;
        }
        if (Interlocked.Decrement(ref _headlessOutstanding) <= 0)
        {
            Close();
        }
    }

    /// <summary>
    /// Whether this launch is "open exactly one archive" — a double-click or a
    /// lone "Open With", which should show the contents view as the whole
    /// window rather than the drop window. The verb is <c>"open"</c> (a bare
    /// file argument, with no explicit compress/extract verb); a real verb, a
    /// second file, or a non-archive all fall through to the drop window.
    /// </summary>
    internal static bool IsSingleArchiveOpen(InvokeRequest? invoke) =>
        invoke is { Verb: "open", Paths.Count: 1 }
        && File.Exists(invoke.Paths[0])
        && IsArchive(invoke.Paths[0]);

    internal static bool IsArchive(string path)
    {
        string name = Path.GetFileName(path);
        foreach (string ext in ArchiveExtensions)
        {
            if (name.EndsWith(ext, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        // `.tar.*` compounds are covered by their own entries above; this
        // catches the bare double extension case (`foo.tar.br`) the core may
        // still route to the tar family.
        return name.EndsWith(".tar.gz", StringComparison.OrdinalIgnoreCase)
            || name.EndsWith(".tar.bz2", StringComparison.OrdinalIgnoreCase)
            || name.EndsWith(".tar.xz", StringComparison.OrdinalIgnoreCase)
            || name.EndsWith(".tar.zst", StringComparison.OrdinalIgnoreCase);
    }

    private void RefreshPendingBar()
    {
        PendingBar.IsVisible = _pending.Count > 0;
        if (_pending.Count > 0)
        {
            PendingLabel.Text = Strings.Format("Linux_PendingCountFormat", _pending.Count);
        }
    }

    // -------------------------------------------------------------- commands

    private async void OnAddFilesClick(object? sender, RoutedEventArgs e)
    {
        IReadOnlyList<IStorageFile> picked = await StorageProvider.OpenFilePickerAsync(
            new FilePickerOpenOptions
            {
                Title = Strings.Get("ConfigPanel_DropHintTitle"),
                AllowMultiple = true,
            });
        Accept(LocalPaths(picked));
    }

    private async void OnAddFolderClick(object? sender, RoutedEventArgs e)
    {
        IReadOnlyList<IStorageFolder> picked = await StorageProvider.OpenFolderPickerAsync(
            new FolderPickerOpenOptions
            {
                Title = Strings.Get("Linux_AddFolder"),
                AllowMultiple = true,
            });
        Accept(LocalPaths(picked));
    }

    private static List<string> LocalPaths(IEnumerable<IStorageItem> items)
    {
        var paths = new List<string>();
        foreach (IStorageItem item in items)
        {
            string? local = item.TryGetLocalPath();
            if (!string.IsNullOrEmpty(local))
            {
                paths.Add(local);
            }
        }
        return paths;
    }

    private void OnClearClick(object? sender, RoutedEventArgs e)
    {
        _pending.Clear();
        RefreshPendingBar();
    }

    private void OnCompressClick(object? sender, RoutedEventArgs e)
    {
        if (_pending.Count == 0)
        {
            return;
        }
        var sources = new List<string>(_pending);
        _pending.Clear();
        RefreshPendingBar();
        QueueCompress(sources, FormatCombo.SelectedItem as string);
    }

    private async void OnSettingsClick(object? sender, RoutedEventArgs e)
    {
        var window = new SettingsWindow();
        await window.ShowDialog(this);
        // Language and theme can both change in there; re-read rather than
        // making the settings window reach back into this one.
        Strings.Reload();
        App.ApplyTheme();
        ApplyStrings();
        RefreshPendingBar();
    }

    private void OnCancelJobClick(object? sender, RoutedEventArgs e)
    {
        if (JobOf(sender) is { } job)
        {
            job.StatusLabel = Strings.Get("Job_StatusCancelling");
            job.RequestCancel();
        }
    }

    private void OnDismissJobClick(object? sender, RoutedEventArgs e)
    {
        if (JobOf(sender) is { } job)
        {
            _queue.Remove(job);
        }
    }

    private void OnRevealClick(object? sender, RoutedEventArgs e)
    {
        if (JobOf(sender)?.ResultPath is { Length: > 0 } path)
        {
            _ = Win32Helper.RevealInExplorer(path);
        }
    }

    private static JobItem? JobOf(object? sender) =>
        (sender as Control)?.DataContext as JobItem;

    // ------------------------------------------------------------ job wiring

    private void QueueCompress(IReadOnlyList<string> sources, string? formatTag)
    {
        CompressEngine.CompressPlan plan;
        try
        {
            plan = CompressEngine.BuildPlan(sources, formatTag ?? "zip");
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException)
        {
            _ = _queue.ReportError(
                JobKind.Compress,
                Path.GetFileName(sources[0]),
                ErrorMessages.Localize(ex));
            return;
        }

        var item = new JobItem(JobKind.Compress, Path.GetFileName(plan.Destination))
        {
            ResultPath = plan.Destination,
            ReservedOutputPath = plan.Destination,
        };
        TrackHeadless();

        _queue.Submit(item, async (ct, progress) =>
        {
            item.StatusLabel = Strings.Get("Job_StatusCompressing");
            ulong inputBytes = OperationSummary.TotalInputBytes(sources);
            try
            {
                ArchiveBuildReport report = await CompressEngine.RunAsync(
                    plan, sources, password: null, BridgeProgress(progress), ct);
                await CompressEngine.MaybeVerifyAsync(plan.Destination, ct);
                CompressEngine.MaybeRecycleSources(sources);
                item.StatusText = OperationSummary.Compress(
                    inputBytes,
                    report.BytesWritten,
                    TimeSpan.FromMilliseconds(report.ElapsedMs));
                MaybeChime("Settings_PlaySoundOnCompress");
            }
            catch (Exception)
            {
                // The queue localizes and records the failure; our only extra
                // job is not leaving half an archive on disk.
                CompressEngine.TryDeletePartialArchive(plan.Destination, plan.VolumeSizeBytes);
                throw;
            }
        });
    }

    private void QueueExtract(string archivePath, ShellExtractMode mode)
    {
        var item = new JobItem(JobKind.Extract, Path.GetFileName(archivePath))
        {
            SourcePath = archivePath,
        };
        TrackHeadless();

        _queue.Submit(item, async (ct, progress) =>
        {
            // Volume sets open through their first part; the detector finds
            // the siblings so dropping `foo.7z.003` still extracts the set.
            SplitArchiveDetector.DetectionResult split = SplitArchiveDetector.Probe(archivePath);
            bool spanned = split.Kind != SplitKind.None;

            // A split set must be opened as a set: `OpenMulti` hands the core
            // every volume so it can span the central directory across them,
            // where opening just the dropped part would report a truncated
            // archive.
            using Archive archive = spanned
                ? Archive.OpenMulti(split.Volumes)
                : Archive.Open(archivePath);
            string destination = ResolveDestination(archivePath, mode);
            item.ResultPath = destination;

            ExtractReport report = await archive.ExtractAllAsync(
                destination,
                ExtractDefaults.ResolveOverwrite(),
                BridgeProgress(progress),
                preserveZoneIdentifier: false, // no Zone.Identifier off Windows
                ct);

            item.StatusText = OperationSummary.Extract(
                report.BytesWritten, TimeSpan.FromMilliseconds(report.ElapsedMs));

            if (SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false))
            {
                _ = Win32Helper.MoveToRecycleBin(archivePath);
            }
            MaybeChime("Settings_PlaySoundOnExtract");
        });
    }

    /// <summary>
    /// Decide where an archive unpacks to.
    /// </summary>
    /// <remarks>
    /// <see cref="ShellExtractMode.Here"/> drops the entries straight into the
    /// archive's own directory — what the user asked for, litter included.
    /// <see cref="ShellExtractMode.Subfolder"/> always wraps them in a folder
    /// named after the archive. <see cref="ShellExtractMode.Smart"/> asks the
    /// core whether the archive is already single-rooted and only wraps when
    /// it is not, which is the behaviour that avoids both `foo/foo/…` and a
    /// hundred loose files in Downloads.
    /// </remarks>
    private static string ResolveDestination(string archivePath, ShellExtractMode mode)
    {
        string parent = Path.GetDirectoryName(Path.GetFullPath(archivePath))
                        ?? Directory.GetCurrentDirectory();
        if (mode == ShellExtractMode.Here)
        {
            return parent;
        }
        if (mode == ShellExtractMode.Smart && IsSingleRootFolderArchive(archivePath))
        {
            return parent;
        }
        string stem = OutputNamer.SourceStem(archivePath);
        return OutputNamer.ReserveUniqueDirectory(Path.Combine(parent, stem));
    }

    /// <summary>
    /// Whether every entry lives under one shared top-level folder, so a flat
    /// extract already yields one tidy directory.
    /// </summary>
    /// <remarks>
    /// Computed in managed code from the already-bound <c>ReadEntries</c>
    /// rather than through the core's <c>detect_root_layout</c>, matching the
    /// Windows build exactly — the two front ends must agree on where an
    /// archive lands, and the surest way to guarantee that is to run the same
    /// rule. Any failure (encrypted headers, read error) answers
    /// <c>false</c>, which wraps in a subfolder: the safe way to be wrong.
    /// </remarks>
    private static bool IsSingleRootFolderArchive(string archivePath)
    {
        try
        {
            using var arc = Archive.Open(archivePath);
            string? root = null;
            foreach (EntryInfo entry in arc.ReadEntries())
            {
                string p = entry.Path.Replace('\\', '/');
                int slash = p.IndexOf('/', StringComparison.Ordinal);
                if (slash <= 0)
                {
                    return false; // a root-level file → flat layout
                }
                string top = p[..slash];
                if (root is null)
                {
                    root = top;
                }
                else if (!string.Equals(root, top, StringComparison.Ordinal))
                {
                    return false; // more than one top-level component
                }
            }
            return root is not null;
        }
        catch (Exception ex) when (ex is OtterzipException or IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    /// <summary>
    /// Adapt the FFI's rich <see cref="ProgressUpdate"/> down to the fraction
    /// the queue's reporter consumes, and mirror the per-entry fields onto
    /// the card while we have them.
    /// </summary>
    private static Progress<ProgressUpdate> BridgeProgress(IProgress<double> fraction) =>
        new Progress<ProgressUpdate>(u =>
        {
            if (u.BytesTotal > 0)
            {
                fraction.Report((double)u.BytesProcessed / u.BytesTotal);
            }
            else if (u.EntriesTotal > 0)
            {
                fraction.Report((double)u.EntriesProcessed / u.EntriesTotal);
            }
        });

    private static void MaybeChime(string settingKey)
    {
        if (SettingsService.Get<bool>(settingKey, false))
        {
            Win32Helper.PlayCompletionSound();
        }
    }

    // ------------------------------------------------------- verb dispatch

    private Task DispatchInvokeAsync(InvokeRequest request)
    {
        IReadOnlyList<string> paths = request.Paths;
        switch (request.Verb)
        {
            case "extract-here":
                QueueEach(paths, p => QueueExtract(p, ShellExtractMode.Here));
                break;
            case "extract-smart":
                QueueEach(paths, p => QueueExtract(p, ShellExtractMode.Smart));
                break;
            case "extract-to-subfolder":
            case "extract-dialog":
                QueueEach(paths, p => QueueExtract(p, ShellExtractMode.Subfolder));
                break;
            case "compress":
                QueueCompress(paths, SettingsService.Get<string>("Settings_DefaultFormat", "zip"));
                break;
            case "compress-zip":
                QueueCompress(paths, "zip");
                break;
            case "compress-7z":
                QueueCompress(paths, "7z");
                break;
            case "compress-individually":
                // One archive per item, not one archive of everything.
                foreach (string p in paths)
                {
                    QueueCompress([p], SettingsService.Get<string>("Settings_DefaultFormat", "zip"));
                }
                break;
            case "open":
                // No verb: classify exactly as a drop would.
                Accept(paths);
                break;
            default:
                _ = _queue.ReportError(
                    JobKind.Extract,
                    "OtterZip",
                    Strings.Format("Main_StatusBarUnknownInvokeFormat", request.Verb));
                break;
        }
        return Task.CompletedTask;
    }

    private static void QueueEach(IReadOnlyList<string> paths, Action<string> action)
    {
        foreach (string p in paths)
        {
            action(p);
        }
    }

    /// <summary>
    /// Count a job the window will wait for before auto-closing. Only counts
    /// for a verb launch: a plain launch keeps the window open regardless of
    /// how many jobs come and go.
    /// </summary>
    private void TrackHeadless()
    {
        if (_invoke is not null && !string.Equals(_invoke.Verb, "open", StringComparison.Ordinal))
        {
            Interlocked.Increment(ref _headlessOutstanding);
        }
    }

    private void OnJobSettled(object? sender, JobItem item)
    {
        if (_invoke is null || string.Equals(_invoke.Verb, "open", StringComparison.Ordinal))
        {
            return;
        }
        if (Interlocked.Decrement(ref _headlessOutstanding) > 0)
        {
            return;
        }
        // A failed verb keeps the window up so the user can read why. A
        // successful one disappears, which is what a context-menu action
        // should do.
        if (item.State == JobState.Error)
        {
            return;
        }
        Close();
    }
}

/// <summary>
/// Where an extract puts its output. Mirrors the Windows build's shell
/// extract modes so the two context menus offer the same three choices.
/// </summary>
internal enum ShellExtractMode
{
    /// <summary>Straight into the archive's own directory.</summary>
    Here,

    /// <summary>Always into a new folder named after the archive.</summary>
    Subfolder,

    /// <summary>Into a new folder only when the archive is not single-rooted.</summary>
    Smart,
}
