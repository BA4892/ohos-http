#!/bin/sh
# ============================================================
# ohosHttp 智能构建脚本
# 自动检测系统 → 安装 Rust → 安装依赖 → 编译
# 用法: ./build.sh [target-triple]
#   不传参数: 自动检测当前系统并编译原生版本
#   传目标三元组: 交叉编译到指定平台
# ============================================================

set -e

# ---- 颜色输出 ----
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

info()  { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$*"; }
step()  { printf "\n${CYAN}━━━ %s ━━━${NC}\n" "$*"; }

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$PROJECT_DIR"

# ============================================================
# 步骤 1: 检测系统信息
# ============================================================
step "步骤 1/5: 检测系统信息"

OS="$(uname -s)"
ARCH="$(uname -m)"
DISTRO="unknown"
KERNEL_VERSION="$(uname -r)"

info "内核:      $OS $KERNEL_VERSION"
info "架构:      $ARCH"

# Linux 发行版检测
detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        DISTRO="${ID:-linux}"
        DISTRO_VERSION="${VERSION_ID:-}"
        info "发行版:    $DISTRO $DISTRO_VERSION"
    elif [ -f /etc/redhat-release ]; then
        DISTRO=$(head -1 /etc/redhat-release | awk '{print tolower($1)}')
        info "发行版:    $(head -1 /etc/redhat-release)"
    elif [ "$(uname -s)" = "Darwin" ]; then
        DISTRO="macos"
        MACOS_VERSION=$(sw_vers -productVersion 2>/dev/null || echo "unknown")
        info "macOS:      $MACOS_VERSION"
    elif [ "$(uname -s)" = "HarmonyOS" ]; then
        DISTRO="ohos"
        info "发行版:    HarmonyOS / OpenHarmony"
    else
        warn "无法识别发行版，使用通用配置"
    fi
}
detect_distro

# ============================================================
# 步骤 2: 确定目标平台
# ============================================================
step "步骤 2/5: 确定编译目标"

USER_TARGET="${1:-auto}"

if [ "$USER_TARGET" = "auto" ]; then
    # 自动检测当前平台的目标三元组
    case "$OS" in
        HarmonyOS|Linux)
            case "$ARCH" in
                x86_64)
                    TARGET="x86_64-unknown-linux-gnu"
                    ;;
                aarch64)
                    if [ "$DISTRO" = "ohos" ]; then
                        TARGET="aarch64-unknown-linux-ohos"
                    else
                        TARGET="aarch64-unknown-linux-gnu"
                    fi
                    ;;
                armv7l|armhf)
                    TARGET="armv7-unknown-linux-gnueabihf"
                    ;;
                riscv64)
                    TARGET="riscv64gc-unknown-linux-gnu"
                    ;;
                *)
                    echo "不支持的原生架构: $ARCH"
                    echo "请手动指定目标三元组，例如:"
                    echo "  ./build.sh x86_64-unknown-linux-gnu"
                    exit 1
                    ;;
            esac
            ;;
        Darwin)
            case "$ARCH" in
                x86_64)  TARGET="x86_64-apple-darwin" ;;
                aarch64) TARGET="aarch64-apple-darwin" ;;
                *)       echo "不支持的 macOS 架构: $ARCH"; exit 1 ;;
            esac
            ;;
        *)
            echo "不支持的操作系统: $OS"
            exit 1
            ;;
    esac
else
    TARGET="$USER_TARGET"
fi

info "目标平台:  ${CYAN}$TARGET${NC}"

# 判断是否为 ohos 目标
IS_OHOS=0
case "$TARGET" in
    *ohos*) IS_OHOS=1 ;;
esac

# ============================================================
# 步骤 3: 安装 Rust（如未安装）
# ============================================================
step "步骤 3/5: 检查 Rust 环境"

install_rust_via_rustup() {
    info "正在安装 Rust (rustup)..."
    if ! command -v curl >/dev/null 2>&1; then
        error "需要 curl 来安装 Rust，请先安装 curl"
        exit 1
    fi
    # 非交互式安装
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # 加载 cargo 环境
    . "$HOME/.cargo/env"
    ok "Rust 已安装: $(rustc --version)"
}

# 检查 rustc 是否可用
if command -v rustc >/dev/null 2>&1; then
    RUSTC_VERSION=$(rustc --version)
    ok "Rust 已安装: $RUSTC_VERSION"
elif [ "$IS_OHOS" = "1" ] && [ "$DISTRO" = "ohos" ]; then
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
        error "在 HarmonyOS 上，请先安装 Rust aarch64-unknown-linux-ohos 工具链"
        error "或从 https://gitcode.com/cncoder/ohos-http/releases 下载预编译版本"
        exit 1
    fi
