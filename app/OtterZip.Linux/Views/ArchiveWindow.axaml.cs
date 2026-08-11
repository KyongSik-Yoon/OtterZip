// OtterZip for Linux — archive contents window.
//
// This is the answer to "I double-clicked an archive to see what's inside and
// got a background extract job instead." Opening an archive now shows its
// entries, and from here you can extract everything or add more files — the
// two things a user reaches for after making an archive.
//
// Adding runs the shipped `otterzip` command-line tool rather than a new FFI
// entry point. The CLI already implements ZIP append (tested), sits next to
// otterzip-gui in every package, and shelling out keeps the C ABI — which is
// version-locked and checked in CI — untouched. It is the same pattern the
// desktop-integration code uses for gio/xdg-open.

using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.Linux.Views;

/// <summary>One row in the entry list.</summary>
public sealed class EntryRow
{
    public required string Name { get; init; }
    public required string SizeText { get; init; }
    public required string ModifiedText { get; init; }
}

public partial class ArchiveWindow : Window
{
    private readonly string _archivePath;
    private readonly ObservableCollection<EntryRow> _rows = [];

    public ArchiveWindow() : this(string.Empty)
    {
    }

    public ArchiveWindow(string archivePath)
    {
        InitializeComponent();
        _archivePath = archivePath;
        EntryList.ItemsSource = _rows;
        ApplyStrings();
        Title = Path.GetFileName(archivePath);
        ArchiveName.Text = Path.GetFileName(archivePath);
        LoadEntries();
    }

    private void ApplyStrings()
    {
        ColName.Text = Strings.Get("Linux_ColName");
        ColSize.Text = Strings.Get("Linux_ColSize");
        ColModified.Text = Strings.Get("Linux_ColModified");
        AddFilesButton.Content = Strings.Get("Linux_AddFiles");
        AddFolderButton.Content = Strings.Get("Linux_AddFolder");
        ExtractButton.Content = Strings.Get("ExtractDialog_PrimaryButton");
    }

    /// <summary>
    /// Read the entries and fill the list. Called on open and after every
    /// successful add, so the window always reflects what is actually in the
    /// archive rather than what we hoped we wrote.
    /// </summary>
    private void LoadEntries()
    {
        _rows.Clear();
        try
        {
            using Archive archive = Archive.Open(_archivePath);
            ulong total = 0;
            int count = 0;
            foreach (EntryInfo entry in archive.ReadEntries())
            {
                if (entry.IsDirectory)
                {
                    continue; // directories are implied by their children
                }
                count++;
                total += entry.UncompressedSize;
                _rows.Add(new EntryRow
                {
                    Name = entry.Path,
                    SizeText = OperationSummary.FormatSize(entry.UncompressedSize),
                    ModifiedText = FormatModified(entry.ModifiedUnixMs),
                });
            }
            ArchiveSummary.Text = Strings.Format(
                "Linux_ArchiveSummaryFormat", count, OperationSummary.FormatSize(total));
        }
        catch (Exception ex) when (ex is OtterzipException or IOException or UnauthorizedAccessException)
        {
            // A corrupt or encrypted-header archive can't be listed. Say so in
            // the summary line rather than showing an empty window that looks
            // like an empty archive.
            ArchiveSummary.Text = ErrorMessages.Localize(ex);
        }
    }

    private static string FormatModified(long unixMs)
    {
        if (unixMs <= 0)
        {
            return string.Empty;
        }
        return DateTimeOffset.FromUnixTimeMilliseconds(unixMs)
            .LocalDateTime
            .ToString("yyyy-MM-dd HH:mm", CultureInfo.CurrentCulture);
    }

    // ------------------------------------------------------------ add / extract

    private async void OnAddFilesClick(object? sender, RoutedEventArgs e)
    {
        IReadOnlyList<IStorageFile> picked = await StorageProvider.OpenFilePickerAsync(
            new FilePickerOpenOptions
            {
                Title = Strings.Get("Linux_AddFiles"),
                AllowMultiple = true,
            });
        await AddPathsAsync(LocalPaths(picked));
    }

