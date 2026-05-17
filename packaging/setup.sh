#!/bin/bash
# Make packaging scripts executable
find "$(dirname "$0")" -type f \( -name "*.sh" -o -name "postinstall" -o -name "preinstall" \) -exec chmod +x {} +
echo "All scripts made executable"
