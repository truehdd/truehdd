#!/usr/bin/env bash
# The files a release is made of, named once.
#
# The release notes table and the check that the build produced what the notes promise both
# read this list, so a target that is added, renamed or dropped cannot leave the two
# disagreeing. Every name is built from the same variables the build jobs name their output
# with, so a file is expected because it is produced, not because someone typed it twice.
#
# One row per file: platform|cpu|filename. Run it to see what a release will contain:
#
#   ARCHIVE_PREFIX=truehdd-0.6.0 \
#   WINDOWS_TARGET=x86_64-pc-windows-msvc AARCH64... .github/artifacts.sh
set -euo pipefail

for required in ARCHIVE_PREFIX WINDOWS_TARGET WINDOWS_ARM_TARGET LINUX_GNU_TARGET \
    LINUX_GNU_ARM_TARGET LINUX_MUSL_TARGET LINUX_MUSL_ARM_TARGET; do
    if [ -z "${!required:-}" ]; then
        echo "$(basename "$0"): $required is not set" >&2
        exit 1
    fi
done

row() { printf '%s|%s|%s\n' "$1" "$2" "$3"; }

row "Windows" "Intel or AMD (almost all PCs)"       "${ARCHIVE_PREFIX}-${WINDOWS_TARGET}.zip"
row "Windows" "ARM (Snapdragon, Windows on ARM)"    "${ARCHIVE_PREFIX}-${WINDOWS_ARM_TARGET}.zip"
# The two macOS builds are lipo'd into one binary, so this name carries no target triple.
row "macOS"   "Intel and Apple Silicon (universal)" "${ARCHIVE_PREFIX}-universal-macOS.zip"
row "Linux"   "Intel or AMD"                        "${ARCHIVE_PREFIX}-${LINUX_GNU_TARGET}.tar.gz"
row "Linux"   "Intel or AMD, static build"          "${ARCHIVE_PREFIX}-${LINUX_MUSL_TARGET}.tar.gz"
row "Linux"   "ARM"                                 "${ARCHIVE_PREFIX}-${LINUX_GNU_ARM_TARGET}.tar.gz"
row "Linux"   "ARM, static build"                   "${ARCHIVE_PREFIX}-${LINUX_MUSL_ARM_TARGET}.tar.gz"
