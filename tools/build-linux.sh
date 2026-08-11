#!/usr/bin/env bash
#
# Build the Linux distribution of OtterZip.
#
# Produces, under dist/linux-<arch>/:
#
#   otterzip-gui            the Avalonia app (self-contained, no .NET needed)
#   otterzip                the CLI
#   libotterzip_ffi.so      the Rust engine, loaded by the GUI through P/Invoke
#   Strings/                the ten-language catalogue
#   install.sh              per-user installer (no root)
#   uninstall.sh            removes exactly what install.sh added
#
# and a matching .tar.gz.
#
# Usage:
#   tools/build-linux.sh [--arch x64|arm64] [--cli-only]
#                        [--no-self-contained] [--skip-rust]
#
# Self-contained is the default because "download, extract, run" is the
# promise; requiring a matching .NET runtime first is not that. Pass
# --no-self-contained for a distro package where the runtime is a dependency.
#
# --cli-only builds just the `otterzip` command line. That half is pure Rust
# and needs no .NET SDK at all, so it is the right target on a build box that
# has a Rust toolchain and nothing else.

set -euo pipefail

ARCH="x64"
SELF_CONTAINED=1
SKIP_RUST=0
CLI_ONLY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="${2:?--arch needs a value}"; shift 2 ;;
        --cli-only) CLI_ONLY=1; shift ;;
        --no-self-contained) SELF_CONTAINED=0; shift ;;
        --skip-rust) SKIP_RUST=1; shift ;;
        -h|--help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$ARCH" in
    x64|arm64) ;;
    *) echo "--arch must be x64 or arm64 (got '$ARCH')" >&2; exit 2 ;;
esac

# --- Preflight --------------------------------------------------------------
# Checked BEFORE anything is built. The .NET failure surfaces at `dotnet
# publish`, which is the last step, so without this the script would spend a
# full release compile of the Rust engine and only then tell the user it
# cannot finish — and it would tell them in the .NET SDK resolver's words
# ("The application 'publish' does not exist"), which do not say what to do.

die_missing_dotnet() {
    cat >&2 <<'EOF'
error: building the GUI needs the .NET 9 SDK, and it was not found.

  Install it with your package manager:
      Debian/Ubuntu   sudo apt install dotnet-sdk-9.0
      Fedora          sudo dnf install dotnet-sdk-9.0
      Arch            sudo pacman -S dotnet-sdk
      openSUSE        sudo zypper install dotnet-sdk-9.0

  Or without root, into your home directory:
      curl -fsSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel 9.0
      export PATH="$PATH:$HOME/.dotnet"

  Or skip the GUI entirely — the command-line tool is pure Rust and needs
  no .NET at all:
      tools/build-linux.sh --cli-only
EOF
    exit 1
}

if [ "${CLI_ONLY}" -eq 0 ]; then
    command -v dotnet >/dev/null 2>&1 || die_missing_dotnet
    # `dotnet` on PATH is not the same as an SDK being installed: the runtime-
    # only package ships the same launcher, and it fails the same opaque way.
    if ! dotnet --list-sdks 2>/dev/null | grep -q '^9\.'; then
        echo "error: a .NET runtime is present but no 9.x SDK is installed." >&2
        echo "       (global.json pins 9.0.100 with rollForward=latestFeature," >&2
        echo "        so any 9.0.1xx SDK will do.)" >&2
        echo >&2
        die_missing_dotnet
    fi
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RID="linux-${ARCH}"
OUT="${REPO_ROOT}/dist/${RID}"
VERSION="$(grep -m1 '^version' "${REPO_ROOT}/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"

echo "==> OtterZip ${VERSION} for ${RID}"
rm -rf "${OUT}"
mkdir -p "${OUT}"

# --- 1. The Rust engine -----------------------------------------------------
# The GUI P/Invokes into libotterzip_ffi.so and the CLI is a separate binary
# that statically links the same core.
if [ "${SKIP_RUST}" -eq 0 ]; then
    if [ "${CLI_ONLY}" -eq 1 ]; then
        echo "==> cargo build --release (CLI)"
        (cd "${REPO_ROOT}" && cargo build --release -p otterzip-cli)
    else
        echo "==> cargo build --release (engine + CLI)"
        (cd "${REPO_ROOT}" && cargo build --release -p otterzip-ffi -p otterzip-cli)
    fi
fi

# Ship the CLI as `otterzip`. On Windows it cannot use that name (the WinUI
# host exe is OtterZip.exe and MSIX payload names are case-insensitive), but
# Linux has no such collision and `otterzip` is the name users will type.
cp "${REPO_ROOT}/target/release/otterzip-cli" "${OUT}/otterzip"
chmod +x "${OUT}/otterzip"

