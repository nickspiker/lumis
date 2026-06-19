#!/bin/bash
# Wrapper to ensure GNU ld is used instead of Android NDK's LLD
PATH=/usr/bin:$PATH exec gcc "$@"