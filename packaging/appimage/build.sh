# Orix AppImage build script
# Produces a self-contained, portable AppImage from a compiled orix binary
set -e

if [ -z "$VERSION" ]; then
    echo "Error: VERSION must be set" >&2
    exit 1
fi

if [ -z "$ARCH" ]; then
    echo "Error: ARCH must be set (e.g. x86_64, aarch64)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BIN_PATH="${REPO_ROOT}/target/${ARCH}-unknown-linux-gnu/release/orix"
APPDIR="${SCRIPT_DIR}/AppDir_${ARCH}"

echo "Building Orix AppImage v${VERSION} for ${ARCH}"

# Download appimagetool if not cached
APPIMAGETOOL_URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
APPIMAGETOOL="${HOME}/.local/bin/appimagetool"

if [ ! -f "${APPIMAGETOOL}" ]; then
    echo "Downloading appimagetool..."
    mkdir -p "$(dirname "${APPIMAGETOOL}")"
    curl -sSL "${APPIMAGETOOL_URL}" -o "${APPIMAGETOOL}"
    chmod +x "${APPIMAGETOOL}"
fi

# Create AppDir structure
echo "Creating AppDir..."
rm -rf "${APPDIR}"
mkdir -p "${APPDIR}/usr/bin"
mkdir -p "${APPDIR}/usr/lib"
mkdir -p "${APPDIR}/usr/share/icons/hicolor/256x256/apps"
mkdir -p "${APPDIR}/usr/share/applications"

# Copy binary
cp "${BIN_PATH}" "${APPDIR}/usr/bin/orix"
chmod +x "${APPDIR}/usr/bin/orix"

# Copy AppRun and desktop file
cp "${SCRIPT_DIR}/AppRun" "${APPDIR}/AppRun"
chmod +x "${APPDIR}/AppRun"
cp "${SCRIPT_DIR}/orix.desktop" "${APPDIR}/orix.desktop"

# Generate minimal icon (placeholder SVG → PNG conversion if needed)
# For now, use a simple approach - the binary works without icon
touch "${APPDIR}/.DirIcon"

# Patch ELF interpreter if needed for musl-based systems
if [ "${ARCH}" = "x86_64" ]; then
    # Use x86_64 Linux standard interpreter
    PATCHELF="${HOME}/.local/bin/patchelf"
    if command -v patchelf >/dev/null 2>&1; then
        patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "${APPDIR}/usr/bin/orix" 2>/dev/null || true
    fi
fi

# Build AppImage
echo "Building AppImage..."
cd "${APPDIR}"
"${APPIMAGETOOL}" "${APPDIR}" "${REPO_ROOT}/dist/orix-${ARCH}.AppImage"

echo "Done: orix-${ARCH}.AppImage"