# --- 2. The GUI -------------------------------------------------------------
# Skipped entirely for --cli-only: `otterzip` links otterzip-core statically,
# so it has no use for libotterzip_ffi.so either. The shared library exists
# for the GUI's P/Invoke and for nothing else.
if [ "${CLI_ONLY}" -eq 0 ]; then
    cp "${REPO_ROOT}/target/release/libotterzip_ffi.so" "${OUT}/"

    echo "==> dotnet publish (Avalonia front end)"
    PUBLISH_ARGS=(
        "${REPO_ROOT}/app/OtterZip.Linux/OtterZip.Linux.csproj"
        -c Release
        -r "${RID}"
        -o "${OUT}"
        --nologo
    )
    if [ "${SELF_CONTAINED}" -eq 1 ]; then
        PUBLISH_ARGS+=(--self-contained true -p:PublishSingleFile=false)
    else
        PUBLISH_ARGS+=(--self-contained false)
    fi
    dotnet publish "${PUBLISH_ARGS[@]}"

    chmod +x "${OUT}/otterzip-gui"
else
    echo "==> skipping the GUI (--cli-only)"
fi

# --- 3. Installer -----------------------------------------------------------
# Per-user by default: everything lands under ~/.local, which needs no root
# and no package manager. A distro packager can ignore this and stage the
# same tree under /usr instead.
cat > "${OUT}/install.sh" <<'INSTALL'
#!/usr/bin/env sh
# Install OtterZip for the current user. No root required.
#
#   PREFIX=/usr/local sudo ./install.sh   # system-wide instead
set -eu

SRC="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
LIBDIR="$PREFIX/lib/otterzip"
BINDIR="$PREFIX/bin"

echo "Installing OtterZip to $LIBDIR"
mkdir -p "$LIBDIR" "$BINDIR"
# Copy everything except the installer scripts themselves.
find "$SRC" -mindepth 1 -maxdepth 1 \
     ! -name 'install.sh' ! -name 'uninstall.sh' \
     -exec cp -R {} "$LIBDIR/" \;

chmod +x "$LIBDIR/otterzip"

# Symlinks rather than copies so an upgrade is a single re-run of this
# script and the commands never drift from the payload.
ln -sf "$LIBDIR/otterzip" "$BINDIR/otterzip"

# The GUI is absent from a --cli-only package. Everything below is therefore
# conditional rather than assumed, so the same installer script serves both
# package shapes.
if [ -f "$LIBDIR/otterzip-gui" ]; then
    chmod +x "$LIBDIR/otterzip-gui"
    ln -sf "$LIBDIR/otterzip-gui" "$BINDIR/otterzip-gui"
fi

echo
echo "Installed:"
echo "  otterzip      $BINDIR/otterzip"
if [ -f "$LIBDIR/otterzip-gui" ]; then
    echo "  otterzip-gui  $BINDIR/otterzip-gui"
fi
case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *) echo
       echo "NOTE: $BINDIR is not on your PATH. Add it to your shell profile:"
       echo "      export PATH=\"\$PATH:$BINDIR\"" ;;
esac
if [ -f "$LIBDIR/otterzip-gui" ]; then
    echo
    echo "To add OtterZip to your file manager's right-click menu, run"
    echo "  otterzip-gui --install-integration"
    echo "or use Settings -> Integration in the app. It writes only into"
    echo "your home directory and can be removed from the same place."
fi
INSTALL

cat > "${OUT}/uninstall.sh" <<'UNINSTALL'
#!/usr/bin/env sh
# Remove what install.sh added. Leaves settings under
# ~/.config/otterzip alone — uninstalling the program is not a request to
# forget your preferences; delete that directory yourself if you want to.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
LIBDIR="$PREFIX/lib/otterzip"
BINDIR="$PREFIX/bin"

rm -f "$BINDIR/otterzip" "$BINDIR/otterzip-gui"
rm -rf "$LIBDIR"
echo "Removed OtterZip from $PREFIX."
echo "Desktop integration (if installed) is removed from the app's"
echo "Settings -> Integration pane, or by deleting:"
echo "  ~/.local/share/applications/io.github.lumibearstudio.OtterZip*.desktop"
echo "  ~/.local/share/kio/servicemenus/otterzip.desktop"
echo "  ~/.local/share/nautilus/scripts/OtterZip*"
UNINSTALL

chmod +x "${OUT}/install.sh" "${OUT}/uninstall.sh"

cp "${REPO_ROOT}/LICENSING.md" "${REPO_ROOT}/THIRD-PARTY-NOTICES.md" "${OUT}/" 2>/dev/null || true

# --- 4. Tarball -------------------------------------------------------------
# A CLI-only package and a full one are different artifacts; giving them the
# same filename would let one silently overwrite the other in dist/ and, worse,
# on a releases page.
SUFFIX=""
[ "${CLI_ONLY}" -eq 1 ] && SUFFIX="-cli"
TARBALL="${REPO_ROOT}/dist/OtterZip-${VERSION}-${RID}${SUFFIX}.tar.gz"
echo "==> packaging ${TARBALL}"
tar -czf "${TARBALL}" -C "${REPO_ROOT}/dist" --transform "s,^${RID},OtterZip-${VERSION}${SUFFIX}," "${RID}"

echo
echo "Done."
echo "  tree:    ${OUT}"
echo "  tarball: ${TARBALL}"
echo
echo "Install with:  cd ${OUT} && ./install.sh"
