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

//! HTTP 服务器核心 — 单线程 Tokio 运行时 + SO_REUSEPORT
//!
//! ## 进程层优化 (Process)
//!
//! 使用 `SO_REUSEPORT` 套接字选项使多个 Worker 进程绑定到同一端口，
//! 由操作系统内核在进程间分发连接。每个 Worker 独立处理其接受的连接。
//!
//! ## 协程层优化 (Coroutine)
//!
//! 每个 HTTP 连接由一个 tokio::spawn 异步任务处理（协程层）。
//! 单线程事件循环 + 异步任务 = 零成本上下文切换。
//!
//! ## Workerman 架构
//!
//! - Master 进程: 信号管理, Worker 监控, 零停机热重启
//! - Worker 进程: 预 fork, 每个 Worker = 1 个 OS 线程 + 1 个事件循环
//! - SO_REUSEPORT: 内核层负载均衡
//! - 无共享状态: 进程隔离, 无需锁
//!
//! ## 虚拟主机路由 (VirtualHost)
//!
//! 当多个 `[[server]]` 配置同一端口时，共享一个 TcpListener，
//! 在应用层根据 HTTP Host 头路由到正确的站点配置。

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request};
use hyper_util::rt::TokioIo;
use log::{error, info, warn};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::config::ServerConfig;
use crate::handler::RequestHandler;
use crate::manage::ManageHandler;

/// HTTP 服务器实例（单站点）
pub struct HttpServer {
    config: ServerConfig,
    manage_handler: Option<ManageHandler>,
}

/// 虚拟主机路由器 — 同一端口多个站点共享一个 Listener
///
/// 根据 HTTP Host 头匹配对应的 ServerConfig：
/// 1. 精确匹配 domains 列表
/// 2. 无匹配时回退到第一个配置（默认站点）
pub struct VirtualHostRouter {
    /// 默认站点索引（列表中的第一个）
    #[allow(dead_code)]
    default_idx: usize,
    /// 域名 → 配置索引 的映射
    domain_map: HashMap<String, usize>,
    /// 所有共享此端口的站点配置
    configs: Vec<ServerConfig>,
    /// 可选的全局管理处理器（仅第一个非空）
    manage_handler: Option<ManageHandler>,
}

impl VirtualHostRouter {
    /// 从一组共享同一端口的 ServerConfig 创建路由器
    pub fn new(configs: Vec<ServerConfig>, manage_handler: Option<ManageHandler>) -> Self {
        let mut domain_map = HashMap::new();
        for (idx, cfg) in configs.iter().enumerate() {
            for domain in &cfg.domains {
                domain_map.insert(domain.to_lowercase(), idx);
            }
        }
        VirtualHostRouter {
            default_idx: 0,
            domain_map,
            configs,
            manage_handler,
        }
    }

    /// 根据 Host 头查找对应的 ServerConfig
    #[allow(dead_code)]
    pub fn resolve(&self, host_header: Option<&str>) -> &ServerConfig {
        if let Some(host) = host_header {
            // 去除端口号
            let hostname = host.split(':').next().unwrap_or(host).to_lowercase();
            if let Some(idx) = self.domain_map.get(&hostname) {
                return &self.configs[*idx];
            }
        }
        &self.configs[self.default_idx]
    }

    /// 返回所有配置引用
    pub fn all_configs(&self) -> &[ServerConfig] {
        &self.configs
    }

    /// 获取管理处理器
    pub fn manage_handler(&self) -> Option<&ManageHandler> {
        self.manage_handler.as_ref()
    }
}

impl HttpServer {
    pub fn new(config: ServerConfig) -> Self {
        HttpServer { config, manage_handler: None }
    }

    pub fn new_with_manage(config: ServerConfig, manage_handler: ManageHandler) -> Self {
        HttpServer { config, manage_handler: Some(manage_handler) }
    }

