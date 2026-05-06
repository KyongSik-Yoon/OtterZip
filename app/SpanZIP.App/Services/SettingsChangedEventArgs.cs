using System;

namespace SpanZIP.App.Services;

/// <summary>
/// Payload for <see cref="SettingsService.Changed"/>. Carries only the key —
/// subscribers re-read with <see cref="SettingsService.Get{T}"/> to stay in
/// sync with whatever type they expect.
/// </summary>
public sealed class SettingsChangedEventArgs : EventArgs
{
    public SettingsChangedEventArgs(string key) { Key = key; }
    public string Key { get; }
}
