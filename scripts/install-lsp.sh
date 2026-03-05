#!/usr/bin/env bash
#
# Install LSP servers for codescout.
#
# Usage:
#   ./scripts/install-lsp.sh --check          # show what's installed / missing
#   ./scripts/install-lsp.sh --all            # install every supported LSP server
#   ./scripts/install-lsp.sh rust python go   # install specific languages only
#
# Supported languages:
#   rust, python, typescript, go, java, kotlin, c, csharp, ruby
#
# Platform: Linux (x86_64, aarch64) and macOS (x86_64, arm64).

set -euo pipefail

# ── Globals ──────────────────────────────────────────────────────────────────

INSTALL_DIR="${HOME}/.local/bin"
DOWNLOAD_DIR="${TMPDIR:-/tmp}/codescout-lsp-install"

ALL_LANGS=(rust python typescript go java kotlin c csharp ruby)

# ── Helpers ──────────────────────────────────────────────────────────────────

info()  { printf '\033[1;34m[info]\033[0m  %s\n' "$*"; }
ok()    { printf '\033[1;32m[ok]\033[0m    %s\n' "$*"; }
warn()  { printf '\033[1;33m[warn]\033[0m  %s\n' "$*"; }
err()   { printf '\033[1;31m[error]\033[0m %s\n' "$*"; }
skip()  { printf '\033[1;90m[skip]\033[0m  %s\n' "$*"; }

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "macos" ;;
        *)       err "Unsupported OS: $(uname -s)"; exit 1 ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "arm64" ;;
        *)              err "Unsupported architecture: $(uname -m)"; exit 1 ;;
    esac
}

has_cmd() { command -v "$1" &>/dev/null; }

ensure_install_dir() {
    mkdir -p "$INSTALL_DIR"
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        warn "$INSTALL_DIR is not on PATH. Add it:"
        warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi
}

ensure_download_dir() {
    mkdir -p "$DOWNLOAD_DIR"
}

# Fetch the latest GitHub release tag for a repo (e.g. "fwcd/kotlin-language-server").
github_latest_tag() {
    local repo="$1"
    curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" \
        | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/'
}

# ── Prerequisite checks ─────────────────────────────────────────────────────

check_prereq() {
    local name="$1" cmd="$2" hint="$3"
    if has_cmd "$cmd"; then
        return 0
    else
        warn "Prerequisite missing: $name ($cmd)"
        warn "  $hint"
        return 1
    fi
}

# ── Per-language installers ──────────────────────────────────────────────────

binary_for() {
    case "$1" in
        rust)       echo "rust-analyzer" ;;
        python)     echo "pyright-langserver" ;;
        typescript) echo "typescript-language-server" ;;
        go)         echo "gopls" ;;
        java)       echo "jdtls" ;;
        kotlin)     echo "kotlin-language-server" ;;
        c)          echo "clangd" ;;
        csharp)     echo "OmniSharp" ;;
        ruby)       echo "solargraph" ;;
    esac
}

is_installed() {
    has_cmd "$(binary_for "$1")"
}

install_rust() {
    if ! check_prereq "Rust toolchain" "rustup" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"; then
        return 1
    fi
    info "Installing rust-analyzer via rustup..."
    rustup component add rust-analyzer
    ok "rust-analyzer installed"
}

install_python() {
    if ! check_prereq "Node.js" "npm" "Install Node.js 18+: https://nodejs.org/"; then
        return 1
    fi
    info "Installing pyright via npm..."
    npm install -g pyright
    ok "pyright-langserver installed"
}

install_typescript() {
    if ! check_prereq "Node.js" "npm" "Install Node.js 18+: https://nodejs.org/"; then
        return 1
    fi
    info "Installing typescript-language-server via npm..."
    npm install -g typescript-language-server typescript
    ok "typescript-language-server installed (covers TS, JS, TSX, JSX)"
}

install_go() {
    if ! check_prereq "Go" "go" "Install Go 1.21+: https://go.dev/dl/"; then
        return 1
    fi
    info "Installing gopls via go install..."
    go install golang.org/x/tools/gopls@latest
    ok "gopls installed (ensure \$(go env GOPATH)/bin is on PATH)"
}

