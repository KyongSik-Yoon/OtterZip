// OtterZip.Shell — Win32-only fast cache for the menu-build hot path.
//
// Why this exists (2026-06-18 cold-boot fix):
//   The IExplorerCommand menu-build methods (GetState / GetTitle / GetIcon)
//   originally read LocalSettings / ResourceLoader / Package.InstalledLocation
//   — all WinRT calls that require the package's AppX activation context.
//   Explorer establishes that context lazily on the FIRST packaged-handler
//   use after boot, and that one-time cost blows the Windows 11 modern
//   context-menu timeout → our verbs are dropped from the first menu and
//   only appear on the second right-click. The in-DllGetClassObject warmup
//   couldn't fix it because it runs *inside* that same first build.
//
//   This reader serves the same settings + localized strings from a small
//   file the host app mirrors into the package LocalState. It is read with
//   pure Win32 (GetCurrentPackageFamilyName + %LOCALAPPDATA% + file I/O),
//   so it never touches the AppX activation context — the first menu build
//   after boot stays under the timeout and the verbs appear immediately.
//
// Layering contract: these are a fast-path placed IN FRONT of the existing
// WinRT readers. A `false` return (cache absent / key missing / any error)
// means the caller must fall through to its current LocalSettings /
// ResourceLoader logic, so the worst case is exactly today's behavior —
// no regression. The cache becomes effective once the host app has run at
// least once (the file persists across reboots).

#pragma once

#include <string>

namespace OtterZip::Shell
{
    // Look up a cached value. Returns false when the cache file can't be
    // located/read or the key isn't present. `key` is the verbatim key the
    // host app wrote (e.g. L"Shell_CompressDialog_Title", L"ShellMenuMode").
    bool TryGetCachedString(wchar_t const* key, std::wstring& out) noexcept;

    // Convenience bool reader ("1" => true, anything else => false).
    bool TryGetCachedBool(wchar_t const* key, bool& out) noexcept;
}
