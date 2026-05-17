#!/bin/sh
# ohosHttp 构建脚本
# 用法: ./build.sh [target]
# 默认构建当前平台 (aarch64-unknown-linux-ohos)
# 构建 x86_64: ./build.sh x86_64-unknown-linux-gnu

set -e

TARGET="${1:-aarch64-unknown-linux-ohos}"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== ohosHttp 构建脚本 ==="
echo "目标平台: $TARGET"
echo "项目目录: $PROJECT_DIR"
echo ""

cd "$PROJECT_DIR"

case "$TARGET" in
    aarch64-unknown-linux-ohos)
        echo "构建 HarmonyOS (OpenHarmony) ARM64 版本..."
        cargo build --release --target aarch64-unknown-linux-ohos
        cp target/aarch64-unknown-linux-ohos/release/ohosHttp target/release/ohosHttp-aarch64-ohos
        echo "输出: target/release/ohosHttp-aarch64-ohos"
        ;;
    x86_64-unknown-linux-gnu)
        echo "构建 Linux x86_64 版本..."
        echo "需要先安装目标: rustup target add x86_64-unknown-linux-gnu"
        cargo build --release --target x86_64-unknown-linux-gnu
        cp target/x86_64-unknown-linux-gnu/release/ohosHttp target/release/ohosHttp-x86_64-linux
        echo "输出: target/release/ohosHttp-x86_64-linux"
        ;;
    *)
        echo "不支持的目标: $TARGET"
        echo "支持的目标: aarch64-unknown-linux-ohos, x86_64-unknown-linux-gnu"
        exit 1
        ;;
esac

echo ""
echo "构建完成！"
ls -lh target/release/ohosHttp-* 2>/dev/null || true
