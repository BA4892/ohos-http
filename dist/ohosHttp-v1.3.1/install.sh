#!/system/bin/sh
# ============================================================
# ohosHttp - 鸿蒙系统一键安装脚本
# 适用于 OpenHarmony / HarmonyOS ARM64 平台
# 用法: chmod +x install.sh && ./install.sh
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

# ---- 横幅 ----
cat << 'BANNER'
=========================================
  ohosHttp v1.3.1 - 鸿蒙系统一键安装
  高性能 HTTP 服务器
  HarmonyOS ARM64
=========================================
BANNER

# ---- 检查环境 ----
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local}"
BIN_DIR="$INSTALL_DIR/bin"
WWW_DIR="${WWW_DIR:-$HOME/www}"
CONFIG_DIR="${CONFIG_DIR:-$HOME/.config/ohosHttp}"
LOGS_DIR="$HOME/logs/ohosHttp"

# ── 平台检测 ──
detect_platform() {
    local arch
    arch=$(uname -m 2>/dev/null)
    local os
    os=$(uname -s 2>/dev/null)

    # 检测是否为鸿蒙系统：
    #   1. uname -s = HarmonyOS（标准 OHOS 内核返回 HarmonyOS）
    #   2. 存在 /system/build.prop
    #   3. uname -a 包含 ohos, harmony 或 hongmeng
    if [ "$os" = "HarmonyOS" ] || \
       [ -f "/system/build.prop" ] 2>/dev/null || \
       echo "$(uname -a 2>/dev/null)" | grep -qiE "ohos|harmony|hongmeng"; then
        echo "ohos"
    elif [ "$os" = "Linux" ] && [ "$arch" = "x86_64" ]; then
        echo "linux-x86_64"
    elif [ "$os" = "Linux" ] && [ "$arch" = "aarch64" ]; then
        echo "linux-aarch64"
    elif [ "$os" = "Darwin" ] && [ "$arch" = "x86_64" ]; then
        echo "darwin-x86_64"
    elif [ "$os" = "Darwin" ] && [ "$arch" = "arm64" ]; then
        echo "darwin-arm64"
    else
        echo "unknown"
    fi
}
CURRENT_PLATFORM=$(detect_platform)

# ---- 步骤 1: 创建目录结构 ----
info "创建目录结构..."
mkdir -p "$BIN_DIR"
mkdir -p "$WWW_DIR"
mkdir -p "$CONFIG_DIR"
mkdir -p "$LOGS_DIR"
ok "目录创建完成"

# ---- 步骤 2: 安装二进制文件 ----
info "安装 ohosHttp 二进制..."

# 查找二进制文件（支持 build.sh 所有输出名）
BINARY_SOURCE=""
for candidate in \
    "$SCRIPT_DIR/target/release/ohosHttp" \
    "$SCRIPT_DIR/target/release/ohosHttp-aarch64-ohos" \
    "$SCRIPT_DIR/target/release/ohosHttp-x86_64-linux" \
    "$SCRIPT_DIR/target/release/ohosHttp-aarch64-linux" \
    "$SCRIPT_DIR/target/release/ohosHttp-aarch64-darwin" \
    "$SCRIPT_DIR/target/release/ohosHttp-x86_64-darwin" \
    "$SCRIPT_DIR/ohosHttp" \
    "$SCRIPT_DIR/ohosHttp-aarch64-ohos" \
    "$SCRIPT_DIR/target/aarch64-unknown-linux-ohos/release/ohosHttp" \
    "$SCRIPT_DIR/target/x86_64-unknown-linux-gnu/release/ohosHttp"; do
    if [ -f "$candidate" ] && [ -x "$candidate" ]; then
        BINARY_SOURCE="$candidate"
        break
    fi
done

