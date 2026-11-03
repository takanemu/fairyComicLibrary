#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="$(rustc --print host-tuple)"
if [[ "$target_triple" != "x86_64-unknown-linux-gnu" ]]; then
  echo "PDFium is currently bundled only for x86_64-unknown-linux-gnu (got $target_triple)." >&2
  exit 1
fi

version="8125"
archive_sha256="e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659"
library_sha256="1c251efce460d67437a4c76d4be9033ae2f17bf5603f5bff1af10209b94ca723"
archive_url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$version/pdfium-linux-x64.tgz"
work_dir="$project_dir/src-tauri/target/pdfium-build-$version"
archive="$work_dir/pdfium-linux-x64.tgz"
library="$work_dir/lib/libpdfium.so"
sidecar="$project_dir/src-tauri/binaries/libpdfium-$target_triple"

mkdir -p "$work_dir" "$project_dir/src-tauri/binaries"
if [[ ! -f "$archive" ]]; then
  curl --fail --location --silent --show-error "$archive_url" --output "$archive.part"
  printf '%s  %s\n' "$archive_sha256" "$archive.part" | sha256sum --check
  mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$archive_sha256" "$archive" | sha256sum --check

if [[ -f "$sidecar" ]]; then
  printf '%s  %s\n' "$library_sha256" "$sidecar" | sha256sum --check
  echo "PDFium $version is already bundled for $target_triple."
  exit 0
fi

if [[ ! -f "$library" ]]; then
  tar --extract --gzip --file "$archive" --directory "$work_dir" lib/libpdfium.so
fi

printf '%s  %s\n' "$library_sha256" "$library" | sha256sum --check
install -m 755 "$library" "$sidecar"
echo "Bundled PDFium $version for $target_triple."
