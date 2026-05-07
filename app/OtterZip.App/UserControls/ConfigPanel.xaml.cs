using System;
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;

namespace OtterZip.App.UserControls;

/// <summary>
/// ConfigPanel — Keka-pattern main window body.
///
/// Vertical card stack: Format / Method / Split / Password+AES / four bottom checkboxes.
/// Public dependency properties expose the user's choices to the host window.
///
/// See docs/02-design/design-system.md §3 and docs/02-design/mockup-spec.md S1.
/// </summary>
public sealed partial class ConfigPanel : UserControl
{
    // Segoe Fluent Icons glyph code points.
    private const string GlyphLockClosed = "";
    private const string GlyphLockOpen = "";

    // _strings was used by the retired Method header label (rev 5).
    // Kept here intentionally? No — analyzer CA1823 flags it. Removed.

    /// <summary>Raised when the user clicks the empty drop-hint card.
    /// MainWindow handles the file-picker fallback so the panel stays
    /// decoupled from picker APIs.</summary>
    public event EventHandler? DropHintTapped;

    public ConfigPanel()
    {
        InitializeComponent();
        // Phase 6+ rev 4: hydrate the panel from saved Settings defaults
        // before the first paint, so the user sees their preference rather
        // than the XAML hard-coded fallback. Loaded fires too late — the
        // user could glance and start a job before defaults applied.
        ApplyDefaultsFromSettings();
        ApplyFormatConstraints();
        AdvancedExpander.IsExpanded =
            Services.SettingsService.Get<bool>("Settings_AdvancedExpanded", false);
    }

    private void OnDropHintTapped(object sender, Microsoft.UI.Xaml.Input.TappedRoutedEventArgs e)
    {
        DropHintTapped?.Invoke(this, EventArgs.Empty);
    }

    private void OnAdvancedExpanderToggled(object sender, object e)
    {
        // Persist the expanded state so the user doesn't have to re-open
        // the panel every session.
        Services.SettingsService.Set("Settings_AdvancedExpanded", AdvancedExpander.IsExpanded);
    }

    /// <summary>
    /// Single handler for every checkbox inside the Advanced expander.
    /// The checkbox's Tag is its <see cref="Services.SettingsService"/> key —
    /// keeps the panel and Settings dialog in lock-step, since both write
    /// to the same store.
    /// </summary>
    private void OnAdvancedToggle(object sender, RoutedEventArgs e)
    {
        if (sender is CheckBox cb && cb.Tag is string key)
        {
            Services.SettingsService.Set(key, cb.IsChecked.GetValueOrDefault());
        }
    }

    private void ApplyDefaultsFromSettings()
    {
        // Format default — match the ComboBoxItem index by Tag.
        string fmt = Services.SettingsService.Get<string>("Settings_DefaultFormat", "ZIP");
        for (int i = 0; i < FormatCombo.Items.Count; i++)
        {
            if (FormatCombo.Items[i] is ComboBoxItem item
                && item.Tag is string tag
                && string.Equals(tag, fmt, StringComparison.Ordinal))
            {
                FormatCombo.SelectedIndex = i;
                break;
            }
        }

        // Advanced expander checkboxes — keep in lock-step with
        // Settings → Compression so both surfaces show the same state.
        // Defaults match settings-catalog §3.1 rev 3 (excludeMeta = true,
        // verify = false, deleteSource = false, separately = false).
        ExcludeMetaCheck.IsChecked   = Services.SettingsService.Get<bool>("Settings_ExcludeSystemMetadata", true);
        VerifyCheck.IsChecked        = Services.SettingsService.Get<bool>("Settings_VerifyAfterCompress", false);
        DeleteSourceCheck.IsChecked  = Services.SettingsService.Get<bool>("Settings_DeleteSourceAfterCompress", false);
        SeparatelyCheck.IsChecked    = Services.SettingsService.Get<bool>("Settings_CompressSeparately", false);

        // Method default — Card 1 was retired in rev 5; the value lives
        // in Settings → Compression and is read by MainWindow.PlanCompress
        // directly. Nothing to apply here.
    }

