#!/bin/bash
# The environment the Rust builds need, printed as NAME=value lines for CI to append to $GITHUB_ENV (or to source
# after `set -a`). The app repos that build lazy-link call it too.
#
# aws-lc (under rustls) compiles C and assembly. With these set the platform's own compiler is all it needs: no CMake
# (the cc builder), no NASM on Windows x86-64 (prebuilt objects), and no system copy of aws-lc picked up by chance.
# Apple targets need the same oldest system for the C compiler and for rustc's linker, or the iOS link fails on
# symbols the oldest system does not have; keep them equal to what the apps ask of App Store builds.
set -euo pipefail

cat <<'ENV'
AWS_LC_SYS_CMAKE_BUILDER=0
AWS_LC_SYS_PREBUILT_NASM=1
AWS_LC_SYS_USE_SYSTEM=0
IPHONEOS_DEPLOYMENT_TARGET=18.0
MACOSX_DEPLOYMENT_TARGET=14.0
ENV