    /// 启动服务器（含 TLS、HTTP/2、HTTP/3 支持）
    ///
    /// 接收一个 shutdown 信号通道，用于优雅关闭。
    pub async fn start(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr: SocketAddr = self.config.bind.parse()
            .map_err(|e| format!("绑定地址格式错误 '{}': {}", self.config.bind, e))?;

        // 创建 Handler（带管理 API）
        let handler = if let Some(ref manage) = self.manage_handler {
            Arc::new(RequestHandler::new_with_manage(
                self.config.clone(),
                manage.clone(),
            ))
        } else {
            Arc::new(RequestHandler::new(
                self.config.clone(),
            ))
        };

        // ─── TLS 配置（如果提供了 cert/key） ───
        let tls_config = if let (Some(cert_path), Some(key_path)) = (&self.config.cert, &self.config.key) {
            info!("配置 TLS: cert={}, key={}", cert_path, key_path);
            Some(load_tls_config(cert_path, key_path)?)
        } else {
            None
        };

        // ─── TCP Listener ───
        let listener = create_tcp_listener(addr).await?;

        info!("ohosHttp 服务器启动: {} (根目录: {}, Worker 单线程事件循环)",
            self.config.bind, self.config.root);

        if tls_config.is_some() {
            info!("TLS/HTTPS 已启用 — ALPN: h2, http/1.1");
        }

        // ─── HTTP/3 (QUIC) 任务 ───
        let h3_port: u16 = self.config.http3_port.parse().unwrap_or(0);
        if h3_port > 0 && tls_config.is_some() {
            let h3_handler = handler.clone();
            let h3_addr: SocketAddr = format!("0.0.0.0:{}", h3_port).parse()?;
            let tls_for_h3 = load_tls_config_for_h3(
                self.config.cert.as_ref().unwrap(),
                self.config.key.as_ref().unwrap(),
            )?;
            let mut shutdown_rx_h3 = shutdown_rx.clone();

            tokio::spawn(async move {
                info!("HTTP/3 (QUIC) 正在监听 UDP {}:{}", h3_addr.ip(), h3_addr.port());
                if let Err(e) = run_h3_server(h3_handler, h3_addr, tls_for_h3, &mut shutdown_rx_h3).await {
                    error!("HTTP/3 服务器错误: {}", e);
                }
            });
        }

        // ─── 主 accept 循环 (TCP + TLS) — 协程层：每个连接一个轻量级异步任务 ───
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("ohosHttp 服务器 {} 收到关闭信号，正在优雅关闭...", self.config.bind);
                    break;
                }
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            let handler = handler.clone();
                            let remote = peer_addr.to_string();
                            let tls_config = tls_config.clone();

                            let remote_for_log = remote.clone();
                            // 每个连接一个独立异步任务（协程），在 Tokio 线程池中调度
                            tokio::spawn(async move {

                                // 处理 TLS 连接
                                if let Some(tls_cfg) = tls_config {
                                    let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_cfg);
                                    match tls_acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            // 检查 ALPN 协商结果
                                            let (_, session) = tls_stream.get_ref();
                                            let alpn = session.alpn_protocol()
                                                .and_then(|p| std::str::from_utf8(p).ok())
                                                .unwrap_or("http/1.1")
                                                .to_string();

                                            let io = TokioIo::new(tls_stream);

                                            let service = service_fn(move |req: Request<Incoming>| {
                                                let handler = handler.clone();
                                                let remote = remote.clone();
                                                async move { handler.handle(req, remote).await }
                                            });

                                            if alpn == "h2" {
                                                info!("HTTP/2 连接: {} (ALPN: h2)", remote_for_log);
                                                let conn = hyper::server::conn::http2::Builder::new(
                                                    hyper_util::rt::TokioExecutor::new()
                                                )
                                                .timer(hyper_util::rt::TokioTimer::new())
                                                .keep_alive_interval(Some(std::time::Duration::from_secs(30)))
                                                .serve_connection(io, service);

                                                if let Err(err) = conn.await {
                                                    error!("HTTP/2 连接错误 ({}): {}", remote_for_log, err);
                                                }
                                            } else {
                                                let conn = hyper::server::conn::http1::Builder::new()
                                                    .keep_alive(true)
                                                    .serve_connection(io, service)
                                                    .with_upgrades();

                                                if let Err(err) = conn.await {
                                                    error!("HTTP/1.1 连接错误 ({}): {}", remote_for_log, err);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("TLS 握手失败 ({}): {}", remote_for_log, e);
                                        }
                                    }
                                } else {
                                    // 无 TLS — 纯 HTTP/1.1
                                    let service = service_fn(move |req: Request<Incoming>| {
                                        let handler = handler.clone();
                                        let remote = remote.clone();
                                        async move { handler.handle(req, remote).await }
                                    });

                                    let io = TokioIo::new(stream);
                                    let conn = hyper::server::conn::http1::Builder::new()
                                        .keep_alive(true)
                                        .serve_connection(io, service)
                                        .with_upgrades();

                                    if let Err(err) = conn.await {
                                        error!("连接处理错误: {}", err);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("接受连接失败: {}", e);
                        }
                    }
                }
            }
        }

        info!("ohosHttp 服务器 {} 已停止", self.config.bind);
        Ok(())
    }
}

