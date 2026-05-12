// OtterZip.Interop — Native bindings
// See docs/03-api/ffi-api.md for the full contract.
// AOT-friendly: uses [LibraryImport] source generator exclusively.

using System;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.AssemblyDirectory)]

namespace OtterZip.Interop.Native;

/// <summary>POD mirror of the C <c>OtterzipExtractOptions</c> struct (ffi-api.md §8.1).</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct OtterzipExtractOptions
{
    public byte* DestinationUtf8;
    public nuint DestinationLen;
    public uint OverwritePolicy;
    public byte FlattenPaths;
    public byte PreservePermissions;
    public byte PreserveTimestamps;
    public byte FollowSymlinks;
    public byte BlockPathTraversal;
    /// <summary>PR-7A (Phase 7): propagate Zone.Identifier ADS from source archive.
    /// Reuses the first reserved byte — older callers sending 0 get the safe
    /// "off" default.</summary>
    public byte PreserveZoneIdentifier;
    public byte Reserved1;
    public byte Reserved2;
    public uint MaxCompressionRatio;
    public uint MaxTotalCompressionRatio;
    public ulong MaxTotalOutputBytes;
    public byte* PasswordUtf8;
    public nuint PasswordLen;
    public byte* EntryFilterUtf8;
    public nuint EntryFilterLen;
}

/// <summary>POD mirror of the C <c>OtterzipExtractReport</c>.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct OtterzipExtractReport
{
    public uint EntriesExtracted;
    public uint EntriesSkipped;
    public ulong BytesWritten;
    public uint WarningsCount;
    public ulong ElapsedMs;
}

/// <summary>POD mirror of <c>OtterzipProgressView</c> from ffi-api.md §8.2.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct OtterzipProgressView
{
    public ulong BytesProcessed;
    public ulong BytesTotal;
    public uint EntriesProcessed;
    public uint EntriesTotal;
    public byte* CurrentEntryUtf8;
    public nuint CurrentEntryLen;
    public uint Phase;
    public ulong ElapsedMs;
}

/// <summary>POD mirror of <c>OtterzipTestReport</c> (ffi-api.md §10).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct OtterzipTestReport
{
    public uint EntriesTested;
    public uint EntriesCorrupted;
    public ulong ElapsedMs;
}

/// <summary>POD mirror of <c>OtterzipCreateOptions</c> from ffi-api.md §9.1.
/// ABI v6 (2026-04-30) appended <see cref="ExcludeSystemMetadata"/>.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct OtterzipCreateOptions
{
    public uint Format;
    public uint Compression;
    public byte CompressionLevel;
    public byte Solid;
    public byte PreservePermissions;
    public byte PreserveTimestamps;
    public uint Encryption;
    public byte* PasswordUtf8;
    public nuint PasswordLen;
    public ulong VolumeSizeBytes;
    public uint ThreadCount;
    /// <summary>ABI v6: 1 = skip Thumbs.db / .DS_Store / desktop.ini /
    /// $RECYCLE.BIN / etc. when adding directories. 0 = include everything
    /// (legacy v5 behavior — uninitialised tail reads as 0).</summary>
    public byte ExcludeSystemMetadata;
}

/// <summary>POD mirror of <c>OtterzipEntryView</c> from ffi-api.md §6.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct OtterzipEntryView
{
    public byte* PathUtf8;
    public nuint PathLen;
    public byte IsDirectory;
    public byte IsSymlink;
    public byte IsEncrypted;
    public byte Reserved;
    public ulong UncompressedSize;
    public ulong CompressedSize;
    public uint Compression;
    public uint Encryption;
    public uint Crc32;
    public long ModifiedUnixMs;
    public long AccessedUnixMs;
    public long CreatedUnixMs;
    public uint Attributes;
    public uint HostOs;
    public byte* CommentUtf8;
    public nuint CommentLen;
}

internal static partial class NativeMethods
{
    private const string Lib = "otterzip_ffi";

    // --- Library lifecycle ---

    [LibraryImport(Lib, EntryPoint = "otterzip_init")]
    internal static partial int Init();

    [LibraryImport(Lib, EntryPoint = "otterzip_shutdown")]
    internal static partial void Shutdown();

    [LibraryImport(Lib, EntryPoint = "otterzip_version")]
    internal static partial nint VersionRaw();

    [LibraryImport(Lib, EntryPoint = "otterzip_abi_version")]
    internal static partial uint AbiVersion();

    [LibraryImport(Lib, EntryPoint = "otterzip_last_error_message")]
    internal static partial nint LastErrorMessageRaw();

    [LibraryImport(Lib, EntryPoint = "otterzip_clear_last_error")]
    internal static partial void ClearLastError();

    // --- Format detection ---

    [LibraryImport(Lib, EntryPoint = "otterzip_detect_format")]
    internal static unsafe partial int DetectFormat(
        byte* pathUtf8,
        nuint pathLen,
        out uint outFormat);

