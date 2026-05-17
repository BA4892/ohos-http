// Copyright 2025 ohosHttp Contributors
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

mod banner;
mod config;
mod handler;
mod load_balancer;
mod logger;
mod proxy;
mod rate_limiter;
mod rewrite;
mod server;
mod session;

use clap::Parser;
use log::{info, error};
use std::process;
use tokio::sync::watch;

/// ohosHttp - 高性能HTTP服务器，支持鸿蒙和Linux x86_64平台
#[derive(Parser, Debug)]
#[command(name = "ohosHttp")]
#[command(about = "ohosHttp - 高性能HTTP服务器", long_about = None)]
#[command(version = "1.1.0")]
struct CliArgs {
    /// 绑定地址，如 "127.0.0.1:8089"
    #[arg(short = 'a', long = "addr", default_value = "")]
    addr: String,

    /// 网站根目录
    #[arg(short = 'r', long = "root", default_value = "./www")]
    root: String,

    /// 配置文件路径
    #[arg(short = 'c', long = "config", default_value = "")]
    config: String,

    /// 工作线程数
    #[arg(short = 't', long = "threads", default_value_t = 0)]
    threads: usize,

    /// 生成默认配置文件
    #[arg(long = "gen-config", default_value_t = false)]
    gen_config: bool,

    /// 守护进程模式（后台运行）
    #[arg(short = 'd', long = "daemon", default_value_t = false)]
    daemon: bool,

    /// PID文件路径（守护进程模式）
    #[arg(long = "pidfile", default_value = "")]
    pidfile: String,

    /// CGI解释器路径，如 "/usr/bin/php-cgi"
    #[arg(long = "interpreter", default_value = "")]
    interpreter: String,

    /// CGI文件扩展名，逗号分隔，如 ".php,.phtml"
    #[arg(long = "cgi-ext", default_value = "")]
    cgi_ext: String,

    /// 监听所有服务器（多站点模式，配合 -c 使用）
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    /// HTTPS 证书文件路径
    #[arg(long = "cert", default_value = "")]
    cert: String,

    /// HTTPS 私钥文件路径
    #[arg(long = "key", default_value = "")]
    key: String,

    /// HTTP/3 (QUIC) 端口，如 "4433"
    #[arg(long = "http3-port", default_value = "")]
    http3_port: String,
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    let args = CliArgs::parse();

    // 生成默认配置文件
    if args.gen_config {
        println!("{}", config::DEFAULT_CONFIG);
        return;
    }

    // 加载配置
    let app_config = load_config(&args);

    // 守护进程模式 - 后台运行
    if args.daemon {
        if let Err(e) = daemonize(&args.pidfile) {
            error!("守护进程化失败: {}", e);
            process::exit(1);
        }
        info!("ohosHttp 已转入后台运行 (PID: {})", std::process::id());
    }

    // 启动服务器（支持信号控制）
    run_servers(app_config).await;
}

/// 加载配置
fn load_config(args: &CliArgs) -> config::AppConfig {
    if !args.config.is_empty() {
        // 从配置文件加载
        match config::AppConfig::from_file(&args.config) {
            Ok(cfg) => {
                info!("成功加载配置文件: {}", args.config);
                cfg
            }
            Err(e) => {
                error!("{}", e);
                process::exit(1);
            }
        }
    } else if !args.addr.is_empty() {
        // 从命令行参数创建
        let mut cfg = config::AppConfig::from_cli(&args.addr, &args.root);
        // 命令行覆盖配置
        if args.threads > 0 {
            for s in &mut cfg.server {
                s.threads = args.threads;
            }
        }
        // CGI解释器
        if !args.interpreter.is_empty() {
            let exts: Vec<String> = if !args.cgi_ext.is_empty() {
                args.cgi_ext.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                vec![".php".to_string()]
            };
            let cgi = config::CgiConfig {
                extensions: exts,
                interpreter: args.interpreter.clone(),
            };
            for s in &mut cfg.server {
                s.cgi.push(cgi.clone());
            }
        }
        cfg
    } else {
        // 默认启动方式
        eprintln!("ohosHttp 1.1.0");
        eprintln!("用法:");
        eprintln!("  ohosHttp -a 127.0.0.1:8089 -r ./www          # 快速启动");
        eprintln!("  ohosHttp -c config.toml                        # 从配置文件启动");
        eprintln!("  ohosHttp --gen-config                          # 生成默认配置文件");
        eprintln!("  ohosHttp -a 0.0.0.0:8080 -d                   # 守护进程模式");
        eprintln!("  ohosHttp -a 0.0.0.0:8080 --interpreter /usr/bin/php-cgi  # 指定PHP解释器");
        eprintln!("");
        eprintln!("示例:");
        eprintln!("  ohosHttp --addr=0.0.0.0:8080 --root=/var/www");
        process::exit(1);
    }
}

