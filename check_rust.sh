#!/bin/bash
cd rust
cargo ndk -t arm64-v8a check --message-format=json
