using System;

namespace SpanZIP.Interop;

/// <summary>
/// Thrown when a native SpanZIP call returns a non-zero error code.
/// Maps to <c>SpanzipErrorCode</c> from <c>ffi-api.md</c> §2.1.
/// </summary>
/// <summary>FFI error code constants — mirror <c>spanzip-ffi/src/error.rs</c>.</summary>
public static class SpanzipErrorCodes
{
    public const int WrongPassword = -22;
    public const int OperationCanceled = -30;
    public const int FeatureDisabled = -40;
    public const int PathTraversal = -41;
    public const int ZipBomb = -42;
}

public sealed class SpanzipException : Exception
{
    public int ErrorCode { get; }

    /// <summary>True when the native call failed because of a missing/incorrect password.</summary>
    public bool IsWrongPassword => ErrorCode == SpanzipErrorCodes.WrongPassword;

    public SpanzipException() : base() { }

    public SpanzipException(string message) : base(message) { }

    public SpanzipException(string message, Exception innerException) : base(message, innerException) { }

    public SpanzipException(int code, string message) : base(message)
    {
        ErrorCode = code;
    }

    public SpanzipException(int code, string message, Exception inner) : base(message, inner)
    {
        ErrorCode = code;
    }

    internal static void ThrowIfError(int rc, string contextMessage = "SpanZIP native call failed")
    {
        if (rc == 0) return;
        string nativeMsg = Native.NativeMethods.LastErrorMessage() ?? string.Empty;
        string message = string.IsNullOrEmpty(nativeMsg)
            ? contextMessage
            : $"{contextMessage}: {nativeMsg}";
        throw new SpanzipException(rc, message);
    }
}
