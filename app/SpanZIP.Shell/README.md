# SpanZIP.Shell — Windows Explorer Shell Extension (Sprint 4 scaffold)

C++/WinRT COM in-proc DLL exposing three `IExplorerCommand` verbs:

| Verb | CLSID | Action |
|------|-------|--------|
| `SpanzipExtractHere` | `{22222222-2222-3333-4444-555555555555}` | Extract archive to a sibling folder |
| `SpanzipExtractTo` | `{33333333-2222-3333-4444-555555555555}` | Extract archive to a folder picker target |
| `SpanzipCompress` | `{44444444-2222-3333-4444-555555555555}` | Compress selected files/folders into a ZIP |

Per `docs/03-api/shell-extension.md`, these are registered through MSIX
(`uap3:FileExplorerContextMenus`). For local development without MSIX, the
DLL can be hand-registered with `regsvr32` once `IExplorerCommand` plumbing
lands in Sprint 5.

## Sprint 4 status

- ✅ `vcxproj` scaffold with Windows App SDK + C++/WinRT references
- ✅ `dllmain.cpp` + skeleton `IExplorerCommand` implementation files
- ✅ Package manifest fragment for MSIX integration
- ⏳ Verb invoke logic — calls `SpanZIP.exe --invoke <verb>` (Sprint 5)
- ⏳ MSIX packaging integration (Sprint 5)
- ⏳ COM registration test (Sprint 5)

## Build prerequisites

- Visual Studio 2022 17.8+ with C++/WinRT workload
- Windows SDK 10.0.22621
- Microsoft.Windows.CppWinRT 2.0.x NuGet package (auto-restored)

## Local invocation contract

Each verb spawns the host app with a CLI:

```
SpanZIP.exe --invoke <verb> --files <path>[;<path>...]
```

Verbs are documented in `docs/03-api/shell-extension.md` §4.