/// 虚拟主机服务器 — 同一端口多个站点共享一个 TcpListener
///
/// 根据 HTTP Host 头在应用层做域名路由，解决多站同端口时
/// SO_REUSEPORT 内核分发导致请求被错误站点处理的问题。
pub struct VirtualHostServer {
    router: VirtualHostRouter,
}

impl VirtualHostServer {
    pub fn new(router: VirtualHostRouter) -> Self {
        VirtualHostServer { router }
    }

    /// 多站点共享端口启动（含 TLS、HTTP/2、HTTP/3 支持）
    pub async fn start(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let configs = self.router.all_configs();
        if configs.is_empty() {
            return Err("没有可用的站点配置".into());
        }

        // 使用第一个配置的 bind 地址创建共享 Listener
        let bind = if let Some(cfg) = configs.first() {
            cfg.bind.as_str()
        } else {
            return Err("没有可用的站点配置".into());
        };
        let addr: SocketAddr = bind.parse()
            .map_err(|e| format!("绑定地址格式错误 '{}': {}", bind, e))?;

        // 为每个站点创建独立的 RequestHandler
        let handlers: Vec<Arc<RequestHandler>> = configs.iter().map(|cfg| {
            if let Some(manage) = self.router.manage_handler() {
                Arc::new(RequestHandler::new_with_manage(cfg.clone(), manage.clone()))
            } else {
                Arc::new(RequestHandler::new(cfg.clone()))
            }
        }).collect();

        // ─── TLS 配置（取第一个非空的 cert/key） ───
        let tls_config = configs.iter()
            .find_map(|cfg| {
                if let (Some(cert_path), Some(key_path)) = (&cfg.cert, &cfg.key) {
                    match load_tls_config(cert_path, key_path) {
                        Ok(cfg) => Some(cfg),
                        Err(e) => {
                            warn!("TLS 配置加载失败 (跳过): {}", e);
                            None
                        }
                    }
                } else {
                    None
                }
            });

        // ─── TCP Listener（共享，只创建一个） ───
        let listener = create_tcp_listener(addr).await?;

        let site_count = configs.len();
        info!("ohosHttp 多站点服务器启动: {} ({} 个站点, Worker 单线程事件循环)",
            bind, site_count);

        if tls_config.is_some() {
            info!("TLS/HTTPS 已启用 — ALPN: h2, http/1.1");
        }

        // ─── HTTP/3 (QUIC) 任务（取第一个配置的 h3 端口） ───
        if let Some(h3_cfg) = configs.iter().find(|cfg| {
            cfg.http3_port.parse::<u16>().unwrap_or(0) > 0 && cfg.cert.is_some()
        }) {
            if tls_config.is_some() {
                let h3_port: u16 = h3_cfg.http3_port.parse().unwrap_or(0);
                let h3_addr: SocketAddr = format!("0.0.0.0:{}", h3_port).parse()?;
                let h3_handler = handlers[0].clone();
                let tls_for_h3 = load_tls_config_for_h3(
                    h3_cfg.cert.as_ref().unwrap(),
                    h3_cfg.key.as_ref().unwrap(),
                )?;
                let mut shutdown_rx_h3 = shutdown_rx.clone();

                tokio::spawn(async move {
                    info!("HTTP/3 (QUIC) 正在监听 UDP {}:{}", h3_addr.ip(), h3_addr.port());
                    if let Err(e) = run_h3_server(h3_handler, h3_addr, tls_for_h3, &mut shutdown_rx_h3).await {
                        error!("HTTP/3 服务器错误: {}", e);
                    }
                });
            }
        }

        // ─── 主 accept 循环 ───
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("ohosHttp 多站点服务器 {} 收到关闭信号，正在优雅关闭...", bind);
                    break;
                }
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            let handlers = handlers.clone();
                            let remote = peer_addr.to_string();
                            let tls_config = tls_config.clone();
                            let router_domain_map = self.router.domain_map.clone();

