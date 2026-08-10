// OtterZip for Linux — settings persistence.
//
// Deliberately in the OtterZip.App.Services namespace with the same public
// surface as the WinUI implementation (Get<T>/Set<T>/Remove/Changed), because
// the shared files linked from OtterZip.App — CompressEngine, ExtractDefaults,
// OutputNamer — call straight into it. Keeping the shape identical is what
// lets those files compile unchanged in both front ends.
//
// The WinUI build stores values in `ApplicationData.Current.LocalSettings`,
// a WinRT/registry-backed bag with no Linux counterpart. Here the store is a
// JSON object under the XDG base directory:
//
//     $XDG_CONFIG_HOME/otterzip/settings.json   (default ~/.config/otterzip)
//
// which is where a Linux user expects to find it, and which survives the
// desktop-integration install/uninstall cycle.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace OtterZip.App.Services;

/// <summary>
/// Generic key-value persistence layer, backed by a JSON file under
/// <c>$XDG_CONFIG_HOME/otterzip</c>.
/// </summary>
/// <remarks>
/// Only the scalar types the settings surface actually uses are supported —
/// <see cref="bool"/>, <see cref="int"/>, <see cref="double"/> and
/// <see cref="string"/> — matching the WinUI implementation's constraint that
/// values be things <c>LocalSettings</c> can round-trip natively.
/// </remarks>
public static class SettingsService
{
    private static readonly System.Threading.Lock s_gate = new();

    /// <summary>
    /// In-memory mirror of the on-disk document. Reads never touch the disk
    /// after the first load: settings are read on nearly every compress plan
    /// and every job card refresh, and the WinUI implementation's backing
    /// store is an in-process bag too.
    /// </summary>
    private static JsonObject? s_doc;

    private static readonly JsonSerializerOptions s_writeOptions = new()
    {
        WriteIndented = true,
    };

    public static event EventHandler<SettingsChangedEventArgs>? Changed;

    /// <summary>
    /// Absolute path of the settings file. Public so Settings → Info can
    /// show the user where their preferences actually live.
    /// </summary>
    public static string SettingsPath => Path.Combine(ConfigDirectory, "settings.json");

    /// <summary>
    /// <c>$XDG_CONFIG_HOME/otterzip</c>, falling back to
    /// <c>~/.config/otterzip</c> when the variable is unset or relative.
    /// The XDG spec requires an absolute path and says to ignore the
    /// variable otherwise, which is exactly the case a container or a
    /// hand-edited profile produces.
    /// </summary>
    public static string ConfigDirectory
    {
        get
        {
            string? xdg = Environment.GetEnvironmentVariable("XDG_CONFIG_HOME");
            string root = !string.IsNullOrEmpty(xdg) && Path.IsPathRooted(xdg)
                ? xdg
                : Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                    ".config");
            return Path.Combine(root, "otterzip");
        }
    }

    public static T Get<T>(string key, T defaultValue)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        lock (s_gate)
        {
            JsonNode? raw = Document()[key];
            if (raw is null)
            {
                return defaultValue;
            }
            try
            {
                return Convert<T>(raw, defaultValue);
            }
            catch (Exception ex) when (ex is FormatException or InvalidOperationException or JsonException)
            {
                // A hand-edited or version-skewed file must not take the app
                // down on startup: fall back to the caller's default, exactly
                // as the WinUI build does on a type mismatch in LocalSettings.
                return defaultValue;
            }
        }
    }

    public static void Set<T>(string key, T value)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        lock (s_gate)
        {
            JsonObject doc = Document();
            doc[key] = value switch
            {
                null => null,
                bool b => JsonValue.Create(b),
                int i => JsonValue.Create(i),
                double d => JsonValue.Create(d),
                string s => JsonValue.Create(s),
                _ => JsonValue.Create(System.Convert.ToString(value, CultureInfo.InvariantCulture)),
            };
            Flush(doc);
        }
        Changed?.Invoke(null, new SettingsChangedEventArgs(key));
    }

    public static void Remove(string key)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        lock (s_gate)
        {
            JsonObject doc = Document();
            if (!doc.Remove(key))
            {
                return;
            }
            Flush(doc);
        }
        Changed?.Invoke(null, new SettingsChangedEventArgs(key));
    }

    /// <summary>
    /// Every key currently stored. Used by the export path in Settings and
    /// by the desktop-integration installer, which mirrors a few values into
    /// the generated `.desktop` files.
    /// </summary>
    public static IReadOnlyDictionary<string, string> Snapshot()
    {
        lock (s_gate)
        {
            var result = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (KeyValuePair<string, JsonNode?> kv in Document())
            {
                result[kv.Key] = kv.Value?.ToString() ?? string.Empty;
            }
            return result;
        }
    }

    private static T Convert<T>(JsonNode raw, T defaultValue)
    {
        // Go through the node's own primitive accessors rather than
        // `Deserialize<T>()`: the file is user-editable, so `"true"` written
        // as a string for a bool key is a case worth surviving instead of
        // discarding the whole preference.
        object? converted = default(T) switch
        {
            bool => raw.GetValueKind() == JsonValueKind.String
                ? bool.Parse(raw.GetValue<string>())
                : raw.GetValue<bool>(),
            int => raw.GetValueKind() == JsonValueKind.String
                ? int.Parse(raw.GetValue<string>(), CultureInfo.InvariantCulture)
                : raw.GetValue<int>(),
            double => raw.GetValueKind() == JsonValueKind.String
                ? double.Parse(raw.GetValue<string>(), CultureInfo.InvariantCulture)
                : raw.GetValue<double>(),
            _ => typeof(T) == typeof(string) ? raw.ToString() : null,
        };
        return converted is T typed ? typed : defaultValue;
    }

    /// <summary>Load-on-first-use. Caller must hold <see cref="s_gate"/>.</summary>
    private static JsonObject Document()
    {
        if (s_doc is not null)
        {
            return s_doc;
        }
        try
        {
            string path = SettingsPath;
            if (File.Exists(path))
            {
                s_doc = JsonNode.Parse(File.ReadAllText(path)) as JsonObject;
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        {
            // Unreadable or corrupt: run on defaults for this session rather
            // than refusing to start. The next Set() rewrites the file.
            s_doc = null;
        }
        s_doc ??= [];
        return s_doc;
    }

    /// <summary>
    /// Persist through a temp file + rename so a crash mid-write cannot
    /// leave a truncated settings.json — which would otherwise reset every
    /// preference the user has. Caller must hold <see cref="s_gate"/>.
    /// </summary>
    private static void Flush(JsonObject doc)
    {
        try
        {
            string dir = ConfigDirectory;
            Directory.CreateDirectory(dir);
            string target = SettingsPath;
            string temp = target + ".tmp";
            File.WriteAllText(temp, doc.ToJsonString(s_writeOptions));
            File.Move(temp, target, overwrite: true);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            // Read-only home, full disk, or a sandbox without write access.
            // The in-memory document stays authoritative for this session so
            // the UI still behaves; nothing is lost that was not already
            // unwritable.
        }
    }
}
