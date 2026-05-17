#!/bin/bash
# Build Linux .deb package
set -e

if [ -z "$VERSION" ]; then
    echo "Error: VERSION must be set" >&2
    exit 1
fi

if [ -z "$ARCH" ]; then
    echo "Error: ARCH must be set (amd64 or arm64)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building Orix .deb v${VERSION} for ${ARCH}"

ARCH_DEB="${ARCH}"
[ "${ARCH}" = "x86_64" ] && ARCH_DEB="amd64"
[ "${ARCH}" = "aarch64" ] && ARCH_DEB="arm64"

DEB_DIR="${SCRIPT_DIR}/deb_${ARCH}"
rm -rf "${DEB_DIR}"
mkdir -p "${DEB_DIR}/DEBIAN"
mkdir -p "${DEB_DIR}/usr/bin"
mkdir -p "${DEB_DIR}/usr/share/doc/orix"
mkdir -p "${DEB_DIR}/usr/share/lintian/overrides"

# Copy binary
BIN_PATH="${SCRIPT_DIR}/../target/${ARCH}-unknown-linux-gnu/release/orix"
cp "${BIN_PATH}" "${DEB_DIR}/usr/bin/orix"
chmod 755 "${DEB_DIR}/usr/bin/orix"

# Generate control file
sed -e "s/\$(VERSION)/${VERSION}/g" \
    -e "s/\$(ARCH)/${ARCH_DEB}/g" \
    "${SCRIPT_DIR}/control" > "${DEB_DIR}/DEBIAN/control"

# Copy maintainer scripts
[ -f "${SCRIPT_DIR}/postinst" ] && cp "${SCRIPT_DIR}/postinst" "${DEB_DIR}/DEBIAN/postinst" && chmod 755 "${DEB_DIR}/DEBIAN/postinst"
[ -f "${SCRIPT_DIR}/prerm" ] && cp "${SCRIPT_DIR}/prerm" "${DEB_DIR}/DEBIAN/prerm" && chmod 755 "${DEB_DIR}/DEBIAN/prerm"
[ -f "${SCRIPT_DIR}/postrm" ] && cp "${SCRIPT_DIR}/postrm" "${DEB_DIR}/DEBIAN/postrm" && chmod 755 "${DEB_DIR}/DEBIAN/postrm"

# Copy copyright
cp "${SCRIPT_DIR}/../LICENSE" "${DEB_DIR}/usr/share/doc/orix/copyright" 2>/dev/null || true

# Compress docs
if [ -d "${DEB_DIR}/usr/share/doc/orix" ]; then
    gzip -9 "${DEB_DIR}/usr/share/doc/orix/copyright" 2>/dev/null || true
fi

# Build package
DIST_DIR="${SCRIPT_DIR}/../dist"
mkdir -p "${DIST_DIR}"
dpkg-deb --build --root-owner-group "${DEB_DIR}" "${DIST_DIR}/orix_${VERSION}_${ARCH_DEB}.deb"

# Cleanup
rm -rf "${DEB_DIR}"

echo "Done: orix_${VERSION}_${ARCH_DEB}.deb"
