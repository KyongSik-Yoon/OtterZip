using System;
using Windows.Security.Credentials;

namespace SpanZIP.App.Services;

/// <summary>
/// Phase 7 (PR-7C) — wraps Windows <see cref="PasswordVault"/> for SpanZIP's
/// stored password. Replaces the v1.0 path of writing plain into
/// <c>SettingsService</c> (LocalSettings/DPAPI) with the OS-managed
/// per-user credential store.
///
/// Why PasswordVault over LocalSettings?
///   * **Per-resource visibility:** users can list / revoke SpanZIP's
///     credential through Control Panel → Credential Manager.
///   * **Roaming-safe:** PasswordVault entries don't sync to roaming
///     profiles by accident.
///   * **Future Hello binding:** PasswordVault stays the right surface
///     when we eventually bind a credential to a Windows Hello prompt.
///
/// Behavior under unpackaged dev runs: PasswordVault throws
/// <see cref="System.Runtime.InteropServices.COMException"/> outside a
/// package identity. We catch that and degrade silently to no-op (Get
/// returns empty, Set is a no-op) — the existing
/// <c>SettingsService.Get&lt;string&gt;("Settings_DefaultPassword")</c>
/// path remains the dev fallback so the password feature still works
/// during F5 sessions, just without the OS-level surface.
/// </summary>
public static class CredentialStore
{
    // The vault keys credentials by (resource, username). We use a fixed
    // pair because SpanZIP's "default password" is a single-slot setting,
    // not a multi-account one.
    private const string ResourceName = "SpanZIP/DefaultPassword";
    private const string UserName = "default";

    /// <summary>
    /// Returns the stored password, or empty string when none is set
    /// (or the vault isn't available — see class remarks).
    /// </summary>
    public static string Get()
    {
        try
        {
            var vault = new PasswordVault();
            var cred = vault.Retrieve(ResourceName, UserName);
            cred.RetrievePassword();
            return cred.Password ?? string.Empty;
        }
        catch (Exception)
        {
            // Two flavours of failure are common and benign:
            //   1. No credential stored (Retrieve throws).
            //   2. Unpackaged context — COMException at PasswordVault().
            // Either way the caller's "no stored password" branch handles
            // the empty-string return correctly.
            return string.Empty;
        }
    }

    /// <summary>
    /// Stores or replaces the SpanZIP default password. Empty string
    /// removes any existing entry (so the user clearing the field really
    /// wipes the vault, not just blanks the in-memory copy).
    /// </summary>
    public static void Set(string password)
    {
        ArgumentNullException.ThrowIfNull(password);
        try
        {
            var vault = new PasswordVault();
            // Always remove the existing slot first — PasswordVault.Add
            // doesn't update in place, it appends, so subsequent
            // Retrieve calls would surface the old value.
            try
            {
                var existing = vault.Retrieve(ResourceName, UserName);
                vault.Remove(existing);
            }
            catch (Exception)
            {
                // No existing credential — nothing to remove.
            }
            if (!string.IsNullOrEmpty(password))
            {
                vault.Add(new PasswordCredential(ResourceName, UserName, password));
            }
        }
        catch (Exception)
        {
            // Same fallback contract as Get() — silent no-op when the
            // vault is unavailable. Dev runs continue to use the
            // SettingsService path.
        }
    }

    /// <summary>
    /// One-time migration: lifts a password persisted under
    /// <c>Settings_DefaultPassword</c> into the vault, then clears the
    /// SettingsService entry. Idempotent — running twice is a no-op
    /// because the source key is gone after the first run.
    ///
    /// Called from <c>App.OnLaunched</c> so existing users don't lose
    /// their stored password when v1.0 → v1.0.x rolls out.
    /// </summary>
    public static void MigrateFromSettingsServiceOnce()
    {
        const string LegacyKey = "Settings_DefaultPassword";
        string legacy = SettingsService.Get<string>(LegacyKey, "");
        if (string.IsNullOrEmpty(legacy))
        {
            return;
        }
        Set(legacy);
        // Verify the vault accepted it before wiping the source — if the
        // vault is unavailable (unpackaged), Get() returns empty and we
        // *keep* the legacy value so the user doesn't lose it.
        if (!string.IsNullOrEmpty(Get()))
        {
            SettingsService.Remove(LegacyKey);
        }
    }
}
