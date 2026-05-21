# ohosHttp 完整使用文档

## 目录

- [命令行选项](#命令行选项)
- [快速启动](#快速启动)
- [守护进程模式](#守护进程模式)
- [CGI 解释器配置](#cgi-解释器配置)
- [配置文件详解](#配置文件详解)
- [伪静态重写规则](#伪静态重写规则)
- [主流框架伪静态配置](#主流框架伪静态配置)
  - [ThinkPHP (5/6/8)](#thinkphp-568)
  - [Laravel](#laravel)
  - [WordPress](#wordpress)
  - [Yii2](#yii2)
- [反向代理](#反向代理)
- [WebSocket 代理转发](#websocket-代理转发)
- [路径规则](#路径规则)
- [缓存配置](#缓存配置)
- [CORS 跨域配置](#cors-跨域配置)
- [HTTP 方法支持](#http-方法支持)
- [访问日志与日志轮转](#访问日志与日志轮转)
- [停止与重启](#停止与重启)
- [HTTPS 加密](#https-加密)
- [HTTP/2 支持](#http2-支持)
- [HTTP/3 (QUIC) 支持](#http3-quic-支持)
- [多站点配置](#多站点配置)
- [启动画面说明](#启动画面说明)
- [常见问题](#常见问题)

---

## 命令行选项

```
ohosHttp - 高性能HTTP服务器

Usage: ohosHttp [OPTIONS]

Options:
  -a, --addr <ADDR>                绑定地址，如 "127.0.0.1:8089" [default: ""]
  -r, --root <ROOT>                网站根目录 [default: ./www]
  -c, --config <CONFIG>            配置文件路径 [default: ""]
  -t, --threads <THREADS>          工作线程数 [default: 0] (0=自动, 等于CPU核数)
      --gen-config                 生成默认配置文件
  -d, --daemon                     守护进程模式（后台运行）
      --pidfile <PIDFILE>          PID文件路径（守护进程模式） [default: ""]
      --interpreter <INTERPRETER>  CGI解释器路径，如 "/usr/bin/php-cgi" [default: ""]
      --cgi-ext <CGI_EXT>          CGI文件扩展名，逗号分隔，如 ".php,.phtml" [default: ""]
      --all                        监听所有服务器（多站点模式，配合 -c 使用）
      --cert <CERT>                HTTPS 证书文件路径 [default: ""]
      --key <KEY>                  HTTPS 私钥文件路径 [default: ""]
      --http3-port <HTTP3_PORT>    HTTP/3 (QUIC) 端口，如 "4433" [default: ""]
  -h, --help                       Print help
  -V, --version                    Print version
```

---

## 快速启动

### 基本用法

```bash
# 指定地址和根目录启动
ohosHttp -a 127.0.0.1:8080 -r ./www

# 监听所有网络接口，4个工作线程
ohosHttp -a 0.0.0.0:80 -r /var/www -t 4

# 仅指定地址（使用默认根目录 ./www）
ohosHttp -a 127.0.0.1:8080

# 启用 HTTPS（需要证书和私钥）
ohosHttp -a 0.0.0.0:443 -r ./www --cert server.crt --key server.key

# 同时启用 HTTPS 和 HTTP/3 (QUIC)
ohosHttp -a 0.0.0.0:443 -r ./www --cert server.crt --key server.key --http3-port 4433
```

### 使用配置文件

```bash
# 先生成默认配置
ohosHttp --gen-config > config.toml

# 编辑配置文件后启动
ohosHttp -c config.toml

# 启动配置文件中所有站点
ohosHttp -c config.toml --all
```

### 查看帮助和版本

```bash
ohosHttp --help       # 显示帮助信息
ohosHttp --version    # 显示版本号
```

---

## 守护进程模式

让服务器在后台运行，不占用终端。

### 基本守护进程

```bash
ohosHttp -a 0.0.0.0:8080 -d
```

启动后终端立即返回，服务器在后台持续运行。

### 指定 PID 文件

```bash
ohosHttp -a 0.0.0.0:8080 -d --pidfile /var/run/ohos.pid
```

PID 文件可用于：
- 检查进程是否运行：`cat /var/run/ohos.pid`
- 停止服务：`kill $(cat /var/run/ohos.pid)`
- 服务管理脚本使用

### 配置文件 + 守护进程

```bash
ohosHttp -c config.toml -d
```

### 停止守护进程

```bash
# 方式1：使用 PID 文件
kill $(cat /var/run/ohos.pid)

# 方式2：使用 ps 查找进程
ps aux | grep ohosHttp
kill -TERM <PID>

# 方式3：使用 pkill
pkill ohosHttp
```

### 注意事项

- 守护进程模式下，启动日志写入 stderr，守护进程化后标准日志通过 `env_logger` 输出
- 建议始终配合 `--pidfile` 使用以便管理
- PID 文件需使用**绝对路径**（守护进程 fork 后工作目录变为 `/`）

---

## CGI 解释器配置

ohosHttp 支持通过 CGI 协议运行动态脚本（PHP、Python、Perl 等）。

### 命令行快速配置

```bash
# 使用 PHP-CGI 解释器，处理 .php 文件
ohosHttp -a 0.0.0.0:8080 --interpreter /usr/bin/php-cgi

# 使用 Python，处理 .py 和 .cgi 文件
ohosHttp -a 0.0.0.0:8080 --interpreter /usr/bin/python3 --cgi-ext ".py,.cgi"

# 完整示例
ohosHttp -a 0.0.0.0:8080 -r ./www --interpreter /usr/bin/php-cgi --cgi-ext ".php,.phtml"
```

**说明**：
- 当 `--cgi-ext` 未指定时，默认使用 `.cgi` 扩展名
- 当 `--interpreter` 未指定时，CGI 仅处理 shebang 脚本（文件第一行以 `#!` 开头）

### 配置文件中配置 CGI

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./www"

# 可以配置多个 CGI 解释器
[[server.cgi]]
extensions = [".php", ".phtml", ".php5"]
interpreter = "/usr/bin/php-cgi"

[[server.cgi]]
extensions = [".py", ".cgi"]
interpreter = "/usr/bin/python3"

[[server.cgi]]
extensions = [".pl"]
interpreter = "/usr/bin/perl"
```

### 路径级 CGI 配置

可以为特定路径单独指定 CGI 解释器：

```toml
[[server.location]]
path = "/admin"
cgi = { interpreter = "/usr/bin/php-cgi", extensions = [".php"] }

[[server.location]]
path = "/cgi-bin"
cgi = { interpreter = "", extensions = [".cgi", ".pl"] }  # 使用 shebang
```

### CGI 工作原理

ohosHttp 实现 CGI/1.1 标准：

1. 客户端请求 CGI 文件时，服务器解析请求
2. 设置标准 CGI 环境变量：`REQUEST_METHOD`、`QUERY_STRING`、`SCRIPT_FILENAME`、`CONTENT_TYPE`、`CONTENT_LENGTH`、`HTTP_*` 等
3. POST/PUT 请求的 body 通过 stdin 传递给 CGI 程序
4. CGI 程序通过 stdout 输出响应
5. 服务器解析 CGI 输出中的 `Status`、`Content-Type`、`Location` 头
6. 将处理后的响应返回给客户端

### 支持的 CGI 环境变量

| 变量 | 说明 |
|------|------|
| `REQUEST_METHOD` | 请求方法 (GET/POST/PUT/DELETE 等) |
| `QUERY_STRING` | URL 查询参数 |
| `SCRIPT_NAME` | 脚本路径 |
| `SCRIPT_FILENAME` | 脚本在文件系统中的完整路径 |
| `PATH_INFO` | 路径信息 |
| `CONTENT_TYPE` | 请求 Content-Type |
| `CONTENT_LENGTH` | 请求 body 长度 |
| `HTTP_*` | 所有 HTTP 请求头 |
| `REMOTE_ADDR` | 客户端 IP |
| `SERVER_NAME` | 服务器名称 |
| `SERVER_PORT` | 服务器端口 |
| `GATEWAY_INTERFACE` | CGI 版本 (CGI/1.1) |
| `REDIRECT_STATUS` | 重定向状态 (PHP-CGI 需要) |

---

## 配置文件详解

### 生成默认配置

```bash
ohosHttp --gen-config > config.toml
```

生成的配置文件包含完整的注释和示例。

### 完整配置项

```toml
[[server]]
# ==== 基本配置 ====
bind = "0.0.0.0:8080"           # 绑定地址和端口（必填）
root = "./www"                   # 网站根目录（必填）
domains = ["example.com"]        # 绑定的域名列表（虚拟主机）
threads = 4                      # 工作线程数（0=自动）
upload_max_size = "10MB"         # 上传文件最大大小

# ==== 缓存配置 ====
cache_enabled = true             # 是否启用内存缓存
cache_ttl = "1h"                 # 缓存过期时间（可选：10s/5m/2h/1d）
cache_max_size = "100MB"         # 缓存最大占用内存

# ==== 目录列表 ====
directory_listing = false        # 是否允许目录列表

# ==== 访问日志 ====
access_log = "./logs/access.log" # 访问日志路径（不配置则不记录）

# ==== 默认页面 ====
index_files = ["index.html", "index.htm", "index.php"]

# ==== CGI 配置 ====
[[server.cgi]]
extensions = [".php", ".phtml"]  # 需要 CGI 处理的文件扩展名
interpreter = "/usr/bin/php-cgi" # CGI 解释器路径

[[server.cgi]]
extensions = [".py"]
interpreter = "/usr/bin/python3"

# ==== 伪静态重写规则 ====
[[server.rewrite]]
from = "^/article/(\\d+)$"                          # 正则表达式匹配 URL
to = "/article.html?id=$1"                          # 重写目标（$1, $2 引用分组）

# ==== 路径规则 ====
# 类型1：反向代理到后端服务
[[server.location]]
path = "/api"                                        # 匹配的路径前缀
proxy_pass = "http://127.0.0.1:3000"                 # 后端服务地址

# 类型2：映射到其他目录
[[server.location]]
path = "/static"                                     # 匹配的路径前缀
root = "/var/www/static"                             # 文件系统路径
expires = "7d"                                       # 缓存过期时间（浏览器端）

# 类型3：路径级别 CGI
[[server.location]]
path = "/admin"
cgi = { interpreter = "/usr/bin/php-cgi", extensions = [".php"] }

# ==== 第二个站点 ====
[[server]]
bind = "0.0.0.0:8081"
root = "./www2"
domains = ["blog.example.com"]
threads = 2
upload_max_size = "50MB"
directory_listing = true
```

### 配置项速查表

| 配置项 | 类型 | 必填 | 说明 |
|--------|------|------|------|
| `bind` | 字符串 | 是 | 绑定地址和端口 |
| `root` | 字符串 | 是 | 网站根目录 |
| `domains` | 字符串数组 | 否 | 虚拟主机域名列表 |
| `threads` | 整数 | 否 | 工作线程数（0=自动） |
| `upload_max_size` | 字符串 | 否 | 上传最大大小 |
| `cache_enabled` | 布尔 | 否 | 启用缓存 |
| `cache_ttl` | 字符串 | 否 | 缓存过期时间 |
| `cache_max_size` | 字符串 | 否 | 缓存最大内存 |
| `directory_listing` | 布尔 | 否 | 目录列表 |
| `access_log` | 字符串 | 否 | 访问日志路径 |
| `index_files` | 字符串数组 | 否 | 默认索引文件 |
| `cgi` | 对象数组 | 否 | CGI 解释器配置 |
| `rewrite` | 对象数组 | 否 | 伪静态重写规则 |
| `location` | 对象数组 | 否 | 路径规则 |

---

## 伪静态重写规则

URL 重写（伪静态）允许将美观的 URL 映射到实际的处理脚本。

### 配置格式

```toml
[[server.rewrite]]
from = "^/article/(\\d+)$"
to = "/article.html?id=$1"
```

- `from`：正则表达式，匹配请求 URL 路径
- `to`：重写目标，`$1`、`$2` 等引用正则中的分组

### 常用重写规则示例

| 用途 | from | to |
|------|------|-----|
| 文章详情 | `^/article/(\d+)$` | `/article.html?id=$1` |
| 分类页面 | `^/category/(\w+)$` | `/category.html?name=$1` |
| 用户主页 | `^/user/([^/]+)$` | `/profile.php?user=$1` |
| 标签页面 | `^/tag/([^/]+)$` | `/tag.php?name=$1` |
| 分页列表 | `^/list/(\d+)$` | `/list.php?page=$1` |
| 产品详情 | `^/product/([a-zA-Z0-9_-]+)$` | `/product.php?slug=$1` |
| API 路由 | `^/api/v1/(\w+)/(\d+)$` | `/api.php?action=$1&id=$2` |
| 博客文章 | `^/post/(\d+)/([^/]+)$` | `/post.php?id=$1` |
| 归档页面 | `^/archive/(\d{4})/(\d{2})$` | `/archive.php?year=$1&month=$2` |
| RSS 订阅 | `^/feed$` | `/feed.xml` |

### 重写规则执行顺序

规则按在配置文件中出现的顺序从上到下匹配。第一个匹配的规则生效。

### 特殊变量 `$0`

除了 `$1`、`$2` 等捕获组变量外，还支持 `$0` 代表**整个匹配的路径**。常用于静态资源排除规则：

```toml
# 静态资源匹配后返回自身（不重写）
[[server.rewrite]]
from = "^/(css|js|img)/.*$"
to = "$0"
```

---

## 主流框架伪静态配置

ohosHttp 的伪静态引擎支持所有主流 PHP 框架。**关键特性**：当 URL 重写后的路径与原始路径不同时，服务器会自动检查原始路径是否对应一个真实存在的文件，如果是则跳过重写直接服务该文件。这意味着您可以使用一个简单的 catch-all 规则而无需手动排除静态资源目录。

### ThinkPHP (5/6/8)

ThinkPHP 默认使用 PathInfo 模式，URL 格式为 `index.php/模块/控制器/操作`。

#### 标准配置

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./public"        # ThinkPHP 入口在 public 目录
domains = ["example.com"]

# CGI: 所有 .php 文件用 PHP 解释器执行
[[server.cgi]]
php = "/usr/bin/php"
extensions = ["php"]

# 伪静态: 所有请求路由到 index.php
[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php/$1"
```

**工作原理**：

| 请求 URL | 处理方式 |
|:---------|:---------|
| `/index.html` | 真实文件存在 → 跳过重写，直接返回 HTML |
| `/css/style.css` | 真实文件存在 → 跳过重写，直接返回 CSS |
| `/home/index` | 文件不存在 → 重写为 `/index.php/home/index` → PHP CGI 执行 |
| `/admin/user/edit/id/1` | 文件不存在 → 重写为 `/index.php/admin/user/edit/id/1` → PHP CGI 执行 |
| `/` | 重写为 `/index.php/` → PHP CGI 执行（ThinkPHP 默认路由） |

#### 排除特定静态目录

如果希望显式排除某些目录的 URL 重写（防止意外匹配）：

```toml
[[server.rewrite]]
from = "^/(assets|uploads|static|runtime)/.*$"
to = "$0"               # $0 = 整个匹配路径，即不改变

[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php/$1"
```

> **注意**：ThinkPHP 的 `runtime` 目录应通过禁止访问规则保护，而非通过重写规则：
> ```toml
> [[server]]
> forbidden_dirs = ["/runtime"]
> ```

#### ThinkPHP 兼容模式（?s= 参数）

部分 ThinkPHP 版本使用兼容模式 URL（如 `index.php?s=/home/index`）：

```toml
[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php?s=$1"
```

### Laravel

Laravel 使用前端控制器模式，所有请求通过 `public/index.php` 处理。

#### 标准配置（document root = public 目录）

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./public"        # Laravel 入口在 public 目录
domains = ["example.com"]

# CGI: 所有 .php 文件用 PHP 解释器执行
[[server.cgi]]
php = "/usr/bin/php"
extensions = ["php"]

# 伪静态: 所有请求路由到 index.php（等价于 Apache 的 RewriteRule ^(.*)$ index.php [QSA,L]）
[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php/$1"
```

#### Laravel 子目录部署（项目根目录 = document root）

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./laravel"       # Laravel 项目根目录
domains = ["example.com"]

[[server.cgi]]
php = "/usr/bin/php"
extensions = ["php"]

# 伪静态: 所有请求路由到 public/index.php
[[server.rewrite]]
from = "^/(.*)$"
to = "/public/index.php/$1"

# 静态资源目录
[[server.rewrite]]
from = "^/public/(css|js|img|fonts|uploads)/.*$"
to = "$0"
```

**工作原理**：

| 请求 URL | 处理方式 |
|:---------|:---------|
| `/` | 重写为 `/index.php/` → PHP CGI 执行（Laravel 路由处理） |
| `/login` | 重写为 `/index.php/login` → PHP CGI 执行 |
| `/css/app.css` | 真实文件存在 → 跳过重写，直接返回 CSS |
| `/js/app.js` | 真实文件存在 → 跳过重写，直接返回 JS |
| `/api/users` | 重写为 `/index.php/api/users` → PHP CGI 执行 |
| `/storage/...` | 真实文件或符号链接存在 → 跳过重写，直接服务 |

> **说明**：Laravel 会创建 `public/storage` 符号链接指向 `storage/app/public`。由于 ohosHttp 自动检查文件存在性，真实文件（包括符号链接）会被直接服务，不会被重写到 index.php。

### WordPress

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./wordpress"
domains = ["example.com"]

[[server.cgi]]
php = "/usr/bin/php"
extensions = ["php"]

# WordPress 伪静态
[[server.rewrite]]
from = "^/$"
to = "/index.php"

[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php/$1"

# 确保 wp-content/uploads 等不触发重写
```

### Yii2

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./yii2"
domains = ["example.com"]

[[server.cgi]]
php = "/usr/bin/php"
extensions = ["php"]

# Yii2 伪静态（使用 r= 参数模式）
[[server.rewrite]]
from = "^/(.*)$"
to = "/index.php?r=$1"
```

### 注意事项

1. **Rust 正则引擎限制**：ohosHttp 使用 Rust 的 `regex` 库，不支持零宽断言（负向前瞻 `(?!...)`、正向前瞻 `(?=...)` 等）。需要使用多条规则组合来实现排除效果。
2. **文件存在性检查**：当规则将 URL 重写为不同路径时，服务器会自动检查原始路径是否对应真实文件。如果是，则跳过重写。这实现了类似 nginx `try_files` 的语义。
3. **路径匹配优先于 CGI**：如果配置了 `[[server.location]]` 规则（如反向代理），其优先级高于 CGI 处理。确保 `location` 规则不会意外抓走 PHP 请求。
4. **HTTP Basic Auth**：如果后端 PHP 框架需要认证，可在项目中通过 `.htaccess` 类似的方式或框架中间件实现。

---

## 反向代理

将请求透明转发到后端 HTTP 服务。

### 配置方式

```toml
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"
```

### 工作原理

1. 客户端请求 `/api/users` 
2. ohosHttp 将请求转发到 `http://127.0.0.1:3000/api/users`
3. 后端返回的响应透传给客户端
4. 支持 HTTP/1.1 协议的转发

### 常见用途

```toml
# 转发到 Node.js 后端
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"

# 转发到 Python 后端
[[server.location]]
path = "/app"
proxy_pass = "http://127.0.0.1:5000"

# 转发到 Java 后端
[[server.location]]
path = "/service"
proxy_pass = "http://127.0.0.1:8080"
```

---

## WebSocket 代理转发

ohosHttp 内置 WebSocket 代理转发功能，无需额外配置——任何已有的 `proxy_pass` 路径规则**自动支持 WebSocket 升级**。当客户端发起 WebSocket 握手请求时，服务器自动建立到后端的 TCP 隧道并桥接双向数据流。

### 自动识别原理

1. 客户端发送带有 `Upgrade: websocket` 和 `Connection: Upgrade` 头的 HTTP 请求
2. ohosHttp 通过 `is_websocket_upgrade()` 检测到 WebSocket 升级请求
3. 查找匹配的 `[[server.location]]` 路径规则（按最长前缀匹配），要求配有 `proxy_pass`
4. 如果匹配：建立到后端的 TCP 连接，转发原始 WebSocket 握手头
5. 后端返回 **101 Switching Protocols** 后，回复 101 给客户端，开始桥接双向数据
6. 如果不匹配：降级到普通 HTTP 处理（不产生错误，不会中断请求）

### 配置方式

**配置与普通反向代理完全相同**——WebSocket 支持自动启用：

```toml
# ==== 站点配置 ====
[[server]]
bind = "0.0.0.0:8080"
root = "./www"
domains = ["example.com", "www.example.com"]

# 普通 HTTP API 代理
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"

# WebSocket 服务代理（同一配置，自动识别升级）
[[server.location]]
path = "/ws"
proxy_pass = "http://127.0.0.1:8081"
```

### 完整示例：Node.js WebSocket 后端

#### 1. 创建 Node.js WebSocket 服务

```javascript
// server.js
const WebSocket = require('ws');
const wss = new WebSocket.Server({ port: 8081 });

wss.on('connection', function connection(ws, req) {
  console.log('客户端已连接, URL:', req.url);

  ws.on('message', function incoming(data) {
    console.log('收到:', data.toString());
    // 原样返回消息（回声）
    ws.send(`服务端回复: ${data}`);
  });

  ws.send('连接成功！欢迎使用 ohosHttp WebSocket 代理');
});

console.log('WebSocket 服务运行在 ws://localhost:8081');
```

启动：
```bash
node server.js
```

#### 2. 配置 ohosHttp

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./www"
domains = ["example.com"]

# 将 /ws 路径转发到 WebSocket 后端
[[server.location]]
path = "/ws"
proxy_pass = "http://127.0.0.1:8081"
```

启动 ohosHttp：
```bash
ohosHttp -b 0.0.0.0:8080
```

#### 3. 客户端连接

```javascript
// client.js
const WebSocket = require('ws');
const ws = new WebSocket('ws://example.com:8080/ws');

ws.on('open', function open() {
  ws.send('你好，ohosHttp!');
});

ws.on('message', function incoming(data) {
  console.log('收到:', data.toString());
});

ws.on('error', function error(err) {
  console.error('连接错误:', err.message);
});
```

运行：
```bash
node client.js
# 输出: 收到: 连接成功！欢迎使用 ohosHttp WebSocket 代理
# 输出: 收到: 服务端回复: 你好，ohosHttp!
```

### 多路径 WebSocket 代理

可以为不同路径配置不同的 WebSocket 后端：

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./www"
domains = ["example.com"]

# 聊天 WebSocket
[[server.location]]
path = "/chat"
proxy_pass = "http://127.0.0.1:9001"

# 通知 WebSocket
[[server.location]]
path = "/notify"
proxy_pass = "http://127.0.0.1:9002"

# 游戏 WebSocket
[[server.location]]
path = "/game"
proxy_pass = "http://127.0.0.1:9003"
```

### 与负载均衡配合

WebSocket 代理同样支持负载均衡后端：

```toml
[[server.location]]
path = "/ws"
load_balance_strategy = "round_robin"
[[server.location.load_balance_targets]]
address = "http://127.0.0.1:9001"
[[server.location.load_balance_targets]]
address = "http://127.0.0.1:9002"
[[server.location.load_balance_targets]]
address = "http://127.0.0.1:9003"
```

> **注意**：WebSocket 是长连接，负载均衡器会尽量将同一客户端的 WebSocket 连接分发到同一后端（基于源 IP 哈希）。

### 工作原理

```
客户端 (wss://example.com/ws)
    │
    │  HTTP Upgrade 请求 (Upgrade: websocket)
    ▼
ohosHttp (0.0.0.0:8080)
    │
    │  1. 检测到 WebSocket 升级
    │  2. 查找匹配路径 /ws → proxy_pass = "http://127.0.0.1:8081"
    │  3. TCP 连接到 127.0.0.1:8081
    │  4. 转发原始握手请求头（保留 Upgrade、Connection、Sec-WebSocket-* 等）
    ▼
Node.js WebSocket 后端 (127.0.0.1:8081)
    │
    │  返回 101 Switching Protocols
    ▼
ohosHttp 回复 101 给客户端
    │
    │  tokio::select! 双向桥接
    │  ┌──────────────────────────────┐
    │  │  客户端 ──→ 后端 (copy)      │
    │  │  后端   ──→ 客户端 (copy)    │
    │  └──────────────────────────────┘
    │  任意方向断开 → 隧道关闭
    ▼
连接保持，双向实时通信
```

### 关键特性

| 特性 | 说明 |
|:----|:-----|
| **配置零额外开销** | 已有的 `proxy_pass` 规则自动支持 WebSocket，无需添加任何标记 |
| **保留 WebSocket 头** | `Upgrade`、`Connection`、`Sec-WebSocket-*` 等关键头完整转发 |
| **双向桥接** | 使用 `tokio::io::copy` 实现客户端↔后端全双工数据流 |
| **自动清理** | 任一端断开连接，隧道自动关闭，资源释放 |
| **错误处理** | 后端连接失败、非 101 响应、升级失败均有日志记录，不会挂起连接 |
| **路径匹配** | 按最长前缀匹配，支持多路径多后端 |

### 注意事项

1. **后端必须支持 WebSocket**：ohosHttp 仅做代理转发，后端服务本身需要完整实现 WebSocket 协议
2. **保持连接存活**：WebSocket 是长连接，请确保后端有合理的连接管理机制（心跳、超时断开）
3. **跨域问题**：如果前端页面和 WebSocket 服务不同源，需要在 ohosHttp 中配置 CORS（见 [CORS 跨域配置](#cors-跨域配置) 章节）
4. **端口开放**：确保 ohosHttp 和后端服务之间的网络可达
5. **协议降级**：如果请求路径没有匹配的 `proxy_pass`，ohosHttp 自动降级为普通 HTTP 处理，WebSocket 握手请求会作为普通请求处理（通常返回 400 或 404）

### 调试方法

如果 WebSocket 代理不工作，可以按以下步骤排查：

```bash
# 1. 确认后端 WebSocket 服务正常运行
curl -i -N -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Host: localhost" http://127.0.0.1:8081/
# 应返回 HTTP/1.1 101 Switching Protocols

# 2. 确认 ohosHttp 代理路径配置正确
curl -i -N -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Host: example.com" http://127.0.0.1:8080/ws
# 应返回 HTTP/1.1 101 Switching Protocols

# 3. 查看 ohosHttp 日志
tail -f server.log | grep -i "websocket"

# 4. 使用 wscat 测试（需要安装）
# npm install -g wscat
wscat -c ws://127.0.0.1:8080/ws
# 连接成功后会进入交互模式，可以发送和接收消息
```

---

## 路径规则

路径规则支持在同一站点内将不同路径映射到不同的文件系统位置或后端服务。

### 类型1：映射到其他目录（root）

```toml
[[server.location]]
path = "/static"
root = "/var/www/static"        # 文件系统路径
expires = "7d"                  # 浏览器缓存过期时间（可选）
```

请求 `/static/css/style.css` 会映射到 `/var/www/static/css/style.css`

### 类型2：反向代理（proxy_pass）

```toml
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"
```

### 类型3：路径级 CGI

```toml
[[server.location]]
path = "/admin"
cgi = { interpreter = "/usr/bin/php-cgi", extensions = [".php"] }
```

### 路径匹配顺序

路径按长度降序匹配（最长匹配优先），确保 `/api/v2` 优先于 `/api` 被匹配。

---

## 缓存配置

ohosHttp 提供两级缓存：

### 1. 内存缓存（服务器端）

减少磁盘 I/O，提升静态文件响应速度：

```toml
cache_enabled = true       # 启用缓存
cache_ttl = "1h"          # 缓存有效时间
cache_max_size = "100MB"  # 最大缓存内存占用
```

支持的缓存时间格式：
- `10s` - 10 秒
- `5m` - 5 分钟
- `1h` - 1 小时
- `1d` - 1 天

### 2. 浏览器缓存（expires）

通过位置规则中的 `expires` 字段控制：

```toml
[[server.location]]
path = "/static"
root = "/var/www/static"
expires = "7d"              # 浏览器缓存 7 天
```

启用浏览器缓存时，服务器会返回 `Cache-Control: max-age=...` 头。

---

## CORS 跨域配置

CORS（跨域资源共享）允许网页从不同域名访问服务器的资源。

### 配置文件启用

```toml
[[server]]
# ...
cors_origin = "*"                                              # 允许所有来源
cors_methods = "GET,POST,PUT,DELETE,PATCH,OPTIONS"             # 允许的HTTP方法
cors_headers = "Content-Type,Authorization,X-Requested-With"   # 允许的自定义头
```

### CORS 配置项

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `cors_origin` | `""`（空=不启用） | 允许的源，`*` 表示所有域名 |
| `cors_methods` | `"GET,POST,PUT,DELETE,PATCH,OPTIONS"` | 允许的 HTTP 方法 |
| `cors_headers` | `"*"` | 允许的请求头 |

### 工作原理

1. **OPTIONS 预检请求**：浏览器发送 OPTIONS 请求时，服务器返回 204 No Content，并设置：
   - `Access-Control-Allow-Origin`
   - `Access-Control-Allow-Methods`
   - `Access-Control-Allow-Headers`
   - `Access-Control-Max-Age: 86400`
2. **所有响应**：都会自动添加 `Access-Control-Allow-Origin` 头
3. **反向代理**：代理响应也会自动添加 CORS 头

### 不启用 CORS

CORS 默认不启用。如果 `cors_origin` 为空或未配置，不会添加任何 CORS 头。

---

## HTTP 方法支持

ohosHttp 完整支持以下 HTTP 方法：

| 方法 | 用途 | 处理方式 |
|------|------|----------|
| `GET` | 获取资源 | 返回静态文件 / CGI 输出 |
| `HEAD` | 仅获取响应头 | 同 GET，不返回 Body |
| `POST` | 提交数据 | 上传文件 / CGI 执行（带 Body）|
| `PUT` | 上传资源 | 同 POST，保存请求体到文件 |
| `PATCH` | 部分更新 | 同 PUT，保存请求体到文件 |
| `DELETE` | 删除资源 | 删除匹配的文件（返回 200/404）|
| `OPTIONS` | CORS 预检 | 返回 204 + CORS 头 |

### 说明

- **GET/HEAD**：静态文件服务、CGI 执行
- **POST/PUT/PATCH**：Body 会保存到 `{root}/uploads/` 目录，同时支持 CGI 程序处理
- **DELETE**：删除 `{root}` 下的文件（路径穿越防护生效）
- **OPTIONS**：专门处理 CORS 预检请求（无论是否启用 CORS 都返回 204）

所有方法都经过 URL 重写、路径规则匹配和路径穿越防护。

---

## IP 直接访问控制

ohosHttp 支持禁止 IP 直接访问站点，只允许通过绑定的域名访问。

### 配置方式

在站点配置中设置 `allow_ip_access` 为 `false`：

```toml
[[server]]
bind = "0.0.0.0:443"
root = "/var/www/html"
domains = ["example.com", "www.example.com"]
allow_ip_access = false
```

### 工作原理

- `allow_ip_access = true`（默认）：允许通过 IP 地址直接访问
- `allow_ip_access = false`：检查请求的 `Host` 头是否匹配配置的 `domains` 列表
- 匹配失败时返回 **403 Forbidden**，提示 "Direct IP access is not allowed"
- Host 头中的端口号会被自动忽略（例如 `Host: example.com:8080` 仍能正常匹配）

### 使用场景

- 防止恶意用户直接扫描 IP 地址绕过域名防火墙
- 多个虚拟主机共享同一 IP 时，确保每个站点只能通过其域名访问
- 配合 CDN 或反向代理使用时，限制只有已知域名才能回源

---

## 限流与安全

ohosHttp 提供基于令牌桶的 IP 限流器，支持黑名单和按 IP 自定义速率。

### 配置文件启用限流

```toml
[[server]]
# ...
rate_limit = { enabled = true, requests_per_second = 100, burst_size = 200 }
```

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `enabled` | `true` | 是否启用限流 |
| `requests_per_second` | `100` | 每 IP 每秒允许的请求数 |
| `burst_size` | `200` | 令牌桶容量（允许短时突发流量） |

### IP 黑名单

完全屏蔽指定 IP 的访问：

```toml
[[server]]
# ...
blacklist = ["10.0.0.1", "192.168.1.*", "203.0.113.0"]
```

支持 `*` 通配符进行 IP 段匹配。

### 按 IP 自定义限流

为特定 IP 配置不同的速率（覆盖全局 `requests_per_second`）：

```toml
[[server]]
# ...
[server.per_ip_rates]
"192.168.1.50" = 50      # 该 IP 每秒最多 50 个请求
"10.0.0.2" = 1000        # 该 IP 每秒最多 1000 个请求（API 服务器）
```

### 工作原理

- 使用令牌桶算法，每个 IP 独立计数
- 白名单中的 IP 不受限流影响
- 黑名单中的 IP 直接返回 403 Forbidden
- 被限流的 IP 返回 429 Too Many Requests
- 每 60 秒自动清理过期 IP 记录（超过 10000 条时清空）

### 禁止访问目录/文件

ohosHttp 支持按目录和文件类型禁止特定路径的访问，适用于保护敏感文件不被泄露。

**注意：**
1. 此功能仅对静态文件服务生效。如果站点配置了反向代理（`proxy_pass` 或 `load_balance_targets`），禁止访问规则会被自动跳过。
2. **TOML 位置限制**：`forbidden_dirs` 和 `forbidden_files` 必须放在 `[[server.location]]`、`[[server.rewrite]]`、`[server.rate_limiter]` 等子表定义**之前**，否则会被 TOML 解析器当成子表内的字段而忽略（这是 TOML 规范行为）。

#### 禁止目录

```toml
[[server]]
# ...
forbidden_dirs = ["/runtime/*", "/private/*", "/backup/*"]
```

`/runtime/*` 会禁止 `/runtime/`、`/runtime/config.json`、`/runtime/subdir/` 等所有以 `/runtime/` 开头的路径。

#### 禁止文件类型

```toml
[[server]]
# ...
forbidden_files = ["*.toml", "*.env", "*.json", "*.yaml", "*.lock"]
```

`*.toml` 会禁止所有以 `.toml` 结尾的文件访问（如 `/config.toml`、`/subdir/app.toml`）。

**工作原理**

- 在 URL 重写（伪静态）之后、路径规则匹配之前进行检查
- 匹配规则使用前缀匹配（目录）和后缀匹配（文件类型）
- 对配置了 `proxy_pass` 或 `load_balance_targets` 的站点自动跳过
- 匹配时返回 403 Forbidden，并记录访问日志

---

## Session 支持

ohosHttp 提供基于 Cookie 的内存 Session 管理。

### 启用 Session

```toml
[[server]]
# ...
session = { enabled = true, cookie_name = "OHOS_SESSION", ttl = 3600 }
```

### Session 配置项

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `enabled` | `true` | 是否启用 Session |
| `cookie_name` | `"OHOS_SESSION"` | Cookie 名称 |
| `ttl` | `3600` | Session 过期时间（秒） |

### 工作原理

1. 客户端首次请求时，服务器创建 Session 并下发 `Set-Cookie` 头
2. 后续请求携带 Cookie，服务器解析 Session ID
3. 无请求超过 TTL 后，Session 自动过期并清理
4. Session 数据存储在服务器内存中

### Cookie 属性

```
Set-Cookie: OHOS_SESSION=<id>; HttpOnly; SameSite=Lax; Path=/; Max-Age=3600
```

- `HttpOnly`：防止 XSS 窃取 Cookie
- `SameSite=Lax`：防止 CSRF 攻击
- `Path=/`：全站有效
- `Max-Age`：与 Session TTL 一致

---

## 负载均衡

ohosHttp 支持将请求分发到多个后端服务器，实现负载均衡和高可用。

### 配置方式

在 `[[server.location]]` 中配置多个 `load_balance_targets`：

```toml
[[server.location]]
path = "/api"
load_balance_strategy = "round_robin"
proxy_headers = ["X-Forwarded-For"]

[[server.location.load_balance_targets]]
url = "http://127.0.0.1:3001"
weight = 5

[[server.location.load_balance_targets]]
url = "http://127.0.0.1:3002"
weight = 3

[[server.location.load_balance_targets]]
url = "http://127.0.0.1:3003"
weight = 2
```

### 配置项

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `load_balance_strategy` | `"round_robin"` | 负载均衡策略（目前仅支持 round_robin） |
| `load_balance_targets` | `[]` | 后端目标列表 |
| `url` | — | 后端服务 URL |
| `weight` | `1` | 权重，值越大分配请求越多 |

### 工作原理

1. 请求匹配路径规则后，负载均衡器按加权轮询选择后端
2. 将原请求透传给选中的后端服务器
3. 后端响应原样返回给客户端
4. 自动添加代理头（X-Forwarded-For 等）

### 与传统反向代理的区别

| 特性 | 单点代理 (proxy_pass) | 负载均衡 (load_balance_targets) |
|------|----------------------|-------------------------------|
| 后端数量 | 1 个 | 多个 |
| 流量分配 | 全部到同一后端 | 加权轮询分发 |
| 高可用 | 不支持（单点故障） | 支持（多后端冗余） |
| 配置复杂度 | 低 | 中 |

---

## 访问日志与日志轮转

ohosHttp 支持 Apache Combined Log Format 风格的访问日志，并支持按日期和大小自动轮转。

### 配置

```toml
[[server]]
# ...
access_log = "./logs/access.log"       # 日志文件路径
log_rotate_size = "100MB"              # 单文件大小限制（0=不限制，仅按日期轮转）
```

### 日志格式

每条日志使用 Combined Log Format + 响应时间：

```
192.168.1.1 - - [17/May/2026:10:30:15 +0800] "GET /index.html HTTP/1.1" 200 1234 "-" "Mozilla/5.0" 12ms
```

字段说明：
| 字段 | 说明 |
|------|------|
| `remote_addr` | 客户端 IP 地址 |
| `date` | 请求时间（HTTP 标准日期格式） |
| `method` | 请求方法 |
| `path` | 请求路径 |
| `status` | HTTP 状态码 |
| `size` | 响应 Body 字节数 |
| `referer` | Referer 头（`-` 表示无） |
| `user_agent` | User-Agent 头 |
| `duration_ms` | 响应耗时（毫秒） |

### 日志轮转机制

#### 按日期轮转

每天自动创建新的日志文件，格式为：

```
logs/access_2026-05-17.log
logs/access_2026-05-18.log
logs/access_2026-05-19.log
```

日期基于服务器本地时间（`Local::now()`）。

#### 按大小轮转

当单个日志文件超过 `log_rotate_size` 后，当前文件会被重命名为 `.1` 版本，并开始新文件：

```
# 超过 100MB 前：
logs/access_2026-05-17.log

# 超过 100MB 后：
logs/access_2026-05-17.log          # 新文件（继续写入）
logs/access_2026-05-17.1.log        # 旧文件（已轮转）
```

**注意**：日志轮转仅保留一个备份（`.1`），更早的备份会被覆盖。

#### 不配置日志

如果不设置 `access_log`，则不记录访问日志。此时不会创建日志目录和文件。

---

## 停止与重启

ohosHttp 支持通过 Unix 信号进行优雅的停止和重启。

### 停止服务

#### 使用 SIGTERM（推荐）

```bash
# 发送 SIGTERM 信号
kill -TERM <PID>

# 或使用 PID 文件
kill $(cat /var/run/ohoshttp.pid)

# 或使用 pkill
pkill ohosHttp
```

收到 SIGTERM 后，服务器会：
1. 停止接受新连接
2. 等待已建立连接处理完成（优雅关闭）
3. 输出停止日志后退出

#### 使用 Ctrl+C（前台模式）

直接在前台按 `Ctrl+C` 即可。

### 热重启（SIGHUP）

通过配置文件启动的服务支持热重启：

```bash
# 发送 SIGHUP 信号
kill -HUP <PID>

# 或
kill -1 $(cat /var/run/ohoshttp.pid)
```

热重启流程：
1. 收到 SIGHUP 信号
2. 优雅关闭所有当前连接
3. 重新读取配置文件
4. 使用新配置启动服务
5. 重新打印启动画面

**注意**：热重启仅对使用 `-c config.toml` 启动的服务有效。CLI 模式（`-a` + `-r`）不支持热重启。

### 完整服务管理示例

```bash
# 启动（守护进程模式）
ohosHttp -c config.toml -d --pidfile /var/run/ohoshttp.pid

# 查看运行状态
cat /var/run/ohoshttp.pid
ps -p $(cat /var/run/ohoshttp.pid)

# 热重启（重新加载配置）
kill -HUP $(cat /var/run/ohoshttp.pid)

# 优雅停止
kill -TERM $(cat /var/run/ohoshttp.pid)

# 清理 PID 文件（停止后）
rm -f /var/run/ohoshttp.pid
```

---

## HTTPS 加密

ohosHttp 支持通过 `--cert` 和 `--key` 命令行参数开启 TLS 加密，同时支持配置文件配置。

### 命令行快速启用

```bash
# 使用自签名证书（测试用）
ohosHttp -a 0.0.0.0:443 -r ./www --cert server.crt --key server.key

# 使用 Let's Encrypt 证书
ohosHttp -a 0.0.0.0:443 -r ./www --cert /etc/letsencrypt/live/example.com/fullchain.pem --key /etc/letsencrypt/live/example.com/privkey.pem
```

### 生成自签名证书（测试用）

```bash
openssl req -x509 -newkey rsa:4096 -nodes -keyout server.key -out server.crt -days 365 -subj "/CN=localhost"
```

### 配置文件方式

```toml
[[server]]
bind = "0.0.0.0:443"
root = "./www"
cert = "/path/to/cert.pem"
key = "/path/to/key.pem"
```

### TLS 配置说明

- 使用 `rustls` 库实现 TLS，纯 Rust 实现，无需 OpenSSL 依赖
- 默认启用 TLS 1.2 和 TLS 1.3
- HTTP/2 和 HTTP/1.1 自动协商（通过 ALPN）
- 证书和私钥必须为 PEM 格式

---

## HTTP/2 支持

当启用 HTTPS 后，ohosHttp **自动**支持 HTTP/2，无需任何额外配置。

### 工作原理

1. TLS 握手时通过 ALPN（Application-Layer Protocol Negotiation）协商协议
2. 客户端支持 HTTP/2 时，自动升级到 HTTP/2 连接
3. 客户端仅支持 HTTP/1.1 时，回退到 HTTP/1.1

### 验证 HTTP/2 是否生效

```bash
# 使用 curl 测试
curl -I --http2 https://localhost:443/

# 查看响应头中的协议信息
curl -v --http2 https://localhost:443/ 2>&1 | grep "ALPN\|HTTP/2"
```

### HTTP/2 特性

- **多路复用**：单个连接上并行处理多个请求
- **头部压缩**：使用 HPACK 算法减少头部开销
- **服务器推送**：暂不支持（计划中）
- **流优先级**：支持请求优先级排序

### 配置 HTTP/2 keep-alive

HTTP/2 连接默认每 30 秒发送一次 PING 帧保持连接活跃。此行为不可配置（固定 30 秒）。

---

## HTTP/3 (QUIC) 支持

ohosHttp 通过 `--http3-port` 参数支持 HTTP/3 over QUIC（UDP）。

### 启用 HTTP/3

```bash
# 同时启用 HTTPS (TCP) 和 HTTP/3 (UDP)
ohosHttp -a 0.0.0.0:443 -r ./www --cert server.crt --key server.key --http3-port 4433

# 仅 HTTP/3（需要证书）
ohosHttp -a 0.0.0.0:80 -r ./www --cert server.crt --key server.key --http3-port 4433
```

### 配置文件方式

```toml
[[server]]
bind = "0.0.0.0:443"
root = "./www"
cert = "/path/to/cert.pem"
key = "/path/to/key.pem"
http3_port = 4433   # 可选，不设置则不启动 HTTP/3
```

### HTTP/3 工作原理

1. ohosHttp 在指定 UDP 端口上建立 QUIC 连接
2. QUIC 连接内置 TLS 1.3 加密
3. 使用 `h3` 和 `h3-quinn` 库实现 HTTP/3 帧传输
4. 每个客户端请求通过 QUIC 双向流处理
5. 支持请求体读取和响应发送

### 测试 HTTP/3

```bash
# 使用 curl 测试（curl 需支持 HTTP/3）
curl --http3 https://localhost:4433/

# 使用浏览器（Chrome/Firefox 支持 HTTP/3）
# 打开 chrome://net-export/ 查看 QUIC 连接状态
```

### 已知限制

- HTTP/3 端口与 HTTP/HTTPS 端口不同（因为使用 UDP 而非 TCP）
- 需要有效的 TLS 证书（HTTP/3 强制加密）
- 某些网络环境（如公司防火墙）可能阻止 UDP 443 端口
- HTTP/3 不支持服务器推送

---

## 多站点配置

一个配置文件可以管理多个独立的站点。

### 配置文件示例

```toml
# ==== 站点1：主站 ====
[[server]]
bind = "0.0.0.0:8080"
root = "./www"
domains = ["example.com", "www.example.com"]
threads = 4
cache_enabled = true

# ==== 站点2：博客 ====
[[server]]
bind = "0.0.0.0:8081"
root = "./www2"
domains = ["blog.example.com"]
threads = 2
directory_listing = true

# ==== 站点3：API 服务 ====
[[server]]
bind = "127.0.0.1:9000"
root = "./api"
```

### 启动多站点

```bash
# 启动所有站点
ohosHttp -c config.toml --all

# 或逐个启动（每个命令一个站点）
ohosHttp -c config.toml       # 仅启动第一个站点
```

### 虚拟主机机制

当站点配置了 `domains` 时：
- 请求的 `Host` 头匹配对应域名的站点处理
- 不匹配任何域名的请求由第一个站点处理（默认站点）

---

## 启动画面说明

ohosHttp 在启动时显示一个信息画面，包含：

```
╔═══════════════════════════════════════════════════════════════╗
  ║     ██████╗ ██╗  ██╗ ██████╗ ███████╗                        ║
  ║    ██╔═══╝ ██║  ██║██╔═══╝ ██╔════╝                        ║
  ║    ██║     ███████║██║     █████╗                          ║
  ║    ██║     ██╔══██║██║     ██╔══╝                          ║
  ║    ╚██████╗██║  ██║╚██████╗███████╗                        ║
  ║     ╚═════╝╚═╝  ╚═╝ ╚═════╝╚══════╝                        ║
  ║   ohosHttp v1.0.0    ─ 高性能 HTTP 服务器                    ║
  ╚═══════════════════════════════════════════════════════════════╝

  ┌─── 站点 #1 ────────────────────────────────────────────────┐
  │   绑定地址    │  0.0.0.0:8080                                  │
  │   根目录      │  ./www                                          │
  │   域名        │  example.com, www.example.com                   │
  │   ...                                                          │
  │   伪静态规则  │  7 条                                            │
  │   CGI解释器   │  /usr/bin/php-cgi                               │
  └─────────────────────────────────────────────────────────────────┘
```

画面信息包括：
- 版本号、启动时间、PID
- 每个站点的配置详情
- 伪静态重写规则列表
- 反向代理和路径规则
- CGI 解释器配置
- 禁止访问规则数量（目录/文件）
- 启动状态提示

---

## 常见问题

### 1. "地址已占用"错误

```
thread 'main' panicked at '地址已被占用: 127.0.0.1:8080'
```

原因：端口被其他程序占用。

解决：
```bash
# 查看占用端口的程序
lsof -i :8080
# 或
netstat -tlnp | grep 8080

# 改用其他端口
ohosHttp -a 127.0.0.1:8081 -r ./www
```

### 2. 配置文件格式错误

```
错误: TOML 解析错误: ...
```

解决：
- 使用 `ohosHttp --gen-config` 生成模板后修改
- 检查 TOML 格式（注意字符串引号、数组逗号）
- 不要将手写的重写规则正则中反斜杠写错

### 3. CGI 脚本不执行

检查项：
- 确认解释器路径正确：`which php-cgi`
- 确认 CGI 扩展名配置正确
- 确认脚本文件存在且有执行权限
- 检查服务器日志中的错误信息

### 4. 静态文件 404

检查项：
- 确认 `root` 目录存在且文件在其中
- 确认路径大小写（Linux 区分大小写）
- 检查 `index_files` 配置（默认页面名称）
- 检查伪静态规则是否误匹配了正常路径

### 5. 守护进程无法启动

检查项：
- 确认端口未被占用
- 检查 PID 文件写入权限
- 使用 `ohosHttp` 前台启动查看错误信息（不加 `-d`）

### 6. 如何重启服务

```bash
# 使用 PID 文件
kill -HUP $(cat /var/run/ohos.pid)   # 重启（暂不支持）
kill $(cat /var/run/ohos.pid)        # 停止
ohosHttp -c config.toml -d           # 重新启动

# 无 PID 文件
pkill ohosHttp
ohosHttp -c config.toml -d
```

---

## 管理 API（鸿蒙 ArkTS 接口）

ohosHttp 提供了一套 RESTful 管理 API，允许通过 HTTP 接口管理服务器。特别为鸿蒙 PC/设备端 ArkTS 应用提供了完整的客户端 SDK。

### 启用管理 API

通过 `--manage-auth` 参数启用：

```bash
ohosHttp -a 0.0.0.0:8089 -r ./www --manage-auth mySecretToken
```

如欲停止使用 Arg 也能够在启动时使用 `OHOS_MANAGE_TOKEN` 环境变量：

```bash
export OHOS_MANAGE_TOKEN=mySecretToken
ohosHttp -a 0.0.0.0:8089 -r ./www --manage-auth-auto
```

启用后，启动画面会显示：

```
  │   管理 API    │  已启用 (/_ohos/, 需要 Bearer Token 认证)
```

### API 端点一览

| 方法 | 路径 | 描述 | 请求体 |
|------|------|------|--------|
| GET | `/_ohos/config` | 获取服务器完整配置 | — |
| PUT | `/_ohos/config` | 更新配置（TOML 字符串或 JSON） | TOML / JSON |
| GET | `/_ohos/status` | 获取服务器运行状态 | — |
| GET | `/_ohos/metrics` | 获取运行时指标 | — |
| POST | `/_ohos/start` | 恢复服务（取消暂停） | — |
| POST | `/_ohos/pause` | 暂停服务（新请求返回 503） | — |
| POST | `/_ohos/stop` | 优雅停止服务器 | — |
| POST | `/_ohos/restart` | 热重启（重新加载配置 + 零停机） | — |

### 认证方式

所有 API 请求需要在 HTTP 头中携带 Bearer Token：

```
Authorization: Bearer mySecretToken
```

### 通用响应结构

```json
{
  "code": 0,
  "message": "success",
  "data": { ... }
}
```

错误时：

```json
{
  "code": 401,
  "message": "Unauthorized",
  "error": "Invalid or missing auth token"
}
```

### 鸿蒙 ArkTS 客户端

项目提供了完整的 ArkTS 接口文件 `examples/ohos-http-api.ets`，包含：

- **类型定义**：`ServerConfig`、`AppConfig`、`StatusData`、`MetricsData` 等全部配置项
- **客户端类** `OhosHttpClient`：封装所有 API 调用
- **开发示例**：完整的鸿蒙 @Entry @Component 页面

#### 引入方式

将 `examples/ohos-http-api.ets` 复制到你的鸿蒙项目中：

```typescript
import { OhosHttpClient } from './ohos-http-api';
```

#### 基础用法

```typescript
// 创建客户端（地址为 ohosHttp 绑定地址 + 管理 Token）
const client = new OhosHttpClient('http://192.168.1.100:8089', 'mySecretToken');

// 获取状态
const status = await client.getStatus();
if (status.code === 0) {
  console.info(`服务器已运行 ${status.data!.status.uptime_human}`);
}

// 获取完整配置
const config = await client.getConfig();
console.info(`站点数: ${config.data!.server_count}`);

// 暂停/恢复
await client.pause();
await client.start();

// 热重启
await client.restart();
```

#### 开发示例

参考 `examples/ohos-http-api.ets` 文件末尾的完整 ArkTS 页面示例，包含：

- 状态实时刷新
- 启动 / 暂停 / 重启 / 停止按钮
- 配置查看与修改界面

### API 响应数据类型

#### `GET /_ohos/config` → `ConfigData`

```typescript
interface ConfigData {
  config: AppConfig;    // 完整应用配置（含所有 server 配置项）
  config_path: string;  // 配置文件路径
  server_count: number; // 站点数
}
```

#### `GET /_ohos/status` → `StatusData`

```typescript
interface StatusData {
  server: {
    version: string;
    name: string;
    description: string;
  };
  status: {
    paused: boolean;
    uptime_secs: number;
    uptime_human: string;
    pid: number;
    ppid: number;
  };
  config: {
    config_path: string;
    server_count: number;
  };
  sites: Array<{
    bind: string;
    root: string;
    domains: string[];
    https: boolean;
    workers: number;
  }>;
}
```

#### `GET /_ohos/metrics` → `MetricsData`

```typescript
interface MetricsData {
  requests: {
    total: number;
    per_second: number;
  };
  uptime: {
    seconds: number;
    human: string;
  };
  process: {
    pid: number;
    worker_index: number;
  };
  memory: Record<string, string>;
}
```

### 配置文件所有配置项

管理 API 暴露的配置项与 TOML 配置文件一一对应。详见下方 ArkTS 类型定义中的 `ServerConfig` 接口：

| 配置字段 | 类型 | 说明 |
|----------|------|------|
| `bind` | `string` | 绑定地址和端口 |
| `root` | `string` | 网站根目录 |
| `domains` | `string[]` | 虚拟主机域名列表 |
| `upload_max_size` | `string` | 上传最大大小（如 "10MB"） |
| `workers` | `number` | Worker 进程数（0=自动） |
| `cache_enabled` | `boolean` | 是否启用缓存 |
| `cache_ttl` | `string` | 缓存过期时间（如 "1h"） |
| `cache_max_size` | `string` | 缓存最大内存（如 "100MB"） |
| `directory_listing` | `boolean` | 目录列表 |
| `access_log` | `string?` | 访问日志路径 |
| `log_rotate_size` | `string` | 日志轮转大小 |
| `rewrite` | `RewriteRule[]` | URL 重写规则 |
| `cgi` | `CgiConfig[]` | CGI 解释器配置 |
| `location` | `LocationConfig[]` | 路径规则（代理/静态/CGI） |
| `cors_origin` | `string` | CORS 允许的源 |
| `cors_methods` | `string` | CORS 允许的方法 |
| `cors_headers` | `string` | CORS 允许的头 |
| `cert` | `string?` | TLS 证书路径 |
| `key` | `string?` | TLS 私钥路径 |
| `http3_port` | `string` | HTTP/3 (QUIC) 端口（0=不启用） |
| `rate_limit` | `RateLimitConfig?` | 限流配置 |
| `blacklist` | `string[]` | IP 黑名单 |
| `per_ip_rates` | `Record<string, number>` | 自定义 IP 限流 |
| `session` | `SessionConfig?` | Session 配置 |
| `allow_ip_access` | `boolean` | 允许 IP 直连 |
| `forbidden_dirs` | `string[]` | 禁止访问的目录列表 |
| `forbidden_files` | `string[]` | 禁止访问的文件类型列表 |

### cURL 使用示例

```bash
AUTH="Authorization: Bearer mySecretToken"

# 获取运行状态
curl -s -H "$AUTH" http://localhost:8089/_ohos/status | jq .

# 获取指标
curl -s -H "$AUTH" http://localhost:8089/_ohos/metrics | jq .

# 获取配置
curl -s -H "$AUTH" http://localhost:8089/_ohos/config | jq .

# 更新配置（TOML 格式）
curl -X PUT -H "$AUTH" -H "Content-Type: text/plain" \
  -d @config.toml http://localhost:8089/_ohos/config

# 更新配置（JSON 格式）
curl -X PUT -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"format": "json"}' http://localhost:8089/_ohos/config

# 暂停服务
curl -X POST -H "$AUTH" http://localhost:8089/_ohos/pause

# 恢复服务
curl -X POST -H "$AUTH" http://localhost:8089/_ohos/start

# 热重启
curl -X POST -H "$AUTH" http://localhost:8089/_ohos/restart

# 停止服务
curl -X POST -H "$AUTH" http://localhost:8089/_ohos/stop
```
