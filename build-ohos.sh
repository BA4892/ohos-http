#!/bin/sh
# ============================================================
# ohosHttp - HarmonyOS/OpenHarmony ARM64 专用构建脚本
# 用法: ./build-ohos.sh [--rebuild]
#   --rebuild  重新编译（clean 构建）
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
TARGET="aarch64-unknown-linux-ohos"

# ---- 解析参数 ----
REBUILD=0
for arg in "$@"; do
    [ "$arg" = "--rebuild" ] && REBUILD=1
done

# ============================================================
# 步骤 1: 检查 Rust 环境（鸿蒙预安装版）
# ============================================================
step "步骤 1/2: 检查 Rust 环境"

if command -v rustc >/dev/null 2>&1; then
    RUSTC_VERSION=$(rustc --version)
    ok "Rust 已安装: $RUSTC_VERSION"
else
    # HarmonyOS 上查找预安装的 Rust
    RUST_DIR=""
    for dir in /storage/Users/currentUser/usr/rust-* "$HOME/rust-*" /opt/rust-* /usr/local/rust-*; do
        if [ -d "$dir" ] && [ -f "$dir/bin/rustc" ]; then
            RUST_DIR="$dir"
            break
        fi
    done
    if [ -n "$RUST_DIR" ]; then
        export PATH="$RUST_DIR/bin:$PATH"
        RUSTC_VERSION=$(rustc --version)
        ok "发现 HarmonyOS Rust: $RUSTC_VERSION ($RUST_DIR)"
    else
        error "未找到 Rust 编译器！"
        error "请从 https://gitcode.com/OpenHarmonyPCDeveloper/rust/releases 下载安装"
        exit 1
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    error "cargo 不可用，请检查 Rust 安装"
    exit 1
fi
ok "Cargo 可用: $(cargo --version)"

# ============================================================
# 步骤 2: 编译
# ============================================================
step "步骤 2/2: 编译 $TARGET"

if [ "$REBUILD" -eq 1 ]; then
    info "执行 clean 构建..."
    cargo clean --target "$TARGET"
fi

if command -v rustup >/dev/null 2>&1; then
    if ! rustup target list --installed 2>/dev/null | grep -q "^$TARGET$"; then
        info "安装目标: $TARGET ..."
        rustup target add "$TARGET"
        ok "目标安装完成: $TARGET"
    else
        ok "目标已安装: $TARGET"
    fi
fi

info "编译 HarmonyOS (OpenHarmony) ARM64 版本..."
cargo build --release --target "$TARGET"
mkdir -p target/release
cp "target/$TARGET/release/ohosHttp" "target/release/ohosHttp-aarch64-ohos"

echo ""
ok "构建成功！"
printf "  输出: ${CYAN}target/release/ohosHttp-aarch64-ohos${NC}\n"
printf "  运行: ${YELLOW}./target/release/ohosHttp-aarch64-ohos -a 127.0.0.1:8089 -r ./www${NC}\n"
echo ""
