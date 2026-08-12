# OtterZip on Linux

OtterZip runs on Linux with the same Rust engine as the Windows build and a
native GTK-free Avalonia front end. This page covers what you get, how to
install it, and what is deliberately different from Windows.

## What's included

| Component | Windows | Linux |
|---|---|---|
| Archive engine | `otterzip_ffi.dll` | `libotterzip_ffi.so` — same Rust core |
| Command line | `otterzip.exe` | `otterzip` |
| GUI | WinUI 3 | Avalonia (`otterzip-gui`) |
| Context menu | Explorer shell extension (COM) | `.desktop` entries + file-manager actions |
| Settings | `ApplicationData.LocalSettings` | `~/.config/otterzip/settings.json` |
| "Recycle Bin" | `SHFileOperation` | XDG Trash (`gio trash`, then the spec by hand) |

Formats, compression levels, encryption, split archives, the ten-language UI
and the legacy-codepage handling for Korean/Japanese/Chinese filenames are all
the same code — there is no Linux-specific feature subset.

## Install

### From a release tarball

```sh
tar xzf OtterZip-<version>-linux-x64.tar.gz
cd OtterZip-<version>
./install.sh                    # installs to ~/.local, no root needed
```

`PREFIX=/usr/local sudo ./install.sh` installs system-wide instead.

The tarball is self-contained: it carries its own .NET runtime, so nothing
else has to be installed first. `./uninstall.sh` removes what it added.

### From source

```sh
tools/build-linux.sh              # → dist/linux-x64/ and a .tar.gz
tools/build-linux.sh --arch arm64 # aarch64
tools/build-linux.sh --cli-only   # command line only — no .NET SDK needed
```

The full build needs a Rust toolchain **and the .NET 9 SDK**. Install the
*versioned* package — the unsuffixed one is the newest major, which
`global.json` will refuse:

```sh
sudo pacman -S dotnet-sdk-9.0      # Arch — NOT plain `dotnet-sdk`
sudo apt install dotnet-sdk-9.0    # Debian/Ubuntu
sudo dnf install dotnet-sdk-9.0    # Fedora
```

Having a newer SDK installed as well is fine; they coexist and `global.json`
picks. The script checks all this before it builds anything, and prints which
SDKs it actually found when the check fails. Add `--no-self-contained` for a
distro package where the runtime is a declared dependency.

`--cli-only` builds just `otterzip`. That binary links the engine statically,
so it needs neither the .NET SDK nor `libotterzip_ffi.so` — a Rust toolchain
is the whole requirement, and the tarball comes out around 2 MB instead of
50. Its `install.sh` is the same script and simply skips the GUI parts. The
artifact is named `OtterZip-<version>-linux-<arch>-cli.tar.gz` so it cannot
be confused with the full package.

## File-manager integration

There is no cross-desktop context-menu API on Linux, so "right-click →
extract" is assembled from three freedesktop mechanisms. Install them from
**Settings → Integration**, or headlessly:

```sh
otterzip-gui --install-integration
otterzip-gui --uninstall-integration
```

Both print every path they touch. Everything lands under your home directory —
no root, no package manager, no polkit prompt:

| File | What it does |
|---|---|
| `~/.local/share/applications/io.github.lumibearstudio.OtterZip*.desktop` | Puts OtterZip in "Open With" and the launcher |
| `~/.config/mimeapps.list` | Makes OtterZip the default for archive types (merged, not overwritten) |
| `~/.local/share/nautilus/scripts/OtterZip — *` | GNOME Files → right-click → Scripts |
| `~/.local/share/kio/servicemenus/otterzip.desktop` | Dolphin → right-click → OtterZip submenu (installed `chmod +x`, which KIO requires outside system directories) |
| `~/.config/Thunar/uca.xml.otterzip` | Thunar actions — **needs a manual merge**, see below |

Uninstall removes all of it, including the `mimeapps.list` entries, and leaves
your other file associations and your OtterZip settings alone.

### Thunar

Thunar rewrites `uca.xml` wholesale while it is running, so merging into it
automatically risks losing custom actions you added yourself. The installer
writes a fragment to `~/.config/Thunar/uca.xml.otterzip` instead; paste the
two `<action>` blocks into `~/.config/Thunar/uca.xml` with Thunar closed, or
add them through **Edit → Configure custom actions**.

## Command line

`otterzip` is the scripting interface and needs no display server:

```sh
otterzip a archive.zip ./src        # compress
otterzip x archive.zip -o ./out     # extract
otterzip l archive.7z               # list
otterzip t archive.rar              # test integrity
```

`otterzip-gui` also accepts the context-menu verbs directly, which is what the
`.desktop` files call:

```sh
otterzip-gui --invoke extract-here --files a.zip b.tar.gz
otterzip-gui --invoke compress-zip --files ./project
otterzip-gui --help
```

