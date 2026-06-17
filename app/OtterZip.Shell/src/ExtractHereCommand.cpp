// OtterZip.Shell — IExplorerCommand "Extract here" — Sprint 4 scaffold.
//
// Real Invoke spawns `OtterZip.exe --invoke extract-here --files <paths>`;
// the rest of the methods produce localized strings and standard cmd
// state. Sprint 5 connects to .resw via Windows.ApplicationModel.Resources.

#include "ExtractHereCommand.h"
#include "ArchiveDetection.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <shellapi.h>
#include <shlwapi.h>     // SHStrDupW
#include <pathcch.h>
#include <strsafe.h>

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell
{
    IFACEMETHODIMP ExtractHereCommand::GetTitle(IShellItemArray* /*items*/, LPWSTR* ppszName) noexcept
    {
        // i18n: looked up at every hover via cached ResourceLoader. The
        // fallback is English so non-localized installs (e.g. a fresh
        // dev cert with no language tag in package identity) still get
        // a readable label. v0.15-v0.19 hardcoded this literal — Korean
        // testers saw "Extract here (OtterZip)" in English while the
        // Compress verbs (also hardcoded, but in Korean) showed in
        // Hangul. ResourceLoader heals the inconsistency end-to-end.
        std::wstring title = LoadShellString(L"Shell_ExtractHere_Title", L"Extract here (OtterZip)");
        return SHStrDupW(title.c_str(), ppszName);
    }

    IFACEMETHODIMP ExtractHereCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Brand-icon path: every top-level OtterZip verb shows the same
        // multi-resolution AppIcon.ico so the menu reads as a coherent
        // OtterZip cluster (Bandizip / 7-Zip do the same with their own
        // brand marks). `GetPackagedAssetPath` returns an absolute path
        // resolved against `Package.Current.InstalledLocation` because
        // Explorer's surrogate has no idea about our package's relative
        // tree. Empty path = falls back to E_NOTIMPL.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }

    IFACEMETHODIMP ExtractHereCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_ExtractHere_Tooltip",
            L"Extract this archive next to its current folder.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
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
        // Bandizip parity (spec §E): Extract verbs only render when
        // EVERY selected item is an archive. The manifest's `.zip` /
        // `.7z` / etc. ItemType matches a multi-select if ANY item
        // carries that extension, so a mixed selection (sample.zip +
        // readme.txt) would otherwise show both Compress AND Extract
        // verbs simultaneously — confusing because the verb only
        // makes sense for the archive items. We hide here so the
        // mixed-selection menu shows Compress verbs only, just like
        // Bandizip's behavior captured in `docs/02-design/shell-context-
        // menu-spec.md` §1.4.
        if (!ItemsAreAllArchives(items))
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        *pCmdState = ECS_ENABLED;
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
