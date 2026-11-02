#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
svt_version="v4.2.0"
libavif_version="v1.4.2"
work_dir="$project_dir/src-tauri/target/avifenc-build-$libavif_version-$svt_version"
target_triple="$(rustc --print host-tuple)"
sidecar="$project_dir/src-tauri/binaries/avifenc-$target_triple"
parallelism="${CMAKE_BUILD_PARALLEL_LEVEL:-4}"

if [[ "${FORCE_BUILD_AVIFENC:-0}" != "1" && -x "$sidecar" ]]; then
  version="$("$sidecar" --version 2>&1 || true)"
  help="$("$sidecar" --help 2>&1 || true)"
  if [[ "$version" == *"$libavif_version"* && "$help" == *"svt"* ]]; then
    echo "SVT-enabled avifenc is already built ($libavif_version, $svt_version)."
    exit 0
  fi
fi

mkdir -p "$work_dir"
if [[ ! -d "$work_dir/SVT-AV1/.git" ]]; then
  git clone --depth 1 --branch "$svt_version" \
    https://github.com/AOMediaCodec/SVT-AV1.git "$work_dir/SVT-AV1"
fi
if [[ ! -d "$work_dir/libavif/.git" ]]; then
  git clone --depth 1 --branch "$libavif_version" \
    https://github.com/AOMediaCodec/libavif.git "$work_dir/libavif"
fi

install_prefix="$work_dir/install"
cmake -S "$work_dir/SVT-AV1" -B "$work_dir/svt-build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$install_prefix" \
  -DBUILD_SHARED_LIBS=OFF \
  -DBUILD_TESTING=OFF
cmake --build "$work_dir/svt-build" --parallel "$parallelism"
cmake --install "$work_dir/svt-build"

export PKG_CONFIG_PATH="$install_prefix/lib/pkgconfig:$install_prefix/lib64/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
cmake -S "$work_dir/libavif" -B "$work_dir/libavif-build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_PREFIX_PATH="$install_prefix" \
  -DBUILD_SHARED_LIBS=OFF \
  -DAVIF_BUILD_APPS=ON \
  -DAVIF_BUILD_TESTS=OFF \
  -DAVIF_CODEC_AOM=OFF \
  -DAVIF_CODEC_DAV1D=SYSTEM \
  -DAVIF_CODEC_LIBGAV1=OFF \
  -DAVIF_CODEC_RAV1E=OFF \
  -DAVIF_CODEC_SVT=SYSTEM \
  -DAVIF_JPEG=LOCAL \
  -DAVIF_LIBYUV=OFF \
  -DAVIF_ZLIBPNG=SYSTEM
cmake --build "$work_dir/libavif-build" --target avifenc --parallel "$parallelism"

built_avifenc="$work_dir/libavif-build/avifenc"
if [[ ! -x "$built_avifenc" ]]; then
  echo "Could not find the built avifenc at $built_avifenc" >&2
  exit 1
fi
help="$("$built_avifenc" --help 2>&1 || true)"
if [[ "$help" != *"svt"* ]]; then
  echo "Built avifenc does not advertise the SVT-AV1 codec." >&2
  exit 1
fi

install -m 755 "$built_avifenc" "$sidecar"
echo "Built SVT-enabled avifenc ($libavif_version, $svt_version) for $target_triple."
