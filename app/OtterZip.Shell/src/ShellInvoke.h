// OtterZip.Shell — helper for ShellExecuting the host app with --invoke args.
//
// Sprint 5: each IExplorerCommand::Invoke flattens the IShellItemArray
// into a `;`-separated UTF-8 path list and launches:
//
//   OtterZip.exe --invoke <verb> --files "<p1>;<p2>;…"
//
// Resolution of `OtterZip.exe`: in MSIX deployment we use the package
// family root + Application's Executable element; for development/local
// builds we resolve the host EXE next to this DLL.

#pragma once

#include <windows.h>
#include <shobjidl_core.h>
#include <string>
#include <string_view>

namespace OtterZip::Shell
{
    /// Spawn the host app with the given verb and the paths from `items`.
    /// Returns S_OK on successful launch, hresult of ShellExecute on failure.
    HRESULT InvokeHostApp(IShellItemArray* items, std::wstring_view verb) noexcept;
}
