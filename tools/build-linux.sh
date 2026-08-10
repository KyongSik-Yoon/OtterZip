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
#   tools/build-linux.sh [--arch x64|arm64] [--no-self-contained] [--skip-rust]
#
# Self-contained is the default because "download, extract, run" is the
# promise; requiring a matching .NET runtime first is not that. Pass
# --no-self-contained for a distro package where the runtime is a dependency.

set -euo pipefail

ARCH="x64"
SELF_CONTAINED=1
SKIP_RUST=0

while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="${2:?--arch needs a value}"; shift 2 ;;
        --no-self-contained) SELF_CONTAINED=0; shift ;;
        --skip-rust) SKIP_RUST=1; shift ;;
        -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$ARCH" in
    x64|arm64) ;;
    *) echo "--arch must be x64 or arm64 (got '$ARCH')" >&2; exit 2 ;;
esac

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
    echo "==> cargo build --release (engine + CLI)"
    (cd "${REPO_ROOT}" && cargo build --release -p otterzip-ffi -p otterzip-cli)
fi

cp "${REPO_ROOT}/target/release/libotterzip_ffi.so" "${OUT}/"
# Ship the CLI as `otterzip`. On Windows it cannot use that name (the WinUI
# host exe is OtterZip.exe and MSIX payload names are case-insensitive), but
# Linux has no such collision and `otterzip` is the name users will type.
cp "${REPO_ROOT}/target/release/otterzip-cli" "${OUT}/otterzip"

# --- 2. The GUI -------------------------------------------------------------
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

chmod +x "${OUT}/otterzip" "${OUT}/otterzip-gui"

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

chmod +x "$LIBDIR/otterzip" "$LIBDIR/otterzip-gui"

# Symlinks rather than copies so an upgrade is a single re-run of this
# script and the two commands never drift from the payload.
ln -sf "$LIBDIR/otterzip" "$BINDIR/otterzip"
ln -sf "$LIBDIR/otterzip-gui" "$BINDIR/otterzip-gui"

echo
echo "Installed:"
echo "  otterzip      $BINDIR/otterzip"
echo "  otterzip-gui  $BINDIR/otterzip-gui"
case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *) echo
       echo "NOTE: $BINDIR is not on your PATH. Add it to your shell profile:"
       echo "      export PATH=\"\$PATH:$BINDIR\"" ;;
esac
echo
echo "To add OtterZip to your file manager's right-click menu, launch"
echo "otterzip-gui and use Settings -> Integration. It writes only into"
echo "your home directory and can be removed from the same place."
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
TARBALL="${REPO_ROOT}/dist/OtterZip-${VERSION}-${RID}.tar.gz"
echo "==> packaging ${TARBALL}"
tar -czf "${TARBALL}" -C "${REPO_ROOT}/dist" --transform "s,^${RID},OtterZip-${VERSION},"  "${RID}"

echo
echo "Done."
echo "  tree:    ${OUT}"
echo "  tarball: ${TARBALL}"
echo
echo "Install with:  cd ${OUT} && ./install.sh"
