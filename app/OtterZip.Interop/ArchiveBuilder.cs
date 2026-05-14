// OtterZip.Interop — managed builder for write-mode archives.
//
// Sprint 4. Wraps the create/add/commit triplet so callers can produce
// a ZIP / 7z / .tar.gz from a single source directory in a single
// async call. Multi-source / per-entry callbacks land later.

using System;
using System.IO;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

using OtterZip.Interop.Native;

namespace OtterZip.Interop;

public enum CompressionMethod : uint
{
    Store = 0,
    Deflate = 1,
    Deflate64 = 2,
    Bzip2 = 3,
    Lzma = 4,
    Lzma2 = 5,
    Zstd = 6,
    Ppmd = 7,
}

public enum EncryptionMethod : uint
{
    None = 0,
    ZipCrypto = 1,
    Aes128 = 2,
    Aes256 = 3,
}

/// <summary>
/// High-level builder for creating a new archive from a source directory.
/// Disposable: <see cref="Dispose"/> rolls back if <see cref="Commit"/> wasn't called.
/// </summary>
public sealed class ArchiveBuilder : IDisposable
{
    private IntPtr _handle;
    private bool _committed;

    private ArchiveBuilder(IntPtr handle)
    {
        _handle = handle;
    }

    /// <summary>
    /// Open a new archive at <paramref name="destinationPath"/>.
    ///
    /// <paramref name="excludeSystemMetadata"/> (ABI v6, default <c>true</c>):
    /// when true, <c>.DS_Store</c>, <c>Thumbs.db</c>, <c>desktop.ini</c>,
    /// <c>$RECYCLE.BIN/</c>, <c>System Volume Information/</c> and other
    /// OS-noise files/folders are skipped during <see cref="AddDirectory"/>.
    /// </summary>
    public static ArchiveBuilder Create(
        string destinationPath,
        ArchiveFormat format,
        CompressionMethod compression = CompressionMethod.Deflate,
        byte compressionLevel = 3,
        bool solid = false,
        bool excludeSystemMetadata = true,
        string? password = null,
        EncryptionMethod encryption = EncryptionMethod.Aes256)
    {
        ArgumentException.ThrowIfNullOrEmpty(destinationPath);
        OtterzipLibrary.Initialize();

        // Empty/null password disables encryption regardless of the
        // requested method — sevenz-rust + zip backends both treat the
        // pair (None, no-password) as the only valid "no-encryption"
        // combination, and (anything, no-password) as a backend error.
        bool hasPassword = !string.IsNullOrEmpty(password);
        var effectiveEncryption = hasPassword ? encryption : EncryptionMethod.None;

        IntPtr handle;
        unsafe
        {
            var pathBytes = Encoding.UTF8.GetBytes(destinationPath);
            var passwordBytes = hasPassword
                ? Encoding.UTF8.GetBytes(password!)
                : Array.Empty<byte>();
            fixed (byte* p = pathBytes)
            fixed (byte* pwd = passwordBytes)
            {
                var opts = new OtterzipCreateOptions
                {
                    Format = (uint)format,
                    Compression = (uint)compression,
                    CompressionLevel = compressionLevel,
                    Solid = solid ? (byte)1 : (byte)0,
                    PreservePermissions = 1,
                    PreserveTimestamps = 1,
                    Encryption = (uint)effectiveEncryption,
                    PasswordUtf8 = hasPassword ? pwd : null,
                    PasswordLen = (nuint)passwordBytes.Length,
                    VolumeSizeBytes = 0,
                    ThreadCount = 0,
                    ExcludeSystemMetadata = excludeSystemMetadata ? (byte)1 : (byte)0,
                };
                int rc = NativeMethods.ArchiveCreate(p, (nuint)pathBytes.Length, &opts, out handle);
                ThrowIfError(rc);
            }
        }
        return new ArchiveBuilder(handle);
    }

    /// <summary>Append one file to the archive.</summary>
    public void AddFile(string sourcePath, string entryPath)
    {
        EnsureOpen();
        ArgumentException.ThrowIfNullOrEmpty(sourcePath);
        ArgumentException.ThrowIfNullOrEmpty(entryPath);
        unsafe
        {
            var srcBytes = Encoding.UTF8.GetBytes(sourcePath);
            var entryBytes = Encoding.UTF8.GetBytes(entryPath);
            fixed (byte* sp = srcBytes)
            fixed (byte* ep = entryBytes)
            {
                int rc = NativeMethods.ArchiveAddFile(
                    _handle,
                    sp, (nuint)srcBytes.Length,
                    ep, (nuint)entryBytes.Length);
                ThrowIfError(rc);
            }
        }
    }

    /// <summary>Append a directory recursively under <paramref name="entryPrefix"/>.</summary>
    public void AddDirectory(string sourceDirectory, string entryPrefix = "")
        => AddDirectory(sourceDirectory, entryPrefix, progress: null, cancellationToken: default);