/// 运行服务器，支持信号控制（SIGTERM=停止，SIGHUP=重启）
async fn run_servers(mut app_config: config::AppConfig) {
    let config_path = app_config.config_path.clone();

    loop {
        let servers = std::mem::take(&mut app_config.server);
        if servers.is_empty() {
            error!("没有可用的服务器配置");
            process::exit(1);
        }

        // 打印美观的启动画面
        banner::print_startup_banner(&servers);

        info!("ohosHttp v{} 启动中...", env!("CARGO_PKG_VERSION"));
        info!("共 {} 个服务器站点配置", servers.len());

        // 创建关闭信号 channel
        let (shutdown_tx, _) = watch::channel(false);

        // 启动所有服务器
        let mut handles = Vec::new();
        for srv_config in servers {
            let shutdown_rx = shutdown_tx.subscribe();
            let server = server::HttpServer::new(srv_config);
            handles.push(tokio::spawn(async move {
                if let Err(e) = server.start(shutdown_rx).await {
                    error!("服务器启动失败: {}", e);
                }
            }));
        }

        // 等待信号：SIGTERM → 停止，SIGHUP → 重启
        let signal_result = wait_for_signal().await;

        // 发送关闭信号给所有服务器
        let _ = shutdown_tx.send(true);

        // 等待所有服务器优雅关闭
        for handle in handles {
            let _ = handle.await;
        }

        match signal_result {
            SignalAction::Stop => {
                info!("ohosHttp 已正常停止");
                break;
            }
            SignalAction::Restart => {
                info!("ohosHttp 正在重启...");
                // 重新加载配置
                if !config_path.is_empty() {
                    match config::AppConfig::from_file(&config_path) {
                        Ok(cfg) => {
                            app_config = cfg;
                            info!("配置文件已重新加载: {}", config_path);
                        }
                        Err(e) => {
                            error!("重新加载配置失败: {}，退出", e);
                            process::exit(1);
                        }
                    }
                } else {
                    // CLI模式不支持热重启 (SIGHUP)
                    error!("CLI模式不支持热重启 (SIGHUP)，请使用 Ctrl+C 停止后重新启动");
                    break;
                }
                // 继续循环，重新启动服务器
                continue;
            }
        }
    }
}

/// 信号动作
enum SignalAction {
    Stop,
    Restart,
}

/// 等待系统信号：SIGTERM → 停止，SIGHUP → 重启
async fn wait_for_signal() -> SignalAction {
    use tokio::signal::unix::{signal, SignalKind};

    // 同时监听 SIGTERM 和 SIGHUP
    let mut sigterm = signal(SignalKind::terminate())
        .expect("无法注册 SIGTERM 信号处理器");

    let mut sighup = signal(SignalKind::hangup())
        .expect("无法注册 SIGHUP 信号处理器");

    tokio::select! {
        _ = sigterm.recv() => {
            info!("收到 SIGTERM 信号，正在停止...");
            SignalAction::Stop
        }
        _ = sighup.recv() => {
            info!("收到 SIGHUP 信号，正在重启...");
            SignalAction::Restart
        }
    }
}

/// 守护进程化 - 将进程转入后台运行
fn daemonize(pidfile: &str) -> Result<(), String> {
    use std::io::Write;

    // 第一次 fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("fork 失败".to_string());
    }
    if pid > 0 {
        // 父进程退出
        std::process::exit(0);
    }

    // 子进程：创建新会话
    unsafe {
        libc::setsid();
    }

    // 第二次 fork，彻底脱离终端
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("第二次 fork 失败".to_string());
    }
    if pid > 0 {
        std::process::exit(0);
    }

    // 切换到根目录
    let _ = std::env::set_current_dir("/");

    // 关闭标准输入输出
    if let Ok(null) = std::fs::File::open("/dev/null") {
        let null_fd = std::os::fd::AsRawFd::as_raw_fd(&null);
        unsafe {
            libc::dup2(null_fd, libc::STDIN_FILENO);
            libc::dup2(null_fd, libc::STDOUT_FILENO);
            libc::dup2(null_fd, libc::STDERR_FILENO);
        }
    }

    // 写PID文件
    if !pidfile.is_empty() {
        let pid_str = format!("{}\n", std::process::id());
        if let Ok(mut file) = std::fs::File::create(pidfile) {
            let _ = file.write_all(pid_str.as_bytes());
        }
    }

    Ok(())
}
