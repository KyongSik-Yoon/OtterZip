// SpanZIP.Shell — IExplorerCommand "Compress" — Sprint 4 scaffold.

#include "CompressCommand.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"

#include <shellapi.h>
#include <shlwapi.h>     // SHStrDupW

#pragma comment(lib, "Shlwapi.lib")

namespace SpanZIP::Shell
{
    IFACEMETHODIMP CompressCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        return SHStrDupW(L"Compress with SpanZIP", ppszName);
    }

    IFACEMETHODIMP CompressCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }

    IFACEMETHODIMP CompressCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Create a new archive from the selected files.", ppszInfotip);
    }

    IFACEMETHODIMP CompressCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(CompressCommand);
        return S_OK;
    }

    IFACEMETHODIMP CompressCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (!pCmdState) return E_POINTER;
        // PR-7E: master Settings_ShellMenuEnabled gate.
        if (!IsShellMenuEnabled())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
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

    IFACEMETHODIMP CompressCommand::Invoke(IShellItemArray* items, IBindCtx* /*pbc*/) noexcept
    {
        return InvokeHostApp(items, L"compress");
    }

    IFACEMETHODIMP CompressCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }

    IFACEMETHODIMP CompressCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }
}
