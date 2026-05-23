# ohos-server - 高性能 HTTP 服务器

跨平台 HTTP 服务器，支持 HarmonyOS（OpenHarmony）和 Linux x86_64 平台。

## 功能特性

- ✅ **静态文件服务** - 提供网站根目录的静态文件访问
- ✅ **多站点配置** - 通过配置文件支持多个虚拟主机
- ✅ **虚拟主机** - 基于域名的请求分发
- ✅ **URL 重写（伪静态）** - 正则匹配 URL 重写规则
- ✅ **反向代理** - 将请求转发到后端服务，支持路径前缀匹配
- ✅ **CGI 支持** - 支持 CGI 脚本（如 PHP）执行
- ✅ **访问日志** - 自动轮转的访问日志，支持日期和大小切割
- ✅ **CORS 跨域** - 可配置的跨域资源共享支持
- ✅ **多请求方法** - 支持 GET、POST、PUT、DELETE、HEAD、OPTIONS、PATCH
- ✅ **HTTPS (TLS)** - 通过 `--cert` 和 `--key` 参数开启 TLS 加密
- ✅ **HTTP/2 支持** - 自动 ALPN 协商，TLS 连接上自动升级 HTTP/2
- ✅ **HTTP/3 (QUIC)** - 通过 `--http3-port` 参数开启 UDP/QUIC 连接
- ✅ **信号控制** - SIGTERM 优雅关闭，SIGHUP 热重启
- ✅ **守护进程模式** - 后台运行并管理 PID 文件
- ✅ **路径安全** - 防止路径穿越攻击
- ✅ **跨平台** - 支持 HarmonyOS 和 Linux
- ✅ **IP 限流** - 令牌桶算法，每 IP 独立限流，可配置速率和突发大小
- ✅ **IP 黑名单** - 完全屏蔽指定 IP 或 IP 段，支持 `*` 通配符
- ✅ **自定义限流** - 按 IP 配置不同限流速率
- ✅ **IP 访问控制** - 可关闭 IP 直连，只允许绑定的域名访问
- ✅ **Session 支持** - 基于 Cookie 的内存 Session 管理，支持 TTL 过期
- ✅ **禁止访问控制** - 按目录和文件类型禁止访问，对反向代理站点自动跳过
- ✅ **负载均衡** - 加权轮询分发请求到多个后端服务器

## 完整文档

请参阅 **[USAGE.md](USAGE.md)** 获取完整的使用文档，包括：

- 所有命令行选项说明
- 守护进程模式（后台运行）
- CGI 解释器配置（PHP、Python、Perl 等）
- 配置文件完整参数详解
- 伪静态重写规则示例
- 反向代理和路径规则
- 限流与黑名单配置
- IP 访问控制
- Session 会话管理
- 负载均衡配置
- 多站点配置
- 缓存配置
- 常见问题排查

---

## 快速开始

### 方式0：一键安装（推荐）

```bash
# 下载并一键安装
chmod +x install.sh
./install.sh
```

### 方式1：命令行快速启动

```bash
# 绑定地址和端口，指定网站目录
./ohos-server -a 127.0.0.1:8080 -r ./www

# 指定工作线程数
./ohos-server --addr=0.0.0.0:8089 --root=/var/www --threads=4
```

### 方式2：配置文件启动

```bash
# 生成默认配置文件
./ohos-server --gen-config > config.toml

# 编辑配置文件...
# 启动服务
./ohos-server -c config.toml
```

## 架构设计

ohos-server 采用 **进程 (Process) × 线程 (Thread) × 协程 (Coroutine)** 三层架构：

- **进程层**：Master-Worker 多进程模型，每核一个 Worker，`SO_REUSEPORT` 内核级负载均衡
- **线程层**：Tokio 多线程运行时，Work-Stealing 调度器，真正并行执行
- **协程层**：每个连接一个异步任务，零成本上下文切换，百万级并发

详见 [架构文档](docs/ARCHITECTURE.md)

## 配置文件说明

配置文件使用 TOML 格式，支持多站点配置：

```toml
# 第一个站点
[[server]]
bind = "0.0.0.0:8080"          # 绑定地址和端口
root = "./www"                  # 网站根目录
domains = ["example.com"]       # 绑定的域名
upload_max_size = "10MB"        # 上传最大大小
threads = 4                     # 工作线程数
cache_enabled = true            # 启用缓存
cache_ttl = "1h"                # 缓存过期时间
cache_max_size = "100MB"        # 缓存最大大小
directory_listing = false       # 目录列表
access_log = "./logs/access.log" # 访问日志（可选）

# URL 重写规则（伪静态）
[[server.rewrite]]
from = "^/article/(\\d+)$"
to = "/article.html?id=$1"

# 路径规则（反向代理/静态文件）
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"

[[server.location]]
path = "/static"
root = "/var/www/static"
expires = "7d"

# 第二个站点
[[server]]
bind = "0.0.0.0:8081"
root = "./www2"
threads = 2
```

