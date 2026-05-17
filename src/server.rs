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
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::fs;

use hyper::{body::Incoming, Request, Response};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use log::{info, error, warn};
use bytes::Bytes;

use crate::config::ServerConfig;
use crate::handler::RequestHandler;

/// HTTP服务器实例
pub struct HttpServer {
    config: ServerConfig,
}

impl HttpServer {
    pub fn new(config: ServerConfig) -> Self {
        HttpServer { config }
    }

    pub async fn start(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr: SocketAddr = self.config.bind.parse()
            .map_err(|e| format!("绑定地址格式错误 '{}': {}", self.config.bind, e))?;

        let handler = Arc::new(RequestHandler::new(self.config.clone()));

        // ─── TLS 配置（如果提供了 cert/key） ───
        let tls_config = if let (Some(cert_path), Some(key_path)) = (&self.config.cert, &self.config.key) {
            info!("配置 TLS: cert={}, key={}", cert_path, key_path);
            Some(load_tls_config(cert_path, key_path)?)
        } else {
            None
        };

        // ─── TCP Listener ───
        let listener = TcpListener::bind(addr).await
            .map_err(|e| format!("监听 {} 失败: {}", self.config.bind, e))?;

        info!("ohosHttp 服务器启动: {} (根目录: {}, 线程: {})",
            self.config.bind, self.config.root, self.config.threads);

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

        // ─── 主 accept 循环 (TCP + TLS) ───
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

/// 加载 TLS 配置
fn load_tls_config(cert_path: &str, key_path: &str) -> Result<Arc<rustls::ServerConfig>, Box<dyn std::error::Error + Send + Sync>> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

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

/// 为 HTTP/3 加载 TLS 配置（无需 ALPN）
fn load_tls_config_for_h3(cert_path: &str, key_path: &str) -> Result<quinn::crypto::rustls::QuicServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let cert_file = &mut std::io::BufReader::new(fs::File::open(cert_path)?);
    let certs: Vec<CertificateDer> = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;

    let key_file = &mut std::io::BufReader::new(fs::File::open(key_path)?);
    let key = rustls_pemfile::private_key(key_file)
        .map_err(|e| format!("读取私钥失败: {}", e))?
        .ok_or_else(|| "未找到私钥".to_string())?;

    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS 配置失败: {}", e))?;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
        .map_err(|e| format!("QUIC TLS 配置失败: {}", e))?;

    Ok(quic_config)
}

/// 运行 HTTP/3 服务器（QUIC + h3）
async fn run_h3_server(
    handler: Arc<RequestHandler>,
    addr: SocketAddr,
    tls_config: quinn::crypto::rustls::QuicServerConfig,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::net::{UdpSocket, SocketAddr as StdSocketAddr};

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

/// 处理单个 HTTP/3 连接
async fn handle_h3_connection(
    handler: Arc<RequestHandler>,
    conn: h3_quinn::Connection,
    remote: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut h3_conn = h3::server::Connection::new(conn).await?;

    loop {
        match h3_conn.accept().await {
            Ok(Some(resolver)) => {
                let handler = handler.clone();
                let remote = remote.clone();
                tokio::spawn(async move {
                    match resolver.resolve_request().await {
                        Ok((req, mut stream)) => {
                            // 读取 HTTP/3 请求体
                            let mut body_buf = Vec::new();
                            while let Ok(Some(mut data)) = stream.recv_data().await {
                                use bytes::Buf;
                                body_buf.extend_from_slice(data.chunk());
                                data.advance(data.remaining());
                            }
                            let (parts, _) = req.into_parts();
                            let req_with_body = http::Request::from_parts(parts, Bytes::from(body_buf));

                            let resp = handler.handle_h3(req_with_body, remote).await;
                            let (parts, body) = resp.into_parts();
                            let body_bytes = http_body_util::BodyExt::collect(body).await
                                .map(|c| c.to_bytes())
                                .unwrap_or(Bytes::new());

                            let response = http::Response::from_parts(parts, ());
                            if let Err(e) = stream.send_response(response).await {
                                error!("H3 发送响应头失败: {}", e);
                                return;
                            }
                            if let Err(e) = stream.send_data(body_bytes).await {
                                error!("H3 发送响应体失败: {}", e);
                                return;
                            }
                            if let Err(e) = stream.finish().await {
                                error!("H3 结束流失败: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("H3 请求解析错误: {}", e);
                        }
                    }
                });
            }
            Ok(None) => break, // 连接关闭
            Err(e) => {
                error!("H3 accept 错误: {}", e);
                break;
            }
        }
    }

    Ok(())
}
