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

//! ohosHttp — Workerman 架构 HTTP 服务器
//!
//! ## 架构
//!
//! - **Master 进程**: 信号管理、Worker 监控与重启
//! - **Worker 进程**: 预 fork 的独立进程，各自绑定端口（SO_REUSEPORT）
//!   - 每个 Worker 运行一个单线程 Tokio 运行时（事件循环）
//!   - 互不共享状态，进程隔离

mod banner;
mod config;
mod handler;
mod logger;
mod load_balancer;
mod proxy;
mod rate_limiter;
mod rewrite;
mod server;
mod session;

use std::process;

use log::{error, info, warn};
use clap::Parser;

use config::AppConfig;

// ============================================================================
//  信号标志（Master 进程使用，原子操作避免信号处理函数中的复杂逻辑）
// ============================================================================
use std::sync::atomic::{AtomicBool, Ordering};

static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);
static SIGHUP_RECEIVED: AtomicBool = AtomicBool::new(false);

/// 信号处理函数（仅在 Master 进程注册）
extern "C" fn sigterm_handler(_: i32) {
    SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
}

extern "C" fn sighup_handler(_: i32) {
    SIGHUP_RECEIVED.store(true, Ordering::SeqCst);
}

// ============================================================================
//  命令行参数
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "ohosHttp", version = env!("CARGO_PKG_VERSION"), about = "高性能 HTTP 服务器 (Workerman 架构)")]
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

    /// Worker 进程数 (0 = CPU 核心数)
    #[arg(short = 'w', long, default_value_t = 0)]
    workers: usize,

    /// 向后兼容: Worker 进程数 (别名)
    #[arg(short = 't', long, default_value_t = 0, hide(true))]
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

// ============================================================================
//  主入口
// ============================================================================

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

    // 确定 Worker 数量
    let worker_count = determine_workers(&app_config);
    let mut cfg = app_config;

    if args.daemon {
        if let Err(e) = daemonize(&args.pidfile) {
            error!("守护进程化失败: {}", e);
            process::exit(1);
        }
        info!("ohosHttp 已转入后台运行 (PID: {})", std::process::id());
    }

    // ─── Master 进程 ───
    run_master(&mut cfg, worker_count);
}

// ============================================================================
//  Master 进程：Worker 管理 + 信号处理
// ============================================================================