## Viewing and adding to an archive

Double-clicking an archive (or dropping a single one onto the window) opens a
contents view listing its entries. From there:

* **Extract** unpacks everything to a folder you pick.
* **Add files… / Add folder…** appends to the archive in place — ZIP only.
  Existing entries are preserved as-is (nothing is recompressed); a name that
  is already inside the archive is skipped rather than duplicated. (Dragging
  files from the file manager onto the window is not supported on Linux yet —
  see *Differences from the Windows build* below.)

The command line does the same append:

```sh
otterzip a existing.zip newfile.txt morestuff/   # appends to an existing ZIP
otterzip a new.zip files/                         # creates when it doesn't exist
```

Append is ZIP-only by design. 7z, tar and the rest stay create-only; RAR is
extract-only by licence.

## Differences from the Windows build

**Permissions are preserved.** Both directions. A `.tar.gz` or `.zip` created
on Linux records each file's mode, and extracting restores it, so a script
tree round-trips with its execute bits intact. Two deliberate narrowings:
`setuid`/`setgid`/sticky are always dropped (an archive is untrusted input),
and your umask is applied, matching `tar` and `unzip` for a non-root user.
Turn the whole thing off with the extract option `preserve_permissions`.

**Symlinks can be restored.** Off by default, as on Windows. When enabled,
symlink entries become real links, with the target checked to stay inside the
destination — that closes the two-entry tar escape, where a link entry and a
following file entry both look contained but together write outside the
destination.

**No Mark-of-the-Web.** `Zone.Identifier` is an NTFS alternate data stream and
a SmartScreen input; there is no Linux equivalent that any component consumes,
so the propagation is a no-op here rather than a fake.

**Filename encoding does not depend on your locale.** A CP949 archive from
Bandizip decodes correctly whether your desktop is Korean or `LANG=C` — the
codepage is chosen from the bytes, not from the environment. Pin it explicitly
in Settings if you have a corpus that needs it.

**Windows-illegal names extract fine.** A file called `aux.txt` or one with a
`:` in it is legal here, and OtterZip writes it out as named. The reverse also
holds: an entry with a drive-letter prefix (`C:\…`) is rejected on Linux
rather than being rewritten, because there is no drive to strip.

**RAR is still extract-only**, for licensing reasons, not technical ones.

**No drag-and-drop from the file manager.** Files dragged out of Dolphin,
Nautilus or Thunar do not reach the window. Use **Add files… / Add folder…**,
"Open With", or the right-click **context-menu actions** instead — all of which
do work, and all of which reach the same code a drop would.

Two separate things block it, and the second is why upgrading the toolkit is
not the fix it looks like:

1. Avalonia's X11 backend implements no XDND drop target before **12.1**
   (AvaloniaUI/Avalonia#20926, not backported to 11.x), so on the 11.x line
   this build ships, no drop event is delivered at all.
2. On **12.1 the drop event does arrive, and the payload is still unusable**
   under a KDE Plasma Wayland session. Plasma advertises `File`,
   `application/x-kde4-urilist` and `application/vnd.portal.filetransfer`, but
   every one of them reads back as the *same* buffer — and that buffer holds
   the X clipboard's current text, not the drag. Measured directly against the
   X server with an independent connection (bypassing Avalonia entirely): same
   wrong bytes, byte-for-byte, tracking whatever was last copied. The document
   portal then correctly rejects the resulting non-key with `AccessDenied:
   Invalid transfer`.

Point 2 puts the fault at or below the X selection layer rather than in
Avalonia's format dispatch — consistent with the drag source (Dolphin) being a
native Wayland client while OtterZip is an XWayland client, with the
Wayland↔X11 drag bridge in between. No application-side code can recover the
paths in that situation, which is why this build stays on the mature 11.2
rather than carrying a newer toolkit that does not actually deliver the
feature. The drop handlers remain wired and work on Windows and macOS.

**Wayland.** OtterZip runs on the X11 backend, which is Avalonia's production
Linux path; on a Wayland session it runs through XWayland with no configuration
needed — there is no separate "enable Wayland" step, and no native-Wayland
backend to switch to in this Avalonia version. The window advertises its
application id (`io.github.lumibearstudio.OtterZip`, matching the installed
`.desktop` file) so the compositor and the taskbar show the OtterZip icon and
name rather than a generic placeholder.

## Building and testing

```sh
cargo test --workspace                                   # engine
dotnet build app/OtterZip.Linux/OtterZip.Linux.csproj    # GUI
xvfb-run -a ./otterzip-gui --invoke compress-zip --files ./src   # headless smoke test
```

CI runs the engine suite on Linux twice — once normally, once under `LC_ALL=C`
so the locale-independent filename decoding stays honest — plus a Linux GUI
build, a packaging run, and `desktop-file-validate` over the generated
integration files.
