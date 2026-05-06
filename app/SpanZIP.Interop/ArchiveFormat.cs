// SpanZIP.Interop — managed enum mirroring spanzip_core::ArchiveFormat.

namespace SpanZIP.Interop;

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