fn run_master(app_config: &mut AppConfig, worker_count: usize) {
    // 打印启动画面（仅 Master 打印一次）
    banner::print_startup_banner(&app_config.server, worker_count);
    info!("ohosHttp v{} 启动中... ({} Worker 进程)", env!("CARGO_PKG_VERSION"), worker_count);

    // 注册信号处理函数
    unsafe {
        libc::signal(libc::SIGTERM, sigterm_handler as usize);
        libc::signal(libc::SIGQUIT, sigterm_handler as usize);
        libc::signal(libc::SIGINT, sigterm_handler as usize);
        libc::signal(libc::SIGHUP, sighup_handler as usize);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        // SIGCHLD 用 waitpid 处理
    }

    // 启动 Worker 进程
    let mut children: Vec<libc::pid_t> = Vec::with_capacity(worker_count);
    for id in 0..worker_count {
        match spawn_worker(app_config, id) {
            Ok(pid) => children.push(pid),
            Err(e) => {
                error!("Worker #{} fork 失败: {}", id, e);
                // 如果第一个 worker 就失败了，直接退出
                if children.is_empty() {
                    process::exit(1);
                }
            }
        }
    }

    info!("所有 {} 个 Worker 进程已启动 (PID: {:?})", worker_count, children);

    // ─── Master 事件循环 ───
    loop {
        // 等待信号
        unsafe { libc::pause(); }

        // 检查是否需要重新加载
        if SIGHUP_RECEIVED.swap(false, Ordering::SeqCst) {
            info!("收到 SIGHUP 信号，开始热重启...");

            // 重新加载配置（如果使用配置文件）
            if !app_config.config_path.is_empty() {
                match config::AppConfig::from_file(&app_config.config_path) {
                    Ok(cfg) => {
                        *app_config = cfg;
                        info!("配置文件已重新加载: {}", app_config.config_path);
                    }
                    Err(e) => {
                        error!("重新加载配置失败: {}，跳过重启", e);
                        continue;
                    }
                }
            }

            // 逐个重启 Worker（零停机）
            let mut new_children = Vec::with_capacity(children.len());
            for (i, old_pid) in children.iter().enumerate() {
                info!("热重启 Worker #{} (PID: {})", i, old_pid);
                // 发送 SIGTERM 让旧 Worker 优雅退出
                unsafe { libc::kill(*old_pid, libc::SIGTERM); }
                // 等待旧 Worker 退出（最多 35 秒）
                let mut status: i32 = 0;
                let waited = unsafe { libc::waitpid(*old_pid, &mut status, 0) };
                if waited == -1 {
                    warn!("Worker #{} waitpid 失败, errno: {}", i, unsafe { *libc::__errno_location() });
                }

                // fork 新 Worker
                match spawn_worker(app_config, i) {
                    Ok(pid) => new_children.push(pid),
                    Err(e) => {
                        error!("热重启 Worker #{} fork 失败: {}", i, e);
                    }
                }
            }
            children = new_children;
            info!("热重启完成，当前 {} 个 Worker 进程", children.len());
            continue;
        }

        // 检查是否收到停止信号
        if SIGTERM_RECEIVED.swap(false, Ordering::SeqCst) {
            info!("收到停止信号，正在关闭所有 Worker...");
            // 通知所有 Worker 优雅退出
            for (i, pid) in children.iter().enumerate() {
                info!("正在停止 Worker #{} (PID: {})", i, pid);
                unsafe { libc::kill(*pid, libc::SIGTERM); }
            }
            // 等待所有 Worker 退出（最多 35 秒）
            for pid in &children {
                let mut status: i32 = 0;
                let _ = unsafe { libc::waitpid(*pid, &mut status, 0) };
            }
            info!("所有 Worker 已停止，ohosHttp 已正常退出");
            break;
        }

        // 处理 Worker 意外退出（SIGCHLD）
        loop {
            let mut status: i32 = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                break; // 没有更多子进程退出
            }
            if let Some(pos) = children.iter().position(|&p| p == pid) {
                let exit_code = libc::WEXITSTATUS(status);
                warn!("Worker #{} (PID: {}) 意外退出，退出码: {}", pos, pid, exit_code);
                // 自动重启 Worker
                match spawn_worker(app_config, pos) {
                    Ok(new_pid) => {
                        children[pos] = new_pid;
                        info!("Worker #{} 已自动重启 (新 PID: {})", pos, new_pid);
                    }
                    Err(e) => {
                        error!("Worker #{} 自动重启失败: {}", pos, e);
                    }
                }
            }
        }
    }
}

/// fork 一个 Worker 进程
fn spawn_worker(app_config: &AppConfig, worker_id: usize) -> Result<libc::pid_t, String> {
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(format!("fork 失败 (errno: {})", unsafe { *libc::__errno_location() })),
        0 => {
            // ─── Worker 进程 ───
            run_worker(app_config, worker_id);
            // Worker 返回后退出
            process::exit(0);
        }
        n => Ok(n),
    }
}

// ============================================================================
//  Worker 进程：单线程 Tokio 运行时，独立端口绑定
// ============================================================================

fn run_worker(app_config: &AppConfig, worker_id: usize) {
    // 重置信号处理（Worker 使用 Tokio 信号）
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::signal(libc::SIGHUP, libc::SIG_DFL);
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGQUIT, libc::SIG_DFL);
        // SIGPIPE 继续忽略
    }

    // 创建单线程 Tokio 运行时（每个 Worker 一个事件循环）
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("创建 Worker Tokio 运行时失败");

    rt.block_on(async {
        run_worker_async(app_config, worker_id).await;
    });
}