## 命令行参数

| 参数 | 简写 | 说明 |
|------|------|------|
| `--addr` | `-a` | 绑定地址，如 `127.0.0.1:8089` |
| `--root` | `-r` | 网站根目录（默认 `./www`） |
| `--config` | `-c` | 配置文件路径 |
| `--threads` | `-t` | 工作线程数（0=自动） |
| `--gen-config` | | 生成默认配置文件内容 |
| `--daemon` | `-d` | 守护进程模式（后台运行） |
| `--pidfile` | | PID 文件路径（守护进程模式） |
| `--interpreter` | | CGI 解释器路径 |
| `--cgi-ext` | | CGI 文件扩展名，逗号分隔 |
| `--all` | | 监听所有站点（多站点模式） |
| `--help` | `-h` | 显示帮助信息 |
| `--version` | `-V` | 显示版本信息 |

## 编译构建

### 环境要求

- Rust 1.75+
- Cargo

### 本地编译

```bash
# 编译 debug 版本
cargo build

# 编译 release 版本
cargo build --release

# 输出在 target/release/ohos-server
```

### 交叉编译

```bash
# HarmonyOS ARM64 (当前平台)
cargo build --release

# Linux x86_64（需要在 x86_64 机器上执行）
cargo build --release --target x86_64-unknown-linux-gnu
```

### 使用构建脚本

```bash
# 构建当前平台
chmod +x build.sh
./build.sh

# 构建 x86_64 Linux 版本（需安装对应 target）
# rustup target add x86_64-unknown-linux-gnu
# ./build.sh x86_64-unknown-linux-gnu

# 重新编译（安装系统依赖 + clean 构建）
./build.sh --rebuild

# 或在特定平台脚本上使用
# ./build-x86_64.sh --rebuild    # Linux x86_64
# ./build-arm.sh --rebuild       # Linux ARM64 本地/交叉编译
# ./build-ohos.sh --rebuild      # HarmonyOS ARM64
```

> `--rebuild` 参数会自动检测并安装缺少的系统依赖（如 gcc、build-essential），
> 然后执行 `cargo clean` 从头编译。生产服务器上遇到编译失败时推荐使用此模式。

## 项目结构

```
ohos-server/
├── README.md             # 项目说明
├── USAGE.md              # 完整使用文档
├── Cargo.toml            # 项目配置和依赖
├── build.sh              # 构建脚本
├── install.sh            # 鸿蒙系统一键安装脚本
├── www/                  # 默认网站根目录
│   └── config.toml       # 示例配置文件
└── src/
    ├── main.rs           # 入口和 CLI 参数解析
    ├── banner.rs         # 启动画面
    ├── config.rs         # 配置解析
    ├── server.rs         # HTTP 服务器核心
    ├── handler.rs        # 请求处理（静态文件、上传、CGI）
    ├── proxy.rs          # 反向代理
    ├── rewrite.rs        # URL 重写引擎（伪静态）
    ├── load_balancer.rs  # 负载均衡
    ├── rate_limiter.rs   # IP 限流
    ├── session.rs        # Session 管理
    └── logger.rs         # 访问日志
```

## 已编译文件

本项目已为 HarmonyOS ARM64 编译完成，二进制文件位于：
- `target/release/ohos-server` - HarmonyOS ARM64 可执行文件
- `target/release/ohos-server-aarch64-ohos` - 同上（备份）

## 鸿蒙系统一键安装

使用 `install.sh` 脚本在鸿蒙系统上一键完成所有环境配置：

```bash
chmod +x install.sh
./install.sh
```

该脚本会自动执行以下操作：
1. **创建目录结构** - `~/.local/bin`、`~/.config/ohos-server`、`~/www`、`~/logs/ohos-server`
2. **安装二进制文件** - 将 `ohos-server` 复制到系统 PATH
3. **配置环境变量** - 自动添加 `~/.local/bin` 到 PATH
4. **创建默认网站** - 生成漂亮的默认首页
5. **生成默认配置** - 创建开箱即用的 `config.toml`
6. **注册系统服务** - 可选，注册 init.d 服务（需 root）
7. **验证安装** - 确认安装成功

如需 x86_64 Linux 版本，请在 x86_64 Linux 环境下编译。

## 技术栈

- **Rust** - 系统编程语言
- **Tokio** - 异步运行时
- **Hyper** - HTTP 库
- **Clap** - 命令行参数解析
- **TOML** - 配置格式
- **Regex** - 正则表达式

## 开源协议

本项目基于 **Apache License 2.0** 协议开源，采用宽泛的开源条款。

```
Copyright 2025 ohos-server Contributors

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

**任何人可以自由使用、修改和再发行本代码**，只需保留版权声明和许可声明。

## 维护者

该项目由 **鸿蒙 PC 的 AI 代理** 维护。代码提交、功能开发、问题修复均由 AI Agent 完成。
