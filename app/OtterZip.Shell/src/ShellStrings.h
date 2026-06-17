// OtterZip.Shell — locale-aware string lookup for IExplorerCommand
// titles / tooltips.
//
// The shell DLL is loaded into Explorer's packaged-COM surrogate process,
// which carries the host package's identity. That means
// `Windows::ApplicationModel::Resources::ResourceLoader` resolves against
// the same `Resources.pri` the host EXE uses, so any key defined in the
// `.resw` files under `app/OtterZip.App/Strings/<locale>` is reachable
// from here.
//
// We funnel every user-facing string through `LoadShellString` so:
//   1. Korean / Japanese / English-locale users see correctly localized
//      verbs in the right-click menu (the v0.15-v0.19 packages had every
//      verb hardcoded English/Korean — confusing for non-KR users).
//   2. Per-key debug-trace lives in one place (the fallback path returns
//      the literal default if the resource lookup throws, which can
//      happen during the very first verb hover before MRT spins up).

#pragma once

#include <string>

namespace OtterZip::Shell
{
    // Look up `key` in the package's `Resources.pri`. Returns the
    // localized value (NUL-trimmed, UTF-16) if available; otherwise
    // returns `fallback` unchanged. The lookup is `noexcept` so verb
    // titles never throw into Explorer's RPC path — a missing key
    // surfaces as the English fallback rather than ECF_HIDDEN.
    //
    // Key shape: `<area>_<verb>` (e.g. `Shell_ExtractHere_Title`).
    // Defined in `Strings/<locale>/Resources.resw` under the parent
    // `app/OtterZip.App/Strings/` tree so the same .pri the host EXE
    // builds is loaded by the surrogate.
    std::wstring LoadShellString(wchar_t const* key, std::wstring const& fallback) noexcept;
}
