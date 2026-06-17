// OtterZip.Shell — packaged asset path resolution for IExplorerCommand
// icon paths.
//
// `IExplorerCommand::GetIcon` expects an ABSOLUTE path (or a comma-
// suffixed resource locator) — relative paths inside the package don't
// resolve in Explorer's surrogate process. We ask the WinRT
// `Package.Current.InstalledLocation` for the install root at first
// use and cache it for the DLL lifetime.

#pragma once

#include <string>

namespace OtterZip::Shell
{
    // Returns the absolute filesystem path to a packaged asset, e.g.
    //   GetPackagedAssetPath(L"Assets\\AppIcon.ico")
    // -> L"C:\\Program Files\\WindowsApps\\LumiBearStudio.OtterZip_..._x64__.../Assets/AppIcon.ico"
    //
    // Returns an empty string on failure (no package identity, WinRT
    // throws, etc.). Callers should treat empty as "no icon" and fall
    // back to E_NOTIMPL so Explorer skips icon rendering instead of
    // showing a broken-image placeholder.
    //
    // `relative` may use either '/' or '\\' separators — the helper
    // does not normalize, it concatenates with a backslash separator
    // which Windows shell APIs accept either way.
    std::wstring GetPackagedAssetPath(wchar_t const* relative) noexcept;
}
