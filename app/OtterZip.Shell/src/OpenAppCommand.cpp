// OtterZip.Shell — implementation of the empty-space "OtterZip 열기" verb.

#include "OpenAppCommand.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <shlwapi.h>   // SHStrDupW
#include <string>

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell
{
    IFACEMETHODIMP OpenAppCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        std::wstring title = LoadShellString(L"Shell_OpenApp_Title", L"OtterZip 열기");
        return SHStrDupW(title.c_str(), ppszName);
    }

    IFACEMETHODIMP OpenAppCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon == nullptr)
        {
            return E_POINTER;
        }
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }

    IFACEMETHODIMP OpenAppCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tip = LoadShellString(L"Shell_OpenApp_Tooltip", L"OtterZip 창을 엽니다.");
        return SHStrDupW(tip.c_str(), ppszInfotip);
    }

    IFACEMETHODIMP OpenAppCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (pguidCommandName == nullptr)
        {
            return E_POINTER;
        }
        *pguidCommandName = __uuidof(OpenAppCommand);
        return S_OK;
    }

    IFACEMETHODIMP OpenAppCommand::GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (pCmdState == nullptr)
        {
            return E_POINTER;
        }
        // Master gate only; registered exclusively for Directory\Background.
        *pCmdState = IsShellMenuEnabled() ? ECS_ENABLED : ECS_HIDDEN;
        return S_OK;
    }

    IFACEMETHODIMP OpenAppCommand::Invoke(IShellItemArray*, IBindCtx*) noexcept
    {
        // Launch the OtterZip main window with no arguments — App.OnLaunched
        // sees no --invoke verb and shows MainWindow (the drop zone).
        return LaunchHostApp();
    }

    IFACEMETHODIMP OpenAppCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (pFlags == nullptr)
        {
            return E_POINTER;
        }
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }

    IFACEMETHODIMP OpenAppCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum != nullptr)
        {
            *ppEnum = nullptr;
        }
        return E_NOTIMPL;
    }
}
