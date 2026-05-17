#!/bin/bash
# Build Windows MSI using WiX Toolset v3
set -e

if [ -z "$VERSION" ]; then
    echo "Error: VERSION must be set" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building Orix MSI v${VERSION}"

# The WiX build runs on Windows; this script is for reference / Unix-style invocation.
# On Windows runners, WiX is installed via chocolatey or NuGet.
# We invoke candle.exe + light.exe here.
BIN_NAME="orix.exe"
BIN_PATH="${SCRIPT_DIR}/../target/x86_64-pc-windows-msvc/release/${BIN_NAME}"

if [ ! -f "${BIN_PATH}" ]; then
    echo "Error: Binary not found at ${BIN_PATH}" >&2
    exit 1
fi

DIST_DIR="${SCRIPT_DIR}/../dist"
mkdir -p "${DIST_DIR}"

# In the GitHub Actions workflow, WiX is installed via chocolatey.
# The actual build is done via PowerShell/cmd steps that call candle + light.
# This file serves as documentation / local reference.
# NOTE: light.exe must be invoked with -ext WixUtilExtension to support
# Environment elements (PATH modification). See release.yml for CI steps.
echo "WiX build requires Windows environment. See release.yml for CI steps."