install_java() {
    local os arch tag url
    os="$(detect_os)"
    arch="$(detect_arch)"

    if ! check_prereq "Java JDK 17+" "java" "Install JDK 17+: https://adoptium.net/"; then
        return 1
    fi

    ensure_install_dir
    ensure_download_dir

    info "Fetching latest jdtls release..."
    # jdtls uses a milestone page; we fetch the latest from download.eclipse.org
    local base_url="https://download.eclipse.org/jdtls/milestones"
    # Use a known stable approach: download the latest snapshot listing
    local version
    version=$(curl -fsSL "${base_url}/" \
        | grep -oP 'href="\K[0-9]+\.[0-9]+\.[0-9]+/' | sort -V | tail -1 | tr -d '/')

    if [[ -z "$version" ]]; then
        err "Could not determine latest jdtls version. Install manually:"
        err "  https://download.eclipse.org/jdtls/milestones/"
        return 1
    fi

    local tarball="jdt-language-server-${version}.tar.gz"
    url="${base_url}/${version}/${tarball}"

    info "Downloading jdtls ${version}..."
    curl -fSL -o "${DOWNLOAD_DIR}/${tarball}" "$url"

    local install_path="${HOME}/.local/share/jdtls"
    rm -rf "$install_path"
    mkdir -p "$install_path"
    tar xzf "${DOWNLOAD_DIR}/${tarball}" -C "$install_path"

    # Create wrapper script
    cat > "${INSTALL_DIR}/jdtls" <<'WRAPPER'
#!/usr/bin/env bash
JDTLS_HOME="${HOME}/.local/share/jdtls"
LAUNCHER=$(find "$JDTLS_HOME/plugins" -name 'org.eclipse.equinox.launcher_*.jar' | head -1)
CONFIG_DIR="$JDTLS_HOME/config_$(uname -s | tr '[:upper:]' '[:lower:]')"
DATA_DIR="${HOME}/.cache/jdtls/workspace-$(echo "$PWD" | md5sum | cut -d' ' -f1)"
exec java \
    -Declipse.application=org.eclipse.jdt.ls.core.id1 \
    -Dosgi.bundles.defaultStartLevel=4 \
    -Declipse.product=org.eclipse.jdt.ls.core.product \
    -noverify \
    -Xms256m \
    -jar "$LAUNCHER" \
    -configuration "$CONFIG_DIR" \
    -data "$DATA_DIR" \
    "$@"
WRAPPER
    chmod +x "${INSTALL_DIR}/jdtls"
    ok "jdtls ${version} installed to ${install_path}"
}

install_kotlin() {
    local os arch tag url
    os="$(detect_os)"
    arch="$(detect_arch)"

    ensure_install_dir
    ensure_download_dir

    info "Fetching latest kotlin-language-server release..."
    tag=$(github_latest_tag "fwcd/kotlin-language-server")
    if [[ -z "$tag" ]]; then
        err "Could not determine latest kotlin-language-server release."
        return 1
    fi

    url="https://github.com/fwcd/kotlin-language-server/releases/download/${tag}/server.zip"
    info "Downloading kotlin-language-server ${tag}..."
    curl -fSL -o "${DOWNLOAD_DIR}/kls.zip" "$url"

    local install_path="${HOME}/.local/share/kotlin-language-server"
    rm -rf "$install_path"
    mkdir -p "$install_path"
    unzip -qo "${DOWNLOAD_DIR}/kls.zip" -d "$install_path"

    # The zip extracts to server/ subdirectory
    if [[ -d "${install_path}/server" ]]; then
        # Symlink the launcher script
        ln -sf "${install_path}/server/bin/kotlin-language-server" "${INSTALL_DIR}/kotlin-language-server"
    else
        ln -sf "${install_path}/bin/kotlin-language-server" "${INSTALL_DIR}/kotlin-language-server"
    fi
    chmod +x "${INSTALL_DIR}/kotlin-language-server"
    ok "kotlin-language-server ${tag} installed"
}

install_c() {
    local os
    os="$(detect_os)"

    if [[ "$os" == "macos" ]]; then
        if ! check_prereq "Homebrew" "brew" "Install Homebrew: https://brew.sh/"; then
            info "Alternatively: xcode-select --install (includes clangd)"
            return 1
        fi
        info "Installing clangd via brew..."
        brew install llvm
        # brew llvm puts clangd in a non-standard path
        local llvm_bin
        llvm_bin="$(brew --prefix llvm)/bin"
        if [[ -f "${llvm_bin}/clangd" ]] && ! has_cmd clangd; then
            ln -sf "${llvm_bin}/clangd" "${INSTALL_DIR}/clangd"
            info "Symlinked clangd to ${INSTALL_DIR}/clangd"
        fi
    else
        # Linux — try apt, dnf, pacman in order
        if has_cmd apt-get; then
            info "Installing clangd via apt..."
            sudo apt-get install -y clangd
        elif has_cmd dnf; then
            info "Installing clangd via dnf..."
            sudo dnf install -y clang-tools-extra
        elif has_cmd pacman; then
            info "Installing clangd via pacman..."
            sudo pacman -S --noconfirm clang
        else
            err "No supported package manager found (apt, dnf, pacman)."
            err "Install clangd manually: https://clangd.llvm.org/installation"
            return 1
        fi
    fi
    ok "clangd installed (covers C and C++)"
}

