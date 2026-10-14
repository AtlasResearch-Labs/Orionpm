#!/usr/bin/env bash
set -euo pipefail

# Orion installer for Linux and macOS
# Built by Atlas Research Labs. Built for the World.

REPO="AtlasResearch-Labs/Orionpm"
INSTALL_DIR="${HOME}/.orion/bin"

main() {
  echo "Installing Orion process manager..."

  local os
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  local arch
  arch="$(uname -m)"

  local target=""
  case "${os}" in
    linux)
      case "${arch}" in
        x86_64) target="orion-linux-x86_64" ;;
        aarch64|arm64) target="orion-linux-arm64" ;;
        *) echo "Error: Unsupported Linux architecture: ${arch}" >&2; exit 1 ;;
      esac
      ;;
    darwin)
      case "${arch}" in
        x86_64) target="orion-macos-x86_64" ;;
        arm64) target="orion-macos-arm64" ;;
        *) echo "Error: Unsupported macOS architecture: ${arch}" >&2; exit 1 ;;
      esac
      ;;
    *)
      echo "Error: Unsupported operating system: ${os}" >&2
      exit 1
      ;;
  esac

  local archive="${target}.tar.gz"
  local url="https://github.com/${REPO}/releases/latest/download/${archive}"

  echo "Downloading ${archive}..."
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "${tmp_dir}"' EXIT

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "${url}" -o "${tmp_dir}/${archive}"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "${tmp_dir}/${archive}" "${url}"
  else
    echo "Error: curl or wget is required to download Orion." >&2
    exit 1
  fi

  echo "Extracting to ${INSTALL_DIR}..."
  mkdir -p "${INSTALL_DIR}"
  tar -xzf "${tmp_dir}/${archive}" -C "${INSTALL_DIR}"
  chmod +x "${INSTALL_DIR}/orion" "${INSTALL_DIR}/oriond"

  echo "Orion installed successfully."

  # PATH check
  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      echo ""
      echo "Add Orion to your PATH by adding this line to your ~/.bashrc or ~/.zshrc:"
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo ""
      ;;
  esac

  echo "Run 'orion --help' to get started."
}

main "$@"
