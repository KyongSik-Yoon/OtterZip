using System;
using System.Collections.Generic;
using Windows.Storage;

namespace SpanZIP.App.Services;

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
            // A type mismatch usually means a schema change — fall back to default
            // rather than throwing, so an old key with the wrong type doesn't
            // crash the app on startup.
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
        return defaultValue;
    }

    /// <summary>
    /// Persist a typed value. Subsequent <see cref="Get{T}"/> calls will see
    /// the new value, and <see cref="Changed"/> fires for live UI updates.
    /// </summary>
    public static void Set<T>(string key, T value)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
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
            // Same fallback path as Get — silent no-op when no store.
            return;
        }
        catch (System.Runtime.InteropServices.COMException)
        {
            return;
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
        try
        {
            ApplicationData.Current.LocalSettings.Values.Remove(key);
        }
        catch (InvalidOperationException)
        {
            return;
        }
        catch (System.Runtime.InteropServices.COMException)
        {
            return;
        }
        Changed?.Invoke(null, new SettingsChangedEventArgs(key));
    }
}