    /// <summary>
    /// Append a directory recursively with optional progress + cancel
    /// callback. Routes through ABI v7's <c>otterzip_archive_add_directory_p</c>
    /// — the native side ticks the callback after each file written
    /// (Phase Scanning once + Writing per-entry) and aborts with
    /// <see cref="OperationCanceledException"/> as soon as the token is
    /// signalled OR the progress callback returns non-zero.
    /// </summary>
    public void AddDirectory(
        string sourceDirectory,
        string entryPrefix,
        IProgress<ProgressUpdate>? progress,
        CancellationToken cancellationToken)
    {
        EnsureOpen();
        ArgumentException.ThrowIfNullOrEmpty(sourceDirectory);

        // Wire IProgress + CT into the unmanaged callback via the same
        // ProgressBridge the extract path uses. The bridge's
        // [UnmanagedCallersOnly] trampoline is AOT-safe; GCHandle keeps
        // the managed instance reachable for the duration of the call.
        var bridge = new Archive.ProgressBridge(progress, cancellationToken);
        var bridgeHandle = GCHandle.Alloc(bridge);
        try
        {
            unsafe
            {
                var srcBytes = Encoding.UTF8.GetBytes(sourceDirectory);
                var prefixBytes = Encoding.UTF8.GetBytes(entryPrefix);
                fixed (byte* sp = srcBytes)
                fixed (byte* pp = prefixBytes)
                {
                    int rc = NativeMethods.ArchiveAddDirectoryP(
                        _handle,
                        sp, (nuint)srcBytes.Length,
                        pp, (nuint)prefixBytes.Length,
                        progressCb: &Archive.ProgressBridge.UnmanagedTrampoline,
                        userData: GCHandle.ToIntPtr(bridgeHandle));
                    if (rc == -30 /* OperationCanceled */)
                    {
                        cancellationToken.ThrowIfCancellationRequested();
                        throw new OperationCanceledException("otterzip compress canceled");
                    }
                    ThrowIfError(rc);
                }
            }
        }
        finally
        {
            bridgeHandle.Free();
        }
    }

    /// <summary>Finalise the archive on disk. After this call the builder is closed.</summary>
    public void Commit()
    {
        EnsureOpen();
        // Native side reclaims and frees the handle.
        int rc = NativeMethods.ArchiveCommit(_handle);
        _handle = IntPtr.Zero;
        _committed = true;
        ThrowIfError(rc);
    }

    /// <summary>
    /// One-shot helper: create + add directory + commit on a background thread.
    /// </summary>
    public static Task<ArchiveBuildReport> CreateFromDirectoryAsync(
        string destinationPath,
        string sourceDirectory,
        ArchiveFormat format,
        CompressionMethod compression = CompressionMethod.Deflate,
        byte compressionLevel = 3,
        IProgress<ProgressUpdate>? progress = null,
        bool excludeSystemMetadata = true,
        string? password = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrEmpty(destinationPath);
        ArgumentException.ThrowIfNullOrEmpty(sourceDirectory);

        return Task.Run(() =>
        {
            cancellationToken.ThrowIfCancellationRequested();
            var startTicks = Environment.TickCount64;

            using var builder = Create(
                destinationPath, format, compression, compressionLevel,
                solid: false,
                excludeSystemMetadata: excludeSystemMetadata,
                password: password);

            // ABI v7: the native side now drives a real Progress sink that
            // reports entries / bytes per file and checks the cancel
            // callback after every entry, so we can stop mid-write.
            builder.AddDirectory(sourceDirectory, entryPrefix: "", progress, cancellationToken);

            cancellationToken.ThrowIfCancellationRequested();
            builder.Commit();

            long elapsed = Environment.TickCount64 - startTicks;
            ulong outputSize = 0;
            try
            {
                outputSize = (ulong)new FileInfo(destinationPath).Length;
            }
            catch
            {
                // Non-fatal: caller can still see the file.
            }
            return new ArchiveBuildReport
            {
                ElapsedMs = (ulong)Math.Max(elapsed, 0),
                BytesWritten = outputSize,
            };
        }, cancellationToken);
    }

    public void Dispose()
    {
        if (_handle != IntPtr.Zero && !_committed)
        {
            // Best-effort rollback. We deliberately ignore the rc — the
            // Dispose contract forbids exceptions, and there's no
            // recovery path for a failed cleanup beyond logging.
            int rc = NativeMethods.ArchiveRollback(_handle);
            _ = rc;
            _handle = IntPtr.Zero;
        }
    }

    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private void EnsureOpen()
    {
        ObjectDisposedException.ThrowIf(_handle == IntPtr.Zero, this);
    }

    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void ThrowIfError(int rc)
    {
        if (rc != 0)
        {
            throw new OtterzipException(rc, NativeMethods.LastErrorMessage() ?? $"FFI error rc={rc}");
        }
    }
}

public enum ArchiveBuildPhase
{
    Scanning,
    Writing,
    Finalizing,
}

public sealed class ArchiveBuildProgress
{
    public ArchiveBuildPhase Phase { get; init; }
}

public sealed class ArchiveBuildReport
{
    public ulong BytesWritten { get; init; }
    public ulong ElapsedMs { get; init; }
}
