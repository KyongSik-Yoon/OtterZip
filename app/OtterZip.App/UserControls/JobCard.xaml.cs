using System;
using System.ComponentModel;
using System.IO;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using OtterZip.App.Models;
using OtterZip.App.Services;

namespace OtterZip.App.UserControls;

/// <summary>
/// A single-job card. Lives inside <c>FloatLayer</c> and reflects
/// every change on the bound <see cref="JobItem"/> by subscribing to
/// <c>PropertyChanged</c> — no x:Bind plumbing, no codegen.
///
/// The card paints itself differently per <see cref="JobState"/>:
///   Queued     · clock glyph, indeterminate-collapsed bar, no follow-up
///   Running    · package glyph, live progress bar
///   Done       · check glyph, progress full, "open folder" link if path
///   Cancelled  · close glyph, bar collapses, error caption colors
///   Error      · warning glyph + caption red
///
/// Cancel button delegates to <see cref="JobItem.RequestCancel"/>; once
/// the job is terminal the same X removes the card via
/// <see cref="JobQueue.Remove"/>.
/// </summary>
public sealed partial class JobCard : UserControl
{
    // Segoe Fluent Icons glyph codepoints used per state. Keeping them
    // named avoids magic numbers sprinkled inside the switch below.
    private const string GlyphClock      = ""; // Clock
    private const string GlyphPackage    = ""; // FileExplorerApp (archive)
    private const string GlyphExtract    = ""; // OpenFolderHorizontal
    private const string GlyphCheck      = ""; // Accept (checkmark)
    private const string GlyphClose      = ""; // Cancel
    private const string GlyphWarning    = ""; // Warning

    private JobItem? _item;
    private JobQueue? _queue;

    public JobCard()
    {
        InitializeComponent();
        Loaded += (_, _) => AttachDropShadow();
        SizeChanged += (_, _) => ResizeDropShadow();
    }

    private Microsoft.UI.Composition.SpriteVisual? _shadowSprite;

    /// <summary>
    /// Attach a composition DropShadow under <c>ShadowHost</c>. We pick
    /// blur / offset / opacity to match the v2 mockup's CSS
    /// <c>0 10px 30px rgba(0,0,0,0.45)</c>. WinUI's built-in
    /// <c>ThemeShadow</c> washes out on dark backgrounds and gives the
    /// card a flat feel; composition lets us control the shadow
    /// explicitly per design.
    /// </summary>
    private void AttachDropShadow()
    {
        if (_shadowSprite is not null) return;
        var rootVisual = Microsoft.UI.Xaml.Hosting.ElementCompositionPreview
            .GetElementVisual(ShadowHost);
        var compositor = rootVisual.Compositor;

        var shadow = compositor.CreateDropShadow();
        shadow.BlurRadius = 28;
        shadow.Offset = new System.Numerics.Vector3(0, 6, 0);
        shadow.Opacity = 0.55f;
        shadow.Color = Microsoft.UI.Colors.Black;

        _shadowSprite = compositor.CreateSpriteVisual();
        _shadowSprite.Shadow = shadow;
        _shadowSprite.Size = new System.Numerics.Vector2(
            (float)ShadowHost.ActualWidth,
            (float)ShadowHost.ActualHeight);

        Microsoft.UI.Xaml.Hosting.ElementCompositionPreview
            .SetElementChildVisual(ShadowHost, _shadowSprite);
    }

    private void ResizeDropShadow()
    {
        if (_shadowSprite is null) return;
        _shadowSprite.Size = new System.Numerics.Vector2(
            (float)ShadowHost.ActualWidth,
            (float)ShadowHost.ActualHeight);
    }

    /// <summary>
    /// Bind this card to a JobItem. Called by FloatLayer once on
    /// material; safe to re-bind to a different item if needed.
    /// </summary>
    public void Bind(JobItem item, JobQueue queue)
    {
        if (_item is not null)
        {
            _item.PropertyChanged -= OnItemPropertyChanged;
        }
        _item = item;
        _queue = queue;
        _item.PropertyChanged += OnItemPropertyChanged;
        Refresh();
    }

