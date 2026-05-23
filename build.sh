#!/bin/sh
# ============================================================
# ohos-server 智能构建调度脚本
# 自动检测当前平台并调用对应的平台专用构建脚本
# 用法: ./build.sh [target-triple]
#   - 不传参数: 自动检测当前系统并调用对应脚本
#   - 传目标三元组: 直接调用 cargo build --target
# ============================================================

set -e

# ---- 颜色输出 ----
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; NC='\033[0m'
info()  { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$*"; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# ---- 解析参数 ----
REBUILD=0
TARGET=""
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        *) TARGET="$arg" ;;  # 最后一个非标志参数作为目标三元组
    esac
done

# ---- 平台检测 ----
OS="$(uname -s)"
ARCH="$(uname -m)"

detect_ohos() {
    if [ "$OS" = "HarmonyOS" ] || \
       [ -f "/system/build.prop" ] 2>/dev/null || \
       echo "$(uname -a 2>/dev/null)" | grep -qiE "ohos|harmony|hongmeng"; then
        return 0
    fi
    return 1
}

if [ -n "$TARGET" ]; then
    # 用户明确指定目标 → 直接编译
    info "用户指定目标: $TARGET (rebuild=$REBUILD)"
    if [ "$REBUILD" -eq 1 ]; then
        info "执行 clean 构建..."
        cargo clean --target "$TARGET"
    fi
    exec cargo build --release --target "$TARGET"
fi

REBUILD_FLAG=""
[ "$REBUILD" -eq 1 ] && REBUILD_FLAG="--rebuild"

# 自动检测
if detect_ohos; then
    info "检测到 HarmonyOS/OpenHarmony 系统"
    info "调用: ./build-ohos.sh $REBUILD_FLAG"
    exec "$SCRIPT_DIR/build-ohos.sh" $REBUILD_FLAG
elif [ "$OS" = "Linux" ]; then
    case "$ARCH" in
        x86_64)
            info "检测到 Linux x86_64"
            info "调用: ./build-x86_64.sh $REBUILD_FLAG"
            exec "$SCRIPT_DIR/build-x86_64.sh" $REBUILD_FLAG
            ;;
        aarch64)
            info "检测到 Linux ARM64"
            info "调用: ./build-arm.sh $REBUILD_FLAG"
            exec "$SCRIPT_DIR/build-arm.sh" $REBUILD_FLAG
            ;;
        *)
            error "不支持的架构: $ARCH"
            echo ""
            echo "请使用平台专用脚本手动构建:"
            echo "  Linux x86_64:    ./build-x86_64.sh"
            echo "  Linux ARM64:     ./build-arm.sh"
            echo "  HarmonyOS ARM64: ./build-ohos.sh"
            exit 1
            ;;
    esac
elif [ "$OS" = "Darwin" ]; then
    case "$ARCH" in
        x86_64)
            TARGET="x86_64-apple-darwin"
            ;;
        aarch64|arm64)
            TARGET="aarch64-apple-darwin"
            ;;
    esac
    info "检测到 macOS ($ARCH)"
    info "编译目标: $TARGET"
    cargo build --release --target "$TARGET"
else
    error "不支持的操作系统: $OS"
    exit 1
fi

echo ""
ok "构建完成！"
