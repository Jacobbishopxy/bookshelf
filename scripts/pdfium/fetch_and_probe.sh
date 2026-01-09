#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dest="${root}/.pdfium"
download_dir="${dest}/download"
extract_dir="${dest}/extract"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

case "${arch}" in
  x86_64|amd64) arch="x64" ;;
  aarch64|arm64) arch="arm64" ;;
  armv7l|armv7) arch="arm" ;;
esac

case "${os}" in
  linux*)
    platform="linux"
    asset="pdfium-${platform}-${arch}.tgz"
    ;;
  darwin*)
    platform="mac"
    asset="pdfium-${platform}-${arch}.tgz"
    ;;
  msys*|mingw*|cygwin*)
    platform="windows"
    asset="pdfium-${platform}-${arch}.zip"
    ;;
  *)
    echo "unsupported OS: ${os}" >&2
    exit 1
    ;;
esac

url="https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${asset}"

mkdir -p "${download_dir}" "${extract_dir}"
archive="${download_dir}/${asset}"

echo "Downloading: ${url}"
if command -v curl >/dev/null 2>&1; then
  curl -fL --retry 3 --retry-delay 2 -o "${archive}" "${url}"
elif command -v wget >/dev/null 2>&1; then
  wget -O "${archive}" "${url}"
else
  echo "need curl or wget" >&2
  exit 1
fi

rm -rf "${extract_dir:?}/"*
case "${asset}" in
  *.tgz|*.tar.gz)
    tar -xzf "${archive}" -C "${extract_dir}"
    ;;
  *.zip)
    if ! command -v unzip >/dev/null 2>&1; then
      echo "need unzip" >&2
      exit 1
    fi
    unzip -q -o "${archive}" -d "${extract_dir}"
    ;;
  *)
    echo "unknown archive type: ${asset}" >&2
    exit 1
    ;;
esac

lib="$(find "${extract_dir}" -maxdepth 3 -type f \( -name "libpdfium.so" -o -name "libpdfium.dylib" -o -name "pdfium.dll" \) | head -n 1 || true)"
if [[ -z "${lib}" ]]; then
  echo "could not find a pdfium shared library in ${extract_dir}" >&2
  exit 1
fi

echo "Found: ${lib}"

final_lib="${dest}/$(basename "${lib}")"
cp -f "${lib}" "${final_lib}"
echo "Installed: ${final_lib}"

pdf_arg=()
if [[ -d "${root}/tmp" ]]; then
  pdf_candidate="$(ls -1 "${root}/tmp"/*.pdf 2>/dev/null | head -n 1 || true)"
  if [[ -n "${pdf_candidate}" ]]; then
    pdf_arg=(--pdf "${pdf_candidate}")
  fi
fi

echo "Probing via engine binary..."
cargo run -p engine --bin pdfium_probe --locked -- --lib "${final_lib}" "${pdf_arg[@]}"

echo
echo "OK. To use in the app, run:"
echo "  (no env var needed; engine build embeds ${final_lib})"
echo "  cargo run"
