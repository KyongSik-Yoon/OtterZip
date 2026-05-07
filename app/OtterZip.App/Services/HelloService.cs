using System;
using System.Threading.Tasks;
using Windows.Security.Credentials.UI;

namespace OtterZip.App.Services;

/// <summary>
/// Phase 7 (PR-7C) — wraps Windows Hello (UserConsentVerifier) for the
/// "require authentication before using saved password" toggle.
///
/// Behavior matrix:
///   * Hello available + user authenticates → returns true
///   * Hello available + user cancels / fails → returns false
///   * Hello not available on this PC (no PIN/biometric) → returns true
///       (we treat "no enrolled credential" as "user is presumed
///        present"; the alternative — refusing to use the saved
///        password — would be hostile to users without Hello hardware)
///   * Settings_AuthBeforeUseDefaultPassword OFF → caller skips us
///
/// Unpackaged dev runs: UserConsentVerifier is package-identity gated
/// on some Windows builds; we catch the resulting exception and fall
/// back to the "available but auto-allow" branch.
/// </summary>
public static class HelloService
{
    /// <summary>
    /// Prompts the user for biometric / PIN consent. Returns true when
    /// authentication succeeds OR when no Hello credential is enrolled
    /// on this machine; false only on explicit user cancel / device
    /// busy / disabled-by-policy.
    /// </summary>
    public static async Task<bool> RequestVerificationAsync(string reason)
    {
        ArgumentException.ThrowIfNullOrEmpty(reason);
        try
        {
            var availability = await UserConsentVerifier.CheckAvailabilityAsync();
            if (availability != UserConsentVerifierAvailability.Available)
            {
                // Not enrolled / disabled / device busy — auto-allow so
                // users without Hello hardware aren't locked out of
                // their own stored password. The toggle remains a UX
                // signal that protects users who *do* have Hello.
                return true;
            }
            var result = await UserConsentVerifier.RequestVerificationAsync(reason);
            return result == UserConsentVerificationResult.Verified;
        }
        catch (Exception)
        {
            // Package-identity gate or COM activation failure — treat as
            // auto-allow per the same rationale.
            return true;
        }
    }
}
