// OtterZip for Linux — settings.
//
// One scrolling pane rather than the WinUI build's navigation view: the
// Linux settings surface is smaller (no MSIX, no Explorer registration, no
// Windows credential vault) and a five-section list reads better than five
// pages of two controls each.
//
// Every control writes straight through to SettingsService under the SAME
// key the Windows build uses, so a user's preferences describe the same thing
// on both platforms and the shared CompressEngine reads them without knowing
// which front end wrote them.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.Linux.Views;

public partial class SettingsWindow : Window
{
    /// <summary>Combo index → <c>Settings_Theme</c> value.</summary>
    private static readonly string[] ThemeValues = ["system", "light", "dark"];

    /// <summary>Combo index → <c>Settings_DefaultFormat</c> value.</summary>
    private static readonly string[] FormatValues = ["zip", "7z", "tar.gz", "tar"];

    /// <summary>Combo index → <c>Settings_SaveLocation</c> value.</summary>
    private static readonly string[] SaveLocationValues = ["same", "custom"];

    /// <summary>
    /// Combo index → <see cref="OverwritePolicy"/>. Ordered by how likely a
    /// user is to want it, not by the enum's numeric order.
    /// </summary>
    private static readonly OverwritePolicy[] OverwriteValues =
    [
        OverwritePolicy.Rename,
        OverwritePolicy.Always,
        OverwritePolicy.IfNewer,
        OverwritePolicy.Never,
    ];

    /// <summary>
    /// Suppresses the write-back handlers while <see cref="LoadValues"/>
    /// populates the controls. Without it, setting SelectedIndex during load
    /// fires SelectionChanged and writes the value straight back — harmless
    /// for most keys, but it would also fire the language reload on every
    /// open.
    /// </summary>
    private bool _loading;

    public SettingsWindow()
    {
        InitializeComponent();
        ApplyStrings();
        LoadValues();
        WireHandlers();
    }

    private void ApplyStrings()
    {
        Title = Strings.Get("Settings_NavGeneral") is { Length: > 0 } and var _
            ? Strings.Get("Main_SettingsButton.AutomationProperties.Name")
            : "Settings";

        AppearanceHeader.Text = Strings.Get("Settings_ThemeSectionLabel");
        ThemeLabel.Text = Strings.Get("Settings_ThemeSectionLabel");
        LanguageLabel.Text = Strings.Get("Settings_LanguageLabel");

        DefaultsHeader.Text = Strings.Get("Settings_GroupDefaults");
        FormatLabel.Text = Strings.Get("Settings_DefaultFormatLabel");
        FormatDesc.Text = Strings.Get("Settings_DefaultFormatDesc");
        MethodLabel.Text = Strings.Get("Settings_DefaultMethodLabel");
        MethodDesc.Text = Strings.Get("Settings_DefaultMethodDesc");
        SaveLocationLabel.Text = Strings.Get("Settings_SaveLocationLabel");
        BrowseButton.Content = Strings.Get("Settings_BrowseButton");

        ExtractHeader.Text = Strings.Get("Settings_GroupAfterExtract");
        OverwriteLabel.Text = Strings.Get("Settings_OverwritePolicyLabel");
        OverwriteDesc.Text = Strings.Get("Settings_OverwritePolicyDesc");
        DeleteArchiveCheck.Content = Strings.Get("Settings_DeleteArchiveDesc");
        SoundExtractCheck.Content = Strings.Get("Settings_PlaySoundOnExtractDesc");
        SoundCompressCheck.Content = Strings.Get("Settings_PlaySoundOnCompressDesc");
        VerifyCheck.Content = Strings.Get("Settings_VerifyAfterCompress");
        DeleteSourceCheck.Content = Strings.Get("Settings_DeleteSourceDesc");

        IntegrationHeader.Text = Strings.Get("Settings_SectionIntegration");
        IntegrationDesc.Text = Strings.Get("Linux_IntegrationDesc");
        InstallButton.Content = Strings.Get("Linux_IntegrationInstall");
        RemoveButton.Content = Strings.Get("Linux_IntegrationRemove");

        InfoHeader.Text = Strings.Get("Settings_NavInfo");
        CloseButton.Content = Strings.Get("Settings_CloseButton");

        PopulateChoices();
    }