    // ============================================================
    //  Format
    // ============================================================
    /// <summary>Selected archive format identifier ("ZIP" / "7z" / "tar" / "tar.gz" / "tar.xz").</summary>
    public static readonly DependencyProperty SelectedFormatProperty =
        DependencyProperty.Register(
            nameof(SelectedFormat),
            typeof(string),
            typeof(ConfigPanel),
            new PropertyMetadata("ZIP"));

    public string SelectedFormat
    {
        get => (string)GetValue(SelectedFormatProperty);
        set => SetValue(SelectedFormatProperty, value);
    }

    private void OnFormatChanged(object sender, SelectionChangedEventArgs e)
    {
        if (FormatCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            SelectedFormat = tag;
        }
        ApplyFormatConstraints();
    }

    // Compression Method removed from the main panel in rev 5 —
    // see Settings → Compression → Default method (Settings_DefaultMethodIndex).
    // MainWindow.PlanCompress reads the value directly from SettingsService.

    // ============================================================
    //  Split size
    // ============================================================
    /// <summary>Split size in bytes. 0 = no split, -1 = custom (host should prompt).</summary>
    public static readonly DependencyProperty SplitSizeBytesProperty =
        DependencyProperty.Register(
            nameof(SplitSizeBytes),
            typeof(long),
            typeof(ConfigPanel),
            new PropertyMetadata(0L));

    public long SplitSizeBytes
    {
        get => (long)GetValue(SplitSizeBytesProperty);
        set => SetValue(SplitSizeBytesProperty, value);
    }

    private void OnSplitChanged(object sender, SelectionChangedEventArgs e)
    {
        if (SplitCombo.SelectedItem is ComboBoxItem item
            && item.Tag is string tag
            && long.TryParse(tag, NumberStyles.Integer, CultureInfo.InvariantCulture, out long value))
        {
            SplitSizeBytes = value;
        }
    }

    // ============================================================
    //  Password + AES
    // ============================================================
    public static readonly DependencyProperty PasswordProperty =
        DependencyProperty.Register(
            nameof(Password),
            typeof(string),
            typeof(ConfigPanel),
            new PropertyMetadata(string.Empty));

    public string Password
    {
        get => (string)GetValue(PasswordProperty);
        set => SetValue(PasswordProperty, value);
    }

    public static readonly DependencyProperty UseAes256Property =
        DependencyProperty.Register(
            nameof(UseAes256),
            typeof(bool),
            typeof(ConfigPanel),
            new PropertyMetadata(true));

    public bool UseAes256
    {
        get => (bool)GetValue(UseAes256Property);
        set => SetValue(UseAes256Property, value);
    }

    private void OnPasswordChanged(object sender, RoutedEventArgs e)
    {
        Password = PasswordInput.Password ?? string.Empty;
        bool hasValue = !string.IsNullOrEmpty(Password);

        // Lock-icon state mirrors the password presence so users get a
        // glance-confirmation that the archive will be encrypted.
        if (hasValue)
        {
            LockIcon.Glyph = GlyphLockClosed;
            LockIndicator.Foreground = (Brush)Application.Current.Resources["OtterzipBrandBrush"];
        }
        else
        {
            LockIcon.Glyph = GlyphLockOpen;
            LockIndicator.Foreground = (Brush)Application.Current.Resources["TextFillColorTertiaryBrush"];
        }

        // AES-256 only makes sense when there's a password, and only for
        // formats that support encryption. Container-level disable from
        // ApplyFormatConstraints wins over this fine-grained toggle.
        if (FormatSupportsPassword(SelectedFormat))
        {
            UseAes256Check.IsEnabled = hasValue;
            if (hasValue && !UseAes256Check.IsChecked.GetValueOrDefault())
            {
                UseAes256Check.IsChecked = true;
            }
        }
        UseAes256 = UseAes256Check.IsChecked.GetValueOrDefault();
    }