                            let remote_for_log = remote.clone();
                            tokio::spawn(async move {
                                if let Some(tls_cfg) = tls_config {
                                    let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_cfg);
                                    match tls_acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            let (_, session) = tls_stream.get_ref();
                                            let alpn = session.alpn_protocol()
                                                .and_then(|p| std::str::from_utf8(p).ok())
                                                .unwrap_or("http/1.1")
                                                .to_string();

                                            let io = TokioIo::new(tls_stream);

                                            let service = service_fn(move |req: Request<Incoming>| {
                                                let handler = resolve_handler(&req, &handlers, &router_domain_map);
                                                let remote = remote.clone();
                                                async move { handler.handle(req, remote).await }
                                            });

                                            if alpn == "h2" {
                                                info!("HTTP/2 连接: {} (ALPN: h2)", remote_for_log);
                                                let conn = hyper::server::conn::http2::Builder::new(
                                                    hyper_util::rt::TokioExecutor::new()
                                                )
                                                .timer(hyper_util::rt::TokioTimer::new())
                                                .keep_alive_interval(Some(std::time::Duration::from_secs(30)))
                                                .serve_connection(io, service);

                                                if let Err(err) = conn.await {
                                                    error!("HTTP/2 连接错误 ({}): {}", remote_for_log, err);
                                                }
                                            } else {
                                                let conn = hyper::server::conn::http1::Builder::new()
                                                    .keep_alive(true)
                                                    .serve_connection(io, service)
                                                    .with_upgrades();

                                                if let Err(err) = conn.await {
                                                    error!("HTTP/1.1 连接错误 ({}): {}", remote_for_log, err);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("TLS 握手失败 ({}): {}", remote_for_log, e);
                                        }
                                    }
                                } else {
                                    let service = service_fn(move |req: Request<Incoming>| {
                                        let handler = resolve_handler(&req, &handlers, &router_domain_map);
                                        let remote = remote.clone();
                                        async move { handler.handle(req, remote).await }
                                    });

                                    let io = TokioIo::new(stream);
                                    let conn = hyper::server::conn::http1::Builder::new()
                                        .keep_alive(true)
                                        .serve_connection(io, service)
                                        .with_upgrades();

                                    if let Err(err) = conn.await {
                                        error!("连接处理错误: {}", err);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("接受连接失败: {}", e);
                        }
                    }
                }
            }
        }

        info!("ohosHttp 多站点服务器 {} 已停止", bind);
        Ok(())
    }
}

/// 根据请求的 Host 头解析出对应的 RequestHandler
fn resolve_handler(
    req: &Request<Incoming>,
    handlers: &[Arc<RequestHandler>],
    domain_map: &HashMap<String, usize>,
) -> Arc<RequestHandler> {
    let host = req.headers()
        .get("host")
        .and_then(|v| v.to_str().ok());
    let idx = if let Some(host) = host {
        let hostname = host.split(':').next().unwrap_or(host).to_lowercase();
        domain_map.get(&hostname).copied().unwrap_or(0)
    } else {
        0
    };
    handlers[idx].clone()
}
async fn create_tcp_listener(addr: SocketAddr) -> Result<TcpListener, Box<dyn std::error::Error + Send + Sync>> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::fd::AsRawFd;

    let domain = if addr.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    // SO_REUSEPORT — 多个 Worker 绑定同一端口，内核负责连接分发
    socket.set_reuse_address(true)?;
    // 原生 setsockopt 确保 SO_REUSEPORT 在任何 Linux 上生效
    #[cfg(target_os = "linux")]
    unsafe {
        let optval: libc::c_int = 1;
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &optval as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as u32,
        );
    }

    socket.set_nonblocking(true)?;
    socket.bind(&socket2::SockAddr::from(addr))?;
    socket.listen(1024)?;

    // 转换为 tokio TcpListener
    let std_listener: std::net::TcpListener = socket.into();
    let listener = TcpListener::from_std(std_listener)?;
    Ok(listener)
}

