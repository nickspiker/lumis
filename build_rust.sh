#!/bin/bash
export CARGO_TERM_COLOR=always
export RUST_BACKTRACE=1

cd rust

RUSTFLAGS="-A warnings" cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build --release
if [ $? -ne 0 ]; then
    echo "ARM64 build failed!"
    exit 1
fi

RUSTFLAGS="-A warnings" cargo ndk -t armeabi-v7a -o ../app/src/main/jniLibs build --release
if [ $? -ne 0 ]; then
    echo "ARMv7 build failed!"
    exit 1
fi

cd ..