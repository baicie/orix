#!/bin/bash
# Build macOS .pkg using productbuild
set -e

if [ -z "$VERSION" ]; then
    echo "Error: VERSION must be set" >&2
    exit 1
fi

if [ -z "$ARCH" ]; then
    echo "Error: ARCH must be set (x86_64 or arm64)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

echo "Building Orix .pkg v${VERSION} for ${ARCH}"

# Binary path depends on architecture
if [ "${ARCH}" = "arm64" ]; then
    BIN_PATH="${REPO_ROOT}/target/aarch64-apple-darwin/release/orix"
else
    BIN_PATH="${REPO_ROOT}/target/x86_64-apple-darwin/release/orix"
fi

if [ ! -f "${BIN_PATH}" ]; then
    echo "Error: Binary not found at ${BIN_PATH}" >&2
    exit 1
fi

DIST_DIR="${REPO_ROOT}/dist"
PKG_DIR="${SCRIPT_DIR}/pkgroot_${ARCH}"
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}/usr/local/bin"

# Copy binary
cp "${BIN_PATH}" "${PKG_DIR}/usr/local/bin/orix"
chmod 755 "${PKG_DIR}/usr/local/bin/orix"

# Build the component package
COMPONENT_PKG="${DIST_DIR}/orix-${ARCH}.component.pkg"
pkgbuild --identifier "com.baicie.orix.bin.${ARCH}" \
    --version "${VERSION}" \
    --root "${PKG_DIR}" \
    --install-location "/usr/local" \
    "${COMPONENT_PKG}"

# Build the product package using Distribution.xml
PRODUCT_PKG="${DIST_DIR}/orix-${ARCH}.pkg"
productbuild --distribution "${SCRIPT_DIR}/distribution.xml" \
    --resources "${SCRIPT_DIR}" \
    --package-path "${DIST_DIR}" \
    "${PRODUCT_PKG}"

# Cleanup
rm -rf "${PKG_DIR}" "${COMPONENT_PKG}"

echo "Done: orix-${ARCH}.pkg"
