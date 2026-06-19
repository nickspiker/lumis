#!/bin/bash

echo "Finding phone..."

# Cache file for storing the last known IP (same as deploy.sh)
CACHE_FILE="$HOME/.fairphone_ip_cache"

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
    # Get all local network ranges and scan them
    NETWORKS=$(ip route | grep -E "/(8|16|24)" | grep -v "default" | awk '{print $1}' | grep -E "^(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.)")

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
    exit 1
fi

echo "Found phone at $IP"
echo "Connecting to Fairphone..."
echo "Tip: For logcat, try 'su -c logcat' or 'logcat | grep YourApp'"
echo ""

# Connect to the phone
ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -p 8022 -i ~/.ssh/fairphone_key u0_a10222@$IP 2>/dev/null