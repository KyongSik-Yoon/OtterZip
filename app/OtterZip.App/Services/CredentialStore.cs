using System;
using Windows.Security.Credentials;

namespace OtterZip.App.Services;

/// <summary>
/// Phase 7 (PR-7C) — wraps Windows <see cref="PasswordVault"/> for OtterZip's
/// stored password. Replaces the v1.0 path of writing plain into
/// <c>SettingsService</c> (LocalSettings/DPAPI) with the OS-managed
/// per-user credential store.
///
/// Why PasswordVault over LocalSettings?
///   * **Per-resource visibility:** users can list / revoke OtterZip's
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
    // pair because OtterZip's "default password" is a single-slot setting,
    // not a multi-account one.
    private const string ResourceName = "OtterZip/DefaultPassword";
    private const string UserName = "default";

    /// <summary>
    /// SettingsService fallback key. Used when PasswordVault is
    /// unavailable (typical unpackaged dev runs). Also the legacy v1.0
    /// home of the default password — see
    /// <see cref="MigrateFromSettingsServiceOnce"/>.
    /// </summary>
    private const string LegacyFallbackKey = "Settings_DefaultPassword";

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
            string? fromVault = cred.Password;
            if (!string.IsNullOrEmpty(fromVault))
            {
                return fromVault;
            }
        }
        catch (Exception)
        {
            // Vault threw — either no credential, or running unpackaged
            // (no package identity). Fall through to the dev fallback.
        }
        // Dev / unpackaged fallback: live alongside other settings via
        // SettingsService (which itself has an in-memory mirror so the
        // password persists for the lifetime of an unpackaged run).
        return SettingsService.Get<string>(LegacyFallbackKey, string.Empty);
    }

    /// <summary>
    /// Stores or replaces the OtterZip default password. Empty string
    /// removes any existing entry (so the user clearing the field really
    /// wipes the vault, not just blanks the in-memory copy).
    /// </summary>
    public static void Set(string password)
    {
        ArgumentNullException.ThrowIfNull(password);
        bool vaultAccepted = false;
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
            vaultAccepted = true;
        }
        catch (Exception)
        {
            // PasswordVault unavailable. Fall through to the dev/unpackaged
            // fallback so the feature still works during F5 sessions.
        }
        // Mirror into / clear from the SettingsService fallback. Packaged
        // runs intentionally also drop the legacy key so a previous dev
        // install doesn't leave a stale plaintext copy behind.
        if (vaultAccepted)
        {
            SettingsService.Remove(LegacyFallbackKey);
            return;
        }
        if (string.IsNullOrEmpty(password))
        {
            SettingsService.Remove(LegacyFallbackKey);
        }
        else
        {
            SettingsService.Set(LegacyFallbackKey, password);
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
        string legacy = SettingsService.Get<string>(LegacyFallbackKey, "");
        if (string.IsNullOrEmpty(legacy))
        {
            return;
        }
        // Set() already takes care of clearing the legacy key when the
        // vault accepts the credential, so the migration is just a
        // pass-through. If the vault is unavailable Set() leaves the key
        // intact (it IS the fallback storage in that case) and Get()
        // keeps reading it.
        Set(legacy);
    }
}
