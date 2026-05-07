namespace OtterZip.Interop;

/// <summary>
/// Result of <see cref="Archive.TestAsync"/> — CRC32 verification across
/// every entry. <see cref="EntriesCorrupted"/> being zero means the archive
/// passed integrity check.
/// </summary>
public sealed class TestReport
{
    public uint EntriesTested { get; init; }
    public uint EntriesCorrupted { get; init; }
    public ulong ElapsedMs { get; init; }

    public bool IsHealthy => EntriesCorrupted == 0;
}