    private void OnRevealToggled(object sender, RoutedEventArgs e)
    {
        // PasswordRevealMode requires a focus cycle on some Windows builds
        // for the eye glyph to actually swap; setting it directly is
        // sufficient for the underlying display.
        PasswordInput.PasswordRevealMode = RevealButton.IsChecked.GetValueOrDefault()
            ? PasswordRevealMode.Visible
            : PasswordRevealMode.Hidden;
    }

    // ============================================================
    //  Bottom checkboxes
    // ============================================================
    public static readonly DependencyProperty ExcludeSystemMetaProperty =
        DependencyProperty.Register(
            nameof(ExcludeSystemMeta),
            typeof(bool),
            typeof(ConfigPanel),
            new PropertyMetadata(true));

    public bool ExcludeSystemMeta
    {
        get => (bool)GetValue(ExcludeSystemMetaProperty);
        set => SetValue(ExcludeSystemMetaProperty, value);
    }

    public static readonly DependencyProperty VerifyAfterCompressProperty =
        DependencyProperty.Register(
            nameof(VerifyAfterCompress),
            typeof(bool),
            typeof(ConfigPanel),
            new PropertyMetadata(false));

    public bool VerifyAfterCompress
    {
        get => (bool)GetValue(VerifyAfterCompressProperty);
        set => SetValue(VerifyAfterCompressProperty, value);
    }

    public static readonly DependencyProperty DeleteSourceAfterProperty =
        DependencyProperty.Register(
            nameof(DeleteSourceAfter),
            typeof(bool),
            typeof(ConfigPanel),
            new PropertyMetadata(false));

    public bool DeleteSourceAfter
    {
        get => (bool)GetValue(DeleteSourceAfterProperty);
        set => SetValue(DeleteSourceAfterProperty, value);
    }

    public static readonly DependencyProperty CompressSeparatelyProperty =
        DependencyProperty.Register(
            nameof(CompressSeparately),
            typeof(bool),
            typeof(ConfigPanel),
            new PropertyMetadata(false));

    public bool CompressSeparately
    {
        get => (bool)GetValue(CompressSeparatelyProperty);
        set => SetValue(CompressSeparatelyProperty, value);
    }

    // ============================================================
    //  Format-driven enable/disable (tar / gz / xz can't password-protect or split)
    // ============================================================
    private void ApplyFormatConstraints()
    {
        // FormatCombo's SelectionChanged fires while XAML is still loading
        // top-to-bottom — at that moment PasswordCard / SplitCombo may not
        // exist yet. Skip until the visual tree is fully realized.
        if (PasswordInput is null || RevealButton is null || UseAes256Check is null || SplitCombo is null)
        {
            return;
        }

        bool supportsPassword = FormatSupportsPassword(SelectedFormat);
        bool supportsSplit = FormatSupportsSplit(SelectedFormat);

        PasswordInput.IsEnabled = supportsPassword;
        RevealButton.IsEnabled = supportsPassword;
        UseAes256Check.IsEnabled = supportsPassword && !string.IsNullOrEmpty(Password);

        SplitCombo.IsEnabled = supportsSplit;
        if (!supportsSplit && SplitCombo.SelectedIndex != 0)
        {
            // Reset to "None" so a hidden non-zero doesn't leak into compress.
            SplitCombo.SelectedIndex = 0;
        }
    }

    private static bool FormatSupportsPassword(string format) =>
        format is "ZIP" or "7z";

    private static bool FormatSupportsSplit(string format) =>
        format is "ZIP" or "7z";

    // ============================================================
    //  Card hover micro-animation (Phase 6+ rev 5)
    //
    //  Subtle accent-tinted border on pointer hover. Fluent's built-in
    //  card has no hover state by design; we add one here so the user
    //  gets a tactile confirmation that each card is its own surface.
    // ============================================================
    private void OnCardPointerEntered(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
    {
        if (sender is Border b)
        {
            b.BorderBrush = (Brush)Application.Current.Resources["OtterzipBrandBrush"];
        }
    }

    private void OnCardPointerExited(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
    {
        if (sender is Border b)
        {
            b.BorderBrush = (Brush)Application.Current.Resources["CardStrokeColorDefaultBrush"];
        }
    }
}
