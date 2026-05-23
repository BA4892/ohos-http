// Copyright 2025 ohos-server Contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! ohos-server 初始化模块
//!
//! 首次使用 ohos-server 前必须运行 `ohos-server init`。
//! 初始化程序会：
//! - 创建数据存储目录（默认 `~/.ohos/server/`）
//! - 生成默认配置文件
//! - 创建一个演示站点
//! - 设置日志目录

use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

/// ohos-server 数据目录名称（放在用户家目录下）
const OHOS_DIR_NAME: &str = ".ohos";
const SERVER_DIR_NAME: &str = "server";

/// 检查是否已经初始化
pub fn is_initialized() -> bool {
    let data_dir = data_dir();
    // 检查 .initialized 标记文件存在
    data_dir.join(".initialized").exists()
}

/// 获取用户家目录（Linux/macOS）
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// 获取默认数据目录路径: ~/.ohos/server/
pub fn data_dir() -> PathBuf {
    home_dir().join(OHOS_DIR_NAME).join(SERVER_DIR_NAME)
}

/// 获取默认配置文件路径
pub fn default_config_path() -> PathBuf {
    data_dir().join("config.toml")
}

/// 获取默认网站根目录
pub fn default_www_dir() -> PathBuf {
    data_dir().join("www")
}

/// 获取日志目录
pub fn default_log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// 打印首次使用提示
pub fn print_first_time_prompt() {
    eprintln!("\n╔══════════════════════════════════════════════╗");
    eprintln!("║      🚀  欢迎使用 ohos-server！               ║");
    eprintln!("║                                               ║");
    eprintln!("║  首次使用前需要先初始化程序：                  ║");
    eprintln!("║                                               ║");
    eprintln!("║      ohos-server init                        ║");
    eprintln!("║                                               ║");
    eprintln!("║  初始化程序会：                                ║");
    eprintln!("║  1. 创建数据存储目录                          ║");
    eprintln!("║  2. 生成默认配置文件                          ║");
    eprintln!("║  3. 创建演示站点                              ║");
    eprintln!("║  4. 设置日志目录                              ║");
    eprintln!("╚══════════════════════════════════════════════╝\n");
}

/// 询问用户是否现在运行初始化
/// 返回 true 表示用户同意初始化
pub fn ask_init_now() -> bool {
    eprint!("是否现在运行初始化程序？(Y/n): ");
    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();

    let input = input.trim().to_lowercase();
    input.is_empty() || input == "y" || input == "yes"
}

