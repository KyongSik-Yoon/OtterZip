using System;
using System.Threading.Tasks;

namespace OtterZip.App.Services;

/// <summary>
/// Compress / Extract password resolution extracted from MainWindow so
/// the Bandizip-style ProgressDialog flow (which has no ConfigPanel
/// password field) can run the same precedence:
///
///   1. Explicit per-flow input (the ConfigPanel password textbox on
///      MainWindow; <c>null</c> on the headless flow).
///   2. Vault default — <c>CredentialStore.Get()</c> guarded by the
///      relevant per-flow toggle (Settings_DefaultPasswordOnCompress /
///      Settings_DefaultPasswordOnExtract) and optional Hello gate
///      (Settings_AuthBeforeUseDefaultPassword).
///   3. Settings_AlwaysPromptPassword: show a `ContentDialog` to ask the
///      user. ProgressDialog supplies its own prompter delegate;
///      MainWindow keeps its existing inline implementation.
///
/// All members static. The caller supplies the password-prompt delegate
/// because the prompter needs a `XamlRoot`, which differs between
/// MainWindow and ProgressDialog.
/// </summary>
internal static class PasswordResolver
{
    /// <summary>
    /// Compress password resolution. <paramref name="panelPassword"/>
    /// is the value the user just typed in the ConfigPanel password
    /// textbox (or <c>null</c> on headless flows where there's no
    /// panel). <paramref name="prompter"/> shows a password
    /// <c>ContentDialog</c> with the archive label and returns either
    /// the entered password or <c>null</c> on cancel.
    /// </summary>
    /// <returns>
    /// The password to apply to the compress operation, or <c>null</c>
    /// when no password should be set (either no source provided one
    /// AND <c>Settings_AlwaysPromptPassword</c> is off, OR the user
    /// cancelled the prompt and the caller should abort).
    /// </returns>
    public static async Task<string?> ResolveCompressAsync(
        string? panelPassword,
        string authReason,
        string promptArchiveLabel,
        Func<string, Task<string?>>? prompter)
    {
        // 1. Explicit panel input wins.
        if (!string.IsNullOrEmpty(panelPassword))
        {
            return panelPassword;
        }

        // 2. Stored default (Hello-gated).
        string? stored = await ResolveStoredDefaultAsync(
            "Settings_DefaultPasswordOnCompress", authReason).ConfigureAwait(true);
        if (!string.IsNullOrEmpty(stored))
        {
            return stored;
        }

        // 3. Always-prompt.
        if (SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false)
            && prompter is not null)
        {
            return await prompter(promptArchiveLabel).ConfigureAwait(true);
        }

        return null;
    }

    /// <summary>
    /// Pre-fill source for the inline ExtractPanel password field.
    /// Returns the stored default password ONLY when the silent-extract
    /// path didn't already try it — otherwise the panel would suggest a
    /// known-wrong value. Honours the Hello-gate by leaving the field
    /// empty when the user opted to require auth before each use.
    /// Moved from MainWindow.ResolveExtractPanelPrefill so both flows
    /// share the same precedence.
    /// </summary>
    public static string? ResolveExtractPanelPrefill(bool isEncrypted)
    {
        if (!isEncrypted) return null;
        if (SettingsService.Get<bool>("Settings_DefaultPasswordOnExtract", false))
        {
            return null;
        }
        if (SettingsService.Get<bool>("Settings_AuthBeforeUseDefaultPassword", false))
        {
            return null;
        }
        string stored = CredentialStore.Get();
        return string.IsNullOrEmpty(stored) ? null : stored;
    }

    /// <summary>
    /// Vault read + Hello gate. <c>null</c> when the per-flow toggle is
    /// off, the vault is empty, or Hello verification fails.
    /// </summary>
    public static async Task<string?> ResolveStoredDefaultAsync(string toggleKey, string authReason)
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
}
