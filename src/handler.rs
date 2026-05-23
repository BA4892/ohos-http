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
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use chrono::Utc;
use flate2::{Compression, write::GzEncoder};
use hyper::{body::Body, body::Incoming, HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use lru::LruCache;
use mime_guess::from_path;
use tokio::fs;
use tokio::process::Command as TokioCommand;
use tokio::sync::RwLock;

use crate::config::{CgiConfig, LocationConfig, ServerConfig};
use crate::load_balancer::LoadBalancer;
use crate::logger::AccessLogger;
use crate::proxy::ProxyClient;
use crate::rate_limiter::RateLimiter;
use crate::rewrite::RewriteEngine;
use crate::session::SessionStore;


/// 简单内存缓存（LRU 淘汰）
struct MemCache {
    data: LruCache<String, CacheEntry>,
    max_size: u64,
    current_size: u64,
}

struct CacheEntry {
    data: Arc<Vec<u8>>,
    mime: String,
    created: SystemTime,
    ttl: u64,
    size: u64,
}

/// 响应Body类型
pub type ResponseBody = Full<Bytes>;

/// 请求处理器
pub struct RequestHandler {
    config: ServerConfig,
    site_index: Option<usize>,
    rewrite_engine: RewriteEngine,
    proxy_client: Option<ProxyClient>,
    cache: Option<Arc<RwLock<MemCache>>>,
    access_logger: Option<AccessLogger>,
    rate_limiter: Option<RateLimiter>,
    session_store: Option<SessionStore>,
    load_balancers: HashMap<String, LoadBalancer>,
}

impl RequestHandler {
    pub fn new(config: ServerConfig) -> Self {
        Self::new_internal(config, 0)
    }

    pub fn new_with_site_index(config: ServerConfig, site_index: usize) -> Self {
        Self::new_internal(config, site_index)
    }

    fn new_internal(config: ServerConfig, site_index: usize) -> Self {

        let rewrite_engine = RewriteEngine::new(&config.rewrite);
        let has_proxy = config.location.iter().any(|l| l.proxy_pass.is_some());

        let cache = if config.cache_enabled {
            Some(Arc::new(RwLock::new(MemCache {
                data: LruCache::new(NonZeroUsize::new(10000).unwrap()),
                max_size: config.cache_max_size_bytes,
                current_size: 0,
            })))
        } else {
            None
        };

        let access_logger = config.access_log.as_ref().map(|log_path| {
            AccessLogger::new(log_path, config.log_rotate_size_bytes)
        });

        // 初始化限流器（含 CC/DDoS 防护）
        let rate_limiter = config.rate_limit.as_ref().map(|rl| {
            RateLimiter::new_full(
                rl.enabled,
                rl.requests_per_second,
                rl.burst_size,
                rl.connections_per_second,
                rl.max_concurrent_connections,
                rl.ban_duration_seconds,
                rl.ban_threshold,
                config.blacklist.clone(),
                config.per_ip_rates.clone(),
            )
        });

        // 初始化 Session 存储
        let session_store = config.session.as_ref().filter(|s| s.enabled).map(|sc| {
            SessionStore::new(&sc.cookie_name, sc.ttl)
        });

        // 初始化负载均衡器
        let mut load_balancers = HashMap::new();
        for loc in &config.location {
            if !loc.load_balance_targets.is_empty() {
                let targets: Vec<crate::load_balancer::BalanceTarget> = loc.load_balance_targets
                    .iter()
                    .map(|t| crate::load_balancer::BalanceTarget {
                        url: t.url.clone(),
                        weight: t.weight,
                    })
                    .collect();
                load_balancers.insert(loc.path.clone(), LoadBalancer::new(targets));
            }
        }

        // 检查是否需要代理客户端（负载均衡也需要）
        let has_lb = !load_balancers.is_empty();
        let proxy_client = if has_proxy || has_lb {
            Some(ProxyClient::new())
        } else {
            None
        };

        RequestHandler {
            config,
            site_index: Some(site_index),
            rewrite_engine,
            proxy_client,
            cache,
            access_logger,
            rate_limiter,
            session_store,
            load_balancers,
        }
    }

    pub async fn handle(&self, req: Request<Incoming>, remote_addr: String) -> Result<Response<ResponseBody>, hyper::Error> {
        let (parts, body) = req.into_parts();

        // ─── 提前检查上传请求大小，防止超大请求撑爆内存 ───
        if parts.method == Method::POST || parts.method == Method::PUT || parts.method == Method::PATCH {
            if let Some(size_str) = parts.headers.get("content-length").and_then(|v| v.to_str().ok()) {
                if let Ok(size) = size_str.parse::<u64>() {
                    if size > self.config.upload_max_size_bytes {
                        drop(body);
                        let mut resp = error_response(413, "Request Entity Too Large");
                        self.add_cors_headers(resp.headers_mut(), &parts.headers);
                        if let Some(logger) = &self.access_logger {
                            let path = parts.uri.path();
                            let referer = parts.headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
                            let ua = parts.headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
                            logger.log(&remote_addr, parts.method.as_str(), path, 413, 0, referer, ua, 0);
                        }
                        return Ok(resp);
                    }
                }
            }
        }

        let body_bytes = body.collect().await.map(|b| b.to_bytes()).unwrap_or(Bytes::new());

        // 检查 WebSocket 升级请求
        if is_websocket_upgrade(&parts.headers) {
            if let Some(resp) = self.handle_websocket_proxy(&parts, &remote_addr).await {
                return Ok(resp);
            }
            // 没有匹配的代理，降级到普通处理
        }

        self.handle_internal(&parts.method, &parts.uri, &parts.headers, body_bytes, &remote_addr).await
    }

    /// 处理HTTP/3请求
    #[allow(dead_code)]
    pub async fn handle_h3(&self, req: http::Request<Bytes>, remote_addr: String) -> Response<ResponseBody> {
        let (parts, body) = req.into_parts();
        match self.handle_internal(&parts.method, &parts.uri, &parts.headers, body, &remote_addr).await {
            Ok(r) => r,
            Err(_) => error_response(500, "Internal Server Error")
        }
    }

    /// 获取限流器引用（供 server 层做连接级检查）
    pub fn rate_limiter_ref(&self) -> Option<&RateLimiter> {
        self.rate_limiter.as_ref()
    }

    /// 内部处理方法（协议无关）
    async fn handle_internal(
        &self,
        method: &Method,
        uri: &hyper::Uri,
        headers: &HeaderMap,
        body_bytes: Bytes,
        remote_addr: &str,
    ) -> Result<Response<ResponseBody>, hyper::Error> {
        let start = std::time::Instant::now();
        let path = uri.path();

        // ─── 方法过滤：拒绝不安全的方法（TRACE/TRACK/CONNECT） ───
        match *method {
            Method::TRACE | Method::CONNECT => {
                return Ok(error_response(405, "Method Not Allowed"));
            }
            _ => {}
        }

        // ─── 请求体大小限制（默认 10MB） ───
        if body_bytes.len() > 10_485_760 {
            return Ok(error_response(413, "Request Entity Too Large"));
        }

        // ─── Rate limiting check ───
        if let Some(limiter) = &self.rate_limiter {
            // 先检查黑名单
            if limiter.is_blocked(remote_addr).await {
                let resp = error_response(403, "Forbidden: Your IP is blocked");
                let (mut parts, body) = resp.into_parts();
                self.add_cors_headers(&mut parts.headers, headers);
                if let Some(logger) = &self.access_logger {
                    let referer = headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    logger.log(remote_addr, method.as_str(), path, 403, 0, referer, ua, 0);
                }
                return Ok(Response::from_parts(parts, body));
            }
            // 再检查限流（令牌桶）
            if !limiter.check(remote_addr).await {
                let resp = error_response(429, "Too Many Requests");
                let (mut parts, body) = resp.into_parts();
                self.add_cors_headers(&mut parts.headers, headers);
                if let Some(logger) = &self.access_logger {
                    let referer = headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    logger.log(remote_addr, method.as_str(), path, 429, 0, referer, ua, 0);
                }
                return Ok(Response::from_parts(parts, body));
            }
        }

        // 检查是否禁止 IP 直接访问
        if !self.config.allow_ip_access && !self.config.domains.is_empty() {
            let host_header = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
            let hostname = host_header.split(':').next().unwrap_or(host_header);
            if !self.config.domains.iter().any(|d| d == hostname) {
                let resp = error_response(403, "Forbidden: Direct IP access is not allowed");
                let (mut parts, body) = resp.into_parts();
                self.add_cors_headers(&mut parts.headers, headers);
                if let Some(logger) = &self.access_logger {
                    let referer = headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
                    logger.log(remote_addr, method.as_str(), path, 403, 0, referer, ua, 0);
                }
                return Ok(Response::from_parts(parts, body));
            }
        }

        // Session cookie handling
        if let Some(store) = &self.session_store {
            store.cleanup();
            let cookie_header = headers.get("cookie").and_then(|v| v.to_str().ok());
            let session_id = store.parse_session_id(cookie_header);
            if session_id.is_none() {
                // New session — just create it, cookie will be set later
                let _ = store.create();
            }
        }

        // CORS preflight
        if *method == Method::OPTIONS {
            let resp = self.cors_preflight_response(headers);
            return Ok(resp);
        }

        // URL rewrite
        // 支持 ThinkPHP/Laravel 等框架：如果原始路径对应真实文件则跳过重写
        let final_path = {
            let rewritten = self.rewrite_engine.rewrite(path);
            match rewritten {
                Some(r) if r != path => {
                    // 重写改变了路径，检查原始文件是否存在
                    let original_file = std::path::PathBuf::from(&self.config.root).join(path.trim_start_matches('/'));
                    if original_file.exists() {
                        path.to_string()
                    } else {
                        r
                    }
                }
                _ => path.to_string(),
            }
        };

        // location matching
        let matching_location = {
            let mut locations: Vec<&LocationConfig> = self.config.location.iter().collect();
            locations.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
            locations.into_iter().find(|loc| final_path.starts_with(&loc.path))
        };

        // ─── 禁止访问检查（仅对非反向代理的请求生效） ───
        let has_reverse_proxy = self.config.location.iter().any(|l| l.proxy_pass.is_some() || !l.load_balance_targets.is_empty());
        if !has_reverse_proxy && is_path_forbidden(&final_path, &self.config.forbidden_dirs, &self.config.forbidden_files) {
            let resp = error_response(403, "Forbidden: Access to this resource is denied");
            let (mut parts, body) = resp.into_parts();
            self.add_cors_headers(&mut parts.headers, headers);
            if let Some(logger) = &self.access_logger {
                let referer = headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
                let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
                logger.log(remote_addr, method.as_str(), path, 403, 0, referer, ua, 0);
            }
            return Ok(Response::from_parts(parts, body));
        }

        let result = if let Some(loc) = matching_location {
            // 负载均衡优先于单点代理
            if let Some(lb) = self.load_balancers.get(&loc.path) {
                match lb.proxy_request(method, uri, headers, body_bytes, &loc.proxy_headers).await {
                    Ok(resp) => {
                        let (resp_parts, resp_body) = resp.into_parts();
                        let body_bytes_inner = resp_body.collect().await.map(|b| b.to_bytes()).unwrap_or(Bytes::new());
                        let mut new_resp = Response::new(Full::from(body_bytes_inner));
                        *new_resp.status_mut() = resp_parts.status;
                        *new_resp.headers_mut() = resp_parts.headers;
                        self.add_cors_headers(new_resp.headers_mut(), headers);
                        new_resp
                    }
                    Err(e) => error_response(502, &format!("Bad Gateway: {}", e))
                }
            } else if let Some(proxy_pass) = &loc.proxy_pass {
                if let Some(client) = &self.proxy_client {
                    match client.proxy_request_from_parts(
                        method, uri, headers, body_bytes, proxy_pass, &loc.proxy_headers,
                    ).await {
                        Ok(resp) => {
                            let (resp_parts, resp_body) = resp.into_parts();
                            let body_bytes_inner = resp_body.collect().await.map(|b| b.to_bytes()).unwrap_or(Bytes::new());
                            let mut new_resp = Response::new(Full::from(body_bytes_inner));
                            *new_resp.status_mut() = resp_parts.status;
                            *new_resp.headers_mut() = resp_parts.headers;
                            self.add_cors_headers(new_resp.headers_mut(), headers);
                            new_resp
                        }
                        Err(e) => error_response(502, &format!("Bad Gateway: {}", e))
                    }
                } else {
                    error_response(502, "Proxy not configured")
                }
            } else if let Some(loc_root) = &loc.root {
                let relative = final_path.strip_prefix(&loc.path).unwrap_or(&final_path);
                let file_path = PathBuf::from(loc_root).join(relative.trim_start_matches('/'));
                if let Some(cgi_cfg) = &loc.cgi {
                    self.execute_cgi(&file_path, cgi_cfg, method, headers, body_bytes, uri.query(), remote_addr).await?
                } else {
                    self.serve_file(&file_path, loc.expires.as_deref()).await?
                }
            } else {
                self.handle_regular_request(method.clone(), headers, body_bytes, &final_path, uri.query(), remote_addr).await?
            }
        } else {
            self.handle_regular_request(method.clone(), headers, body_bytes, &final_path, uri.query(), remote_addr).await?
        };

        // ─── HEAD 方法：保留头部和 Content-Length，清空 body（RFC 9110 §9.3.3）───
        let result = if *method == Method::HEAD {
            let (parts, body) = result.into_parts();
            let content_len: u64 = body.size_hint().exact().unwrap_or(0);
            let mut parts = parts;
            if !parts.headers.contains_key(http::header::CONTENT_LENGTH) {
                parts.headers.insert(
                    http::header::CONTENT_LENGTH,
                    http::HeaderValue::from_str(&content_len.to_string()).unwrap(),
                );
            }
            Response::from_parts(parts, Full::from(Bytes::new()))
        } else {
            result
        };

        // CORS headers + logging
        let (mut parts, body) = result.into_parts();
        let status = parts.status.as_u16();
        let body_size: u64 = parts.headers.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
        let duration_ms = start.elapsed().as_millis() as u64;
        self.add_cors_headers(&mut parts.headers, headers);
        self.add_security_headers(&mut parts.headers);

        // Inject session cookie for new sessions
        if let Some(store) = &self.session_store {
            let cookie_header = headers.get("cookie").and_then(|v| v.to_str().ok());
            if store.parse_session_id(cookie_header).is_none() {
                let new_id = store.create();
                parts.headers.insert(
                    http::header::SET_COOKIE,
                    http::HeaderValue::from_str(&store.build_set_cookie(&new_id))
                        .unwrap_or_else(|_| http::HeaderValue::from_static("")),
                );
            }
        }

        // ─── Gzip 压缩 ───
        // 对文本类响应启用 gzip 压缩（如果客户端支持）
        let body_bytes = body.collect().await.map(|b| b.to_bytes()).unwrap_or(Bytes::new());
        let (body, compressed) = self.check_compress_response(headers, &parts.headers, body_bytes);
        if compressed {
            parts.headers.insert(
                http::header::CONTENT_ENCODING,
                http::HeaderValue::from_static("gzip"),
            );
            // 移除 content-length 因为压缩后长度变了
            parts.headers.remove(http::header::CONTENT_LENGTH);
        }

        // ─── 访问日志 ───
        if let Some(logger) = &self.access_logger {
            let referer = headers.get("referer").and_then(|v| v.to_str().ok()).unwrap_or("-");
            let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
            logger.log(remote_addr, method.as_str(), path, status, body_size, referer, ua, duration_ms);
        }

        Ok(Response::from_parts(parts, body))
    }

    /// 处理常规请求（文件服务、CGI、上传等）
    async fn handle_regular_request(
        &self,
        method: Method,
        headers: &HeaderMap,
        body_bytes: Bytes,
        final_path: &str,
        query: Option<&str>,
        remote_addr: &str,
    ) -> Result<Response<ResponseBody>, hyper::Error> {
        // 构造完整文件路径
        let file_path = PathBuf::from(&self.config.root).join(final_path.trim_start_matches('/'));

        match method {
            Method::GET | Method::HEAD => {
                // 检查是否需要 CGI 解释执行
                if let Some(cgi_cfg) = self.find_cgi_config(&file_path) {
                    self.execute_cgi(&file_path, &cgi_cfg, &Method::GET, headers, Bytes::new(), query, remote_addr).await
                } else {
                    self.serve_file(&file_path, None).await
                }
            }
            Method::POST => {
                // POST 始终允许（用于表单提交等），不检查 allow_upload
                // 检查是否需要 CGI 解释执行
                if let Some(cgi_cfg) = self.find_cgi_config(&file_path) {
                    return self.execute_cgi(&file_path, &cgi_cfg, &method, headers, body_bytes, query, remote_addr).await;
                }

                // 检查上传大小
                let content_length = headers
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);

                if content_length > self.config.upload_max_size_bytes {
                    return Ok(error_response(413, "Request Entity Too Large"));
                }

                if body_bytes.len() as u64 > self.config.upload_max_size_bytes {
                    return Ok(error_response(413, "Request Entity Too Large"));
                }

                // 保存上传文件到上传目录
                let upload_dir = PathBuf::from(&self.config.root).join("uploads");
                let _ = fs::create_dir_all(&upload_dir).await;

                let body_for_write = body_bytes.clone();
                let body_len = body_bytes.len();
                let file_id = Utc::now().timestamp();
                let filename = format!("{}.bin", file_id);

                // 非阻塞写入
                let fname = filename.clone();
                tokio::spawn(async move {
                    let filepath = upload_dir.join(&fname);
                    let _ = fs::write(&filepath, &body_for_write).await;
                });

                let resp_json = format!(
                    "{{\"status\":\"ok\",\"size\":{},\"file\":\"uploads/{}\"}}",
                    body_len, filename
                );
                Ok(Response::new(Full::from(Bytes::from(resp_json))))
            }
            Method::PUT | Method::PATCH => {
                if !self.config.allow_upload {
                    return Ok(error_response(405, "Method Not Allowed"));
                }
                // 检查是否需要 CGI 解释执行
                if let Some(cgi_cfg) = self.find_cgi_config(&file_path) {
                    return self.execute_cgi(&file_path, &cgi_cfg, &method, headers, body_bytes, query, remote_addr).await;
                }

                // 检查上传大小
                let content_length = headers
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);

                if content_length > self.config.upload_max_size_bytes {
                    return Ok(error_response(413, "Request Entity Too Large"));
                }

                if body_bytes.len() as u64 > self.config.upload_max_size_bytes {
                    return Ok(error_response(413, "Request Entity Too Large"));
                }

                // 保存上传文件到上传目录
                let upload_dir = PathBuf::from(&self.config.root).join("uploads");
                let _ = fs::create_dir_all(&upload_dir).await;

                let body_for_write = body_bytes.clone();
                let body_len = body_bytes.len();
                let file_id = Utc::now().timestamp();
                let filename = format!("{}.bin", file_id);

                // 非阻塞写入
                let fname = filename.clone();
                tokio::spawn(async move {
                    let filepath = upload_dir.join(&fname);
                    let _ = fs::write(&filepath, &body_for_write).await;
                });

                let resp_json = format!(
                    "{{\"status\":\"ok\",\"size\":{},\"file\":\"uploads/{}\"}}",
                    body_len, filename
                );
                Ok(Response::new(Full::from(Bytes::from(resp_json))))
            }
            Method::DELETE => {
                if !self.config.allow_delete {
                    return Ok(error_response(405, "Method Not Allowed"));
                }
                // DELETE 请求：尝试删除文件
                let normalized = normalize_path(&file_path);
                if !normalized.starts_with(&self.config.root) {
                    return Ok(error_response(403, "Forbidden"));
                }

                if normalized.exists() {
                    match fs::remove_file(&normalized).await {
                        Ok(_) => {
                            let resp_json = format!(
                                "{{\"status\":\"ok\",\"message\":\"deleted\"}}"
                            );
                            Ok(Response::new(Full::from(Bytes::from(resp_json))))
                        }
                        Err(e) => {
                            Ok(error_response(500, &format!("Delete failed: {}", e)))
                        }
                    }
                } else {
                    Ok(error_response(404, "Not Found"))
                }
            }
            _ => Ok(error_response(405, "Method Not Allowed")),
        }
    }

    /// 生成 CORS 预检响应（仅当 Origin 匹配时返回 CORS 头）
    fn cors_preflight_response(&self, req_headers: &HeaderMap) -> Response<ResponseBody> {
        if !self.is_origin_allowed(req_headers) {
            return error_response(403, "CORS: Origin not allowed");
        }
        let mut resp = Response::new(Full::from(Bytes::new()));
        *resp.status_mut() = hyper::StatusCode::NO_CONTENT;
        self.add_cors_headers(resp.headers_mut(), req_headers);
        resp.headers_mut().insert(
            hyper::header::ACCESS_CONTROL_MAX_AGE,
            "86400".parse().unwrap()
        );
        resp
    }

    /// 判断请求的 Origin 是否在 CORS 允许范围内
    fn is_origin_allowed(&self, req_headers: &HeaderMap) -> bool {
        let config_origin = &self.config.cors_origin;
        if config_origin.is_empty() {
            return false;
        }
        if config_origin == "*" {
            return true;
        }
        // 特定源：必须匹配请求的 Origin 头
        if let Some(origin) = req_headers.get("origin").and_then(|v| v.to_str().ok()) {
            return origin == config_origin;
        }
        false
    }

    /// 添加安全响应头
    fn add_security_headers(&self, headers: &mut HeaderMap) {
        use http::HeaderName;
        headers.insert(
            HeaderName::from_static("x-content-type-options"),
            http::HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            HeaderName::from_static("referrer-policy"),
            http::HeaderValue::from_static("strict-origin-when-cross-origin"),
        );
    }

    /// 为响应添加 CORS 头（仅在 Origin 匹配时）
    fn add_cors_headers(&self, headers: &mut HeaderMap, req_headers: &HeaderMap) {
        if !self.is_origin_allowed(req_headers) {
            return;
        }
        let origin_value = if self.config.cors_origin == "*" {
            "*"
        } else {
            // 已验证匹配，直接使用请求的 Origin
            req_headers.get("origin").unwrap().to_str().unwrap()
        };
        headers.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            origin_value.parse().unwrap()
        );
        headers.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_METHODS,
            self.config.cors_methods.parse().unwrap()
        );
        headers.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_HEADERS,
            self.config.cors_headers.parse().unwrap()
        );
    }

    /// 根据文件路径查找匹配的 CGI 配置
    fn find_cgi_config(&self, file_path: &Path) -> Option<CgiConfig> {
        let ext = file_path.extension()?.to_str()?;
        let ext_with_dot = format!(".{}", ext);
        self.config.cgi.iter()
            .find(|c| c.extensions.contains(&ext_with_dot))
            .cloned()
    }

    /// 执行 CGI 脚本
    async fn execute_cgi(
        &self,
        script_path: &Path,
        cgi: &CgiConfig,
        method: &Method,
        headers: &HeaderMap,
        body_bytes: Bytes,
        query_string: Option<&str>,
        remote_addr: &str,
    ) -> Result<Response<ResponseBody>, hyper::Error> {
        let script_path = normalize_path(script_path);

        // 检查脚本是否存在
        if fs::metadata(&script_path).await.is_err() {
            return Ok(error_response(404, "CGI script not found"));
        }

        let server_port = self.config.bind.split(':').nth(1).unwrap_or("80").to_string();

        // 收集所有 HTTP_* 头
        let orig_headers: Vec<(String, String)> = headers.iter()
            .map(|(name, value)| {
                let header_name = format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_"));
                let header_val = value.to_str().unwrap_or("").to_string();
                (header_name, header_val)
            })
            .collect();
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // 构建 CGI 环境变量
        let script_name = format!("/{}", script_path.strip_prefix(&self.config.root)
            .unwrap_or(&script_path).to_string_lossy());
        let script_filename = script_path.to_string_lossy().to_string();
        let query = query_string.unwrap_or("");
        let content_length = body_bytes.len().to_string();
        let request_uri = if query.is_empty() {
            script_name.clone()
        } else {
            format!("{}?{}", script_name, query)
        };

        // 准备子进程
        let mut cmd = TokioCommand::new(&cgi.interpreter);
        cmd.arg(&script_filename)
            .env("SERVER_SOFTWARE", format!("ohos-server/{}", env!("CARGO_PKG_VERSION")))
            .env("SERVER_NAME", "localhost")
            .env("GATEWAY_INTERFACE", "CGI/1.1")
            .env("SERVER_PROTOCOL", "HTTP/1.1")
            .env("SERVER_PORT", server_port)
            .env("REQUEST_METHOD", method.as_str())
            .env("PATH_INFO", &request_uri)
            .env("PATH_TRANSLATED", &script_filename)
            .env("SCRIPT_NAME", &script_name)
            .env("SCRIPT_FILENAME", &script_filename)
            .env("QUERY_STRING", query)
            .env("REMOTE_ADDR", remote_addr)
            .env("CONTENT_TYPE", &content_type)
            .env("CONTENT_LENGTH", &content_length)
            .env("REQUEST_URI", &request_uri)
            .env("REDIRECT_STATUS", "1");

        for (name, value) in &orig_headers {
            cmd.env(name, value);
        }

        // 如果是 POST/PUT，传递请求体到 stdin
        cmd.kill_on_drop(true);
        let output = if !body_bytes.is_empty() {
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        } else {
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        };

        let mut child = match output {
            Ok(c) => c,
            Err(e) => return Ok(error_response(500, &format!("CGI execute error: {}", e))),
        };

        // 写入 stdin（POST/PUT数据）（带 30s 超时）
        if !body_bytes.is_empty() {
            if let Some(stdin) = child.stdin.as_mut() {
                use tokio::io::AsyncWriteExt;
                let _ = tokio::time::timeout(Duration::from_secs(30), stdin.write_all(&body_bytes)).await;
                let _ = tokio::time::timeout(Duration::from_secs(30), stdin.flush()).await;
            }
            // 关闭 stdin，等待子进程
            if let Some(stdin) = child.stdin.take() {
                drop(stdin);
            }
        }

        // 读取 stdout（带 30s 超时）
        // kill_on_drop 已在 spawn 前设置，超时或取消时子进程会被杀死
        let output = tokio::select! {
            result = child.wait_with_output() => {
                match result {
                    Ok(o) => o,
                    Err(e) => return Ok(error_response(500, &format!("CGI wait error: {}", e))),
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                return Ok(error_response(500, "CGI timeout (30s exceeded)"));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(error_response(500, &format!("CGI error: {}", stderr)));
        }

        // 解析 CGI 输出（headers + body）
        let stdout = output.stdout;
        let header_end = stdout.windows(4)
            .position(|w| w == b"\r\n\r\n")
            .or_else(|| stdout.windows(2).position(|w| w == b"\n\n"))
            .unwrap_or(0);

        let header_bytes = if header_end > 0 {
            &stdout[..header_end]
        } else {
            &stdout[..]
        };
        let body_start = if header_end > 0 {
            let offset = if stdout[header_end..].starts_with(b"\r\n\r\n") { 4 } else { 2 };
            header_end + offset
        } else {
            0
        };
        let body_bytes_out = if body_start < stdout.len() {
            stdout[body_start..].to_vec()
        } else {
            Vec::new()
        };

        // 解析 headers
        let header_str = String::from_utf8_lossy(header_bytes);
        let mut status_code = 200;
        let mut content_type_out = "text/html; charset=utf-8".to_string();
        let mut location = None;

        for line in header_str.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(val) = line.strip_prefix("Status:").or_else(|| line.strip_prefix("status:")) {
                if let Some(code_str) = val.trim().split_whitespace().next() {
                    status_code = code_str.parse::<u16>().unwrap_or(200);
                }
            } else if let Some(val) = line.strip_prefix("Content-Type:").or_else(|| line.strip_prefix("content-type:")) {
                content_type_out = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("Location:").or_else(|| line.strip_prefix("location:")) {
                location = Some(val.trim().to_string());
            }
        }

        // 处理重定向
        if let Some(loc) = location {
            let mut resp = Response::new(Full::from(Bytes::from(body_bytes_out)));
            *resp.status_mut() = StatusCode::FOUND;
            resp.headers_mut().insert(
                hyper::header::LOCATION,
                loc.parse().unwrap()
            );
            return Ok(resp);
        }

        let mut resp = Response::new(Full::from(Bytes::from(body_bytes_out)));
        *resp.status_mut() = StatusCode::from_u16(status_code).unwrap_or(StatusCode::OK);
        resp.headers_mut().insert(
            hyper::header::CONTENT_TYPE,
            content_type_out.parse().unwrap()
        );
        resp.headers_mut().insert(
            hyper::header::HeaderName::from_static("server"),
            format!("ohos-server/{}", env!("CARGO_PKG_VERSION")).parse().unwrap()
        );

        Ok(resp)
    }

    /// 服务静态文件
    async fn serve_file(&self, file_path: &Path, expires: Option<&str>) -> Result<Response<ResponseBody>, hyper::Error> {
        let file_path = file_path.to_path_buf();

        // 防止路径穿越攻击
        let normalized = normalize_path(&file_path);
        // 规范化 root 路径（处理相对路径 vs canonicalize 后的绝对路径比较）
        let root_for_check = PathBuf::from(&self.config.root);
        let root_normalized = if root_for_check.exists() {
            root_for_check.canonicalize().unwrap_or(root_for_check)
        } else {
            root_for_check
        };
        if !normalized.starts_with(&root_normalized) {
            // 如果 normalized 是相对路径而 root_normalized 是绝对路径，将 normalized 也转为绝对路径
            if !root_normalized.as_os_str().is_empty() && normalized.is_relative() {
                if let Ok(abs_normalized) = std::env::current_dir().map(|cwd| cwd.join(&normalized)) {
                    if abs_normalized.starts_with(&root_normalized) {
                        // 通过检查，继续使用 normalized
                    } else {
                        return Ok(error_response(403, "Forbidden"));
                    }
                } else {
                    return Ok(error_response(403, "Forbidden"));
                }
            } else {
                return Ok(error_response(403, "Forbidden"));
            }
        }

        match fs::metadata(&normalized).await {
            Ok(meta) => {
                if meta.is_dir() {
                    // 尝试 index.html
                    let index_path = normalized.join("index.html");
                    if fs::metadata(&index_path).await.is_ok() {
                        return self.read_and_respond(&index_path, expires).await;
                    }
                    // 目录列表
                    if self.config.directory_listing {
                        return self.list_directory(&normalized).await;
                    }
                    return Ok(error_response(403, "Forbidden - Directory Listing Disabled"));
                }

                // 检查缓存
                if let Some(cache) = &self.cache {
                    let cache_key = normalized.to_string_lossy().to_string();
                    {
                        let cache_read = cache.read().await;
                        if let Some(entry) = cache_read.data.peek(&cache_key) {
                            if entry.created.elapsed().unwrap_or_default().as_secs() < entry.ttl {
                                let mut resp = Response::new(Full::from(
                                    Bytes::copy_from_slice(&entry.data)
                                ));
                                resp.headers_mut().insert(
                                    hyper::header::CONTENT_TYPE,
                                    entry.mime.parse().unwrap()
                                );
                                resp.headers_mut().insert(
                                    hyper::header::HeaderName::from_static("x-cache"),
                                    "HIT".parse().unwrap()
                                );
                                if let Some(exp) = expires {
                                    resp.headers_mut().insert(
                                        hyper::header::CACHE_CONTROL,
                                        format!("max-age={}", parse_expires(exp)).parse().unwrap()
                                    );
                                }
                                return Ok(resp);
                            }
                        }
                    }
                    // 如果缓存未命中或过期，读取文件并缓存
                    let data = fs::read(&normalized).await.unwrap_or_default();
                    let file_size = data.len() as u64;
                    let mime = mime_type(&normalized).to_string();
                    // 仅缓存小文件（<= 1MB）以避免内存压力
                    if file_size <= 1_048_576 {
                        // LRU 淘汰：如果超过 max_size，逐出最久未用的条目
                        let entry = CacheEntry {
                            data: Arc::new(data.clone()),
                            mime: mime.clone(),
                            created: SystemTime::now(),
                            ttl: self.config.cache_ttl_seconds,
                            size: file_size,
                        };
                        {
                            let mut cache_write = cache.write().await;
                            // 强制淘汰直到有足够空间
                            while !cache_write.data.is_empty()
                                && cache_write.current_size + file_size > cache_write.max_size
                            {
                                if let Some((_, evicted)) = cache_write.data.pop_lru() {
                                    cache_write.current_size = cache_write.current_size.saturating_sub(evicted.size);
                                }
                            }
                            cache_write.current_size += file_size;
                            cache_write.data.put(cache_key, entry);
                        }
                    }
                    let mut resp = Response::new(Full::from(data));
                    resp.headers_mut().insert(
                        hyper::header::CONTENT_TYPE,
                        mime.parse().unwrap()
                    );
                    resp.headers_mut().insert(
                        hyper::header::HeaderName::from_static("x-cache"),
                        "MISS".parse().unwrap()
                    );
                    if let Some(exp) = expires {
                        resp.headers_mut().insert(
                            hyper::header::CACHE_CONTROL,
                            format!("max-age={}", parse_expires(exp)).parse().unwrap()
                        );
                    }
                    return Ok(resp);
                }

                // 无缓存，直接读取
                self.read_and_respond(&normalized, expires).await
            }
            Err(_) => Ok(error_response(404, "Not Found")),
        }
    }

    /// 读取文件并返回响应
    async fn read_and_respond(&self, path: &Path, expires: Option<&str>) -> Result<Response<ResponseBody>, hyper::Error> {
        // 先获取元数据（用于 ETag 和 Last-Modified）
        let meta = match fs::metadata(path).await {
            Ok(m) => m,
            Err(_) => return Ok(error_response(500, "Internal Server Error")),
        };

        match fs::read(path).await {
            Ok(data) => {
                let mime = mime_type(path).to_string();
                let mut resp = Response::new(Full::from(data));
                resp.headers_mut().insert(hyper::header::CONTENT_TYPE, mime.parse().unwrap());
                resp.headers_mut().insert(
                    hyper::header::HeaderName::from_static("server"),
                    "ohos-server/1.0".parse().unwrap()
                );

                // ─── ETag（基于修改时间和文件大小的强验证器） ───
                if let Ok(modified) = meta.modified() {
                    let duration = modified.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                    let etag_val = format!("\"{:x}-{:x}\"", duration.as_secs(), meta.len());
                    resp.headers_mut().insert(
                        hyper::header::ETAG,
                        etag_val.parse().unwrap()
                    );
                    // ─── Last-Modified ───
                    let http_date_str = httpdate::fmt_http_date(modified);
                    resp.headers_mut().insert(
                        hyper::header::LAST_MODIFIED,
                        http_date_str.parse().unwrap()
                    );
                }

                if let Some(exp) = expires {
                    resp.headers_mut().insert(
                        hyper::header::CACHE_CONTROL,
                        format!("max-age={}", parse_expires(exp)).parse().unwrap()
                    );
                }
                Ok(resp)
            }
            Err(_) => Ok(error_response(500, "Internal Server Error")),
        }
    }

    /// 目录列表
    async fn list_directory(&self, dir_path: &Path) -> Result<Response<ResponseBody>, hyper::Error> {
        let mut entries = Vec::new();
        if let Ok(mut read_dir) = fs::read_dir(dir_path).await {
            while let Ok(Some(entry)) = read_dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                entries.push((name, is_dir));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut html = String::new();
        html.push_str("<!DOCTYPE html><html><head><meta charset='utf-8'><title>Directory Listing</title>");
        html.push_str("<style>body{font-family:sans-serif;margin:40px auto;max-width:800px}");
        html.push_str("a{color:#0366d6;text-decoration:none}a:hover{text-decoration:underline}");
        html.push_str("td{padding:4px 16px}tr:nth-child(even){background:#f6f8fa}</style></head>");
        html.push_str("<body><h1>Directory Listing</h1><hr><table>");
        html.push_str("<tr><th>Name</th><th>Type</th></tr>");
        for (name, is_dir) in entries {
            let icon = if is_dir { "📁" } else { "📄" };
            let suffix = if is_dir { "/" } else { "" };
            html.push_str(&format!("<tr><td>{icon} <a href='./{name}{suffix}'>{name}{suffix}</a></td><td>{}</td></tr>",
                if is_dir { "directory" } else { "file" }));
        }
        html.push_str("</table><hr></body></html>");

        let mut resp = Response::new(Full::from(Bytes::from(html)));
        resp.headers_mut().insert(
            hyper::header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap()
        );
        Ok(resp)
    }

    /// 对文本类响应启用 gzip 压缩（如果客户端支持），返回 (body, 是否压缩)
    fn check_compress_response(&self, req_headers: &HeaderMap, resp_headers: &HeaderMap, body: Bytes) -> (ResponseBody, bool) {
        // 检查客户端是否支持 gzip
        let accepts_gzip = req_headers
            .get("accept-encoding")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("gzip"))
            .unwrap_or(false);

        if !accepts_gzip {
            return (Full::from(body), false);
        }

        // 只压缩文本类内容
        let content_type = resp_headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let compressible = content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("javascript")
            || content_type.contains("xml")
            || content_type.contains("yaml");

        if !compressible {
            return (Full::from(body), false);
        }

        // 执行 gzip 压缩
        use std::io::Write;
        let data_len = body.len();
        if data_len < 256 {
            // 太小不压缩
            return (Full::from(body), false);
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        if encoder.write_all(&body).is_ok() {
            if let Ok(compressed) = encoder.finish() {
                if compressed.len() < data_len {
                    return (Full::from(Bytes::from(compressed)), true);
                }
            }
        }
        (Full::from(body), false)
    }

    /// 处理 WebSocket 代理请求
    /// 对于 proxy_pass 配置的路径，建立到后端的 TCP 隧道
    async fn handle_websocket_proxy(&self, parts: &hyper::http::request::Parts, remote_addr: &str) -> Option<Response<ResponseBody>> {
        // 查找匹配的 location，必须有 proxy_pass
        let uri = parts.uri.clone();
        let request_headers = &parts.headers;
        let path = uri.path();

        let matching_location = {
            let mut locations: Vec<&LocationConfig> = self.config.location.iter().collect();
            locations.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
            locations.into_iter().find(|loc| path.starts_with(&loc.path) && loc.proxy_pass.is_some())
        };

        let loc = matching_location?;
        let proxy_pass = loc.proxy_pass.as_ref()?;

        // 构建后端 URL
        let uri_path = uri.path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        let backend_url = format!("{}{}", proxy_pass.trim_end_matches('/'), uri_path);

        // 解析后端地址
        let backend_uri: http::Uri = backend_url.parse().ok()?;
        let host = backend_uri.host()?;
        let port = backend_uri.port_u16().unwrap_or(80);

        let addr = format!("{}:{}", host, port);

        // 获取 OnUpgrade
        let on_upgrade = parts.extensions.get::<hyper::upgrade::OnUpgrade>()?.clone();

        // 尝试连接后端
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(mut backend_stream) => {
                // 构建转发到后端的 HTTP 请求
                let host_header = backend_uri.host().unwrap_or(host);
                let request_line = format!("GET {} HTTP/1.1\r\n", uri_path);
                let mut req_data = request_line.into_bytes();

                // 添加必要的 WebSocket 头
                for (name, value) in request_headers.iter() {
                    let name_lower = name.as_str().to_lowercase();
                    // WebSocket 代理需要保留 Connection 和 Upgrade 头
                    // 只过滤真正对后端无用的 hop-by-hop 头
                    if name_lower == "transfer-encoding"
                        || name_lower == "proxy-authenticate"
                        || name_lower == "proxy-authorization"
                    {
                        continue;
                    }
                    req_data.extend_from_slice(name.as_str().as_bytes());
                    req_data.extend_from_slice(b": ");
                    req_data.extend_from_slice(value.as_bytes());
                    req_data.extend_from_slice(b"\r\n");
                }

                // 确保 Host 头指向后端
                req_data.extend_from_slice(format!("Host: {}\r\n", host_header).as_bytes());
                req_data.extend_from_slice(b"\r\n");

                // 发送请求到后端
                if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut backend_stream, &req_data).await {
                    log::error!("[handler.rs] WebSocket 代理: 发送请求到后端 {} 失败: {}", addr, e);
                    return None;
                }

                // 读取后端响应的第一行和头部（直到 \r\n\r\n）
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(&mut backend_stream);
                let mut response_line = String::new();
                if reader.read_line(&mut response_line).await.ok()? < 12 {
                    log::error!("[handler.rs] WebSocket 代理: 后端 {} 返回无效响应行", addr);
                    return None;
                }

                // 验证是 101 Switching Protocols
                if !response_line.contains(" 101 ") {
                    log::error!("[handler.rs] WebSocket 代理: 后端 {} 未返回 101 (响应: {})", addr, response_line.trim());
                    return None;
                }

                // 读取后端响应头
                let mut backend_headers = HeaderMap::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.ok()? < 2 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if let Ok(h_name) = http::HeaderName::from_bytes(name.trim().as_bytes()) {
                            let h_value = http::HeaderValue::from_str(value.trim()).ok();
                            if let Some(h_value) = h_value {
                                backend_headers.insert(h_name, h_value);
                            }
                        }
                    }
                }

                // 构建 101 响应返回给客户端
                let mut response = Response::new(Full::from(Bytes::new()));
                *response.status_mut() = hyper::StatusCode::SWITCHING_PROTOCOLS;

                // 复制后端返回的关键 WebSocket 头
                for key in &["upgrade", "connection", "sec-websocket-accept", "sec-websocket-protocol"] {
                    if let Some(val) = backend_headers.get(*key) {
                        response.headers_mut().insert(
                            http::HeaderName::from_static(key),
                            val.clone(),
                        );
                    }
                }

                // 在后端响应返回后，当 hyper 完成升级后桥接两个连接
                let remote = remote_addr.to_string();
                let b_addr = addr.clone();

                tokio::spawn(async move {
                    match on_upgrade.await {
                        Ok(upgraded) => {
                            log::info!("WebSocket 隧道已建立: {} <-> {}", remote, b_addr);

                            // TokioIo 桥接 hyper::rt::Read/Write → tokio::io::AsyncRead/AsyncWrite
                            let upgraded = TokioIo::new(upgraded);
                            let (mut ri, mut wi) = tokio::io::split(backend_stream);
                            let (mut ru, mut wu) = tokio::io::split(upgraded);

                            let client_to_backend = tokio::io::copy(&mut ru, &mut wi);
                            let backend_to_client = tokio::io::copy(&mut ri, &mut wu);

                            tokio::select! {
                                result = client_to_backend => {
                                    if let Err(e) = result {
                                        log::warn!("WebSocket 客户端→后端错误: {}", e);
                                    }
                                }
                                result = backend_to_client => {
                                    if let Err(e) = result {
                                        log::warn!("WebSocket 后端→客户端错误: {}", e);
                                    }
                                }
                            }

                            log::info!("WebSocket 隧道已关闭: {} <-> {}", remote, b_addr);
                        }
                        Err(e) => {
                            log::error!("WebSocket 升级失败: {}", e);
                        }
                    }
                });

                Some(response)
            }
            Err(e) => {
                log::error!("[handler.rs] WebSocket 代理: 连接后端 {} 失败: {}", addr, e);
                None
            }
        }
    }
}

