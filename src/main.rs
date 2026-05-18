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

//! ohosHttp — 高性能 HTTP 服务器 (非阻塞 IO)
//!
//! 单进程多线程架构，基于 Tokio 异步运行时。
//! 每个 HTTP 连接对应一个轻量级 Tokio 异步任务（协程），在工作线程间无缝调度。

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

use std::process;

use clap::Parser;
use log::{error, info};

use crate::config::AppConfig;

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "ohosHttp", version, about = "高性能 HTTP 服务器 (非阻塞 IO)")]
struct CliArgs {
    /// 配置文件路径
    #[arg(short = 'c', long, default_value = "")]
    config: String,

    /// 绑定地址 (如 0.0.0.0:8080)
    #[arg(short = 'a', long, default_value = "")]
    addr: String,

    /// 网站根目录
    #[arg(short = 'r', long, default_value = "./www")]
    root: String,

    /// 工作线程数 (Tokio 运行时线程)
    #[arg(short = 't', long, default_value_t = 0)]
    threads: usize,

    /// 守护进程模式
    #[arg(short = 'd', long)]
    daemon: bool,

    /// PID 文件路径
    #[arg(long, default_value = "")]
    pidfile: String,

    /// 生成默认配置文件
    #[arg(long)]
    gen_config: bool,

    /// CGI 解释器路径 (如 /usr/bin/php-cgi)
    #[arg(long, default_value = "")]
    interpreter: String,

    /// CGI 文件扩展名 (逗号分隔，默认 .php)
    #[arg(long, default_value = ".php")]
    cgi_ext: String,
}

fn main() {
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

    // 守护进程模式 — 在启动服务器前转入后台
    if args.daemon {
        if let Err(e) = daemonize(&args.pidfile) {
            error!("守护进程化失败: {}", e);
            process::exit(1);
        }
        info!("ohosHttp 已转入后台运行 (PID: {})", std::process::id());
    }

    // 创建 Tokio 多线程运行时
    let worker_threads = determine_threads(&app_config);
    info!("ohosHttp v{} 运行时线程数: {}", env!("CARGO_PKG_VERSION"), worker_threads);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .expect("创建 Tokio 运行时失败");

    // 在异步运行时中启动服务器
    rt.block_on(async {
        run_servers(app_config).await;
    });
}

// ============================================================================
//  配置加载
// ============================================================================

fn load_config(args: &CliArgs) -> config::AppConfig {
    if !args.config.is_empty() {
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
        let mut cfg = config::AppConfig::from_cli(&args.addr, &args.root);
        if args.threads > 0 {
            for s in &mut cfg.server {
                s.threads = args.threads;
            }
        }
        // CGI 解释器
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
        eprintln!("ohosHttp {}", env!("CARGO_PKG_VERSION"));
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

/// 确定 Tokio 运行时线程数
fn determine_threads(app_config: &AppConfig) -> usize {
    // 取第一个 server 配置的 threads 值，最小为 2
    app_config.server.first().map(|s| s.threads).unwrap_or(0).max(2)
}

// ============================================================================
//  服务器运行
// ============================================================================

/// 运行所有服务器（支持信号控制：SIGTERM=停止，SIGHUP=重启）
async fn run_servers(mut app_config: AppConfig) {
    let config_path = app_config.config_path.clone();

    loop {
        let servers = std::mem::take(&mut app_config.server);
        if servers.is_empty() {
            error!("没有可用的服务器配置");
            process::exit(1);
        }

        // 打印启动画面
        banner::print_startup_banner(&servers);

        info!("ohosHttp v{} 启动中...", env!("CARGO_PKG_VERSION"));
        info!("共 {} 个服务器站点配置", servers.len());

        // 创建关闭信号 channel
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        // 启动所有 HTTP 服务器
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

        // 等待信号
        let signal_result = wait_for_signal().await;

        // 发送关闭信号
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
                info!("ohosHttp 正在热重启...");
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
                    error!("CLI 模式不支持热重启，请使用 Ctrl+C 停止后重新启动");
                    break;
                }
                continue;
            }
        }
    }
}

/// 信号动作
#[derive(Debug, Clone, Copy, PartialEq)]
enum SignalAction {
    Stop,
    Restart,
}

/// 等待操作系统信号（SIGTERM=停止，SIGHUP=重启）
async fn wait_for_signal() -> SignalAction {
    use tokio::signal::unix::{signal, SignalKind};

    let mut term = signal(SignalKind::terminate()).expect("无法注册 SIGTERM 处理器");
    let mut quit = signal(SignalKind::quit()).expect("无法注册 SIGQUIT 处理器");
    let mut int = signal(SignalKind::interrupt()).expect("无法注册 SIGINT 处理器");
    let mut hup = signal(SignalKind::hangup()).expect("无法注册 SIGHUP 处理器");

    tokio::select! {
        _ = term.recv() => {
            info!("收到 SIGTERM 信号");
            SignalAction::Stop
        }
        _ = quit.recv() => {
            info!("收到 SIGQUIT 信号");
            SignalAction::Stop
        }
        _ = int.recv() => {
            info!("收到 SIGINT 信号");
            SignalAction::Stop
        }
        _ = hup.recv() => {
            info!("收到 SIGHUP 信号，准备热重启");
            SignalAction::Restart
        }
    }
}

// ============================================================================
//  守护进程化
// ============================================================================

fn daemonize(pidfile: &str) -> Result<(), String> {
    // 第一次 fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("第一次 fork 失败".to_string());
    }
    if pid > 0 {
        // 父进程退出
        process::exit(0);
    }

    // 子进程：创建新会话
    unsafe {
        libc::setsid();
    }

    // 第二次 fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("第二次 fork 失败".to_string());
    }
    if pid > 0 {
        process::exit(0);
    }

    // 孙进程：真正的 daemon
    // 关闭标准 I/O
    unsafe {
        let null_fd = libc::open("/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDWR);
        if null_fd >= 0 {
            libc::dup2(null_fd, libc::STDIN_FILENO);
            libc::dup2(null_fd, libc::STDOUT_FILENO);
            libc::dup2(null_fd, libc::STDERR_FILENO);
        }
    }

    // 写 PID 文件
    if !pidfile.is_empty() {
        let pid_str = format!("{}\n", std::process::id());
        if let Ok(mut file) = std::fs::File::create(pidfile) {
            use std::io::Write;
            let _ = file.write_all(pid_str.as_bytes());
        }
    }

    Ok(())
}