install_csharp() {
    local os arch tag url
    os="$(detect_os)"
    arch="$(detect_arch)"

    ensure_install_dir
    ensure_download_dir

    info "Fetching latest OmniSharp release..."
    tag=$(github_latest_tag "OmniSharp/omnisharp-roslyn")
    if [[ -z "$tag" ]]; then
        err "Could not determine latest OmniSharp release."
        return 1
    fi

    # Build platform suffix
    local platform
    if [[ "$os" == "macos" ]]; then
        if [[ "$arch" == "arm64" ]]; then
            platform="osx-arm64-net6.0"
        else
            platform="osx-x64-net6.0"
        fi
    else
        if [[ "$arch" == "arm64" ]]; then
            platform="linux-arm64-net6.0"
        else
            platform="linux-x64-net6.0"
        fi
    fi

    url="https://github.com/OmniSharp/omnisharp-roslyn/releases/download/${tag}/omnisharp-${platform}.tar.gz"
    info "Downloading OmniSharp ${tag} (${platform})..."
    curl -fSL -o "${DOWNLOAD_DIR}/omnisharp.tar.gz" "$url"

    local install_path="${HOME}/.local/share/omnisharp"
    rm -rf "$install_path"
    mkdir -p "$install_path"
    tar xzf "${DOWNLOAD_DIR}/omnisharp.tar.gz" -C "$install_path"

    # Create wrapper that invokes with -lsp
    cat > "${INSTALL_DIR}/OmniSharp" <<WRAPPER
#!/usr/bin/env bash
exec "${install_path}/OmniSharp" -lsp "\$@"
WRAPPER
    chmod +x "${INSTALL_DIR}/OmniSharp"
    ok "OmniSharp ${tag} installed"
}

install_ruby() {
    if ! check_prereq "Ruby" "gem" "Install Ruby 3.0+: https://www.ruby-lang.org/en/downloads/"; then
        return 1
    fi
    info "Installing solargraph via gem..."
    gem install solargraph
    ok "solargraph installed"
}

# ── Dispatch ─────────────────────────────────────────────────────────────────

install_lang() {
    local lang="$1"
    if is_installed "$lang"; then
        skip "$lang — $(binary_for "$lang") already on PATH"
        return 0
    fi

    case "$lang" in
        rust)       install_rust ;;
        python)     install_python ;;
        typescript) install_typescript ;;
        go)         install_go ;;
        java)       install_java ;;
        kotlin)     install_kotlin ;;
        c)          install_c ;;
        csharp)     install_csharp ;;
        ruby)       install_ruby ;;
        *)          err "Unknown language: $lang"; return 1 ;;
    esac
}

# ── Check mode ───────────────────────────────────────────────────────────────

do_check() {
    local installed=0 missing=0
    printf '\n  %-14s %-30s %s\n' "Language" "Binary" "Status"
    printf '  %-14s %-30s %s\n' "--------" "------" "------"
    for lang in "${ALL_LANGS[@]}"; do
        local bin
        bin="$(binary_for "$lang")"
        if is_installed "$lang"; then
            local path
            path="$(command -v "$bin")"
            printf '  %-14s %-30s \033[1;32m%s\033[0m\n' "$lang" "$bin" "$path"
            installed=$((installed + 1))
        else
            printf '  %-14s %-30s \033[1;31m%s\033[0m\n' "$lang" "$bin" "not found"
            missing=$((missing + 1))
        fi
    done
    printf '\n  %d installed, %d missing\n\n' "$installed" "$missing"

    if ((missing > 0)); then
        info "Install missing servers:"
        info "  ./scripts/install-lsp.sh --all       # install everything"
        info "  ./scripts/install-lsp.sh rust python  # install specific languages"
    fi
}

# ── Main ─────────────────────────────────────────────────────────────────────

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS] [LANGUAGES...]

Install LSP servers for codescout.

Options:
  --check   Show which LSP servers are installed / missing
  --all     Install all supported LSP servers
  --help    Show this help message

Languages: ${ALL_LANGS[*]}

Examples:
  $(basename "$0") --check
  $(basename "$0") --all
  $(basename "$0") rust python typescript go
EOF
}

main() {
    if [[ $# -eq 0 ]]; then
        usage
        exit 0
    fi

    local os arch
    os="$(detect_os)"
    arch="$(detect_arch)"
    info "Platform: ${os} ${arch}"

    case "$1" in
        --check)
            do_check
            ;;
        --all)
            ensure_install_dir
            for lang in "${ALL_LANGS[@]}"; do
                install_lang "$lang" || true
            done
            echo
            do_check
            ;;
        --help|-h)
            usage
            ;;
        *)
            ensure_install_dir
            for lang in "$@"; do
                install_lang "$lang" || true
            done
            ;;
    esac
}

main "$@"