# 如果未找到二进制，调用 build.sh 源码编译
if [ -z "$BINARY_SOURCE" ]; then
    warn "未找到预编译的 ohosHttp 二进制"
    echo ""
    printf "  ${YELLOW}将调用 build.sh 从源代码编译...${NC}\n"
    echo ""

    # ── 检查 Rust 工具链 ──
    info "检查 Rust 工具链..."
    RUST_INSTALLED=false
    if command -v rustc >/dev/null 2>&1 && command -v cargo >/dev/null 2>&1; then
        RUST_VERSION=$(rustc --version 2>/dev/null | head -1)
        info "已检测到 Rust 工具链: ${RUST_VERSION}"
        RUST_INSTALLED=true
    else
        warn "未检测到 Rust 工具链"
        echo ""

        # ── 根据平台选择 Rust 安装方式 ──
        case "$CURRENT_PLATFORM" in
            ohos)
                # ═══ HarmonyOS OHOS：安装鸿蒙版 Rust ═══
                printf "  ${CYAN}是否自动安装 HarmonyOS 版 Rust v1.95.0？[Y/n]: ${NC}"
                read -r rust_choice </dev/tty 2>/dev/null || rust_choice="y"
                case "$rust_choice" in
                    n|N|no|NO)
                        warn "跳过 Rust 安装，请手动安装 Rust 后重新运行本脚本"
                        warn "鸿蒙 PC 版 Rust 安装地址:"
                        warn "  https://gitcode.com/OpenHarmonyPCDeveloper/rust/releases/download/v1.95.0/install.sh"
                        echo ""
                        printf "  ${CYAN}是否继续编译（若已有 Rust 工具链）？[y/N]: ${NC}"
                        read -r skip_choice </dev/tty 2>/dev/null || skip_choice="n"
                        case "$skip_choice" in
                            y|Y|yes|YES) ;;
                            *) exit 1 ;;
                        esac
                        ;;
                    *)
                        info "正在安装 HarmonyOS 版 Rust v1.95.0..."
                        RUST_INSTALL_URL="https://gitcode.com/OpenHarmonyPCDeveloper/rust/releases/download/v1.95.0/install.sh"
                        RUST_INSTALLER_DOWNLOADED=1
                        if command -v curl >/dev/null 2>&1; then
                            # 先尝试完整参数（支持 OpenSSL 的完整 curl）
                            RUST_INSTALL_SCRIPT=$(curl -fsSL --retry 3 --connect-timeout 30 "${RUST_INSTALL_URL}" 2>/dev/null) && RUST_INSTALLER_DOWNLOADED=0 || \
                            # 再尝试精简参数（busybox curl 不支持长选项）
                            RUST_INSTALL_SCRIPT=$(curl -fsSL "${RUST_INSTALL_URL}" 2>/dev/null) && RUST_INSTALLER_DOWNLOADED=0 || \
                            # 最后尝试跳过证书验证
                            RUST_INSTALL_SCRIPT=$(curl -fsSLk "${RUST_INSTALL_URL}" 2>/dev/null) && RUST_INSTALLER_DOWNLOADED=0 || true
                            if [ "$RUST_INSTALLER_DOWNLOADED" -eq 0 ] && [ -n "$RUST_INSTALL_SCRIPT" ]; then
                                echo "$RUST_INSTALL_SCRIPT" | /bin/sh && {
                                    ok "HarmonyOS Rust 安装成功"
                                    RUST_INSTALLED=true
                                } || {
                                    warn "curl 下载脚本执行失败，尝试 wget..."
                                    RUST_INSTALLER_DOWNLOADED=1
                                }
                            fi
                            if [ "$RUST_INSTALLED" != true ]; then
                                warn "curl 下载失败，尝试 wget..."
                                RUST_INSTALLER_DOWNLOADED=1
                            fi
                        fi
                        if [ "$RUST_INSTALLED" != true ] && command -v wget >/dev/null 2>&1; then
                            TMP_SCRIPT="${HOME}/tmp/.rust-install-$$.sh"
                            mkdir -p "${HOME}/tmp"
                            # 先尝试带超时参数（GNU wget），不支持下则尝试精简参数（toybox wget）
                            if wget -q -O "${TMP_SCRIPT}" --timeout=30 "${RUST_INSTALL_URL}" 2>/dev/null || \
                               wget -q -O "${TMP_SCRIPT}" "${RUST_INSTALL_URL}" 2>/dev/null; then
                                if /bin/sh "${TMP_SCRIPT}"; then
                                    ok "HarmonyOS Rust 安装成功"
                                    RUST_INSTALLED=true
                                fi
                            fi
                            rm -f "${TMP_SCRIPT}"
                        fi
                        if [ "$RUST_INSTALLED" != true ]; then
                            error "HarmonyOS Rust 安装失败"
                            error "请手动安装:"
                            error "  /bin/sh -c \"\$(curl -fsSL ${RUST_INSTALL_URL})\""
                            error "或从以下地址下载 tar.gz 手动解压:"
                            error "  https://gitcode.com/OpenHarmonyPCDeveloper/rust/releases/tag/v1.95.0"
                            exit 1
                        fi
                        ;;
                esac
                ;;
            *)
                # ═══ 标准系统（Linux / macOS）：使用 rustup ═══
                printf "  ${CYAN}是否自动安装 Rust（通过 rustup）？[Y/n]: ${NC}"
                read -r rust_choice </dev/tty 2>/dev/null || rust_choice="y"
                case "$rust_choice" in
                    n|N|no|NO)
                        warn "跳过 Rust 安装，请手动安装 Rust 后重新运行本脚本"
                        warn "安装命令: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
                        echo ""
                        printf "  ${CYAN}是否继续编译（若已有 Rust 工具链）？[y/N]: ${NC}"
                        read -r skip_choice </dev/tty 2>/dev/null || skip_choice="n"
                        case "$skip_choice" in
                            y|Y|yes|YES) ;;
                            *) exit 1 ;;
                        esac
                        ;;
                    *)
                        info "正在通过 rustup 安装 Rust..."
                        if command -v curl >/dev/null 2>&1; then
                            RUSTUP_SCRIPT=$(curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs 2>/dev/null) && {
                                echo "$RUSTUP_SCRIPT" | /bin/sh -s -- -y 2>/dev/null && {
                                    ok "Rust 安装成功"
                                    RUST_INSTALLED=true
                                    export PATH="$HOME/.cargo/bin:$PATH"
                                } || {
                                    warn "rustup 安装失败"
                                }
                            } || {
                                warn "curl 下载 rustup 失败"
                            }
                        fi
                        if [ "$RUST_INSTALLED" != true ] && command -v wget >/dev/null 2>&1; then
                            TMP_SCRIPT="${HOME}/tmp/.rustup-$$.sh"
                            mkdir -p "${HOME}/tmp"
                            if wget -q -O "${TMP_SCRIPT}" https://sh.rustup.rs 2>/dev/null; then
                                /bin/sh "${TMP_SCRIPT}" -y 2>/dev/null && {
                                    ok "Rust 安装成功"
                                    RUST_INSTALLED=true
                                    export PATH="$HOME/.cargo/bin:$PATH"
                                }
                            fi
                            rm -f "${TMP_SCRIPT}"
                        fi
                        if [ "$RUST_INSTALLED" != true ]; then
                            error "Rust 安装失败"
                            error "请手动安装:"
                            error "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
                            exit 1
                        fi
                        ;;
                esac
                ;;
        esac
    fi

    # ── 提前检测 shell 配置文件（供 CC 配置持久化使用） ──
    SHELL_PROFILE=""
    if [ -n "$BASH" ] && [ -f "$HOME/.bashrc" ]; then
        SHELL_PROFILE="$HOME/.bashrc"
    elif [ -n "$ZSH_VERSION" ] && [ -f "$HOME/.zshrc" ]; then
        SHELL_PROFILE="$HOME/.zshrc"
    elif [ -f "$HOME/.profile" ]; then
        SHELL_PROFILE="$HOME/.profile"
    fi

    # ── 处理鸿蒙 PC 无 CC（C 编译器）问题 ──
    info "检查 C 编译器..."
    CC_CONFIGURED=false
    if command -v cc >/dev/null 2>&1; then
        ok "已检测到 C 编译器: $(cc --version 2>/dev/null | head -1)"
        CC_CONFIGURED=true
    elif command -v clang >/dev/null 2>&1; then
        warn "未检测到 cc，但发现 clang，将配置 clang 作为 C 编译器"
        export CC="clang"
        export CC_aarch64-unknown-linux-ohos="clang"
        # 写入 shell 配置
        if [ -n "$SHELL_PROFILE" ]; then
            echo "" >> "$SHELL_PROFILE"
            echo "# ohosHttp: C 编译器（鸿蒙 PC 无 cc）" >> "$SHELL_PROFILE"
            echo "export CC=clang" >> "$SHELL_PROFILE"
            echo "export CC_aarch64-unknown-linux-ohos=clang" >> "$SHELL_PROFILE"
        fi
        ok "已配置 clang 为 C 编译器"
        CC_CONFIGURED=true
    else
        warn "========== 鸿蒙 PC 没有 C 编译器（cc）=========="
        warn "Rust 部分依赖需要通过 C 编译器编译原生代码"
        warn "请通过应用市场安装 DevBox（包含 clang），或手动安装 clang"
        echo ""
        warn "创建临时 cc 包装脚本避免编译中断..."
        CC_WRAPPER="${BIN_DIR}/cc"
        mkdir -p "$BIN_DIR"
        cat > "$CC_WRAPPER" << 'CCWRAP'
