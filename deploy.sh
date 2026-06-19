#!/bin/bash

# Clean up any Android heap dump files
echo "Cleaning up heap dump files..."
find . -name "*.hprof" -type f -delete

# Function to check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Check Android SDK environment
if [ -z "$ANDROID_HOME" ]; then
    echo "Error: ANDROID_HOME environment variable is not set"
    echo "Please set ANDROID_HOME to point to your Android SDK installation"
    exit 1
fi

# Find latest build tools version
LATEST_BUILD_TOOLS=$(ls "$ANDROID_HOME/build-tools/" 2>/dev/null | sort -V | tail -n 1)
if [ -z "$LATEST_BUILD_TOOLS" ]; then
    echo "Error: Could not find any build-tools in $ANDROID_HOME/build-tools/"
    echo "Please install build-tools using the Android SDK Manager"
    exit 1
fi

APKSIGNER="$ANDROID_HOME/build-tools/$LATEST_BUILD_TOOLS/apksigner"
if [ ! -f "$APKSIGNER" ]; then
    echo "Error: apksigner not found at $APKSIGNER"
    echo "Please ensure Android build-tools are properly installed"
    exit 1
fi

echo "Building signed release APK..."

# Find MEGA folder dynamically
if [ -z "$MEGA" ]; then
    MEGA=$(find /home /mnt -maxdepth 3 -name "MEGA" -type d 2>/dev/null | head -1)
    
    if [ -z "$MEGA" ]; then
        echo "Error: Could not find MEGA folder"
        echo "Please set MEGA environment variable: export MEGA=/path/to/mega"
        exit 1
    fi
    
    echo "Found MEGA folder at: $MEGA"
fi

# Set keystore path - check multiple locations
KEYSTORE_LOCATIONS=(
    "$MEGA/code/keys/nicks-apps.keystore"
    "/mnt/Chiton/MEGA/Code/keys/nicks-apps.keystore"
)

KEYSTORE_PATH=""
for path in "${KEYSTORE_LOCATIONS[@]}"; do
    if [ -f "$path" ]; then
        KEYSTORE_PATH="$path"
        echo "Found keystore at: $KEYSTORE_PATH"
        break
    fi
done

if [ -z "$KEYSTORE_PATH" ]; then
    echo "Error: Keystore not found at any of these locations:"
    for path in "${KEYSTORE_LOCATIONS[@]}"; do
        echo "  - $path"
    done
    exit 1
fi

echo "Using keystore: $KEYSTORE_PATH"

# Cache file for storing the last known IP
CACHE_FILE="$HOME/.fairphone_ip_cache"


# Get password from GNOME Keyring (or prompt if not stored)
if [ -z "$LUMIS_KEYSTORE_PASSWORD" ]; then
    LUMIS_KEYSTORE_PASSWORD=$(secret-tool lookup service photon key keystore_password 2>/dev/null)
    if [ -z "$LUMIS_KEYSTORE_PASSWORD" ]; then
        echo "Password not in keyring. Run this once to store it:"
        echo "  secret-tool store --label='Photon Keystore' service photon key keystore_password"
        echo ""
        echo "Enter keystore password:"
        read -s LUMIS_KEYSTORE_PASSWORD
    fi
    export LUMIS_KEYSTORE_PASSWORD
fi

# Use fixed alias and same password for key
KEYSTORE_PASSWORD="$LUMIS_KEYSTORE_PASSWORD"
KEY_ALIAS="lumis"
KEY_PASSWORD="$LUMIS_KEYSTORE_PASSWORD"

echo "Building unsigned APK..."
./build.sh

if [ $? -eq 0 ]; then
    echo "Signing APK..."
    # Sign the APK using apksigner
    "$APKSIGNER" sign \
        --ks "$KEYSTORE_PATH" \
        --ks-pass pass:$KEYSTORE_PASSWORD \
        --ks-key-alias $KEY_ALIAS \
        --key-pass pass:$KEY_PASSWORD \
        --min-sdk-version 21 \
        --out app/build/outputs/apk/release/app-release-signed.apk \
        app/build/outputs/apk/release/app-release-unsigned.apk

    if [ $? -eq 0 ]; then
        echo "Finding phone..."
        IP=""
        # First, try the cached IP if it exists
        if [ -f "$CACHE_FILE" ]; then
            CACHED_IP=$(cat "$CACHE_FILE")
            echo "Trying cached IP: $CACHED_IP"
            # Test if SSH is available on the cached IP
            if nc -z -w 1 "$CACHED_IP" 8022 2>/dev/null; then
                echo "Phone found at cached IP!"
                IP="$CACHED_IP"
            else
                echo "Cached IP not responding, scanning network..."
            fi
        fi
        # If no cached IP or it didn't work, scan the network
        if [ -z "$IP" ]; then
            NETWORKS=$(ip route | grep -E "/(8|16|24)" | grep -v "default" | awk '{print $1}' | grep -E "^(10\\.|172\\.(1[6-9]|2[0-9]|3[01])\\.|192\\.168\\.)")
            for NETWORK in $NETWORKS; do
                echo "Scanning network: $NETWORK"
                IP=$(nmap -p 8022 --open $NETWORK 2>/dev/null | grep "Nmap scan report" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' | head -1)
                if [ ! -z "$IP" ]; then
                    echo "Phone found at $IP"
                    echo "$IP" > "$CACHE_FILE"
                    break
                fi
            done
        fi
        if [ -z "$IP" ]; then
            echo "Could not find Fairphone with SSH running on any local network"
            echo "Scanned networks: $NETWORKS"
            echo ""
            echo "Falling back to ADB installation..."

            # Check if adb is available
            if ! command_exists adb; then
                echo "Error: adb not found. Please install Android SDK platform-tools"
                exit 1
            fi

            # Check if any device is connected
            DEVICE_COUNT=$(adb devices | grep -v "List of devices" | grep -c "device$")
            if [ "$DEVICE_COUNT" -eq 0 ]; then
                echo "Error: No ADB devices connected"
                echo "Please connect your phone via USB and enable USB debugging"
                exit 1
            elif [ "$DEVICE_COUNT" -gt 1 ]; then
                echo "Warning: Multiple ADB devices detected, installing to first available device"
            fi

            echo "Installing APK via ADB..."
            adb install -r app/build/outputs/apk/release/app-release-signed.apk

            if [ $? -eq 0 ]; then
                echo "Deployed via ADB at $(date '+%Y-%m-%d %H:%M:%S')"
                exit 0
            else
                echo "ADB installation failed!"
                exit 1
            fi
        fi
        echo "Using phone at $IP"
        echo "Copying signed APK to Termux home..."
        scp -P 8022 -i ~/.ssh/fairphone_key -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR app/build/outputs/apk/release/app-release-signed.apk u0_a10222@$IP:/data/data/com.termux/files/home/app-release.apk 2>/dev/null
        if [ $? -eq 0 ]; then
            echo "Installing..."
            ssh -p 8022 -i ~/.ssh/fairphone_key -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR u0_a10222@$IP "su -c 'cp /data/data/com.termux/files/home/app-release.apk /data/local/tmp/ && pm install /data/local/tmp/app-release.apk && rm /data/local/tmp/app-release.apk /data/data/com.termux/files/home/app-release.apk'; exit" 2>/dev/null
            echo "Deployed at $(date '+%Y-%m-%d %H:%M:%S')"
            exit 0
        else
            echo "Failed to copy APK."
            exit 1
        fi
    else
        echo "APK signing failed!"
        exit 1
    fi
else
    echo "Build failed!"
    exit 1
fi