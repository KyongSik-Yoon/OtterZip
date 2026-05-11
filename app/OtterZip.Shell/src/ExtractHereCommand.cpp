// OtterZip.Shell — IExplorerCommand "Extract here" — Sprint 4 scaffold.
//
// Real Invoke spawns `OtterZip.exe --invoke extract-here --files <paths>`;
// the rest of the methods produce localized strings and standard cmd
// state. Sprint 5 connects to .resw via Windows.ApplicationModel.Resources.

#include "ExtractHereCommand.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"

#include <shellapi.h>
#include <shlwapi.h>     // SHStrDupW
#include <pathcch.h>
#include <strsafe.h>

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell
{
    IFACEMETHODIMP ExtractHereCommand::GetTitle(IShellItemArray* /*items*/, LPWSTR* ppszName) noexcept
    {
        return SHStrDupW(L"Extract here (OtterZip)", ppszName);
    }

    IFACEMETHODIMP ExtractHereCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Use the host EXE's icon resource (Sprint 5 wires actual asset).
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }

    IFACEMETHODIMP ExtractHereCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Extract this archive next to its current folder.", ppszInfotip);
    }

    IFACEMETHODIMP ExtractHereCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(ExtractHereCommand);
        return S_OK;
    }

    IFACEMETHODIMP ExtractHereCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (!pCmdState) return E_POINTER;
        // PR-7E: master toggle from Settings → Integration tab.
        // When OFF the verb is hidden entirely so Explorer doesn't even
        // show "OtterZip" in the right-click menu.
        if (!IsShellMenuEnabled())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        // Nested-mode wiring (mirrors CompressCommand): hide top-level
        // when the parent submenu is the active layout.
        if (IsShellMenuNested())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        // Show only when at least one item is selected. Detection of
        // archive-vs-non-archive happens in the manifest's FileType
        // association, so when this verb fires the items are already
        // archives by registration.
        DWORD count = 0;
        if (items && SUCCEEDED(items->GetCount(&count)) && count > 0)
        {
            *pCmdState = ECS_ENABLED;
        }
        else
        {
            *pCmdState = ECS_HIDDEN;
        }
        return S_OK;
    }

    IFACEMETHODIMP ExtractHereCommand::Invoke(IShellItemArray* items, IBindCtx* /*pbc*/) noexcept
    {
        // Spawn the host app with `--invoke extract-here --files "..."`.
        // The host's CLI handler (App.OnLaunched) takes over from there.
        return InvokeHostApp(items, L"extract-here");
    }

    IFACEMETHODIMP ExtractHereCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        // ECF_DEFAULT is `0` — i.e. "no special flags". Explorer decides
        // the default verb from the order in `FileExplorerContextMenus`,
        // not from this flag. The Settings_ShellExtractHereAsDefault
        // toggle therefore can't be enforced here without a manifest
        // edit; we keep the toggle UI but document the limitation in
        // the integration tab.
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }

    IFACEMETHODIMP ExtractHereCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }
}