    /// <summary>
    /// Fill the combo boxes. Split out of <see cref="ApplyStrings"/> so each
    /// stays readable — labelling controls and enumerating their options are
    /// two different jobs that happen to run back to back.
    /// </summary>
    private void PopulateChoices()
    {
        ThemeCombo.ItemsSource = new[]
        {
            Strings.Get("Theme_System"),
            Strings.Get("Theme_Light"),
            Strings.Get("Theme_Dark"),
        };
        MethodCombo.ItemsSource = new[]
        {
            Strings.Get("Settings_MethodStore"),
            Strings.Get("Settings_MethodFast"),
            Strings.Get("Settings_MethodNormal"),
            Strings.Get("Settings_MethodBest"),
        };
        SaveLocationCombo.ItemsSource = new[]
        {
            Strings.Get("Settings_SaveLocationSame"),
            Strings.Get("Settings_SaveLocationCustom"),
        };
        OverwriteCombo.ItemsSource = new[]
        {
            Strings.Get("Settings_OverwriteRename"),
            Strings.Get("Settings_OverwriteAlways"),
            Strings.Get("Settings_OverwriteSkip"),
            Strings.Get("Settings_OverwriteReject"),
        };
        FormatCombo.ItemsSource = FormatValues;
        LanguageCombo.ItemsSource = LanguageChoices();
    }

    /// <summary>
    /// "Follow system" plus one entry per catalogue language, labelled in
    /// that language's own words (한국어, Deutsch, …) — a user looking for
    /// their language should not have to read the current one to find it.
    /// </summary>
    private static List<string> LanguageChoices()
    {
        var labels = new List<string> { Strings.Get("Settings_LanguageSystem") };
        foreach (string tag in Strings.AvailableLanguages())
        {
            labels.Add(NativeLanguageName(tag));
        }
        return labels;
    }

    private static string NativeLanguageName(string tag)
    {
        // The catalogue already carries endonyms for the ten shipped
        // languages; fall back to the tag itself for anything added later
        // without a matching key.
        string key = tag switch
        {
            "ko-KR" => "Settings_LanguageKo",
            "en-US" => "Settings_LanguageEn",
            "zh-CN" => "Settings_LanguageZh",
            "ja-JP" => "Settings_LanguageJa",
            "de-DE" => "Settings_LanguageDe",
            "fr-FR" => "Settings_LanguageFr",
            "es-ES" => "Settings_LanguageEs",
            "pt-BR" => "Settings_LanguagePt",
            "ru-RU" => "Settings_LanguageRu",
            "it-IT" => "Settings_LanguageIt",
            _ => string.Empty,
        };
        return key.Length == 0 ? tag : Strings.Get(key);
    }

