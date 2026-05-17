#!/bin/sh
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
NC='\033[0m' # No Color

info()  { printf "${BLUE}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[OK]${NC}    %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$*"; }

# ---- 横幅 ----
cat << 'BANNER'
=========================================
  ohosHttp v1.1.0 - 鸿蒙系统一键安装
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

# ---- 步骤 1: 创建目录结构 ----
info "创建目录结构..."
mkdir -p "$BIN_DIR"
mkdir -p "$WWW_DIR"
mkdir -p "$CONFIG_DIR"
mkdir -p "$LOGS_DIR"
ok "目录创建完成"

# ---- 步骤 2: 安装二进制文件 ----
info "安装 ohosHttp 二进制..."

# 查找二进制文件
BINARY_SOURCE=""
for candidate in \
    "$SCRIPT_DIR/target/release/ohosHttp" \
    "$SCRIPT_DIR/ohosHttp" \
    "$SCRIPT_DIR/ohosHttp-aarch64-ohos" \
    "$SCRIPT_DIR/target/aarch64-unknown-linux-ohos/release/ohosHttp"; do
    if [ -f "$candidate" ] && [ -x "$candidate" ]; then
        BINARY_SOURCE="$candidate"
        break
    fi
done

if [ -z "$BINARY_SOURCE" ]; then
    error "未找到 ohosHttp 二进制文件！"
    error "请先从 https://gitcode.com/cncoder/ohos-http/releases 下载"
    error "或将编译好的 ohosHttp 放在当前目录下"
    exit 1
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
