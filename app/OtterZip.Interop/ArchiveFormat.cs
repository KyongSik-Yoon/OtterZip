// OtterZip.Interop — managed enum mirroring otterzip_core::ArchiveFormat.

namespace OtterZip.Interop;

// CA1027 / CA1008 wants FlagsAttribute or a `None = 0` member once the
// variant count crosses ~12. ArchiveFormat is a sequential-value enum
// (mirrors a Rust `#[repr(u32)]` enum) where each value identifies one
// container format -- bitwise combination is meaningless. `Unknown = 0`
// already plays the role of the zero-default; suppress both rules at
// the type level so future additions don't trigger them again.
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1027:Mark enums with FlagsAttribute",
    Justification = "Sequential value enum mirroring a Rust #[repr(u32)] enum; not a bit field.")]
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1008:Enums should have zero value",
    Justification = "Unknown = 0 already serves as the zero-default member.")]
public enum ArchiveFormat : uint
{
    Unknown = 0,
    Zip = 1,
    SevenZ = 2,
    Rar = 3,
    Tar = 4,
    Gzip = 5,
    TarGz = 6,
    TarBz2 = 7,
    TarXz = 8,
    // Phase 7+ option Y -- ABI v7. Mirrors otterzip-core's
    // ArchiveFormat (slot 12 reserved for the .Z OUT decision).
    Bzip2 = 9,
    Xz = 10,
    Lzma = 11,
    Zstd = 13,
    TarZst = 14,
    Lz4 = 15,
    TarLz4 = 16,
    Zipx = 17,
    Iso = 18,
    Cab = 19,
    Msi = 20,
    Deb = 21,
}

public enum OverwritePolicy : uint
{
    Never = 0,
    Always = 1,
    IfNewer = 2,
    AskCallback = 3,
}

public enum ProgressPhase : uint
{
    Scanning = 0,
    Reading = 1,
    Writing = 2,
    Finalizing = 3,
}

public sealed class ProgressUpdate
{
    public ulong BytesProcessed { get; init; }
    public ulong BytesTotal { get; init; }
    public uint EntriesProcessed { get; init; }
    public uint EntriesTotal { get; init; }
    public string? CurrentEntry { get; init; }
    public ProgressPhase Phase { get; init; }
    public ulong ElapsedMs { get; init; }

    public double FractionComplete => BytesTotal == 0
        ? 0.0
        : (double)BytesProcessed / BytesTotal;
}

public sealed class ExtractReport
{
    public uint EntriesExtracted { get; init; }
    public uint EntriesSkipped { get; init; }
    public ulong BytesWritten { get; init; }
    public uint WarningsCount { get; init; }
    public ulong ElapsedMs { get; init; }
}
