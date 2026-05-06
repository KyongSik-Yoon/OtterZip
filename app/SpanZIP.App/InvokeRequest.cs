using System.Collections.Generic;

namespace SpanZIP.App;

/// <summary>
/// Parsed `--invoke <verb> --files <paths>` payload from the shell extension.
/// Verb is lowercased for case-insensitive dispatch (`extract-here` /
/// `extract` / `compress`).
/// </summary>
internal sealed record InvokeRequest(string Verb, IReadOnlyList<string> Paths);
