// OtterZip for Linux — localized string lookup.
//
// Same namespace and same surface as the WinUI `Strings` helper, so shared
// code can call `Strings.Get` / `Strings.Format` without knowing which front
// end it is running in.
//
// The WinUI build resolves `.resw` through MRT (`ResourceLoader`), which is a
// Windows App SDK component with no Linux counterpart. Rather than fork the
// catalogue — ten languages, ~294 keys, and a CI check that keeps them in
// lockstep — this reads the very same `Resources.resw` files, which are plain
// RESX XML. They are copied next to the executable by the csproj.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Xml.Linq;

namespace OtterZip.App.Services;

/// <summary>
/// Centralised access to the localized string catalogue. All user-facing
/// text MUST go through here — never literals — per CONVENTIONS.md §3.8.
/// </summary>
public static class Strings
{
    /// <summary>
    /// Fallback language, and the one the catalogue is authored in. Every
    /// other locale is checked against it in CI, so a key present anywhere is
    /// present here.
    /// </summary>
    private const string FallbackTag = "en-US";

    private static readonly System.Threading.Lock s_gate = new();
    private static Dictionary<string, string>? s_active;
    private static Dictionary<string, string>? s_fallback;
    private static string? s_activeTag;

    /// <summary>
    /// The BCP-47 tag currently in effect, after applying the user's override
    /// and the availability check. Shown in Settings → General.
    /// </summary>
    public static string ActiveLanguage
    {
        get
        {
            EnsureLoaded();
            return s_activeTag ?? FallbackTag;
        }
    }

    /// <summary>Every language shipped in the catalogue, as BCP-47 tags.</summary>
    public static IReadOnlyList<string> AvailableLanguages()
    {
        var tags = new List<string>();
        string dir = CatalogueRoot;
        if (Directory.Exists(dir))
        {
            foreach (string sub in Directory.EnumerateDirectories(dir))
            {
                if (File.Exists(Path.Combine(sub, "Resources.resw")))
                {
                    tags.Add(Path.GetFileName(sub));
                }
            }
        }
        tags.Sort(StringComparer.Ordinal);
        return tags;
    }

    /// <summary>Look up a localized string by its <c>name</c> in <c>Resources.resw</c>.</summary>
    public static string Get(string key)
    {
        ArgumentException.ThrowIfNullOrEmpty(key);
        EnsureLoaded();

        // Three spellings reach this function and they all mean one entry:
        //   "Job_StatusStarting"        — plain key
        //   "Job_StatusStarting.Text"   — how the .resw actually names it,
        //                                 because WinUI resw entries address a
        //                                 XAML property
        //   "Job_StatusStarting/Text"   — how MRT rewrites that name, and what
        //                                 the shared ErrorMessages passes
        // Normalising here keeps every caller — shared or Linux-only —
        // working against the same catalogue without a per-call-site dance.
        foreach (string candidate in Candidates(key))
        {
            if (s_active is not null && s_active.TryGetValue(candidate, out string? hit))
            {
                return hit;
            }
        }
        foreach (string candidate in Candidates(key))
        {
            if (s_fallback is not null && s_fallback.TryGetValue(candidate, out string? hit))
            {
                return hit;
            }
        }
        // A missing key is a catalogue bug, not a user-facing failure: show
        // the key so it is obvious in a screenshot instead of blank UI.
        return key;
    }

    /// <summary>
    /// Look up + <see cref="string.Format(IFormatProvider, string, object[])"/>
    /// using <see cref="CultureInfo.CurrentCulture"/> so number / date
    /// placeholders follow the user's locale.
    /// </summary>
    public static string Format(string key, params object[] args)
    {
        string template = Get(key);
        try
        {
            return string.Format(CultureInfo.CurrentCulture, template, args);
        }
        catch (FormatException)
        {
            // A translation with a malformed placeholder must not crash the
            // window — fall back to the untranslated template.
            return template;
        }
    }

    /// <summary>
    /// Re-resolve the active language. Called at startup and whenever the
    /// user changes <c>Settings_Language</c>.
    /// </summary>
    public static void Reload()
    {
        lock (s_gate)
        {
            s_active = null;
            s_fallback = null;
            s_activeTag = null;
        }
        EnsureLoaded();
    }

