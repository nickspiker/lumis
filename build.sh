#!/bin/bash
set -e

# Set up Android NDK environment
export ANDROID_NDK_HOME=/home/nick/android-sdk/ndk/25.2.9519653
export ANDROID_HOME=/home/nick/android-sdk
export PATH=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH

# Android target environment (ARM64 only)
export CC_aarch64_linux_android=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang

# CRITICAL: Set host build flags for build scripts (build.rs)
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold"
export CC="clang"
export CXX="clang++"

echo "Building Rust code..."
cd rust

# Build for ARM64 only
echo "Building for ARM64..."
cargo build --target aarch64-linux-android --release

cd ..

# Copy Rust libraries to where Gradle expects them
echo "Copying Rust libraries to app/libs..."
mkdir -p app/libs/arm64-v8a
cp rust/target/aarch64-linux-android/release/liblumis_core.so app/libs/arm64-v8a/

echo "Building Android APK with Gradle..."
./gradlew assembleRelease

echo "APK created at app/build/outputs/apk/release/app-release-unsigned.apk"
echo "Build complete!"