/// Worker 异步主循环
async fn run_worker_async(app_config: &AppConfig, _worker_id: usize) {
    use server::HttpServer;

    let servers = &app_config.server;
    if servers.is_empty() {
        error!("Worker: 没有可用的服务器配置");
        return;
    }

    // 创建关闭信号通道
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // 启动所有站点
    let mut handles = Vec::new();
    for srv_config in servers {
        let shutdown_rx = shutdown_tx.subscribe();
        let server = HttpServer::new(srv_config.clone());
        handles.push(tokio::spawn(async move {
            if let Err(e) = server.start(shutdown_rx).await {
                error!("服务器启动失败: {}", e);
            }
        }));
    }

    // Worker 级别的信号处理
    use tokio::signal::unix::{signal, SignalKind};

    let mut term_signal = signal(SignalKind::terminate())
        .expect("Worker: 无法注册 SIGTERM 处理器");
    let mut hup_signal = signal(SignalKind::hangup())
        .expect("Worker: 无法注册 SIGHUP 处理器");

    tokio::select! {
        _ = term_signal.recv() => {
            // SIGTERM → 优雅关闭
        }
        _ = hup_signal.recv() => {
            // SIGHUP → 优雅关闭（Master 会重启我们）
        }
    }

    // 发送关闭信号
    let _ = shutdown_tx.send(true);

    // 等待所有服务器完成
    for handle in handles {
        let _ = handle.await;
    }
}

// ============================================================================
//  配置加载
// ============================================================================

fn load_config(args: &CliArgs) -> AppConfig {
    // 合并 workers 和 threads（线程作为向后兼容）
    let workers = if args.workers > 0 {
        args.workers
    } else if args.threads > 0 {
        warn!("--threads/-t 已废弃，请使用 --workers/-w");
        args.threads
    } else {
        0
    };

    if !args.config.is_empty() {
        let mut cfg = match config::AppConfig::from_file(&args.config) {
            Ok(cfg) => cfg,
            Err(e) => {
                error!("{}", e);
                process::exit(1);
            }
        };
        if workers > 0 {
            for s in &mut cfg.server {
                s.workers = workers;
            }
        }
        info!("成功加载配置文件: {}", args.config);
        cfg
    } else if !args.addr.is_empty() {
        let mut cfg = config::AppConfig::from_cli(&args.addr, &args.root);
        if workers > 0 {
            for s in &mut cfg.server {
                s.workers = workers;
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
        eprintln!("  ohosHttp -a 0.0.0.0:8080 -w 4                 # 4 个 Worker 进程");
        eprintln!("  ohosHttp -a 0.0.0.0:8080 --interpreter /usr/bin/php-cgi  # 指定PHP解释器");
        eprintln!("");
        eprintln!("示例:");
        eprintln!("  ohosHttp --addr=0.0.0.0:8080 --root=/var/www");
        process::exit(1);
    }
}

/// 确定 Worker 进程数
fn determine_workers(app_config: &AppConfig) -> usize {
    app_config.server.first()
        .map(|s| s.workers)
        .unwrap_or(0)
        .max(1) // 至少 1 个 Worker
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

    // 创建新会话
    if unsafe { libc::setsid() } < 0 {
        return Err("setsid 失败".to_string());
    }

    // 第二次 fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("第二次 fork 失败".to_string());
    }
    if pid > 0 {
        process::exit(0);
    }

    // 切换到根目录
    unsafe {
        let root_dir = std::ffi::CString::new("/").unwrap();
        libc::chdir(root_dir.as_ptr());
    }

    // 关闭标准文件描述符
    unsafe {
        libc::close(0);
        libc::close(1);
        libc::close(2);
    }

    // 写 PID 文件
    if !pidfile.is_empty() {
        if let Err(e) = std::fs::write(pidfile, std::process::id().to_string()) {
            warn!("写入 PID 文件失败: {}", e);
        }
    }

    Ok(())
}