    // --- Archive open / close / metadata ---

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_open")]
    internal static unsafe partial int ArchiveOpen(
        byte* pathUtf8,
        nuint pathLen,
        uint mode,
        byte* passwordUtf8,
        nuint passwordLen,
        out IntPtr outHandle);

    // ABI v8 — open a split / spanned archive given an ordered list of
    // volume paths. The native side reassembles every volume into a
    // virtual byte stream so the standard extract pipeline can walk it
    // as if it were a single file. See `otterzip_archive_open_multi`
    // in `crates/otterzip-ffi/src/archive.rs`.
    [LibraryImport(Lib, EntryPoint = "otterzip_archive_open_multi")]
    internal static unsafe partial int ArchiveOpenMulti(
        byte* packedPathsUtf8,
        nuint packedPathsLen,
        nuint* pathOffsets,
        nuint* pathLengths,
        nuint pathCount,
        uint mode,
        out IntPtr outHandle);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_close")]
    internal static partial void ArchiveClose(IntPtr handle);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_format")]
    internal static partial int ArchiveFormat(IntPtr handle, out uint outFormat);

    // --- Iterator ---

    [LibraryImport(Lib, EntryPoint = "otterzip_iterator_new")]
    internal static partial int IteratorNew(IntPtr handle, out IntPtr outIter);

    [LibraryImport(Lib, EntryPoint = "otterzip_iterator_next")]
    internal static unsafe partial int IteratorNext(IntPtr iter, OtterzipEntryView* outEntry);

    [LibraryImport(Lib, EntryPoint = "otterzip_iterator_free")]
    internal static partial void IteratorFree(IntPtr iter);

    // --- Create / Modify (Sprint 4) ---

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_create")]
    internal static unsafe partial int ArchiveCreate(
        byte* pathUtf8,
        nuint pathLen,
        OtterzipCreateOptions* opts,
        out IntPtr outHandle);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_add_file")]
    internal static unsafe partial int ArchiveAddFile(
        IntPtr handle,
        byte* srcPathUtf8,
        nuint srcPathLen,
        byte* entryPathUtf8,
        nuint entryPathLen);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_add_directory")]
    internal static unsafe partial int ArchiveAddDirectory(
        IntPtr handle,
        byte* srcDirUtf8,
        nuint srcDirLen,
        byte* entryPrefixUtf8,
        nuint entryPrefixLen);

    // ABI v7 (2026-05-12) — variant with progress + cancel callback.
    // Mirrors otterzip_archive_extract_all's progress contract: the
    // callback returns 0 to continue, non-zero to request cancellation.
    [LibraryImport(Lib, EntryPoint = "otterzip_archive_add_directory_p")]
    internal static unsafe partial int ArchiveAddDirectoryP(
        IntPtr handle,
        byte* srcDirUtf8,
        nuint srcDirLen,
        byte* entryPrefixUtf8,
        nuint entryPrefixLen,
        delegate* unmanaged[Cdecl]<OtterzipProgressView*, IntPtr, int> progressCb,
        IntPtr userData);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_commit")]
    internal static partial int ArchiveCommit(IntPtr handle);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_rollback")]
    internal static partial int ArchiveRollback(IntPtr handle);

    // --- Metadata accessors (Phase 8 G1) ---

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_entry_count")]
    internal static partial int ArchiveEntryCount(IntPtr handle, out ulong outCount);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_is_encrypted")]
    internal static partial int ArchiveIsEncrypted(IntPtr handle, out byte outBool);

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_is_solid")]
    internal static partial int ArchiveIsSolid(IntPtr handle, out sbyte outTri);

    // --- Extraction ---
    //
    // The progress callback is a function pointer with cdecl convention. AOT
    // requires us to pass an `[UnmanagedCallersOnly]` static method address;
    // we keep the trampoline in `NativeProgressBridge`.

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_extract_all")]
    internal static unsafe partial int ArchiveExtractAll(
        IntPtr handle,
        OtterzipExtractOptions* opts,
        delegate* unmanaged[Cdecl]<OtterzipProgressView*, IntPtr, int> progressCb,
        IntPtr userData,
        OtterzipExtractReport* outReport);

    // --- Integrity test (Phase 8 G2 / Phase 6+ auto-verify) ---

    [LibraryImport(Lib, EntryPoint = "otterzip_archive_test")]
    internal static unsafe partial int ArchiveTest(
        IntPtr handle,
        delegate* unmanaged[Cdecl]<OtterzipProgressView*, IntPtr, int> progressCb,
        IntPtr userData,
        OtterzipTestReport* outReport);

    // --- Helpers ---

    internal static string? VersionString() => Marshal.PtrToStringUTF8(VersionRaw());
    internal static string? LastErrorMessage() => Marshal.PtrToStringUTF8(LastErrorMessageRaw());

    internal static unsafe string? Utf8(byte* ptr, nuint len)
    {
        if (ptr == null || len == 0)
        {
            return null;
        }
        return Marshal.PtrToStringUTF8((IntPtr)ptr, checked((int)len));
    }
}
