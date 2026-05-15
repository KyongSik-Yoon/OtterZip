// OtterZip.Shell — submenu wrapper IExplorerCommand instances + the
// IEnumExplorerCommand that yields them. Used by OtterzipMenuCommand
// when Settings_ShellMenuMode == "nested".
//
// Why wrappers instead of reusing CompressCommand / ExtractHereCommand
// directly: those classes' GetState methods hide themselves when nested
// mode is on (so they don't double-render at top level). Inside the
// submenu we always want them visible, regardless of mode. The wrappers
// share the same Invoke() routing (-> InvokeHostApp) but report
// ECS_ENABLED unconditionally.

#pragma once

#include <windows.h>
#include <shobjidl_core.h>
#include <winrt/base.h>

namespace OtterZip::Shell
{
    // Compress submenu child — always visible inside the parent submenu.
    // Wraps the "OtterZip으로 압축..." dialog flow.
    struct SubmenuCompressCommand : winrt::implements<SubmenuCompressCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    // Bandizip-style direct ZIP quick-compress, inside the nested
    // submenu. Same Invoke routing as the top-level
    // CompressZipQuickCommand but always-visible (no flat/nested
    // gate) — the parent menu's own gate has already fired.
    struct SubmenuCompressZipCommand : winrt::implements<SubmenuCompressZipCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    // Bandizip-style direct 7z quick-compress, inside the nested
    // submenu. Same Invoke routing as the top-level
    // CompressSevenZQuickCommand.
    struct SubmenuCompressSevenZCommand : winrt::implements<SubmenuCompressSevenZCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    // Extract-here submenu child. Visibility is determined at runtime by
    // a quick filename-extension probe: hidden when none of the selected
    // items look like an archive we can open.
    struct SubmenuExtractHereCommand : winrt::implements<SubmenuExtractHereCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    // Two-element enumerator: yields the two submenu wrappers in order
    // (Compress, then ExtractHere). Implements the standard
    // IEnumExplorerCommand quartet (Next / Skip / Reset / Clone).
    struct SubmenuEnumerator : winrt::implements<SubmenuEnumerator, IEnumExplorerCommand>
    {
        IFACEMETHODIMP Next(ULONG celt, IExplorerCommand** apUICommand, ULONG* pceltFetched) noexcept override;
        IFACEMETHODIMP Skip(ULONG celt) noexcept override;
        IFACEMETHODIMP Reset() noexcept override;
        IFACEMETHODIMP Clone(IEnumExplorerCommand** ppenum) noexcept override;

    private:
        // Submenu order matches the top-level flat layout:
        //   0 = ZIP quick   ("<name>.zip으로 압축")
        //   1 = 7z quick    ("<name>.7z으로 압축")
        //   2 = Compress... ("OtterZip으로 압축…")
        //   3 = Extract here (visible only on archive selections)
        ULONG m_index = 0;
        static constexpr ULONG kCount = 4;
    };
}
