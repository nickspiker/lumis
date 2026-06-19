#!/bin/bash

# Lumis project revert from MEGA (complete local replacement)
# WARNING: This DESTROYS your local lumis directory completely!
# Run this from anywhere - it targets ~/code/lumis specifically

set -e  # Exit on any error

LOCAL_DIR="$HOME/code/lumis"
SOURCE_DIR="$HOME/MEGA/code/lumis"
TEMP_DIR="$HOME/code/.lumis_revert_temp"

echo "=== LUMIS REVERT FROM MEGA ==="
echo "Nuking local lumis and pulling from MEGA..."

# Verify MEGA source exists
if [[ ! -d "$SOURCE_DIR" ]]; then
    echo "Error: MEGA lumis directory not found at $SOURCE_DIR"
    exit 1
fi

echo "Proceeding with nuclear option..."

# Clean up any existing temp directory
if [[ -d "$TEMP_DIR" ]]; then
    echo "Cleaning up previous temp directory..."
    rm -rf "$TEMP_DIR"
fi

# Create temp directory
echo "Creating temporary staging area..."
mkdir -p "$TEMP_DIR"

# Copy from MEGA to temp
echo "Copying clean source from MEGA..."
rsync -av "$SOURCE_DIR/" "$TEMP_DIR/"

# Nuclear option: completely destroy local lumis
echo "NUKING local lumis directory (including all build artifacts)..."
if [[ -d "$LOCAL_DIR" ]]; then
    rm -rf "$LOCAL_DIR"
fi

# Create parent directory if needed
mkdir -p "$(dirname "$LOCAL_DIR")"

# Atomic replacement: move temp to local
echo "Moving clean source to local directory..."
mv "$TEMP_DIR" "$LOCAL_DIR"

echo ""
echo "=== REVERT COMPLETE ==="
echo "Your local lumis directory has been completely replaced with MEGA version."
echo "All build artifacts, local changes, and debris have been nuked."
echo "You now have a pristine copy from MEGA."