else
    warn "未检测到 Rust，正在安装..."
    install_rust_via_rustup
fi

# 确保 cargo 在 PATH 中
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        . "$HOME/.cargo/env"
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        error "cargo 仍不可用，请检查 Rust 安装"
        exit 1
    fi
fi

CARGO_VERSION=$(cargo --version)
ok "Cargo 可用: $CARGO_VERSION"

# ============================================================
# 步骤 4: 安装系统依赖
# ============================================================
step "步骤 4/5: 安装系统依赖"

# 定义各发行版的依赖包
install_deps_debian() {
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
}

install_deps_redhat() {
    info "检测到 RedHat/Fedora 系，安装依赖..."
    if command -v dnf >/dev/null 2>&1; then
        PKG_MGR="dnf"
    else
        PKG_MGR="yum"
    fi
    sudo $PKG_MGR install -y \
        gcc gcc-c++ \
        cmake \
        pkgconfig \
        openssl-devel \
        llvm-devel \
        clang \
        curl 2>&1 | tail -1
}

install_deps_alpine() {
    info "检测到 Alpine Linux，安装依赖..."
    apk add --no-cache \
        build-base \
        cmake \
        pkgconfig \
        openssl-dev \
        llvm-dev \
        clang \
        curl 2>&1 | tail -1
}

install_deps_macos() {
    info "检测到 macOS，检查 Xcode Command Line Tools..."
    if ! command -v xcode-select >/dev/null 2>&1; then
        warn "请先安装 Xcode Command Line Tools:"
        warn "  xcode-select --install"
        warn "安装完成后重新运行此脚本"
        exit 1
    fi
    if ! xcode-select -p >/dev/null 2>&1; then
        warn "Xcode CLT 未安装，正在引导安装..."
        xcode-select --install
        warn "请等待安装完成后重新运行此脚本"
        exit 1
    fi
    ok "Xcode CLT 已安装"
    # macOS 下通过 Homebrew 安装 cmake / pkg-config（如缺失）
    if ! command -v cmake >/dev/null 2>&1; then
        if command -v brew >/dev/null 2>&1; then
            brew install cmake pkg-config
        else
            warn "cmake 未安装，请安装 Homebrew 后运行 'brew install cmake'"
        fi
    fi
}

# 检查关键编译工具
check_essential_tools() {
    local MISSING=""
    for tool in cc gcc clang; do
        if command -v "$tool" >/dev/null 2>&1; then
            return 0
        fi
    done
    # macOS uses cc via Xcode CLT
    if [ "$(uname -s)" = "Darwin" ]; then
        if xcode-select -p >/dev/null 2>&1; then
            return 0
        fi
    fi
    return 1
}

# 检查 cmake（quinn 所需）
check_cmake() {
    command -v cmake >/dev/null 2>&1
}

# 检查 pkg-config（openssl-sys 所需）
check_pkg_config() {
    command -v pkg-config >/dev/null 2>&1
}

NEED_DEPS=0
check_essential_tools || NEED_DEPS=1
check_cmake || NEED_DEPS=1
check_pkg_config || NEED_DEPS=1

if [ "$NEED_DEPS" = "1" ]; then
    info "检测到缺少编译依赖，尝试安装..."
    case "$DISTRO" in
        debian|ubuntu|linuxmint|elementary|pop|zorin)
            install_deps_debian
            ;;
        rhel|centos|fedora|rocky|almalinux|ol|amzn)
            install_deps_redhat
            ;;
        alpine)
            install_deps_alpine
            ;;
        macos)
            install_deps_macos
            ;;
        ohos|*)
            warn "HarmonyOS / 未知系统：跳过依赖安装（请确保已安装编译工具）"
            info "需要的系统依赖：gcc/clang、cmake、pkg-config、libssl-dev、llvm-dev、libclang-dev"
            # ohos 系统上检查必要的库
            if ! check_essential_tools; then
                error "缺少 C 编译器！请先安装 build-essential 或对应工具链"
                exit 1
            fi
            ok "C 编译器可用"
            ;;
    esac

    # 再次验证
    check_essential_tools || { error "C 编译器仍不可用，请手动安装"; exit 1; }
    ok "C 编译器: $(cc --version 2>/dev/null | head -1)"
    check_cmake && ok "cmake: $(cmake --version 2>/dev/null | head -1)" || warn "cmake 未安装（仅 QUIC 功能需要）"
    check_pkg_config && ok "pkg-config: $(pkg-config --version 2>/dev/null)" || warn "pkg-config 未安装"
