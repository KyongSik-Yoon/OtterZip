// OtterZip.Shell — archive-extension detection helpers shared by every
// IExplorerCommand that needs to gate visibility on archive selections.
//
// Bandizip parity (2026-05-19): the right-click menu on an archive shows
// only Extract verbs, never Compress (you don't compress an archive).
// Our manifest registers Compress* under `Type="*"` for files and
// `Directory`/`Directory\Background` for folders — the wildcard matches
// archive extensions too, so we filter at GetState time instead.
//
// The extension list is the union of every format the
// FileExplorerContextMenus block registers archive ItemTypes for; keep
// this header in sync if you add a new archive format to the manifest.

#pragma once

#include <windows.h>
#include <shobjidl_core.h>
#include <string>
#include <string_view>

namespace OtterZip::Shell
{
    /// Returns true if the path's lowercase extension is on the
    /// archive-format allow list (.zip, .7z, .rar, .tar, .gz, .iso, …).
    /// Pure string comparison — no filesystem I/O.
    bool HasArchiveExtension(std::wstring_view path) noexcept;

    /// Returns true if **any** item in the selection carries an
    /// archive extension. Used by SubmenuExtractHereCommand to surface
    /// the submenu "Extract here" verb when archives are in the
    /// selection.
    ///
    /// `items == nullptr` or empty selection returns `false`.
    bool ItemsContainAnyArchive(IShellItemArray* items) noexcept;

    /// Returns true only when **every** item in the selection carries
    /// an archive extension (and there is at least one item).
    /// Used by Compress* GetState — Bandizip parity (`docs/02-design/
    /// shell-context-menu-spec.md` §E): mixed selections of archive +
    /// non-archive items still show Compress verbs because the user
    /// likely intends to pack the lot, not extract them.
    ///
    /// `items == nullptr` or empty selection returns `false`
    /// (Compress verbs stay visible on background right-click).
    bool ItemsAreAllArchives(IShellItemArray* items) noexcept;
}
