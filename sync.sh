#!/bin/bash
set -e  # Exit on any error

SOURCE_DIR="$HOME/code/lumis"
DEST_BASE="$HOME/MEGA/code"
DEST_DIR="$DEST_BASE/lumis"
TEMP_DIR="$DEST_BASE/.lumis_sync_temp"

echo "Lumis atomic sync to MEGA..."
echo "From: $SOURCE_DIR"
echo "To: $DEST_DIR"

# Verify we're in the right place
if [[ ! -f "$SOURCE_DIR/build.gradle" ]]; then
    echo "Error: Not in lumis project directory or build.gradle missing"
    exit 1
fi

# Clean up any existing temp directory
if [[ -d "$TEMP_DIR" ]]; then
    echo "Cleaning up previous temp directory..."
    rm -rf "$TEMP_DIR"
fi

# Create temp directory
echo "Creating temporary staging area..."
mkdir -p "$TEMP_DIR"

# Copy source to temp, excluding build artifacts
echo "Copying source files (excluding build artifacts)..."
rsync -av \
    --exclude='app/build/' \
    --exclude='.gradle/' \
    --exclude='build/' \
    --exclude='*.apk' \
    --exclude='*.aab' \
    --exclude='*.ap_' \
    --exclude='*.dex' \
    --exclude='rust/target/' \
    --exclude='*.so' \
    --exclude='*.rlib' \
    --exclude='*.rmeta' \
    --exclude='app/libs/' \
    --exclude='*.hprof' \
    --exclude='.idea/' \
    --exclude='*.iml' \
    --exclude='.vscode/' \
    --exclude='*.log' \
    --exclude='*.tmp' \
    --exclude='*.swp' \
    --exclude='*.bak' \
    --exclude='.DS_Store' \
    --exclude='Thumbs.db' \
    "$SOURCE_DIR/" "$TEMP_DIR/"

# Atomic replacement: remove old, move new
echo "Performing atomic replacement..."
if [[ -d "$DEST_DIR" ]]; then
    echo "Removing old lumis directory..."
    rm -rf "$DEST_DIR"
fi

echo "Moving staged files to final location..."
mv "$TEMP_DIR" "$DEST_DIR"

echo "MEGA sync complete!"