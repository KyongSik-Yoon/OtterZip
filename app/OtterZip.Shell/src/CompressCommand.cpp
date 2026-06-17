// OtterZip.Shell — IExplorerCommand "Compress" — Sprint 4 scaffold.

#include "CompressCommand.h"
#include "ArchiveDetection.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <shellapi.h>
#include <shlwapi.h>     // SHStrDupW

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell
{
    IFACEMETHODIMP CompressCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        // Generic "dialog-flow" compress verb. Bandizip-style direct
        // format-specific entries (.zip / .7z) live in
        // QuickCompressCommands; this one keeps the trailing ellipsis
        // + the OtterZip brand label so users know it opens the full
        // MainWindow with the format dropdown.
        //
        // i18n: ResourceLoader-resolved at every hover. Korean fallback
        // matches the v0.15-v0.19 hardcoded string so existing testers
        // see the same label they're used to even if MRT briefly fails.
        std::wstring title = LoadShellString(
            L"Shell_CompressDialog_Title",
            L"OtterZip으로 압축...(&O)");
        return SHStrDupW(title.c_str(), ppszName);
    }

    IFACEMETHODIMP CompressCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // See ExtractHereCommand::GetIcon for the rationale. Same brand
        // mark on every top-level verb is the Bandizip / 7-Zip pattern.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }

    IFACEMETHODIMP CompressCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_CompressDialog_Tooltip",
            L"Create a new archive from the selected files.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
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
        // Nested-mode wiring: when the user picked the "OtterZip ▶
        // {Compress, Extract}" layout we hide the top-level Compress
        // verb so it doesn't double-render alongside the parent menu.
        // The submenu's SubmenuCompressCommand carries the same Invoke
        // path inside the parent.
        if (IsShellMenuNested())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        // Bandizip parity (spec §E): hide the dialog-flow compress
        // verb only when EVERY selected item is an archive. Mixed
        // selections still surface Compress so the user can pack
        // mixed bags. See ArchiveDetection.h.
        if (ItemsAreAllArchives(items))
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