/// 获取 MIME 类型
fn mime_type(path: &Path) -> String {
    from_path(path).first_or_octet_stream().to_string()
}

/// 检查是否为 WebSocket 升级请求
fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let upgrade = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("websocket"))
        .unwrap_or(false);

    let connection = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("upgrade"))
        .unwrap_or(false);

    upgrade && connection
}

/// 创建错误响应
fn error_response(status: u16, message: &str) -> Response<ResponseBody> {
    let body = format!("<!DOCTYPE html><html><head><meta charset='utf-8'><title>{} {}</title></head><body><h1>{} {}</h1><hr><p>ohos-server/1.0</p></body></html>",
        status, message, status, message);
    let mut resp = Response::new(Full::from(Bytes::from(body)));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        "text/html; charset=utf-8".parse().unwrap()
    );
    resp.headers_mut().insert(
        hyper::header::HeaderName::from_static("server"),
        "ohos-server/1.0".parse().unwrap()
    );
    resp
}

/// 规范化路径，防止路径穿越
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // 忽略 .. 防止穿越
            }
            std::path::Component::Normal(c) => {
                normalized.push(c);
            }
            _ => {
                normalized.push(component);
            }
        }
    }

    // 如果路径存在，解析符号链接防止符号链接逃避
    if normalized.exists() {
        if let Ok(canonical) = normalized.canonicalize() {
            return canonical;
        }
    }

    normalized
}