#!/bin/sh
# cc 包装脚本 — 鸿蒙 PC 兼容层
# 若 clang 可用则透传，否则输出清晰错误
if command -v clang >/dev/null 2>&1; then
    exec clang "$@"
fi
echo "ERROR: No C compiler available on HarmonyOS PC." >&2
echo "Please install clang via DevBox from the app store." >&2
exit 1
CCWRAP
        chmod +x "$CC_WRAPPER"
        # 将包装脚本路径加入环境
        export PATH="${BIN_DIR}:$PATH"
        if [ -n "$SHELL_PROFILE" ]; then
            echo "" >> "$SHELL_PROFILE"
            echo "# ohosHttp: cc 包装脚本（鸿蒙 PC 无 cc）" >> "$SHELL_PROFILE"
            echo "export PATH=\"${BIN_DIR}:\$PATH\"" >> "$SHELL_PROFILE"
        fi
        warn "已创建 cc 包装脚本: ${CC_WRAPPER}"
        warn "强烈建议安装 DevBox 以获得完整的 C 编译支持"
        CC_CONFIGURED=true
    fi

    # 将 cargo/bin 加入 PATH（鸿蒙 Rust 安装脚本可能已添加，确保可用）
    export PATH="$HOME/.cargo/bin:$PATH"
    export PATH="$HOME/usr/rust-1.95.0-aarch64-unknown-linux-ohos/bin:$PATH"

    # ── 配置国内 Rust 镜像加速（解决鸿蒙设备无法访问 crates.io） ──
    info "配置 Rust crates.io 国内镜像..."
    CARGO_CONFIG_DIR="$HOME/.cargo"
    PROJECT_CARGO_CONFIG="$SCRIPT_DIR/.cargo/config.toml"
    if [ ! -f "$CARGO_CONFIG_DIR/config.toml" ]; then
        mkdir -p "$CARGO_CONFIG_DIR"
        if [ -f "$PROJECT_CARGO_CONFIG" ]; then
            cp "$PROJECT_CARGO_CONFIG" "$CARGO_CONFIG_DIR/config.toml"
            ok "已配置 Rust 国内镜像源（清华大学 tuna）: $CARGO_CONFIG_DIR/config.toml"
        else
            cat > "$CARGO_CONFIG_DIR/config.toml" << 'CARGOEOF'
