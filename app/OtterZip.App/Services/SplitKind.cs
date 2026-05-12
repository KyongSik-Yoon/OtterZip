namespace OtterZip.App.Services;

/// <summary>
/// Multi-volume archive layout categories — see
/// <see cref="SplitArchiveDetector"/> for the per-category probing
/// strategy and the host-side routing in <c>MainWindow</c>.
/// </summary>
internal enum SplitKind
{
    /// <summary>Not part of a multi-volume set.</summary>
    None,

    /// <summary>
    /// Byte-level split — losslessly recoverable via concatenation.
    /// e.g. <c>name.zip.001 + name.zip.002 = name.zip</c>.
    /// </summary>
    RawByteSplit,

    /// <summary>
    /// Container-aware spanning (real spanned ZIP / 7z split). Needs
    /// a multi-volume-aware reader; not supported in v1.0.
    /// </summary>
    Spanned,
}
