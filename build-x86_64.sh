#!/bin/sh
# ============================================================
# ohosHttp - Linux x86_64 专用构建脚本
# 用法: ./build-x86_64.sh [--rebuild]
#   --rebuild  重新编译（安装系统依赖 + clean 构建）
# ============================================================

set -e

# ---- 颜色输出 ----
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; NC='\033[0m'
info()  { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$*"; }
step()  { printf "\n${CYAN}━━━ %s ━━━${NC}\n" "$*"; }

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$PROJECT_DIR"
TARGET="x86_64-unknown-linux-gnu"

# ---- 解析参数 ----
REBUILD=0
for arg in "$@"; do
    [ "$arg" = "--rebuild" ] && REBUILD=1
done

# ============================================================
# 步骤 1: 安装系统依赖（仅 --rebuild 时执行）
# ============================================================
if [ "$REBUILD" -eq 1 ]; then
    step "步骤 1/3: 安装系统依赖"

    if command -v apt-get >/dev/null 2>&1; then
        info "检测到 Debian/Ubuntu 系，安装依赖..."
        sudo apt-get update -qq
        sudo apt-get install -y -qq \
            build-essential \
            cmake \
            pkg-config \
            libssl-dev \
            llvm-dev \
            libclang-dev \
            curl 2>&1 | tail -1
        ok "系统依赖安装完成"
    elif command -v dnf >/dev/null 2>&1; then
        info "检测到 Fedora 系，安装依赖..."
        sudo dnf install -y \
            gcc gcc-c++ cmake pkgconfig openssl-devel llvm-devel clang-devel curl
        ok "系统依赖安装完成"
    elif command -v yum >/dev/null 2>&1; then
        info "检测到 RHEL/CentOS 系，安装依赖..."
        sudo yum install -y \
            gcc gcc-c++ cmake pkgconfig openssl-devel llvm-devel clang-devel curl
        ok "系统依赖安装完成"
    elif command -v pacman >/dev/null 2>&1; then
        info "检测到 Arch 系，安装依赖..."
        sudo pacman -S --noconfirm --needed \
            base-devel cmake pkg-config openssl llvm clang curl
        ok "系统依赖安装完成"
    else
        warn "未识别的包管理器，请确保已安装: gcc, cmake, pkg-config, libssl-dev, curl, llvm-dev, libclang-dev"
    fi
else
    # 不安装依赖，只检查关键命令
    if ! command -v gcc >/dev/null 2>&1; then
        warn "未检测到 gcc，编译可能失败。请运行以下命令安装依赖后重试："
        warn "  - Debian/Ubuntu: sudo apt-get install -y build-essential"
        warn "  - Fedora:        sudo dnf install -y gcc gcc-c++"
        warn "  - 或者直接使用:  $0 --rebuild  (自动安装依赖)"
    fi
fi

# ============================================================
# 步骤 2: 安装 Rust（通过 rustup）
# ============================================================
step "步骤 2/3: 检查 Rust 环境"

install_rust() {
    info "正在安装 Rust (rustup)..."
    if ! command -v curl >/dev/null 2>&1; then
        error "需要 curl 来安装 Rust"
        exit 1
    fi
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    . "$HOME/.cargo/env"
    ok "Rust 已安装: $(rustc --version)"
}

if command -v rustc >/dev/null 2>&1; then
    ok "Rust 已安装: $(rustc --version)"
else
    warn "未检测到 Rust，正在安装..."
    install_rust
fi

# 确保 cargo 可用
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        . "$HOME/.cargo/env"
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        error "cargo 仍不可用，请检查 Rust 安装"
        exit 1
    fi
fi
ok "Cargo 可用: $(cargo --version)"

# 确保目标已安装
if command -v rustup >/dev/null 2>&1; then
    if ! rustup target list --installed 2>/dev/null | grep -q "^$TARGET$"; then
        info "安装目标: $TARGET ..."
        rustup target add "$TARGET"
        ok "目标安装完成: $TARGET"
    else
        ok "目标已安装: $TARGET"
    fi
fi

# ============================================================
# 步骤 3: 编译
# ============================================================
step "步骤 3/3: 编译 $TARGET"

if [ "$REBUILD" -eq 1 ]; then
    info "执行 clean 构建..."
    cargo clean --target "$TARGET"
fi

info "编译 Linux x86_64 版本..."
export CC=gcc
cargo build --release --target "$TARGET"
cp "target/$TARGET/release/ohosHttp" "target/release/ohosHttp-x86_64-linux"

echo ""
ok "构建成功！"
printf "  输出: ${CYAN}target/release/ohosHttp-x86_64-linux${NC}\n"
printf "  运行: ${YELLOW}./target/release/ohosHttp-x86_64-linux -a 127.0.0.1:8089 -r ./www${NC}\n"
echo ""