[registries]
crates-io = { protocol = "sparse" }

[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
CARGOEOF
            ok "已创建 Rust 国内镜像配置（清华大学 tuna）"
        fi
    else
        ok "Rust 镜像配置已存在: $CARGO_CONFIG_DIR/config.toml"
    fi

    # 项目级 .cargo/config.toml 也已就绪
    if [ -f "$PROJECT_CARGO_CONFIG" ]; then
        ok "项目级 Rust 镜像配置已就绪: $PROJECT_CARGO_CONFIG"
    fi

    # 检查 build.sh 是否存在
    if [ ! -f "$SCRIPT_DIR/build.sh" ]; then
        error "找不到 build.sh 构建脚本！"
        error "请从 https://gitcode.com/cncoder/ohos-http/releases 下载预编译二进制"
        error "或手动克隆完整仓库后重新运行安装脚本"
        exit 1
    fi

    # 询问用户是否继续
    printf "  ${CYAN}是否继续编译？[Y/n]: ${NC}"
    read -r user_input </dev/tty 2>/dev/null || user_input="y"
    case "$user_input" in
        n|N|no|NO)
            echo ""
            error "用户取消编译"
            error "请从 https://gitcode.com/cncoder/ohos-http/releases 下载预编译二进制"
            exit 1
            ;;
        *)
            echo ""
            info "开始编译..."
            # 执行 build.sh 自动检测系统并编译
            if [ -x "$SCRIPT_DIR/build.sh" ]; then
                "$SCRIPT_DIR/build.sh"
            else
                sh "$SCRIPT_DIR/build.sh"
            fi
            BUILD_EXIT=$?
            if [ "$BUILD_EXIT" -ne 0 ]; then
                error "编译失败（退出码: $BUILD_EXIT）"
                error "请检查上方错误信息，或从发行版下载预编译二进制"
                exit 1
            fi
            ok "编译完成"
            ;;
    esac

    # 编译后再次查找二进制
    for candidate in \
        "$SCRIPT_DIR/target/release/ohosHttp" \
        "$SCRIPT_DIR/target/release/ohosHttp-aarch64-ohos" \
        "$SCRIPT_DIR/target/release/ohosHttp-x86_64-linux" \
        "$SCRIPT_DIR/target/release/ohosHttp-aarch64-linux" \
        "$SCRIPT_DIR/target/release/ohosHttp-aarch64-darwin" \
        "$SCRIPT_DIR/target/release/ohosHttp-x86_64-darwin" \
        "$SCRIPT_DIR/target/aarch64-unknown-linux-ohos/release/ohosHttp"; do
        if [ -f "$candidate" ] && [ -x "$candidate" ]; then
            BINARY_SOURCE="$candidate"
            break
        fi
    done

    if [ -z "$BINARY_SOURCE" ]; then
        error "编译完成，但未找到生成的二进制文件！"
        error "请检查 target/release/ 目录下的输出"
        ls -lh "$SCRIPT_DIR/target/release/"*ohosHttp* 2>/dev/null || true
        exit 1
    fi
