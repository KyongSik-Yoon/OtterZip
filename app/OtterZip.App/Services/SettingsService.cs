using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using Windows.Storage;

namespace OtterZip.App.Services;

/// <summary>
/// Generic key-value persistence layer over <see cref="ApplicationData.LocalSettings"/>.
///
/// Replaces the per-feature pattern (ThemeService, etc.) so every Phase 6+
/// settings entry shares the same store and change-notification semantics.
///
/// Type contract: only the four primitives that
/// <c>ApplicationData.LocalSettings.Values</c> stores natively are allowed —
/// <c>bool</c>, <c>int</c>, <c>double</c>, <c>string</c>. Composite values
/// would force reflection and break Native AOT trim.
/// </summary>
public static class SettingsService
{
    /// <summary>
    /// In-memory session fallback. <see cref="ApplicationData.Current"/> is
    /// only available when the process has package identity — unpackaged
    /// dev runs throw <see cref="InvalidOperationException"/> on every
    /// Get/Set. Without a fallback, writes silently no-op and reads always
    /// return defaults, so a checked ConfigPanel option would visibly turn
    /// on but never persist to the next read (e.g.
    /// Settings_CompressSeparately stayed false even after the user ticked
    /// the box). This dictionary preserves the user's intent within the
    /// running session.
    /// </summary>
    private static readonly ConcurrentDictionary<string, object?> s_fallback =
        new(StringComparer.Ordinal);

    /// <summary>
    /// Fired when any key is written through <see cref="Set{T}"/>. UI hosts
    /// (SettingsWindow sections, ConfigPanel) subscribe to react live —
    /// e.g. theme change, language switch — without polling.
    /// </summary>
    public static event EventHandler<SettingsChangedEventArgs>? Changed;

    /// <summary>
    /// Read a typed value. Returns <paramref name="defaultValue"/> when the
    /// key is missing OR when the stored value's runtime type doesn't match
    /// <typeparamref name="T"/>. Never throws on the happy path.
    /// </summary>
    public static T Get<T>(string key, T defaultValue)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        try
        {
            object? raw = ApplicationData.Current.LocalSettings.Values[key];
            if (raw is T direct)
            {
                return direct;
            }
            // bool/int/double/string round-trip: LocalSettings preserves type.
            // A type mismatch usually means a schema change — fall through
            // to the in-memory fallback (and then the caller's default),
            // so an old key with the wrong type doesn't crash on startup.
        }
        catch (InvalidOperationException)
        {
            // Unpackaged dev runs: no LocalSettings store available yet.
        }
        catch (System.Runtime.InteropServices.COMException)
        {
            // Same root cause — package identity gate, surfaces as COM
            // hresult instead of InvalidOperationException on some builds.
        }
        if (s_fallback.TryGetValue(key, out var memVal) && memVal is T memDirect)
        {
            return memDirect;
        }
        return defaultValue;
    }

    /// <summary>
    /// Persist a typed value. Subsequent <see cref="Get{T}"/> calls will see
    /// the new value, and <see cref="Changed"/> fires for live UI updates.
    /// </summary>
    public static void Set<T>(string key, T value)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        // Always mirror to the in-memory fallback so subsequent Get<T>
        // calls observe the new value even when LocalSettings isn't
        // available (unpackaged dev). Cheap when both stores work and
        // load-bearing when only one does.
        s_fallback[key] = value;
        try
        {
            // Box T into object — LocalSettings.Values is IPropertySet.
            // Permitted runtime types: bool / int / double / string. Other
            // types may throw at the IPropertySet level; we let that surface
            // because callers shouldn't be storing exotic types here.
            ApplicationData.Current.LocalSettings.Values[key] = value;
        }
        catch (InvalidOperationException)
        {
            // LocalSettings unavailable — fallback dict already holds the
            // value, so notify subscribers as if the persist had succeeded.
        }
        catch (System.Runtime.InteropServices.COMException)
        {
        }
        Changed?.Invoke(null, new SettingsChangedEventArgs(key));
    }

    /// <summary>
    /// Best-effort delete — used by tests and "reset to defaults" flows.
    /// Missing key is not an error.
    /// </summary>
    public static void Remove(string key)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        s_fallback.TryRemove(key, out _);
        try
        {
            ApplicationData.Current.LocalSettings.Values.Remove(key);
        }
        catch (InvalidOperationException)
        {
        }
        catch (System.Runtime.InteropServices.COMException)
        {
        }
        Changed?.Invoke(null, new SettingsChangedEventArgs(key));
    }
}
