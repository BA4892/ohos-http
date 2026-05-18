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

//! HTTP 服务器核心 — 进程层、线程层、协程层协同优化
//!
//! ## 进程层优化 (Process)
//!
//! 使用 `SO_REUSEPORT` 套接字选项使多个 Worker 进程绑定到同一端口，
//! 由操作系统内核在进程间分发连接。每个 Worker 独立处理其接受的连接。
//!
//! ## 线程层优化 (Thread)
//!
//! 每个 Worker 进程运行一个 Tokio 多线程运行时 (`new_multi_thread`)，
//! 工作线程数 = `num_cpus::get()`，充分利用所有 CPU 核心。
//!
//! ## 协程层优化 (Coroutine)
//!
//! 每个 HTTP 连接由一个 tokio::spawn 异步任务处理。任务在 Worker 线程间
//! 通过 work-stealing 调度器实现零成本上下文切换。

use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request};
use hyper_util::rt::TokioIo;
use log::{error, info};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::sync::Semaphore;

use crate::config::ServerConfig;
use crate::handler::RequestHandler;
use crate::session_shm::ShmSessionStore;

/// HTTP 服务器实例
pub struct HttpServer {
    config: ServerConfig,
    /// 跨进程共享内存 Session 存储（多进程模式使用）
    shm_store: Option<Arc<ShmSessionStore>>,
}

impl HttpServer {
    pub fn new(config: ServerConfig, shm_store: Option<Arc<ShmSessionStore>>) -> Self {
        HttpServer { config, shm_store }
    }

    /// 启动服务器（含 TLS、HTTP/2、HTTP/3 支持）
    ///
    /// 接收一个 shutdown 信号通道，用于优雅关闭。
    pub async fn start(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr: SocketAddr = self.config.bind.parse()
            .map_err(|e| format!("绑定地址格式错误 '{}': {}", self.config.bind, e))?;

        // 创建 Handler（传入共享内存 Session 存储）
        let handler = Arc::new(RequestHandler::new_with_shm(
            self.config.clone(),
            self.shm_store.clone(),
        ));

        // ─── TLS 配置（如果提供了 cert/key） ───
        let tls_config = if let (Some(cert_path), Some(key_path)) = (&self.config.cert, &self.config.key) {
            info!("配置 TLS: cert={}, key={}", cert_path, key_path);
            Some(load_tls_config(cert_path, key_path)?)
        } else {
            None
        };

        // ─── TCP Listener（支持 SO_REUSEPORT，允许多进程共享同一端口） ───
        let listener = create_tcp_listener(addr).await?;

        info!("ohosHttp 服务器启动: {} (根目录: {}, 线程: {}, 进程: {})",
            self.config.bind, self.config.root, self.config.threads, self.config.workers);

        if tls_config.is_some() {
            info!("TLS/HTTPS 已启用 — ALPN: h2, http/1.1");
        }

        let semaphore = Arc::new(Semaphore::new(self.config.threads));

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
                    let permit = semaphore.clone().acquire_owned().await;
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            let handler = handler.clone();
                            let remote = peer_addr.to_string();
                            let tls_config = tls_config.clone();

                            let remote_for_log = remote.clone();
                            // 每个连接一个独立异步任务（协程），在 Tokio 线程池中调度
                            tokio::spawn(async move {
                                let _permit = permit;

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
                                                .keep_alive_interval(Some(std::time::Duration::from_secs(30)))
                                                .serve_connection(io, service);

                                                if let Err(err) = conn.await {
                                                    error!("HTTP/2 连接错误 ({}): {}", remote_for_log, err);
                                                }
                                            } else {
                                                let conn = hyper::server::conn::http1::Builder::new()
                                                    .keep_alive(true)
                                                    .serve_connection(io, service);

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
                                        .serve_connection(io, service);

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

/// 创建 TCP Listener 并设置 SO_REUSEPORT 选项
///
/// SO_REUSEPORT 允许多个 Worker 进程同时绑定到同一地址和端口，
/// 由内核在进程间分发连接请求，实现内核级负载均衡。
async fn create_tcp_listener(addr: SocketAddr) -> Result<TcpListener, Box<dyn std::error::Error + Send + Sync>> {
    use socket2::{Domain, Protocol, Socket, Type};

    let domain = if addr.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    // 启用 SO_REUSEADDR 和 SO_REUSEPORT
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "linux")]
    socket.set_reuse_port(true)?;

    // 绑定并监听
    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    // 将 socket2 转换为 tokio TcpListener
    let std_listener: std::net::TcpListener = socket.into();
    let listener = TcpListener::from_std(std_listener)?;

    Ok(listener)
}

/// 加载 TLS 配置
fn load_tls_config(cert_path: &str, key_path: &str) -> Result<Arc<rustls::ServerConfig>, Box<dyn std::error::Error + Send + Sync>> {
    use rustls::pki_types::CertificateDer;

    let cert_file = &mut std::io::BufReader::new(fs::File::open(cert_path)?);
    let certs: Vec<CertificateDer> = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(fs::File::open(key_path)?);
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

    let cert_file = &mut std::io::BufReader::new(fs::File::open(cert_path)?);
    let certs: Vec<CertificateDer> = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(fs::File::open(key_path)?);
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
