using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;

namespace OtterZip.App.Dialogs;

/// <summary>
/// Settings → Password tab (Phase 6+ rev 3).
///
/// Storage backend in MVP: <see cref="SettingsService"/> on top of UWP
/// LocalSettings. Windows DPAPI auto-encrypts roaming user state at rest,
/// so the cleartext never hits disk in the user-readable form. v1.1 will
/// move to <c>Windows.Security.Credentials.PasswordVault</c> for explicit
/// per-resource separation + Windows Hello binding.
/// </summary>
public sealed partial class PasswordSettingsSection : UserControl
{
    public PasswordSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) => LoadFromService();
    }

    private void LoadFromService()
    {
        // PR-7C: read default password from PasswordVault (Credential
        // Manager). Falls back to empty when the vault is unavailable
        // (unpackaged dev) or no credential is stored.
        DefaultPasswordBox.Password = CredentialStore.Get();
        OnCompressCheck.IsOn   = SettingsService.Get<bool>("Settings_DefaultPasswordOnCompress", false);
        OnExtractCheck.IsOn    = SettingsService.Get<bool>("Settings_DefaultPasswordOnExtract", false);
        AuthBeforeUseCheck.IsOn = SettingsService.Get<bool>("Settings_AuthBeforeUseDefaultPassword", false);
    }

    private void OnPasswordChanged(object sender, RoutedEventArgs e)
    {
        // Live-save to the vault — empty value clears the entry, so the
        // user truly wipes their credential by emptying the box.
        CredentialStore.Set(DefaultPasswordBox.Password ?? string.Empty);
    }

    private void OnToggle(object sender, RoutedEventArgs e)
    {
        if (sender is ToggleSwitch ts && ts.Tag is string key)
        {
            SettingsService.Set(key, ts.IsOn);
        }
    }
}
