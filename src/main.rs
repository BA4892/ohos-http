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

//! ohosHttp — 高性能 HTTP 服务器
//!
//! # 架构：进程 × 线程 × 协程
//!
//! - **进程层** (Process)：Master-Worker 多进程架构。Master 负责信号管理与配置热重载，Worker 进程独占 CPU 核心处理请求。
//! - **线程层** (Thread)：每个 Worker 运行 Tokio 多线程运行时 (`multi_thread`)，线程数 = CPU 核数，充分利用 CPU 并行能力。
//! - **协程层** (Coroutine)：每个 HTTP 连接对应一个轻量级 Tokio 异步任务（协程），在 Worker 线程间无缝调度，实现高并发低开销。
//!
//! # 会话一致性
//!
//! 跨进程 Session 通过 POSIX 共享内存 (`shm_open` + `mmap`) + 自旋锁哈希表实现，
//! 所有 Worker 进程共享同一 Session 数据空间，保证请求一致性。

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
mod session_shm;

use std::process;
use std::sync::Arc;

use clap::Parser;
use log::{error, info, warn};

use crate::config::AppConfig;
use crate::session_shm::{cleanup_shm, ShmSessionStore};

/// 默认共享内存 Session 名称
const DEFAULT_SHM_NAME: &str = "/ohos_http_sessions";

/// 命令行参数
#[derive(Parser, Debug)]
#[command(name = "ohosHttp", version, about = "高性能 HTTP 服务器 (进程 × 线程 × 协程)")]
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

    /// 工作线程数 (每个 Worker 的异步运行时线程)
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

    // 守护进程模式 — 先 daemonize 再 fork worker
    if args.daemon {
        if let Err(e) = daemonize(&args.pidfile) {
            error!("守护进程化失败: {}", e);
            process::exit(1);
        }
        info!("ohosHttp 已转入后台运行 (PID: {})", std::process::id());
    }

    // 确定工作进程数
    let workers = determine_workers(&app_config);
    info!("ohosHttp v{} 工作进程数: {}", env!("CARGO_PKG_VERSION"), workers);

    if workers > 1 {
        run_master_workers(app_config, workers);
    } else {
        // 单进程模式：使用多线程 Tokio 运行时
        let shm_name = get_shm_name(&app_config);
        run_single_process(app_config, &shm_name);
    }
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

/// 确定工作进程数
fn determine_workers(app_config: &AppConfig) -> usize {
    // 取第一个 server 配置的 workers 值（所有 server 使用相同 workers 数）
    app_config.server.first().map(|s| s.workers).unwrap_or(0).max(1)
}

/// 获取共享内存 session 名称
fn get_shm_name(_app_config: &AppConfig) -> String {
    // 从配置读取或使用默认
    DEFAULT_SHM_NAME.to_string()
}

// ============================================================================
//  单进程模式
// ============================================================================

/// 单进程模式：Tokio 多线程运行时 + 服务器
fn run_single_process(app_config: AppConfig, _shm_name: &str) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cpus::get())
        .enable_all()
        .build()
        .expect("创建 Tokio 运行时失败");

    rt.block_on(async {
        // 单进程模式不需要共享内存，传入 None
        run_servers(app_config, None).await;
    });
}

// ============================================================================
//  Master-Worker 多进程模式
// ============================================================================

