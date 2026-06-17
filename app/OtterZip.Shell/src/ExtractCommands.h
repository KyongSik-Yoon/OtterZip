// OtterZip.Shell — extract verbs that surface only on archive selections.
//
// Bandizip parity (2026-05-19 spec):
//   * `ExtractHereCommand` (existing) — `여기에 풀기` — extract into
//      the current folder.
//   * `ExtractSmartCommand` — `알아서 풀기` — peek the archive's root
//      layout; if there's a single top-level directory drop entries
//      flat, otherwise wrap in `<stem>/`. Bandizip's `알아서 풀기` is
//      the same heuristic.
//   * `ExtractToSubfolderCommand` — `<stem>\에 풀기` — always wrap in
//      `<stem>/`, collision-resolved by host-side OutputNamer.
//   * `ExtractDialogCommand` — `OtterZip으로 풀기...(B)` — open the
//      standalone `ExtractOptionsDialog` window for destination /
//      overwrite policy / password input.
//
// All four share the same gates (`IsShellMenuEnabled`,
// `IsShellMenuNested`-aware) but differ in title format + Invoke
// payload. The host (`App.OnLaunched`) routes each `--invoke` verb to
// its respective code path.

#pragma once

#include <windows.h>
#include <shobjidl_core.h>
#include <winrt/base.h>

namespace OtterZip::Shell
{
    // --------------------------- ExtractSmart -----------------------
    // CLSID matches the `desktop5:Verb` entry `OtterzipExtractSmart`
    // in Package.appxmanifest. Registered against every archive
    // ItemType.
    struct __declspec(uuid("77777777-2222-3333-4444-555555555555"))
    ExtractSmartCommand : winrt::implements<ExtractSmartCommand, IExplorerCommand>
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

    // ------------------------ ExtractToSubfolder --------------------
    struct __declspec(uuid("88888888-2222-3333-4444-555555555555"))
    ExtractToSubfolderCommand : winrt::implements<ExtractToSubfolderCommand, IExplorerCommand>
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

    // -------------------------- ExtractDialog -----------------------
    struct __declspec(uuid("99999999-2222-3333-4444-555555555555"))
    ExtractDialogCommand : winrt::implements<ExtractDialogCommand, IExplorerCommand>
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
}