/// 加载 TLS 配置
fn load_tls_config(cert_path: &str, key_path: &str) -> Result<Arc<rustls::ServerConfig>, Box<dyn std::error::Error + Send + Sync>> {
    use rustls::pki_types::CertificateDer;

    let cert_file = &mut std::io::BufReader::new(
        fs::File::open(cert_path)
            .map_err(|e| format!("无法打开 TLS 证书文件 '{}': {}", cert_path, e))?,
    );
    let certs: Vec<CertificateDer> = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(
        fs::File::open(key_path)
            .map_err(|e| format!("无法打开 TLS 私钥文件 '{}': {}", key_path, e))?,
    );
    let key = rustls_pemfile::private_key(key_file)
        .map_err(|e| format!("读取私钥失败: {}", e))?
        .ok_or_else(|| "未找到私钥".to_string())?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS 配置失败: {}", e))?;

    // ALPN: h2 (HTTP/2) 和 http/1.1
    // 必须按优先级排序，h2 优先
    config.alpn_protocols = vec![
        b"h2".to_vec(),
        b"http/1.1".to_vec(),
    ];

    Ok(Arc::new(config))
}

/// 为 HTTP/3 (QUIC) 加载 TLS 配置
/// 需要单独的配置因为 quinn 使用不同的 TLS 设置
fn load_tls_config_for_h3(cert_path: &str, key_path: &str) -> Result<quinn::crypto::rustls::QuicServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    use rustls::pki_types::CertificateDer;

    let cert_file = &mut std::io::BufReader::new(
        fs::File::open(cert_path)
            .map_err(|e| format!("无法打开 TLS 证书文件 (h3) '{}': {}", cert_path, e))?,
    );
    let certs: Vec<CertificateDer> = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(
        fs::File::open(key_path)
            .map_err(|e| format!("无法打开 TLS 私钥文件 (h3) '{}': {}", key_path, e))?,
    );
    let key = rustls_pemfile::private_key(key_file)
        .map_err(|e| format!("读取私钥失败: {}", e))?
        .ok_or_else(|| "未找到私钥".to_string())?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS 配置失败: {}", e))?;

    config.alpn_protocols = vec![
        b"h3".to_vec(),
    ];

    Ok(quinn::crypto::rustls::QuicServerConfig::try_from(config)?)
}

/// HTTP/3 (QUIC) 服务器
async fn run_h3_server(
    handler: Arc<RequestHandler>,
    addr: SocketAddr,
    tls_config: quinn::crypto::rustls::QuicServerConfig,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::net::UdpSocket;

    let socket = UdpSocket::bind(addr)?;
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(quinn::ServerConfig::with_crypto(Arc::new(tls_config))),
        socket,
        Arc::new(quinn::TokioRuntime),
    )?;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                info!("HTTP/3 服务器收到关闭信号，正在停止...");
                endpoint.close(0u8.into(), b"shutdown");
                break;
            }
            conn = endpoint.accept() => {
                match conn {
                    Some(connecting) => {
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            match connecting.await {
                                Ok(quinn_conn) => {
                                    let remote = quinn_conn.remote_address().to_string();
                                    let h3_conn = h3_quinn::Connection::new(quinn_conn);
                                    if let Err(e) = handle_h3_connection(handler, h3_conn, remote).await {
                                        error!("H3 连接错误: {}", e);
                                    }
                                }
                                Err(e) => {
                                    error!("QUIC 连接握手失败: {}", e);
                                }
                            }
                        });
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

/// 处理单个 H3 (HTTP/3 over QUIC) 连接
///
/// 注意：H3 API 仍在快速演进中（h3 v0.0.8），当前实现仅接受 QUIC 连接并记录日志。
/// 完整的 H3 请求处理将在后续版本中完成。
async fn handle_h3_connection(
    _handler: Arc<RequestHandler>,
    _conn: h3_quinn::Connection,
    _remote: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // H3 功能暂未实现 — 保持框架可用
    info!("H3 连接已接受 ({}), 请求处理暂未实现", _remote);
    // 等待 QUIC 连接关闭
    let _ = _conn;
    Ok(())
}