fi

cp "$BINARY_SOURCE" "$BIN_DIR/ohosHttp"
chmod +x "$BIN_DIR/ohosHttp"
ok "二进制安装完成: $BIN_DIR/ohosHttp"

# ---- 步骤 3: 配置 PATH ----
info "配置环境变量..."

SHELL_PROFILE=""
if [ -n "$BASH" ] && [ -f "$HOME/.bashrc" ]; then
    SHELL_PROFILE="$HOME/.bashrc"
elif [ -n "$ZSH_VERSION" ] && [ -f "$HOME/.zshrc" ]; then
    SHELL_PROFILE="$HOME/.zshrc"
elif [ -f "$HOME/.profile" ]; then
    SHELL_PROFILE="$HOME/.profile"
fi

if [ -n "$SHELL_PROFILE" ]; then
    if ! grep -q "export PATH=\"$BIN_DIR:\$PATH\"" "$SHELL_PROFILE" 2>/dev/null; then
        printf "\n# ohosHttp PATH\nexport PATH=\"%s:\$PATH\"\n" "$BIN_DIR" >> "$SHELL_PROFILE"
        ok "已添加 PATH 到 $SHELL_PROFILE"
    else
        ok "PATH 已配置"
    fi
else
    warn "未找到 shell 配置文件，请手动将以下内容添加到 shell 配置中："
    warn "  export PATH=\"$BIN_DIR:\$PATH\""
fi

# 当前会话生效
export PATH="$BIN_DIR:$PATH"