/// Master-Worker 多进程模式
fn run_master_workers(app_config: AppConfig, workers: usize) {
    let shm_name = get_shm_name(&app_config);

    // 清理旧的共享内存 (如果存在)
    cleanup_shm(&shm_name);

    // 记录所有 worker PID
    let mut worker_pids: Vec<libc::pid_t> = Vec::with_capacity(workers);

    for worker_id in 0..workers {
        match unsafe { libc::fork() } {
            0 => {
                // ─── Worker 进程 ───
                // CPU 亲和性绑定：将当前进程绑定到指定 CPU 核心
                set_cpu_affinity(worker_id, num_cpus::get());

                // 创建共享内存 Session 存储（每个 worker 独立 shm_open + mmap，指向相同物理内存）
                let shm_store = create_shm_session_store(&app_config, &shm_name);

                // 构建多线程 Tokio 运行时
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(num_cpus::get())
                    .enable_all()
                    .build()
                    .expect("Worker: 创建 Tokio 运行时失败");

                info!(
                    "Worker #{} 启动 (PID: {}, CPU 亲和: core {})",
                    worker_id,
                    std::process::id(),
                    worker_id % num_cpus::get()
                );

                rt.block_on(async {
                    run_servers(app_config, shm_store).await;
                });

                // Worker 正常退出
                process::exit(0);
            }
            -1 => {
                error!("fork worker #{} 失败: {}", worker_id, std::io::Error::last_os_error());
                // 清理已 fork 的 worker
                for &pid in &worker_pids {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
                process::exit(1);
            }
            pid => {
                // ─── Master 进程 ───
                worker_pids.push(pid);
                info!("Worker #{} (PID: {}) 已注册", worker_id, pid);
            }
        }
    }

    info!("所有 {} 个工作进程已启动，Master PID: {}", workers, std::process::id());

    // Master 循环：监听信号，管理 worker 生命周期
    master_loop(worker_pids, &shm_name);
}

/// Master 进程主循环：等待 SIGTERM/SIGHUP 信号并管理 worker
fn master_loop(worker_pids: Vec<libc::pid_t>, shm_name: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // 信号处理
    signal_hook::flag::register(signal_hook::consts::SIGTERM, running.clone())
        .expect("注册 SIGTERM 信号处理器失败");
    signal_hook::flag::register(signal_hook::consts::SIGINT, running)
        .expect("注册 SIGINT 信号处理器失败");

    info!("Master 进程进入信号等待状态 (PID: {})", std::process::id());

    // 等待停止信号
    while r.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    info!("Master 收到停止信号，向所有 Worker 发送 SIGTERM...");

    // 向所有 Worker 发送 SIGTERM
    for &pid in &worker_pids {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }

    // 等待 Worker 退出
    for &pid in &worker_pids {
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(pid, &mut status, 0);
        }
        info!("Worker (PID: {}) 已退出", pid);
    }

    // 清理共享内存
    cleanup_shm(shm_name);

    info!("ohosHttp 已正常停止");
}

/// 设置 CPU 亲和性
fn set_cpu_affinity(worker_id: usize, total_cpus: usize) {
    let cpu_id = worker_id % total_cpus;
    unsafe {
        let mut mask: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu_id, &mut mask);
        let ret = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mask);
        if ret != 0 {
            warn!("设置 CPU 亲和性失败 (core {}): {}", cpu_id, std::io::Error::last_os_error());
        }
    }
}

/// 创建共享内存 Session 存储
fn create_shm_session_store(app_config: &AppConfig, shm_name: &str) -> Option<Arc<ShmSessionStore>> {
    // 检查是否有任何 server 启用了 session
    let has_session = app_config.server.iter().any(|s| {
        s.session.as_ref().map_or(false, |sc| sc.enabled)
    });

    if !has_session {
        return None;
    }

    match ShmSessionStore::new(shm_name, 65536) {
        Ok(store) => {
            info!("跨进程共享内存 Session 存储已就绪: {} (64K buckets)", shm_name);
            Some(Arc::new(store))
        }
        Err(e) => {
            error!("创建共享内存 Session 存储失败: {}，将使用本地内存 (跨进程不一致)", e);
            None
        }
    }
}

// ============================================================================
//  服务器运行
// ============================================================================

/// 运行所有服务器（支持信号控制：SIGTERM=停止，SIGHUP=重启）
///
/// `shm_store` 为多进程模式下的共享内存 Session 存储，单进程模式为 None。
async fn run_servers(mut app_config: AppConfig, shm_store: Option<Arc<ShmSessionStore>>) {
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
            let shm = shm_store.clone();
            let server = server::HttpServer::new(srv_config, shm);
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
