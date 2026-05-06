// SpanZIP.Shell — runtime settings probe.
//
// `IExplorerCommand::GetState` is called on every right-click. Reading
// `ApplicationData::LocalSettings` synchronously each time would tax
// Explorer's responsiveness, so we cache the answer for a short window
// (5 seconds) and serve the cached value on subsequent calls.
//
// All keys live under the same store the C# Settings dialog writes
// (ApplicationData::Current().LocalSettings()), so the user's toggle in
// the SpanZIP UI propagates here automatically — no IPC, no registry.

#pragma once

#include <string>

namespace SpanZIP::Shell
{
    /// True when `Settings_ShellMenuEnabled` is set (or absent → default ON).
    /// Reads the SpanZIP package's LocalSettings under a 5 s cache.
    bool IsShellMenuEnabled() noexcept;

    /// True when `Settings_ShellExtractHereAsDefault` is set (default ON).
    bool IsExtractHereDefault() noexcept;
}