    private static string CatalogueRoot =>
        Path.Combine(AppContext.BaseDirectory, "Strings");

    private static IEnumerable<string> Candidates(string key)
    {
        yield return key;
        int slash = key.LastIndexOf('/');
        if (slash > 0)
        {
            // "Foo/Text" → "Foo.Text" (the on-disk spelling) and "Foo".
            yield return string.Concat(key.AsSpan(0, slash), ".", key.AsSpan(slash + 1));
            yield return key[..slash];
        }
        int dot = key.LastIndexOf('.');
        if (dot > 0)
        {
            yield return key[..dot];
        }
        else
        {
            // A bare key can name either a TextBlock's `.Text` or a Button/
            // ContentControl's `.Content` in the .resw — WinUI's x:Uid model
            // addresses a XAML property, and which one it is depends on the
            // control. Try both so a button label like
            // `ExtractDialog_PrimaryButton.Content` resolves from the bare key
            // instead of leaking the key onto the button face.
            yield return key + ".Text";
            yield return key + ".Content";
        }
    }

    private static void EnsureLoaded()
    {
        lock (s_gate)
        {
            if (s_active is not null)
            {
                return;
            }
            s_fallback = Load(FallbackTag) ?? [];
            string tag = ResolveTag();
            s_activeTag = tag;
            s_active = string.Equals(tag, FallbackTag, StringComparison.OrdinalIgnoreCase)
                ? s_fallback
                : Load(tag) ?? s_fallback;
        }
    }

    /// <summary>
    /// Pick the catalogue language: the explicit user override first, then
    /// the OS locale, then the base language of the OS locale (so a `ko_KR`,
    /// `ko-KP` or plain `ko` environment all land on `ko-KR`), then English.
    /// </summary>
    private static string ResolveTag()
    {
        IReadOnlyList<string> available = AvailableLanguages();
        if (available.Count == 0)
        {
            return FallbackTag;
        }

        string preferred = SettingsService.Get<string>("Settings_Language", "");
        if (!string.IsNullOrWhiteSpace(preferred) && !string.Equals(preferred, "system", StringComparison.OrdinalIgnoreCase))
        {
            string? pick = Match(available, preferred);
            if (pick is not null)
            {
                return pick;
            }
        }
        return Match(available, CultureInfo.CurrentUICulture.Name) ?? FallbackTag;
    }

    private static string? Match(IReadOnlyList<string> available, string tag)
    {
        string normalized = tag.Replace('_', '-');
        foreach (string candidate in available)
        {
            if (string.Equals(candidate, normalized, StringComparison.OrdinalIgnoreCase))
            {
                return candidate;
            }
        }
        // Base-language match: `ko` matches `ko-KR`, `pt-PT` matches `pt-BR`.
        int dash = normalized.IndexOf('-', StringComparison.Ordinal);
        string baseTag = dash > 0 ? normalized[..dash] : normalized;
        foreach (string candidate in available)
        {
            if (candidate.StartsWith(baseTag + "-", StringComparison.OrdinalIgnoreCase)
                || string.Equals(candidate, baseTag, StringComparison.OrdinalIgnoreCase))
            {
                return candidate;
            }
        }
        return null;
    }

    /// <summary>
    /// Parse one `Resources.resw`. RESX is XML with `&lt;data name="..."&gt;
    /// &lt;value&gt;...&lt;/value&gt;&lt;/data&gt;` entries; `resheader`
    /// elements carry the schema metadata and are ignored.
    /// </summary>
    private static Dictionary<string, string>? Load(string tag)
    {
        string path = Path.Combine(CatalogueRoot, tag, "Resources.resw");
        if (!File.Exists(path))
        {
            return null;
        }
        try
        {
            var map = new Dictionary<string, string>(StringComparer.Ordinal);
            XDocument doc = XDocument.Load(path);
            XElement? root = doc.Root;
            if (root is null)
            {
                return null;
            }
            foreach (XElement data in root.Elements("data"))
            {
                string? name = data.Attribute("name")?.Value;
                string? value = data.Element("value")?.Value;
                if (!string.IsNullOrEmpty(name) && value is not null)
                {
                    map[name] = value;
                }
            }
            return map;
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            return null;
        }
    }
}
