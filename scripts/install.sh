#!/usr/bin/env bash
set -euo pipefail

REPO_OWNER="chojs23"
REPO_NAME="ts-bridge"
RELEASE_BASE="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"

VERSION="${TS_BRIDGE_VERSION:-latest}"
INSTALL_DIR="${TS_BRIDGE_INSTALL_DIR:-"$HOME/.local/bin"}"
VERIFY_CHECKSUM=1

usage() {
  cat <<'EOF'
Usage: install.sh [--version VERSION] [--install-dir DIR] [--no-verify]

Options:
  --version      Version tag to install (e.g. v0.4.0). Defaults to latest.
  --install-dir  Install directory (default: $HOME/.local/bin).
  --no-verify    Skip SHA256 verification.
EOF
}

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

download() {
  local url="$1"
  local output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
    return
  fi
  if command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
    return
  fi
  die "curl or wget is required to download release assets"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
  --version)
    VERSION="${2:-}"
    [[ -n "$VERSION" ]] || die "--version requires a value"
    shift 2
    ;;
  --install-dir)
    INSTALL_DIR="${2:-}"
    [[ -n "$INSTALL_DIR" ]] || die "--install-dir requires a value"
    shift 2
    ;;
  --no-verify)
    VERIFY_CHECKSUM=0
    shift 1
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument: $1"
    ;;
  esac
done

if [[ "$VERSION" != "latest" && "$VERSION" != v* ]]; then
  VERSION="v${VERSION}"
fi

os_name="$(uname -s)"
arch_name="$(uname -m)"

case "$os_name" in
Linux)
  case "$arch_name" in
  x86_64 | amd64) ;;
  *) die "unsupported Linux architecture: ${arch_name}" ;;
  esac
  archive="ts-bridge-linux-x86_64.tar.gz"
  ;;
Darwin)
  archive="ts-bridge-macos-universal.tar.gz"
  ;;
*)
  die "unsupported OS: ${os_name}"
  ;;
esac

if [[ "$VERSION" == "latest" ]]; then
  archive_url="${RELEASE_BASE}/latest/download/${archive}"
  checksum_url="${RELEASE_BASE}/latest/download/SHA256SUMS"
else
  archive_url="${RELEASE_BASE}/download/${VERSION}/${archive}"
  checksum_url="${RELEASE_BASE}/download/${VERSION}/SHA256SUMS"
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

archive_path="${tmp_dir}/${archive}"
download "$archive_url" "$archive_path"

if [[ "$VERIFY_CHECKSUM" -eq 1 ]]; then
  checksum_path="${tmp_dir}/SHA256SUMS"
  if download "$checksum_url" "$checksum_path"; then
    expected_checksum="$(awk -v file="$archive" '$2 == file { print $1 }' "$checksum_path")"
    if [[ -z "$expected_checksum" ]]; then
      die "checksum entry for ${archive} not found"
    fi
    if command -v sha256sum >/dev/null 2>&1; then
      actual_checksum="$(sha256sum "$archive_path" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      actual_checksum="$(shasum -a 256 "$archive_path" | awk '{print $1}')"
    else
      log "sha256 tool not found; skipping checksum verification"
      VERIFY_CHECKSUM=0
    fi
    if [[ "$VERIFY_CHECKSUM" -eq 1 && "$actual_checksum" != "$expected_checksum" ]]; then
      die "checksum verification failed for ${archive}"
    fi
  else
    log "unable to download SHA256SUMS; skipping checksum verification"
  fi
fi

extract_dir="${tmp_dir}/extract"
mkdir -p "$extract_dir"
tar -xzf "$archive_path" -C "$extract_dir"

binary_path="${extract_dir}/ts-bridge"
[[ -f "$binary_path" ]] || die "expected binary ${binary_path} not found"

mkdir -p "$INSTALL_DIR"
if command -v install >/dev/null 2>&1; then
  install -m 0755 "$binary_path" "${INSTALL_DIR}/ts-bridge"
else
  cp "$binary_path" "${INSTALL_DIR}/ts-bridge"
  chmod 0755 "${INSTALL_DIR}/ts-bridge"
fi

log "installed ts-bridge to ${INSTALL_DIR}/ts-bridge"
if ! command -v ts-bridge >/dev/null 2>&1; then
  log "ensure ${INSTALL_DIR} is on your PATH"
fi
