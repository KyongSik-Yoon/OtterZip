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
# Checked BEFORE anything is built or deleted. The .NET failure otherwise
# surfaces at `dotnet publish`, the last step, so the script would spend a full
# release compile of the Rust engine and wipe dist/ before reporting it — and
# it would report it in the SDK resolver's words ("The application 'publish'
# does not exist"), which do not say what is wrong or what to do.

# Read the required SDK major out of global.json rather than hardcoding it, so
# this check cannot drift from what the SDK resolver will actually demand.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GLOBAL_JSON="${SCRIPT_DIR}/../global.json"
WANT_VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${GLOBAL_JSON}" 2>/dev/null | head -1)"
WANT_VERSION="${WANT_VERSION:-9.0.300}"
WANT_MAJOR="${WANT_VERSION%%.*}"
# The three-digit SDK "feature band" (e.g. 300 in 9.0.300). Avalonia 12's
# Roslyn analyzers require the 4.14 compiler, which first shipped in .NET SDK
# 9.0.300 — an older 9.0.1xx SDK builds the rest of the repo but fails the GUI
# with a cryptic CS9057 half a minute in, so the check below rejects it up front.
WANT_MM="${WANT_VERSION%.*}"                       # 9.0
WANT_BAND="${WANT_VERSION##*.}"; WANT_BAND="${WANT_BAND%%-*}"   # 300

# True when an installed SDK satisfies global.json: same major.minor and a
# feature band at least WANT_BAND. Mirrors what the SDK resolver will demand,
# so the build cannot get past this check only to fail in the compiler.
sdk_has_supported() {
    local line ver mm band
    while IFS= read -r line; do
        [ -n "${line}" ] || continue
        ver="${line%% *}"                          # "9.0.316" from "9.0.316 [/path]"
        mm="${ver%.*}"                             # 9.0
        band="${ver##*.}"; band="${band%%-*}"      # 316
        [ "${mm}" = "${WANT_MM}" ] || continue
        case "${band}" in ''|*[!0-9]*) continue;; esac
        [ "${band}" -ge "${WANT_BAND}" ] && return 0
    done <<EOF
$(dotnet --list-sdks 2>/dev/null)
EOF
    return 1
}

report_dotnet_state() {
    # Show what IS there. Without this the message says only "not found",
    # which on a rolling distro is actively misleading: the usual cause is a
    # perfectly good SDK of the WRONG MAJOR, and the user has just watched
    # their package manager install it successfully.
    echo "  what this machine has:" >&2
    if ! command -v dotnet >/dev/null 2>&1; then
        echo "      dotnet: not on PATH" >&2
        return
    fi
    echo "      dotnet: $(command -v dotnet)" >&2
    local sdks
    sdks="$(dotnet --list-sdks 2>/dev/null || true)"
    if [ -z "${sdks}" ]; then
        echo "      SDKs:   (none — this is a runtime-only install)" >&2
    else
        echo "${sdks}" | sed 's/^/      SDK:    /' >&2
    fi
    if [ -n "${DOTNET_ROOT:-}" ]; then
        echo "      DOTNET_ROOT=${DOTNET_ROOT}" >&2
    fi
}

die_dotnet() {
    echo "error: building the GUI needs the .NET SDK ${WANT_VERSION} or newer" >&2
    echo "       (same ${WANT_MM} line — feature band ${WANT_BAND}+)." >&2
    echo "       Avalonia 12's Roslyn analyzers need the 4.14 compiler, which" >&2
    echo "       first ships in SDK ${WANT_MM}.${WANT_BAND}; an older ${WANT_MM}.1xx SDK" >&2
    echo "       fails the GUI build with CS9057. global.json (rollForward=" >&2
    echo "       latestFeature) picks the highest ${WANT_MM}.x you have installed." >&2
    echo >&2
    report_dotnet_state
    cat >&2 <<EOF

  Get a current ${WANT_MM} SDK. The most reliable way, no root and independent
  of what your distro packages, installs the latest ${WANT_MM}.x into ~/.dotnet:
      curl -fsSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel ${WANT_MM}
      export PATH="\$HOME/.dotnet:\$PATH"

  Or from the distro — make sure it is actually ${WANT_MM}.${WANT_BAND}+ (update
  if it is older; the "${WANT_MAJOR}.0" suffix picks the ${WANT_MAJOR} line, not the newest major):
      Arch            sudo pacman -Syu dotnet-sdk-${WANT_MAJOR}.0
      Debian/Ubuntu   sudo apt install dotnet-sdk-${WANT_MAJOR}.0
      Fedora          sudo dnf install dotnet-sdk-${WANT_MAJOR}.0
      openSUSE        sudo zypper install dotnet-sdk-${WANT_MAJOR}.0

  Installing it alongside other SDKs is fine: they coexist, and global.json
  decides which one builds this repo.

  Or skip the GUI entirely — the command line is pure Rust and needs no
  .NET at all:
      tools/build-linux.sh --cli-only
EOF
    exit 1
}

if [ "${CLI_ONLY}" -eq 0 ]; then
    command -v dotnet >/dev/null 2>&1 || die_dotnet
    # `dotnet` on PATH is not the same as a SUPPORTED SDK being installed: the
    # runtime-only package ships the same launcher, and a too-old 9.0.1xx SDK
    # builds everything except the GUI. Require the feature band global.json asks.
    sdk_has_supported || die_dotnet
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
