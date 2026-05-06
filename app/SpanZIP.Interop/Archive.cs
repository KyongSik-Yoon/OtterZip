// SpanZIP.Interop — managed Archive facade over the native handle.
//
// Mirrors spanzip_core::Archive but with .NET ergonomics:
//   - IDisposable for handle lifecycle
//   - async-friendly ExtractAllAsync wrapping the synchronous native call
//   - IProgress<T> for UI thread marshaling

using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

using SpanZIP.Interop.Native;

namespace SpanZIP.Interop;

/// <summary>
/// Managed wrapper around a native <c>SpanzipArchive</c> handle.
/// Single-threaded by contract — do not share an instance across threads.
/// </summary>
public sealed class Archive : IDisposable
{
    private IntPtr _handle;
    private readonly string _path;
    private readonly ArchiveFormat _format;

    private Archive(IntPtr handle, string path, ArchiveFormat format)
    {
        _handle = handle;
        _path = path;
        _format = format;
    }

    public string Path => _path;

    public ArchiveFormat Format => _format;

    /// <summary>Open an existing archive in read mode.</summary>
    public static Archive Open(string path) => OpenInternal(path, password: null);

    /// <summary>Open an encrypted archive with the given password.</summary>
    public static Archive OpenWithPassword(string path, string password)
    {
        ArgumentException.ThrowIfNullOrEmpty(password);
        return OpenInternal(path, password);
    }

    private static Archive OpenInternal(string path, string? password)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        SpanzipLibrary.Initialize();

        IntPtr handle;
        unsafe
        {
            var pathBytes = Encoding.UTF8.GetBytes(path);
            byte[]? pwBytes = password is null ? null : Encoding.UTF8.GetBytes(password);
            fixed (byte* p = pathBytes)
            fixed (byte* pw = pwBytes)
            {
                int rc = NativeMethods.ArchiveOpen(
                    p,
                    (nuint)pathBytes.Length,
                    mode: 0, // OpenMode::Read
                    passwordUtf8: pw,
                    passwordLen: pwBytes is null ? (nuint)0 : (nuint)pwBytes.Length,
                    out handle);
                ThrowIfError(rc);
            }
        }