# ---- 步骤 4: 创建默认网站目录 ----
info "配置默认网站目录..."
if [ ! -f "$WWW_DIR/index.html" ]; then
    cat > "$WWW_DIR/index.html" << 'HTML'
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ohosHttp - 运行成功</title>
    <style>
        body { font-family: -apple-system, sans-serif; text-align: center; padding: 80px 20px; background: #f5f5f5; }
        .card { background: white; border-radius: 12px; padding: 40px; max-width: 600px; margin: 0 auto; box-shadow: 0 2px 12px rgba(0,0,0,0.08); }
        h1 { color: #007aff; font-size: 2em; margin-bottom: 10px; }
        p { color: #666; line-height: 1.6; }
        .version { color: #999; font-size: 0.9em; margin-top: 20px; }
    </style>
</head>
<body>
    <div class="card">
        <h1>🎉 ohosHttp 运行成功</h1>
        <p>您的鸿蒙 HTTP 服务器已成功启动！</p>
        <p>项目由 <strong>鸿蒙 PC 的 AI 代理</strong> 维护。</p>
        <p class="version">ohosHttp v1.1.0 - HarmonyOS ARM64</p>
    </div>
</body>
</html>
HTML
    ok "默认首页已创建: $WWW_DIR/index.html"
else
    ok "网站目录已存在"
fi

# ---- 步骤 5: 生成默认配置文件 ----
info "生成默认配置文件..."
if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    "$BIN_DIR/ohosHttp" --gen-config > "$CONFIG_DIR/config.toml" 2>/dev/null || cat > "$CONFIG_DIR/config.toml" << 'TOML'
# ohosHttp 默认配置
[[server]]
bind = "0.0.0.0:8089"
root = "./www"
domains = ["localhost"]
threads = 0
cache_enabled = true
cache_ttl = "1h"
cache_max_size = "100MB"
directory_listing = false
access_log = "./logs/access.log"
TOML
    ok "默认配置已生成: $CONFIG_DIR/config.toml"
else
    ok "配置文件已存在"
fi

# ---- 步骤 6: 创建系统服务（可选） ----
# 如果系统支持 init.d 或 service 命令，尝试注册服务
if command -v service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
    info "注册系统服务..."
    INIT_SCRIPT="/etc/init.d/ohosHttp"
    if [ ! -f "$INIT_SCRIPT" ]; then
        cat > /tmp/ohosHttp.init << 'INIT'
#!/bin/sh
### BEGIN INIT INFO
# Provides:          ohosHttp
# Required-Start:    $network $local_fs
# Required-Stop:     $network $local_fs
# Default-Start:     2 3 4 5
# Default-Stop:      0 1 6
# Description:       ohosHttp - High-performance HTTP server for HarmonyOS
### END INIT INFO

BIN="/storage/Users/currentUser/.local/bin/ohosHttp"
CONFIG="/storage/Users/currentUser/.config/ohosHttp/config.toml"
PIDFILE="/var/run/ohosHttp.pid"

case "$1" in
    start)
        echo "Starting ohosHttp..."
        $BIN -c $CONFIG -d --pidfile $PIDFILE
        ;;
    stop)
        echo "Stopping ohosHttp..."
        [ -f $PIDFILE ] && kill $(cat $PIDFILE) 2>/dev/null && rm -f $PIDFILE
        ;;
    restart)
        $0 stop; sleep 1; $0 start
        ;;
    status)
        if [ -f $PIDFILE ] && kill -0 $(cat $PIDFILE) 2>/dev/null; then
            echo "ohosHttp is running (PID: $(cat $PIDFILE))"
        else
            echo "ohosHttp is not running"
        fi
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac
INIT
        if cp /tmp/ohosHttp.init $INIT_SCRIPT 2>/dev/null; then
            chmod +x $INIT_SCRIPT
            ok "系统服务已注册: $INIT_SCRIPT"
        else
            warn "需要 root 权限才能注册系统服务，跳过"
        fi
        rm -f /tmp/ohosHttp.init
    else
        ok "系统服务已存在"
    fi
fi

# ---- 步骤 7: 验证安装 ----
info "验证安装..."
if command -v ohosHttp >/dev/null 2>&1; then
    VERSION=$("$BIN_DIR/ohosHttp" --version 2>&1)
    ok "安装成功！版本: $VERSION"
else
    warn "请重新打开终端或运行 'source $SHELL_PROFILE' 使 PATH 生效"
fi

# ---- 完成 ----
echo ""
printf "${GREEN}=========================================${NC}\n"
printf "${GREEN}  ohosHttp 安装完成！${NC}\n"
printf "${GREEN}=========================================${NC}\n"
echo ""
printf "  二进制路径:    ${BLUE}%s/ohosHttp${NC}\n" "$BIN_DIR"
printf "  网站目录:      ${BLUE}%s${NC}\n" "$WWW_DIR"
printf "  配置文件:      ${BLUE}%s/config.toml${NC}\n" "$CONFIG_DIR"
printf "  日志目录:      ${BLUE}%s${NC}\n" "$LOGS_DIR"
echo ""
printf "  快速启动示例:\n"
printf "    ${YELLOW}ohosHttp -a 0.0.0.0:8089 -r %s${NC}\n" "$WWW_DIR"
printf "    ${YELLOW}ohosHttp -c %s/config.toml${NC}\n" "$CONFIG_DIR"
echo ""
printf "  守护进程模式:\n"
printf "    ${YELLOW}ohosHttp -c %s/config.toml -d --pidfile /tmp/ohosHttp.pid${NC}\n" "$CONFIG_DIR"
echo ""
printf "  ${GREEN}浏览器访问: http://localhost:8089${NC}\n"
echo ""