    private void LoadValues()
    {
        _loading = true;
        try
        {
            ThemeCombo.SelectedIndex = Math.Max(0, Array.IndexOf(
                ThemeValues, SettingsService.Get<string>("Settings_Theme", "system")));

            string lang = SettingsService.Get<string>("Settings_Language", "system");
            IReadOnlyList<string> tags = Strings.AvailableLanguages();
            int langIndex = 0;
            for (int i = 0; i < tags.Count; i++)
            {
                if (string.Equals(tags[i], lang, StringComparison.OrdinalIgnoreCase))
                {
                    langIndex = i + 1; // +1 for the "follow system" row
                    break;
                }
            }
            LanguageCombo.SelectedIndex = langIndex;

            FormatCombo.SelectedIndex = Math.Max(0, Array.IndexOf(
                FormatValues, SettingsService.Get<string>("Settings_DefaultFormat", "zip")));
            MethodCombo.SelectedIndex = Math.Clamp(
                SettingsService.Get<int>("Settings_DefaultMethodIndex", 2), 0, 3);
            SaveLocationCombo.SelectedIndex = Math.Max(0, Array.IndexOf(
                SaveLocationValues, SettingsService.Get<string>("Settings_SaveLocation", "same")));
            CustomDirBox.Text = SettingsService.Get<string>("Settings_SaveLocationPath", "");

            OverwriteCombo.SelectedIndex = Math.Max(0, Array.IndexOf(
                OverwriteValues, ExtractDefaults.ResolveOverwrite()));

            DeleteArchiveCheck.IsChecked = SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false);
            SoundExtractCheck.IsChecked = SettingsService.Get<bool>("Settings_PlaySoundOnExtract", false);
            SoundCompressCheck.IsChecked = SettingsService.Get<bool>("Settings_PlaySoundOnCompress", false);
            VerifyCheck.IsChecked = SettingsService.Get<bool>("Settings_VerifyAfterCompress", false);
            DeleteSourceCheck.IsChecked = SettingsService.Get<bool>("Settings_DeleteSourceAfterCompress", false);

            RefreshCustomDirRow();
            RefreshIntegrationState();

            VersionText.Text = string.Create(
                CultureInfo.CurrentCulture,
                $"OtterZip {OtterzipLibrary.Version} · ABI {OtterzipLibrary.AbiVersion} · Avalonia");
            ConfigPathText.Text = SettingsService.SettingsPath;
        }
        finally
        {
            _loading = false;
        }
    }

    private void WireHandlers()
    {
        ThemeCombo.SelectionChanged += (_, _) => Write(() =>
        {
            SettingsService.Set("Settings_Theme", Pick(ThemeValues, ThemeCombo.SelectedIndex));
            // Apply immediately: a theme picker that needs a restart to show
            // its effect is a theme picker nobody trusts.
            App.ApplyTheme();
        });

        LanguageCombo.SelectionChanged += (_, _) => Write(() =>
        {
            int i = LanguageCombo.SelectedIndex;
            IReadOnlyList<string> tags = Strings.AvailableLanguages();
            string value = i <= 0 || i - 1 >= tags.Count ? "system" : tags[i - 1];
            SettingsService.Set("Settings_Language", value);
            Strings.Reload();
            // Re-label this window in the new language on the spot; the main
            // window re-applies its own strings when this dialog closes.
            ApplyStrings();
            LoadValues();
        });

        FormatCombo.SelectionChanged += (_, _) => Write(() =>
            SettingsService.Set("Settings_DefaultFormat", Pick(FormatValues, FormatCombo.SelectedIndex)));

        MethodCombo.SelectionChanged += (_, _) => Write(() =>
            SettingsService.Set("Settings_DefaultMethodIndex", Math.Max(0, MethodCombo.SelectedIndex)));

        SaveLocationCombo.SelectionChanged += (_, _) => Write(() =>
        {
            SettingsService.Set("Settings_SaveLocation", Pick(SaveLocationValues, SaveLocationCombo.SelectedIndex));
            RefreshCustomDirRow();
        });

        OverwriteCombo.SelectionChanged += (_, _) => Write(() =>
        {
            int i = Math.Clamp(OverwriteCombo.SelectedIndex, 0, OverwriteValues.Length - 1);
            SettingsService.Set("Settings_OverwritePolicy", (int)OverwriteValues[i]);
        });

        Bind(DeleteArchiveCheck, "Settings_DeleteArchiveAfterExtract");
        Bind(SoundExtractCheck, "Settings_PlaySoundOnExtract");
        Bind(SoundCompressCheck, "Settings_PlaySoundOnCompress");
        Bind(VerifyCheck, "Settings_VerifyAfterCompress");
        Bind(DeleteSourceCheck, "Settings_DeleteSourceAfterCompress");
    }

    private void Bind(CheckBox box, string key)
    {
        box.IsCheckedChanged += (_, _) => Write(() =>
            SettingsService.Set(key, box.IsChecked == true));
    }

    private void Write(Action action)
    {
        if (_loading)
        {
            return;
        }
        action();
    }

    private static string Pick(string[] values, int index) =>
        values[Math.Clamp(index, 0, values.Length - 1)];

    private void RefreshCustomDirRow() =>
        CustomDirRow.IsVisible = SaveLocationCombo.SelectedIndex == 1;

    private void RefreshIntegrationState()
    {
        bool installed = DesktopIntegration.IsInstalled;
        IntegrationState.Text = installed
            ? Strings.Format("Linux_IntegrationInstalled", DesktopIntegration.ExecutablePath)
            : Strings.Get("Linux_IntegrationNotInstalled");
        InstallButton.IsEnabled = !installed;
        RemoveButton.IsEnabled = installed;
    }

    private async void OnBrowseClick(object? sender, RoutedEventArgs e)
    {
        IReadOnlyList<IStorageFolder> picked = await StorageProvider.OpenFolderPickerAsync(
            new FolderPickerOpenOptions
            {
                Title = Strings.Get("Settings_SaveLocationLabel"),
                AllowMultiple = false,
            });
        if (picked.Count == 0)
        {
            return;
        }
        string? path = picked[0].TryGetLocalPath();
        if (string.IsNullOrEmpty(path))
        {
            return;
        }
        CustomDirBox.Text = path;
        SettingsService.Set("Settings_SaveLocationPath", path);
    }

    private void OnInstallClick(object? sender, RoutedEventArgs e)
    {
        ShowIntegrationLog(DesktopIntegration.Install());
        RefreshIntegrationState();
    }

    private void OnRemoveClick(object? sender, RoutedEventArgs e)
    {
        ShowIntegrationLog(DesktopIntegration.Uninstall());
        RefreshIntegrationState();
    }

    /// <summary>
    /// Show exactly which files the integration touched. This writes into
    /// the user's desktop configuration, so "it worked" is not a good enough
    /// answer — they should be able to see, and undo, every path.
    /// </summary>
    private void ShowIntegrationLog(string log)
    {
        IntegrationLog.Text = log;
        IntegrationLogBox.IsVisible = !string.IsNullOrWhiteSpace(log);
    }

    private void OnCloseClick(object? sender, RoutedEventArgs e) => Close();
}
