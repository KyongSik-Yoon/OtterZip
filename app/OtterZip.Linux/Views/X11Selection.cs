// OtterZip for Linux — reading one XDND selection target straight from X11.
//
// Avalonia 12.1's X11 drag-and-drop returns the SAME buffer for every format
// requested during a drop (observed: x-kde4-urilist, vnd.portal.filetransfer
// and x-kde-source-id all come back as one identical 24-byte blob), so the real
// per-format data — including the portal transfer key KDE hands files over —
// can't be read through IDataTransfer. This goes to the X server directly for a
// single named target.
//
// It uses a PRIVATE display connection and a throwaway 1×1 requestor window, so
// it never touches Avalonia's event loop or its XDND state machine: X selection
// transfers are server-mediated, so any client window may request the data
// while the source still owns XdndSelection (which it does until the target
// sends XdndFinished — i.e. throughout the Drop handler). Best-effort: anything
// unexpected (not X11, libX11 missing, the source not answering in time) returns
// null and the caller falls back.

using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace OtterZip.Linux.Views;

internal static partial class X11Selection
{
    /// <summary>
    /// Convert the <c>XdndSelection</c> to <paramref name="targetFormat"/> and
    /// return the bytes as a UTF-8 string, or null on any failure. Intended to
    /// be called from within a Drop handler, while the drag source still owns
    /// the selection.
    /// </summary>
    public static string? Read(string targetFormat, int timeoutMs = 1500)
    {
        if (!OperatingSystem.IsLinux())
        {
            return null;
        }
        nint display = TryOpenDisplay();
        if (display == 0)
        {
            return null;
        }

        ulong window = 0;
        try
        {
            ulong root = XDefaultRootWindow(display);
            window = XCreateSimpleWindow(display, root, 0, 0, 1, 1, 0, 0, 0);
            if (window == 0)
            {
                return null;
            }
            ulong selection = XInternAtom(display, "XdndSelection", 0);
            ulong target = XInternAtom(display, targetFormat, 0);
            ulong property = XInternAtom(display, "OTTERZIP_DND", 0);
            if (selection == 0 || target == 0 || property == 0)
            {
                return null;
            }

            _ = XConvertSelection(display, selection, target, property, window, 0 /* CurrentTime */);
            _ = XFlush(display);
            if (!WaitForSelectionNotify(display, timeoutMs))
            {
                return null;
            }
            return ReadProperty(display, window, property);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            return null;
        }
        finally
        {
            if (window != 0)
            {
                _ = XDestroyWindow(display, window);
            }
            _ = XCloseDisplay(display);
        }
    }

    private static nint TryOpenDisplay()
    {
        try
        {
            return XOpenDisplay(null);
        }
        catch (DllNotFoundException)
        {
            return 0; // no libX11 — we are not on an X server
        }
    }

    private static bool WaitForSelectionNotify(nint display, int timeoutMs)
    {
        const int SelectionNotify = 31;
        nint eventBuffer = Marshal.AllocHGlobal(256);
        try
        {
            var clock = Stopwatch.StartNew();
            while (clock.ElapsedMilliseconds < timeoutMs)
            {
                _ = XPending(display); // pull server input into the queue
                if (XCheckTypedEvent(display, SelectionNotify, eventBuffer) != 0)
                {
                    return true;
                }
                System.Threading.Thread.Sleep(5);
            }
            return false;
        }
        finally
        {
            Marshal.FreeHGlobal(eventBuffer);
        }
    }

    private static string? ReadProperty(nint display, ulong window, ulong property)
    {
        int status = XGetWindowProperty(
            display, window, property,
            0, 0x40000, 1 /* delete */, 0 /* AnyPropertyType */,
            out _, out int format, out ulong nitems, out _, out nint prop);
        if (status != 0 || prop == 0 || nitems == 0)
        {
            return null;
        }
        try
        {
            int byteCount = format == 8 ? (int)nitems : (int)nitems * (format / 8);
            if (byteCount <= 0)
            {
                return null;
            }
            var bytes = new byte[byteCount];
            Marshal.Copy(prop, bytes, 0, byteCount);
            return Encoding.UTF8.GetString(bytes);
        }
        finally
        {
            _ = XFree(prop);
        }
    }

    private const string Lib = "libX11.so.6";

    [LibraryImport(Lib, StringMarshalling = StringMarshalling.Utf8)]
    private static partial nint XOpenDisplay(string? name);

    [LibraryImport(Lib)]
    private static partial int XCloseDisplay(nint display);

    [LibraryImport(Lib)]
    private static partial ulong XDefaultRootWindow(nint display);

    [LibraryImport(Lib, StringMarshalling = StringMarshalling.Utf8)]
    private static partial ulong XInternAtom(nint display, string name, int onlyIfExists);

    [LibraryImport(Lib)]
    private static partial ulong XCreateSimpleWindow(
        nint display, ulong parent, int x, int y, uint width, uint height,
        uint borderWidth, ulong border, ulong background);

    [LibraryImport(Lib)]
    private static partial int XDestroyWindow(nint display, ulong window);

    [LibraryImport(Lib)]
    private static partial int XConvertSelection(
        nint display, ulong selection, ulong target, ulong property, ulong requestor, ulong time);

    [LibraryImport(Lib)]
    private static partial int XFlush(nint display);

    [LibraryImport(Lib)]
    private static partial int XPending(nint display);

    [LibraryImport(Lib)]
    private static partial int XCheckTypedEvent(nint display, int eventType, nint eventReturn);

    [LibraryImport(Lib)]
    private static partial int XGetWindowProperty(
        nint display, ulong window, ulong property, nint longOffset, nint longLength,
        int delete, ulong reqType, out ulong actualType, out int actualFormat,
        out ulong nitems, out ulong bytesAfter, out nint prop);

    [LibraryImport(Lib)]
    private static partial int XFree(nint data);
}
