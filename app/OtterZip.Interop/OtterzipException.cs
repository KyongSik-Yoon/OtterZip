using System;

namespace OtterZip.Interop;

/// <summary>
/// Thrown when a native OtterZip call returns a non-zero error code.
/// Maps to <c>OtterzipErrorCode</c> from <c>ffi-api.md</c> §2.1.
/// </summary>
/// <summary>FFI error code constants — mirror <c>otterzip-ffi/src/error.rs</c>.</summary>
public static class OtterzipErrorCodes
{
    public const int WrongPassword = -22;
    public const int OperationCanceled = -30;
    public const int FeatureDisabled = -40;
    public const int PathTraversal = -41;
    public const int ZipBomb = -42;

    /// <summary>
    /// App-side sentinel (NOT a native code — native codes are all &lt;= 0).
    /// Marks an <see cref="OtterzipException"/> whose <see cref="Exception.Message"/>
    /// is ALREADY a finished, localized, user-facing string that must be shown
    /// verbatim. The error-localization layer (<c>ErrorMessages.Localize</c>)
    /// passes these through instead of replacing them with a generic headline.
    /// Use for app-synthesized failures like "spanned 7z not yet supported".
    /// </summary>
    public const int AlreadyLocalized = 1;
}

public sealed class OtterzipException : Exception
{
    public int ErrorCode { get; }

    /// <summary>True when the native call failed because of a missing/incorrect password.</summary>
    public bool IsWrongPassword => ErrorCode == OtterzipErrorCodes.WrongPassword;

    public OtterzipException() : base() { }

    public OtterzipException(string message) : base(message) { }

    public OtterzipException(string message, Exception innerException) : base(message, innerException) { }

    public OtterzipException(int code, string message) : base(message)
    {
        ErrorCode = code;
    }

    public OtterzipException(int code, string message, Exception inner) : base(message, inner)
    {
        ErrorCode = code;
    }

    internal static void ThrowIfError(int rc, string contextMessage = "OtterZip native call failed")
    {
        if (rc == 0) return;
        string nativeMsg = Native.NativeMethods.LastErrorMessage() ?? string.Empty;
        string message = string.IsNullOrEmpty(nativeMsg)
            ? contextMessage
            : $"{contextMessage}: {nativeMsg}";
        throw new OtterzipException(rc, message);
    }
}
