using System;
using System.Globalization;
using System.IO;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.App.Dialogs;

public sealed partial class InfoSettingsSection : UserControl
{
    private readonly ResourceLoader _strings = new();

    public InfoSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) =>
        {
            // Two-line metadata layout — Version row shows the semver, the
            // Build row shows the git hash + date in monospace so a user
            // can copy/paste it cleanly into a bug report.
            VersionText.Text = AssemblyInfoService.Version;
            BuildText.Text = $"{AssemblyInfoService.GitHash} · {AssemblyInfoService.BuildDate}";
            // Surface the anonymous install id (PRIVACY.md says it's visible
            // here) so a user can quote it in a crash-report deletion request.
            string installId = OtterzipTelemetry.InstallId;
            if (!string.IsNullOrEmpty(installId))
            {
                BuildText.Text += Environment.NewLine + installId;
            }
            TelemetryEnabledCheck.IsOn =
                SettingsService.Get<bool>("Settings_TelemetryEnabled", false);
        };
    }

    private void OnTelemetryToggle(object sender, RoutedEventArgs e)
    {
        bool optedIn = TelemetryEnabledCheck.IsOn;
        SettingsService.Set("Settings_TelemetryEnabled", optedIn);
        // Apply immediately — Sentry init/dispose is idempotent so flipping
        // back-and-forth doesn't accumulate state.
        OtterzipTelemetry.SetUserOptIn(optedIn);
    }

    private void OnShowOnboarding(object sender, RoutedEventArgs e)
        => OnboardingService.ShowAlways(DispatcherQueue);

    private async void OnOpenLicense(object sender, RoutedEventArgs e)
        => await ShowFileContentDialogAsync("LICENSE.txt").ConfigureAwait(true);

    private async void OnOpenThirdParty(object sender, RoutedEventArgs e)
        => await ShowFileContentDialogAsync("THIRD-PARTY-NOTICES.md").ConfigureAwait(true);

    private async Task ShowFileContentDialogAsync(string fileName)
    {
        if (XamlRoot is null) return;
        string path = Path.Combine(AppContext.BaseDirectory, fileName);
        string content;
        try
        {
            content = File.Exists(path)
                ? await File.ReadAllTextAsync(path).ConfigureAwait(true)
                : string.Format(
                    CultureInfo.CurrentCulture,
                    _strings.GetString("Settings_DocUnavailableFormat/Text"),
                    fileName);
        }
        catch (IOException ex)
        {
            content = ex.Message;
        }
        var dialog = new ContentDialog
        {
            Title = fileName,
            Content = new ScrollViewer
            {
                MaxHeight = 480,
                Content = new TextBlock
                {
                    Text = content,
                    TextWrapping = TextWrapping.Wrap,
                    FontFamily = (Microsoft.UI.Xaml.Media.FontFamily)Application.Current.Resources["MonoFontFamily"],
                    FontSize = 12,
                },
            },
            CloseButtonText = _strings.GetString("Settings_CloseButton/Text"),
            XamlRoot = XamlRoot,
        };
        await dialog.ShowAsync();
    }
}