    private async void OnAddFolderClick(object? sender, RoutedEventArgs e)
    {
        IReadOnlyList<IStorageFolder> picked = await StorageProvider.OpenFolderPickerAsync(
            new FolderPickerOpenOptions
            {
                Title = Strings.Get("Linux_AddFolder"),
                AllowMultiple = true,
            });
        await AddPathsAsync(LocalPaths(picked));
    }

    /// <summary>
    /// Append the given paths by driving the `otterzip` CLI, then reload the
    /// list so the window shows the real post-add state.
    /// </summary>
    private async Task AddPathsAsync(List<string> paths)
    {
        if (paths.Count == 0)
        {
            return;
        }
        SetBusy(Strings.Get("Job_StatusStarting"));

        var args = new List<string> { "a", _archivePath };
        args.AddRange(paths);

        (int code, string error) = await RunCliAsync(args);
        ClearBusy();

        if (code == 0)
        {
            LoadEntries();
        }
        else
        {
            // The CLI already prints a human-readable reason; surface its last
            // line rather than a generic failure.
            ShowStatus(string.IsNullOrWhiteSpace(error)
                ? Strings.Get("Error_OperationFailed")
                : error.Trim());
        }
    }

    private async void OnExtractClick(object? sender, RoutedEventArgs e)
    {
        IStorageFolder? dest = await StorageProvider.OpenFolderPickerAsync(
            new FolderPickerOpenOptions
            {
                Title = Strings.Get("ExtractDialog_DestinationLabel"),
                AllowMultiple = false,
            }) is { Count: > 0 } folders ? folders[0] : null;
        string? destination = dest?.TryGetLocalPath();
        if (string.IsNullOrEmpty(destination))
        {
            return;
        }

        SetBusy(Strings.Format("Main_StatusBarExtractingFormat", Path.GetFileName(_archivePath)));
        try
        {
            using Archive archive = Archive.Open(_archivePath);
            ExtractReport report = await archive.ExtractAllAsync(
                destination,
                ExtractDefaults.ResolveOverwrite(),
                progress: null,
                preserveZoneIdentifier: false);
            ClearBusy();
            ShowStatus(OperationSummary.Extract(report.BytesWritten, TimeSpan.Zero));
            _ = Win32Helper.RevealInExplorer(destination);
        }
        catch (Exception ex) when (ex is OtterzipException or IOException or UnauthorizedAccessException)
        {
            ClearBusy();
            ShowStatus(ErrorMessages.Localize(ex));
        }
    }

    // ------------------------------------------------------------------ helpers

    /// <summary>
    /// Run the shipped `otterzip` CLI and return its exit code plus stderr.
    /// The binary sits next to otterzip-gui in every package; PATH is the
    /// fallback for a dev run where only one of the two is on it.
    /// </summary>
    private static async Task<(int code, string error)> RunCliAsync(IReadOnlyList<string> args)
    {
        string exe = Path.Combine(AppContext.BaseDirectory, "otterzip");
        if (!File.Exists(exe))
        {
            exe = "otterzip"; // resolve on PATH
        }
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = exe,
                UseShellExecute = false,
                RedirectStandardError = true,
                RedirectStandardOutput = true,
            };
            foreach (string a in args)
            {
                psi.ArgumentList.Add(a);
            }
            using var p = new Process { StartInfo = psi };
            p.Start();
            string err = await p.StandardError.ReadToEndAsync();
            _ = await p.StandardOutput.ReadToEndAsync();
            await p.WaitForExitAsync();
            return (p.ExitCode, err);
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            return (-1, ex.Message);
        }
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

    private void SetBusy(string message)
    {
        AddFilesButton.IsEnabled = false;
        AddFolderButton.IsEnabled = false;
        ExtractButton.IsEnabled = false;
        ShowStatus(message);
    }

    private void ClearBusy()
    {
        AddFilesButton.IsEnabled = true;
        AddFolderButton.IsEnabled = true;
        ExtractButton.IsEnabled = true;
    }

    private void ShowStatus(string message)
    {
        StatusLine.Text = message;
        StatusLine.IsVisible = !string.IsNullOrEmpty(message);
    }
}