        if (handle == IntPtr.Zero)
        {
            throw new SpanzipException(-1, "spanzip_archive_open returned OK but null handle");
        }
        if (NativeMethods.ArchiveFormat(handle, out uint fmt) != 0)
        {
            NativeMethods.ArchiveClose(handle);
            throw new SpanzipException(-1, "spanzip_archive_format failed");
        }
        return new Archive(handle, path, (ArchiveFormat)fmt);
    }

    /// <summary>Detect a file's archive format without opening it.</summary>
    public static ArchiveFormat DetectFormat(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        SpanzipLibrary.Initialize();

        unsafe
        {
            var pathBytes = Encoding.UTF8.GetBytes(path);
            fixed (byte* p = pathBytes)
            {
                int rc = NativeMethods.DetectFormat(p, (nuint)pathBytes.Length, out uint fmt);
                ThrowIfError(rc);
                return (ArchiveFormat)fmt;
            }
        }
    }

    /// <summary>Enumerate every entry in this archive.</summary>
    public IReadOnlyList<EntryInfo> ReadEntries()
    {
        EnsureOpen();
        IntPtr iter;
        ThrowIfError(NativeMethods.IteratorNew(_handle, out iter));
        try
        {
            var list = new List<EntryInfo>();
            unsafe
            {
                var view = default(SpanzipEntryView);
                while (true)
                {
                    int rc = NativeMethods.IteratorNext(iter, &view);
                    if (rc == -25 /* IteratorEnd */)
                    {
                        break;
                    }
                    ThrowIfError(rc);

                    var path = NativeMethods.Utf8(view.PathUtf8, view.PathLen) ?? string.Empty;
                    var comment = NativeMethods.Utf8(view.CommentUtf8, view.CommentLen);
                    list.Add(new EntryInfo
                    {
                        Path = path,
                        IsDirectory = view.IsDirectory != 0,
                        IsSymlink = view.IsSymlink != 0,
                        IsEncrypted = view.IsEncrypted != 0,
                        UncompressedSize = view.UncompressedSize,
                        CompressedSize = view.CompressedSize,
                        ModifiedUnixMs = view.ModifiedUnixMs,
                        Comment = comment,
                    });
                }
            }
            return list;
        }
        finally
        {
            NativeMethods.IteratorFree(iter);
        }
    }

    /// <summary>
    /// Extract every entry under <paramref name="destination"/>. Runs on a
    /// background thread; progress and cancellation are marshaled through
    /// .NET conventions.
    /// </summary>
    public Task<ExtractReport> ExtractAllAsync(
        string destination,
        OverwritePolicy overwrite = OverwritePolicy.Always,
        IProgress<ProgressUpdate>? progress = null,
        bool preserveZoneIdentifier = true,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrEmpty(destination);
        EnsureOpen();
        Directory.CreateDirectory(destination);

        return Task.Run(
            () => ExtractAllSync(destination, overwrite, progress, preserveZoneIdentifier, cancellationToken),
            cancellationToken);
    }

    private ExtractReport ExtractAllSync(
        string destination,
        OverwritePolicy overwrite,
        IProgress<ProgressUpdate>? progress,
        bool preserveZoneIdentifier,
        CancellationToken cancellationToken)
    {
        unsafe
        {
            var destBytes = Encoding.UTF8.GetBytes(destination);
            fixed (byte* destPtr = destBytes)
            {
                var opts = BuildDefaultExtractOptions(
                    destPtr, destBytes.Length, overwrite, preserveZoneIdentifier);
                return InvokeNativeExtract(&opts, progress, cancellationToken);
            }
        }
    }

    private static unsafe SpanzipExtractOptions BuildDefaultExtractOptions(
        byte* destPtr, int destLen, OverwritePolicy overwrite, bool preserveZoneIdentifier)
    {
        return new SpanzipExtractOptions
        {
            DestinationUtf8 = destPtr,
            DestinationLen = (nuint)destLen,
            OverwritePolicy = (uint)overwrite,
            FlattenPaths = 0,
            PreservePermissions = 1,
            PreserveTimestamps = 1,
            FollowSymlinks = 0,
            BlockPathTraversal = 1,
            PreserveZoneIdentifier = preserveZoneIdentifier ? (byte)1 : (byte)0,
            Reserved1 = 0,
            Reserved2 = 0,
            MaxCompressionRatio = 1000,
            MaxTotalCompressionRatio = 100,
            MaxTotalOutputBytes = 16UL * 1024 * 1024 * 1024,
            PasswordUtf8 = null,
            PasswordLen = 0,
            EntryFilterUtf8 = null,
            EntryFilterLen = 0,
        };
    }

    private unsafe ExtractReport InvokeNativeExtract(
        SpanzipExtractOptions* opts,
        IProgress<ProgressUpdate>? progress,
        CancellationToken cancellationToken)
    {
        var bridge = new ProgressBridge(progress, cancellationToken);
        var bridgeHandle = GCHandle.Alloc(bridge);
        try
        {
            SpanzipExtractReport reportNative;
            int rc = NativeMethods.ArchiveExtractAll(
                _handle,
                opts,
                progressCb: &ProgressBridge.UnmanagedTrampoline,
                userData: GCHandle.ToIntPtr(bridgeHandle),
                outReport: &reportNative);

            if (rc == -30 /* OperationCanceled */)
            {
                cancellationToken.ThrowIfCancellationRequested();
                throw new OperationCanceledException("spanzip extraction canceled");
            }
            ThrowIfError(rc);

            return new ExtractReport
            {
                EntriesExtracted = reportNative.EntriesExtracted,
                EntriesSkipped = reportNative.EntriesSkipped,
                BytesWritten = reportNative.BytesWritten,
                WarningsCount = reportNative.WarningsCount,
                ElapsedMs = reportNative.ElapsedMs,
            };
        }
        finally
        {
            bridgeHandle.Free();
        }
    }

    /// <summary>
    /// Verifies every entry's CRC32 and returns the number of corrupted
    /// entries (0 = archive is healthy). Phase 6+ "Verify after compress"
    /// setting calls this synchronously after a successful create.
    ///
    /// Backed by FFI <c>spanzip_archive_test</c> (ABI v5+).
    /// </summary>
    public Task<TestReport> TestAsync(CancellationToken cancellationToken = default)
    {
        EnsureOpen();
        return Task.Run(() =>
        {
            cancellationToken.ThrowIfCancellationRequested();
            return TestSync();
        }, cancellationToken);
    }

    private unsafe TestReport TestSync()
    {
        SpanzipTestReport reportNative;
        int rc = NativeMethods.ArchiveTest(
            _handle,
            progressCb: null,
            userData: IntPtr.Zero,
            outReport: &reportNative);
        ThrowIfError(rc);
        return new TestReport
        {
            EntriesTested = reportNative.EntriesTested,
            EntriesCorrupted = reportNative.EntriesCorrupted,
            ElapsedMs = reportNative.ElapsedMs,
        };
    }

    public void Dispose()
    {
        if (_handle != IntPtr.Zero)
        {
            NativeMethods.ArchiveClose(_handle);
            _handle = IntPtr.Zero;
        }
    }

    private void EnsureOpen()
    {
        ObjectDisposedException.ThrowIf(_handle == IntPtr.Zero, this);
    }

    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void ThrowIfError(int rc)
    {
        if (rc != 0)
        {
            throw new SpanzipException(rc, NativeMethods.LastErrorMessage() ?? $"FFI error rc={rc}");
        }
    }

    /// <summary>
    /// Bridge holding an <see cref="IProgress{T}"/> + <see cref="CancellationToken"/>
    /// across the unmanaged callback boundary. AOT requires the trampoline to be
    /// a static <c>UnmanagedCallersOnly</c> method; we recover the bridge via
    /// <see cref="GCHandle"/>.
    /// </summary>
    private sealed class ProgressBridge
    {
        private readonly IProgress<ProgressUpdate>? _progress;
        private readonly CancellationToken _cancellationToken;

        public ProgressBridge(IProgress<ProgressUpdate>? progress, CancellationToken token)
        {
            _progress = progress;
            _cancellationToken = token;
        }

        public int OnProgress(in SpanzipProgressView view)
        {
            if (_cancellationToken.IsCancellationRequested)
            {
                return -1;
            }
            if (_progress is not null)
            {
                string? entry;
                unsafe
                {
                    entry = NativeMethods.Utf8(view.CurrentEntryUtf8, view.CurrentEntryLen);
                }
                _progress.Report(new ProgressUpdate
                {
                    BytesProcessed = view.BytesProcessed,
                    BytesTotal = view.BytesTotal,
                    EntriesProcessed = view.EntriesProcessed,
                    EntriesTotal = view.EntriesTotal,
                    CurrentEntry = entry,
                    Phase = (ProgressPhase)view.Phase,
                    ElapsedMs = view.ElapsedMs,
                });
            }
            return 0;
        }

        [UnmanagedCallersOnly(CallConvs = new[] { typeof(System.Runtime.CompilerServices.CallConvCdecl) })]
        public static unsafe int UnmanagedTrampoline(SpanzipProgressView* progress, IntPtr userData)
        {
            try
            {
                if (userData == IntPtr.Zero || progress == null)
                {
                    return 0;
                }
                var handle = GCHandle.FromIntPtr(userData);
                if (handle.Target is ProgressBridge bridge)
                {
                    return bridge.OnProgress(*progress);
                }
                return 0;
            }
            catch
            {
                // Never let a managed exception propagate into native code.
                return -1;
            }
        }
    }
}

/// <summary>Lightweight, copy-on-read snapshot of a single archive entry.</summary>
public sealed class EntryInfo
{
    public required string Path { get; init; }
    public bool IsDirectory { get; init; }
    public bool IsSymlink { get; init; }
    public bool IsEncrypted { get; init; }
    public ulong UncompressedSize { get; init; }
    public ulong CompressedSize { get; init; }
    public long ModifiedUnixMs { get; init; }
    public string? Comment { get; init; }
}
