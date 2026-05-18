#!/system/bin/sh
# ============================================================
# ohosHttp — HarmonyOS PC 卸载脚本
# 版本: 1.3.1
# 用法: sudo sh uninstall.sh [安装路径]
#       默认卸载路径: ${HOME}/Documents/ohosHttp
# ============================================================

set -e

APP_NAME="ohosHttp"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { printf "${GREEN}[INFO]${NC}  %s\n" "$1"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$1"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$1"; }
step()  { printf "\n${CYAN}>>> %s ...${NC}\n" "$1"; }

# ---- 权限检查 ----
if [ "$(id -u)" -ne 0 ]; then
    echo ""
    error "========================================================"
    error "  卸载需要 root 权限，请使用 sudo 执行！"
    error "========================================================"
    echo ""
    error "  sudo sh ${SCRIPT_DIR}/uninstall.sh"
    echo ""
    exit 1
fi

# ---- 获取原始用户 ----
ORIGINAL_USER="${SUDO_USER:-$USER}"
ORIGINAL_HOME=$(eval echo "~${ORIGINAL_USER}" 2>/dev/null || echo "$HOME")

# ---- 解析安装路径 ----
DEFAULT_INSTALL_DIR="${ORIGINAL_HOME}/Documents/${APP_NAME}"
INSTALL_DIR="${1:-${DEFAULT_INSTALL_DIR}}"
INSTALL_DIR="$(echo "$INSTALL_DIR" | sed 's:/*$::')"

info "卸载路径: ${INSTALL_DIR}"

# ---- 停止运行中的进程 ----
step "检查运行中的进程"

PID_FILE="${INSTALL_DIR}/${APP_NAME}.pid"
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE")
    if kill -0 "$OLD_PID" 2>/dev/null; then
        info "正在停止运行中的 ohosHttp (PID: $OLD_PID)..."
        kill "$OLD_PID" 2>/dev/null || true
        sleep 1
        if kill -0 "$OLD_PID" 2>/dev/null; then
            kill -9 "$OLD_PID" 2>/dev/null || true
        fi
        info "进程已停止 ✓"
    fi
    rm -f "$PID_FILE"
else
    if command -v pkill > /dev/null 2>&1; then
        pkill "${APP_NAME}" 2>/dev/null || true
    fi
    info "未发现运行中的实例"
fi

# ---- 清理环境变量配置 ----
step "清理环境变量配置"

clean_profile() {
    local file="$1"
    if [ -f "$file" ]; then
        grep -v "# >>> ohosHttp PATH" "$file" | grep -v "# <<< ohosHttp PATH" | grep -v "export PATH.*${APP_NAME}" > "${file}.tmp" 2>/dev/null || true
        mv "${file}.tmp" "$file" 2>/dev/null || true
        info "已清理: ${file}"
    fi
}

SUDO_HOME="${ORIGINAL_HOME}"
clean_profile "${SUDO_HOME}/.bashrc"
clean_profile "${SUDO_HOME}/.bash_profile"
clean_profile "${SUDO_HOME}/.profile"
clean_profile "${SUDO_HOME}/.zshrc"

# ---- 删除安装目录 ----
step "删除安装目录"

if [ -d "$INSTALL_DIR" ]; then
    rm -rf "$INSTALL_DIR"
    info "已删除安装目录: ${INSTALL_DIR} ✓"
else
    warn "安装目录不存在: ${INSTALL_DIR} (可能已被删除)"
fi

# ---- 完成 ----
echo ""
echo "=========================================="
echo " ohosHttp 卸载完成"
echo "=========================================="
echo ""
info "二进制文件和配置已全部移除"
info "请重新打开终端或执行 'source ~/.bashrc' 刷新环境"
echo ""