else
    ok "编译依赖已满足"
    cc --version 2>/dev/null | head -1 | while read -r line; do printf "    C 编译器: %s\n" "$line"; done
    cmake --version 2>/dev/null | head -1 | while read -r line; do printf "    cmake:     %s\n" "$line"; done
fi

# ============================================================
# 步骤 5: 安装 Rust 目标 + 编译
# ============================================================
step "步骤 5/5: 准备编译"

# 安装目标（如果通过 rustup 管理）
if command -v rustup >/dev/null 2>&1; then
    if rustup target list --installed 2>/dev/null | grep -q "^$TARGET$"; then
        ok "目标已安装: $TARGET"
    else
        info "安装目标: $TARGET ..."
        rustup target add "$TARGET"
        ok "目标安装完成: $TARGET"
    fi
else
    # 没有 rustup — HarmonyOS 场景
    if [ "$TARGET" != "$(rustc -vV | grep host | awk '{print $2}')" ]; then
        if [ "$IS_OHOS" = "0" ]; then
            warn "无法添加目标 $TARGET（无 rustup）"
            warn "将尝试使用当前主机工具链编译..."
        fi
    fi
fi

# ---- 编译 ----
echo ""
case "$TARGET" in
    aarch64-unknown-linux-ohos)
        info "编译 HarmonyOS (OpenHarmony) ARM64 版本..."
        cargo build --release --target aarch64-unknown-linux-ohos
        cp "target/aarch64-unknown-linux-ohos/release/ohosHttp" "target/release/ohosHttp-aarch64-ohos"
        echo ""
        ok "输出: target/release/ohosHttp-aarch64-ohos"
        ;;
    x86_64-unknown-linux-gnu)
        info "编译 Linux x86_64 版本..."
        cargo build --release --target x86_64-unknown-linux-gnu
        cp "target/x86_64-unknown-linux-gnu/release/ohosHttp" "target/release/ohosHttp-x86_64-linux"
        echo ""
        ok "输出: target/release/ohosHttp-x86_64-linux"
        ;;
    aarch64-unknown-linux-gnu)
        info "编译 Linux ARM64 版本..."
        cargo build --release --target aarch64-unknown-linux-gnu
        cp "target/aarch64-unknown-linux-gnu/release/ohosHttp" "target/release/ohosHttp-aarch64-linux"
        echo ""
        ok "输出: target/release/ohosHttp-aarch64-linux"
        ;;
    aarch64-apple-darwin)
        info "编译 macOS ARM64 (Apple Silicon) 版本..."
        cargo build --release --target aarch64-apple-darwin
        cp "target/aarch64-apple-darwin/release/ohosHttp" "target/release/ohosHttp-aarch64-darwin"
        echo ""
        ok "输出: target/release/ohosHttp-aarch64-darwin"
        ;;
    x86_64-apple-darwin)
        info "编译 macOS x86_64 (Intel) 版本..."
        cargo build --release --target x86_64-apple-darwin
        cp "target/x86_64-apple-darwin/release/ohosHttp" "target/release/ohosHttp-x86_64-darwin"
        echo ""
        ok "输出: target/release/ohosHttp-x86_64-darwin"
        ;;
    *)
        info "编译 $TARGET 版本..."
        cargo build --release --target "$TARGET"
        # 让用户自行处理输出文件
        echo ""
        ok "编译完成！"
        info "二进制: target/$TARGET/release/ohosHttp"
        ;;
esac

# ---- 完成 ----
echo ""
printf "${GREEN}═══════════════════════════════════════════${NC}\n"
printf "${GREEN}  构建成功！${NC}\n"
printf "${GREEN}═══════════════════════════════════════════${NC}\n"
echo ""
printf "  目标平台:  ${CYAN}%s${NC}\n" "$TARGET"

# 列出生成的二进制
echo ""
ls -lh target/release/ohosHttp-* 2>/dev/null && echo "" || true

# 如果构建的是当前平台，显示如何运行
HOST_ARCH="$(rustc -vV 2>/dev/null | grep host | awk '{print $2}' || echo 'unknown')"
if echo "$HOST_ARCH" | grep -q "$TARGET" 2>/dev/null || [ "$TARGET" = "$(rustc -vV 2>/dev/null | grep host | awk '{print $2}')" ]; then
    printf "  快速启动:  ${YELLOW}./target/release/ohosHttp -a 127.0.0.1:8089 -r ./www${NC}\n"
    printf "  帮助:      ${YELLOW}./target/release/ohosHttp --help${NC}\n"
fi
echo ""
