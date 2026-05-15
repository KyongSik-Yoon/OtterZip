using System.Collections.Generic;

namespace OtterZip.App;

/// <summary>
/// Parsed `--invoke &lt;verb&gt; --files &lt;paths&gt;` payload from the shell
/// extension.
///
/// <para>
/// <see cref="Verb"/> is the canonical verb root (`extract-here` /
/// `extract` / `compress`). Format-specific quick verbs such as
/// `compress-zip` and `compress-7z` collapse to <c>Verb="compress"</c>
/// + <see cref="QuickFormat"/> + <see cref="IsHeadless"/> so the
/// MainWindow dispatch logic stays simple.
/// </para>
///
/// <para>
/// <see cref="IsHeadless"/> drives <c>App.OnLaunched</c>'s bifurcation:
/// when <c>true</c>, the launch skips <c>MainWindow</c> entirely and
/// shows a <c>ProgressDialog</c> instead. Bandizip's
/// <c>DATA.zip으로 압축</c> UX.
/// </para>
/// </summary>
/// <param name="Verb">Canonical verb root, case preserved.</param>
/// <param name="Paths">File system paths supplied via <c>--files</c>.</param>
/// <param name="QuickFormat">
/// <c>"zip"</c> / <c>"7z"</c> for the format-specific quick verbs,
/// <c>null</c> for the generic <c>compress</c> verb.
/// </param>
/// <param name="IsHeadless">
/// When <c>true</c>, route to <c>ProgressDialog</c> instead of
/// <c>MainWindow</c>.
/// </param>
internal sealed record InvokeRequest(
    string Verb,
    IReadOnlyList<string> Paths,
    string? QuickFormat = null,
    bool IsHeadless = false);