/// 解析过期时间字符串为秒数
fn parse_expires(expires: &str) -> u64 {
    let expires = expires.trim().to_lowercase();
    if expires.ends_with('d') {
        expires.trim_end_matches('d').parse::<u64>().unwrap_or(0) * 86400
    } else if expires.ends_with('h') {
        expires.trim_end_matches('h').parse::<u64>().unwrap_or(0) * 3600
    } else if expires.ends_with('m') {
        expires.trim_end_matches('m').parse::<u64>().unwrap_or(0) * 60
    } else {
        expires.trim_end_matches('s').parse::<u64>().unwrap_or(0)
    }
}

/// 检查请求路径是否匹配禁止访问规则
fn is_path_forbidden(path: &str, forbidden_dirs: &[String], forbidden_files: &[String]) -> bool {
    // 检查目录模式：如 /runtime/* 匹配 /runtime/ 下所有路径
    for dir in forbidden_dirs {
        let dir_trimmed = dir.trim_end_matches('*').trim_end_matches('/');
        if dir_trimmed.is_empty() {
            continue;
        }
        if path == dir_trimmed || path.starts_with(&format!("{}/", dir_trimmed)) {
            return true;
        }
    }
    // 检查文件模式：如 *.toml 匹配所有 .toml 结尾的路径
    for pattern in forbidden_files {
        if let Some(ext) = pattern.strip_prefix('*') {
            if path.ends_with(ext) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_path_forbidden;

    #[test]
    fn test_forbidden_dirs_exact_match() {
        let dirs = vec!["/secret".to_string(), "/admin/*".to_string()];
        let files: Vec<String> = vec![];
        assert!(is_path_forbidden("/secret", &dirs, &files));
        assert!(is_path_forbidden("/secret/", &dirs, &files));
        assert!(is_path_forbidden("/secret/config", &dirs, &files));
    }

    #[test]
    fn test_forbidden_dirs_wildcard() {
        let dirs = vec!["/runtime/*".to_string()];
        let files: Vec<String> = vec![];
        assert!(is_path_forbidden("/runtime/", &dirs, &files));
        assert!(is_path_forbidden("/runtime/data.json", &dirs, &files));
        assert!(is_path_forbidden("/runtime/subdir/file.txt", &dirs, &files));
    }

    #[test]
    fn test_forbidden_dirs_no_match() {
        let dirs = vec!["/private/*".to_string()];
        let files: Vec<String> = vec![];
        assert!(!is_path_forbidden("/public", &dirs, &files));
        assert!(!is_path_forbidden("/public/file.txt", &dirs, &files));
        assert!(!is_path_forbidden("/", &dirs, &files));
    }

    #[test]
    fn test_forbidden_files_wildcard() {
        let dirs: Vec<String> = vec![];
        let files = vec!["*.toml".to_string(), "*.env".to_string()];
        assert!(is_path_forbidden("/config.toml", &dirs, &files));
        assert!(is_path_forbidden("/.env", &dirs, &files));
        assert!(is_path_forbidden("/subdir/app.env", &dirs, &files));
        assert!(!is_path_forbidden("/index.html", &dirs, &files));
    }

    #[test]
    fn test_forbidden_files_no_star_prefix() {
        // 不以 * 开头的模式，strip_prefix 返回 None，应跳过不匹配
        let dirs: Vec<String> = vec![];
        let files = vec![".toml".to_string()];  // 没有 * 前缀
        assert!(!is_path_forbidden("/config.toml", &dirs, &files));
    }

    #[test]
    fn test_forbidden_empty_rules() {
        let dirs: Vec<String> = vec![];
        let files: Vec<String> = vec![];
        assert!(!is_path_forbidden("/any/path.toml", &dirs, &files));
        assert!(!is_path_forbidden("/", &dirs, &files));
    }

    #[test]
    fn test_forbidden_combined_rules() {
        let dirs = vec!["/backup/*".to_string()];
        let files = vec!["*.json".to_string()];
        // 匹配目录规则
        assert!(is_path_forbidden("/backup/db.sql", &dirs, &files));
        // 匹配文件规则
        assert!(is_path_forbidden("/data/config.json", &dirs, &files));
        // 不匹配
        assert!(!is_path_forbidden("/public/index.html", &dirs, &files));
    }

    // ─── URL 重写集成测试 ──────────────────────────────────────

    use crate::config::{RewriteRule, ServerConfig};
    use super::RequestHandler;
    use hyper::{Method, HeaderMap};
    use bytes::Bytes;

    /// 创建最小测试配置
    fn test_config(root: &str, rewrite_rules: Vec<RewriteRule>) -> ServerConfig {
        // 从 TOML 反序列化创建配置，避免逐字段构造
        let rewrite_toml: String = rewrite_rules.iter().map(|r| {
            format!(r#"[[server.rewrite]]
from = "{}"
to = "{}"
"#, r.from.replace('\\', "\\\\"), r.to)
        }).collect();

        let toml_str = format!(r#"
[[server]]
bind = "0.0.0.0:0"
root = "{}"
{}
"#, root, rewrite_toml);

        let mut cfg: crate::config::AppConfig = toml::from_str(&toml_str).unwrap();
        cfg.server.remove(0)
    }

    #[tokio::test]
    async fn test_rewrite_skip_existing_file() {
        let rules = vec![RewriteRule {
            from: "^/(.*)$".to_string(),
            to: "/index.php/$1".to_string(),
            regex: true,
        }];
        let mut config = test_config("./www", rules);
        config.finalize();

        let handler = RequestHandler::new(config);

        // Test 1: 请求 /index.html（真实存在的文件），应不重写，直接返回 200
        let uri: hyper::Uri = "http://localhost/index.html".parse().unwrap();
        let headers = HeaderMap::new();
        let result = handler.handle_internal(&Method::GET, &uri, &headers, Bytes::new(), "127.0.0.1:12345").await;
        assert!(result.is_ok(), "处理 /index.html 应成功");
        let response = result.unwrap();
        assert_eq!(response.status(), 200, "已存在的文件应返回 200");

        // Test 2: 请求 /nonexistent（不存在的路径），应被重写为 /index.php/nonexistent
        // 由于没有 PHP CGI，最终返回 404
        let uri: hyper::Uri = "http://localhost/nonexistent".parse().unwrap();
        let result = handler.handle_internal(&Method::GET, &uri, &headers, Bytes::new(), "127.0.0.1:12345").await;
        assert!(result.is_ok(), "处理 /nonexistent 应成功（返回 404）");
        let response = result.unwrap();
        assert_eq!(response.status(), 404, "不存在的路径应返回 404");

        // Test 3: 请求 / 根路径，应被重写为 /index.php/
        let uri: hyper::Uri = "http://localhost/".parse().unwrap();
        let result = handler.handle_internal(&Method::GET, &uri, &headers, Bytes::new(), "127.0.0.1:12345").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rewrite_no_rewrite_no_match() {
        let rules = vec![RewriteRule {
            from: "^/api/(.*)$".to_string(),
            to: "/api_handler.php?r=$1".to_string(),
            regex: true,
        }];
        let mut config = test_config("./www", rules);
        config.finalize();

        let handler = RequestHandler::new(config);
        let headers = HeaderMap::new();

        // /index.html 不匹配任何重写规则 → 直接服务文件
        let uri: hyper::Uri = "http://localhost/index.html".parse().unwrap();
        let result = handler.handle_internal(&Method::GET, &uri, &headers, Bytes::new(), "127.0.0.1:12345").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        // /api/users 匹配重写规则 → 重写为 /api_handler.php?r=users
        let uri: hyper::Uri = "http://localhost/api/users".parse().unwrap();
        let result = handler.handle_internal(&Method::GET, &uri, &headers, Bytes::new(), "127.0.0.1:12345").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 404);
    }
}
