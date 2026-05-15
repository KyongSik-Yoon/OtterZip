// OtterZip.Shell — Bandizip-style quick-compress verbs.
//
// Two IExplorerCommand classes that produce the format-specific direct
// "DATA.zip으로 압축(&Z)" / "DATA.7z으로 압축(&7)" entries shown at the
// top of Explorer's right-click menu. The third "OtterZip으로
// 압축..." verb stays as `CompressCommand` since it still opens the
// host MainWindow (dialog flow).
//
// Each class registers under its own CLSID so the Package.appxmanifest
// can wire `<desktop4:Verb>` per shape. Both invocations route to the
// host EXE with a format-specific `--invoke compress-zip` /
// `--invoke compress-7z` arg, parsed by App.ParseInvokeArgs (host
// side) into `InvokeRequest { Verb="compress", QuickFormat=..., IsHeadless=true }`.

#pragma once

#include <windows.h>
#include <shobjidl_core.h>
#include <winrt/base.h>

namespace OtterZip::Shell
{
    struct __declspec(uuid("55555555-2222-3333-4444-555555555555"))
    CompressZipQuickCommand : winrt::implements<CompressZipQuickCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray* items, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray* items, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray* items, BOOL fOkToBeSlow, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx* pbc) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    struct __declspec(uuid("66666666-2222-3333-4444-555555555555"))
    CompressSevenZQuickCommand : winrt::implements<CompressSevenZQuickCommand, IExplorerCommand>
    {
        IFACEMETHODIMP GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept override;
        IFACEMETHODIMP GetIcon(IShellItemArray* items, LPWSTR* ppszIcon) noexcept override;
        IFACEMETHODIMP GetToolTip(IShellItemArray* items, LPWSTR* ppszInfotip) noexcept override;
        IFACEMETHODIMP GetCanonicalName(GUID* pguidCommandName) noexcept override;
        IFACEMETHODIMP GetState(IShellItemArray* items, BOOL fOkToBeSlow, EXPCMDSTATE* pCmdState) noexcept override;
        IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx* pbc) noexcept override;
        IFACEMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) noexcept override;
        IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept override;
    };

    /// Compute the basename Bandizip / 7-Zip / WinRAR show as the
    /// selection stem in the right-click verb. Single source = its
    /// own name; multi = first item's name (the host's OutputNamer
    /// has the full Settings_UseParentFolderName logic — the shell
    /// extension shows a preview hint so a small mismatch with the
    /// final filename is acceptable).
    std::wstring DeriveSelectionStem(IShellItemArray* items) noexcept;
}