/// 运行初始化流程
pub fn run_init(dir: Option<String>) {
    // 如果用户没有指定目录，使用默认目录
    let base_dir = match dir {
        Some(ref d) => {
            if d.is_empty() {
                data_dir()
            } else {
                PathBuf::from(d)
            }
        }
        None => data_dir(),
    };

    // 如果已经初始化，提示用户
    if base_dir.join(".initialized").exists() {
        eprintln!(
            "ℹ️  ohos-server 已在 '{}' 目录下初始化。",
            base_dir.display()
        );
        eprint!("是否重新初始化？(y/N): ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            eprintln!("取消初始化。");
            return;
        }
    }

    // 确认数据目录
    let data_dir = if dir.is_some() {
        base_dir.clone()
    } else {
        eprintln!("\n📁 数据存储目录");
        eprintln!("   默认路径: {}", base_dir.display());
        eprint!("   按 Enter 使用默认路径，或输入自定义路径: ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();
        let input = input.trim();
        if input.is_empty() {
            base_dir.clone()
        } else {
            PathBuf::from(input)
        }
    };

    eprintln!("\n⚙️  开始初始化 ohos-server ...\n");

    // 创建目录结构
    let dirs = [
        &data_dir,
        &data_dir.join("www"),
        &data_dir.join("logs"),
    ];

    for dir_path in &dirs {
        match std::fs::create_dir_all(dir_path) {
            Ok(_) => eprintln!("  ✅ 创建目录: {}", dir_path.display()),
            Err(e) => {
                eprintln!("  ❌ 创建目录失败 '{}': {}", dir_path.display(), e);
                process::exit(1);
            }
        }
    }

    // 生成默认配置文件
    let config_path = data_dir.join("config.toml");
    let config_content = generate_config(&data_dir);
    match std::fs::write(&config_path, config_content) {
        Ok(_) => eprintln!("  ✅ 生成配置: {}", config_path.display()),
        Err(e) => {
            eprintln!("  ❌ 生成配置失败 '{}': {}", config_path.display(), e);
            process::exit(1);
        }
    }

    // 创建演示站点首页
    let www_index = data_dir.join("www").join("index.html");
    match std::fs::write(&www_index, DEMO_INDEX_HTML) {
        Ok(_) => eprintln!("  ✅ 创建演示站点首页: {}", www_index.display()),
        Err(e) => {
            eprintln!("  ❌ 创建演示站点首页失败 '{}': {}", www_index.display(), e);
            process::exit(1);
        }
    }

    // 创建 .initialized 标记文件
    let marker = data_dir.join(".initialized");
    match std::fs::write(&marker, format!("ohos-server initialized\ncreated: {}", chrono::Local::now())) {
        Ok(_) => eprintln!("  ✅ 写入初始化标记: {}", marker.display()),
        Err(e) => {
            eprintln!("  ❌ 写入初始化标记失败 '{}': {}", marker.display(), e);
            process::exit(1);
        }
    }

    eprintln!(
        "\n🎉  ohos-server 初始化完成！\n\n\
         数据目录: {}\n\
         配置文件: {}\n\
         站点目录: {}\n\
         日志目录: {}\n",
        data_dir.display(),
        config_path.display(),
        data_dir.join("www").display(),
        data_dir.join("logs").display(),
    );

    eprintln!("启动服务器:");
    eprintln!("  ohos-server -c {}\n", config_path.display());
}

/// 生成默认配置文件内容（以数据目录为基础）
fn generate_config(data_dir: &PathBuf) -> String {
    let www_root = data_dir.join("www").to_string_lossy().to_string();
    let log_dir = data_dir.join("logs").to_string_lossy().to_string();
    let access_log = format!("{}/access.log", log_dir);

    format!(
        r#"# ═══════════════════════════════════════════════
# ohos-server 服务器配置文件
# 由 ohos-server init 自动生成
# 生成时间: {time}
# ═══════════════════════════════════════════════

# =============================================
# 站点 #1 - 演示站点
# =============================================
[[server]]
# 绑定地址和端口
bind = "0.0.0.0:8080"

# 网站根目录
root = "{root}"

# 绑定的域名列表（虚拟主机）
# 留空表示接受所有域名
domains = []

# Worker 进程数（0 = CPU 核心数）
workers = 0

# 上传文件大小限制（如 10MB, 100MB, 1GB）
upload_max_size = "10MB"

# 是否启用文件缓存
cache_enabled = false

# 缓存过期时间
cache_ttl = "1h"

# 缓存最大容量
cache_max_size = "100MB"

# 是否允许目录列表
directory_listing = false

# 访问日志文件路径（留空表示不记录访问日志）
access_log = "{access_log}"

# 日志轮转大小（如 100MB, 1GB；0 表示不轮转）
log_rotate_size = "100MB"

# CORS 配置
# cors_origin = "*"
# cors_methods = "GET,POST,PUT,DELETE,PATCH,OPTIONS"
# cors_headers = "*"

# TLS/SSL 证书（可选）
# cert = "/path/to/cert.pem"
# key = "/path/to/key.pem"

# HTTP/3 (QUIC) 端口（0 表示禁用）
# http3_port = "443"

# =============================================
# 速率限制（全局默认）
# =============================================
# [server.rate_limit]
# enabled = true
# rps = 100
# burst = 200
# max_conn = 100
# ban_duration = 300
# ban_threshold = 3

# =============================================
# 会话管理（全局默认）
# =============================================
# [server.session]
# enabled = true
# cookie_name = "OHOS_SESSION"
# ttl = 3600
"#,
        time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        root = www_root,
        access_log = access_log,
    )
}

/// 演示站点首页 HTML
const DEMO_INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ohos-server - 演示站点</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
        }

        .container {
            background: rgba(255, 255, 255, 0.95);
            border-radius: 16px;
            box-shadow: 0 20px 60px rgba(0, 0, 0, 0.15);
            padding: 40px;
            max-width: 600px;
            width: 100%;
            text-align: center;
        }

        .logo {
            font-size: 48px;
            margin-bottom: 16px;
        }

        h1 {
            font-size: 28px;
            color: #333;
            margin-bottom: 8px;
        }

        .subtitle {
            color: #666;
            font-size: 14px;
            margin-bottom: 30px;
        }

        .info-grid {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 12px;
            margin-bottom: 30px;
        }

        .info-item {
            background: #f8f9fa;
            border-radius: 8px;
            padding: 16px;
            text-align: left;
        }

        .info-label {
            font-size: 11px;
            color: #999;
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 4px;
        }

        .info-value {
            font-size: 16px;
            font-weight: 600;
            color: #333;
        }

        .status-badge {
            display: inline-block;
            background: #27ae60;
            color: white;
            padding: 8px 20px;
            border-radius: 20px;
            font-size: 14px;
            font-weight: 600;
            margin-bottom: 20px;
        }

        .footer {
            color: #999;
            font-size: 12px;
            margin-top: 30px;
            border-top: 1px solid #eee;
            padding-top: 20px;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="logo">⚡</div>
        <h1>ohos-server</h1>
        <p class="subtitle">高性能 HTTP 服务器</p>

        <div class="status-badge">🟢 服务器运行中</div>

        <div class="info-grid">
            <div class="info-item">
                <div class="info-label">服务器</div>
                <div class="info-value">ohos-server</div>
            </div>
            <div class="info-item">
                <div class="info-label">状态</div>
                <div class="info-value">正常</div>
            </div>
            <div class="info-item">
                <div class="info-label">时间</div>
                <div class="info-value" id="server-time">--:--:--</div>
            </div>
            <div class="info-item">
                <div class="info-label">演示站点</div>
                <div class="info-value">🎉 欢迎</div>
            </div>
        </div>

        <p style="color: #666; font-size: 14px;">
            这是一个由 ohos-server 初始化的演示站点。<br>
            您可以编辑 <code>www/index.html</code> 来替换此页面。
        </p>

        <div class="footer">
            ohos-server &copy; 2025 &mdash; Powered by Rust
        </div>
    </div>

    <script>
        function updateTime() {
            document.getElementById('server-time').textContent = new Date().toLocaleTimeString('zh-CN');
        }
        updateTime();
        setInterval(updateTime, 1000);
    </script>
</body>
</html>
"#;