    private void OnItemPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(JobItem.DisplayName):
                NameText.Text = _item!.DisplayName;
                break;
            case nameof(JobItem.State):
                ApplyState();
                break;
            case nameof(JobItem.Progress):
                ProgressEl.Value = _item!.Progress;
                break;
            case nameof(JobItem.IsIndeterminate):
                ProgressEl.IsIndeterminate = _item!.IsIndeterminate;
                break;
            case nameof(JobItem.StatusText):
                StatusTextEl.Text = _item!.StatusText ?? string.Empty;
                break;
        }
    }

    private void Refresh()
    {
        if (_item is null) return;
        NameText.Text = _item.DisplayName;
        ProgressEl.Value = _item.Progress;
        ProgressEl.IsIndeterminate = _item.IsIndeterminate;
        StatusTextEl.Text = _item.StatusText ?? string.Empty;
        ApplyState();
    }

    private void ApplyState()
    {
        if (_item is null) return;
        // Belt-and-suspenders: pull the latest scalar values every time
        // state transitions. If a property-change dispatch was dropped or
        // arrived out of order, the State→Done transition still resyncs
        // the visible text (otherwise the card can freeze on "시작 중…"
        // for fast jobs even after the archive is fully written).
        NameText.Text = _item.DisplayName;
        StatusTextEl.Text = _item.StatusText ?? string.Empty;
        switch (_item.State)
        {
            case JobState.Queued:
                IconGlyph.Glyph = GlyphClock;
                ProgressEl.Visibility = Visibility.Visible;
                ProgressEl.IsIndeterminate = false;
                ProgressEl.Value = 0;
                ActionsRow.Visibility = Visibility.Collapsed;
                IconHost.Background = Resource("ControlFillColorSecondaryBrush");
                IconGlyph.Foreground = Resource("TextFillColorTertiaryBrush");
                break;
            case JobState.Running:
                IconGlyph.Glyph = _item.Kind == JobKind.Compress ? GlyphPackage : GlyphExtract;
                ProgressEl.Visibility = Visibility.Visible;
                ActionsRow.Visibility = Visibility.Collapsed;
                IconHost.Background = Resource("OtterzipBrandSoftBrush");
                IconGlyph.Foreground = Resource("OtterzipBrandBrush");
                break;
            case JobState.Done:
                IconGlyph.Glyph = GlyphCheck;
                ProgressEl.IsIndeterminate = false;
                ProgressEl.Value = 1.0;
                ProgressEl.Visibility = Visibility.Collapsed;
                ActionsRow.Visibility = !string.IsNullOrEmpty(_item.ResultPath)
                    ? Visibility.Visible
                    : Visibility.Collapsed;
                IconHost.Background = Resource("SystemFillColorSuccessBackgroundBrush");
                IconGlyph.Foreground = Resource("SystemFillColorSuccessBrush");
                break;
            case JobState.Cancelled:
                IconGlyph.Glyph = GlyphClose;
                ProgressEl.Visibility = Visibility.Collapsed;
                ActionsRow.Visibility = Visibility.Collapsed;
                IconHost.Background = Resource("ControlFillColorSecondaryBrush");
                IconGlyph.Foreground = Resource("TextFillColorTertiaryBrush");
                break;
            case JobState.Error:
                IconGlyph.Glyph = GlyphWarning;
                ProgressEl.Visibility = Visibility.Collapsed;
                ActionsRow.Visibility = Visibility.Collapsed;
                IconHost.Background = Resource("SystemFillColorCriticalBackgroundBrush");
                IconGlyph.Foreground = Resource("SystemFillColorCriticalBrush");
                break;
        }
    }

    private static Brush Resource(string key)
    {
        // Theme brushes live in Application.Resources. Falling back to
        // transparent keeps a missing key from crashing the card.
        if (Application.Current.Resources.TryGetValue(key, out var v) && v is Brush b)
        {
            return b;
        }
        return new SolidColorBrush(Microsoft.UI.Colors.Transparent);
    }

    private void OnCancelClick(object sender, RoutedEventArgs e)
    {
        if (_item is null || _queue is null) return;
        // While running → cancel. After terminal → reap the card.
        if (_item.State == JobState.Queued || _item.State == JobState.Running)
        {
            _item.RequestCancel();
        }
        else
        {
            _queue.Remove(_item);
        }
    }

    private void OnOpenFolderClick(object sender, RoutedEventArgs e)
    {
        if (_item?.ResultPath is null) return;
        try
        {
            var dir = Directory.Exists(_item.ResultPath)
                ? _item.ResultPath
                : Path.GetDirectoryName(_item.ResultPath);
            if (string.IsNullOrEmpty(dir)) return;
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo
            {
                FileName = dir,
                UseShellExecute = true,
            });
        }
        catch
        {
            // Best effort; this is a quality-of-life shortcut, not the
            // primary success surface.
        }
    }
}
