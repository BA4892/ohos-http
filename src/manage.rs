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

//! 管理 API 模块 — 为鸿蒙 ArkTS 提供 RESTful 管理接口
//!
//! 所有接口前缀为 `/_ohos/`，返回 JSON 格式。
//!
//! # 接口列表
//!
//! | 方法   | 路径                        | 说明                           |
//! |--------|-----------------------------|--------------------------------|
//! | GET    | /_ohos/config               | 获取完整服务器配置              |
//! | PUT    | /_ohos/config               | 更新配置（写入文件+热重载）     |
//! | POST   | /_ohos/start                | 恢复服务（取消全局暂停）        |
//! | POST   | /_ohos/pause                | 暂停服务（返回 503）            |
//! | POST   | /_ohos/stop                 | 停止服务器                      |
//! | POST   | /_ohos/restart              | 热重启服务器                    |
//! | GET    | /_ohos/status               | 获取服务器状态 + 站点列表       |
//! | GET    | /_ohos/metrics              | 获取运行时指标（含内存/CPU）    |
//! | GET    | /_ohos/sites                | 获取每个站点的状态和指标        |
//! | POST   | /_ohos/sites/{index}/pause  | 暂停指定站点                    |
//! | POST   | /_ohos/sites/{index}/start  | 恢复指定站点                    |
//! | POST   | /_ohos/sites/batch/pause    | 批量暂停（body: {"indices":[]}）|
//! | POST   | /_ohos/sites/batch/start    | 批量恢复（body: {"indices":[]}）|

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bytes::Bytes;
use http::Method;
use http_body_util::Full;
use hyper::{HeaderMap, Response, StatusCode};
use serde_json::json;

use crate::config::AppConfig;

// ============================================================================
//  全局原子标志 — 跨 Worker 控制
// ============================================================================

/// 暂停标志：true = 暂停，非管理请求返回 503
pub static PAUSED: AtomicBool = AtomicBool::new(false);

/// 全局请求计数器
pub static GLOBAL_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// 实时请求率追踪（5秒滑动窗口）
static REQUEST_TIMESTAMPS: OnceLock<Mutex<VecDeque<Instant>>> = OnceLock::new();

// ─── 每站点追踪（在 main() 中 init） ───

/// 每站点暂停标志（索引与 config.server 列表对齐）
static SITE_PAUSED: OnceLock<Vec<AtomicBool>> = OnceLock::new();

/// 每站点请求计数器
static SITE_REQUESTS: OnceLock<Vec<AtomicU64>> = OnceLock::new();

/// 初始化站点追踪数组（在 fork 前调用一次）
pub fn init_site_tracking(count: usize) {
    let paused: Vec<AtomicBool> = (0..count).map(|_| AtomicBool::new(false)).collect();
    let requests: Vec<AtomicU64> = (0..count).map(|_| AtomicU64::new(0)).collect();
    let _ = SITE_PAUSED.set(paused);
    let _ = SITE_REQUESTS.set(requests);
}

/// 获取站点暂停状态
pub fn is_site_paused(index: usize) -> bool {
    SITE_PAUSED.get()
        .and_then(|vec| vec.get(index))
        .map(|a| a.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// 反转站点暂停状态
pub fn toggle_site_pause(index: usize, paused: bool) {
    if let Some(vec) = SITE_PAUSED.get() {
        if let Some(atom) = vec.get(index) {
            atom.store(paused, Ordering::SeqCst);
        }
    }
}

/// 增加指定站点的请求计数
pub fn inc_site_requests(index: usize) {
    if let Some(vec) = SITE_REQUESTS.get() {
        if let Some(atom) = vec.get(index) {
            atom.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 获取指定站点请求计数
pub fn get_site_requests(index: usize) -> u64 {
    SITE_REQUESTS.get()
        .and_then(|vec| vec.get(index))
        .map(|a| a.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// 获取所有站点的请求计数
pub fn all_site_requests() -> Vec<u64> {
    SITE_REQUESTS.get()
        .map(|vec| vec.iter().map(|a| a.load(Ordering::Relaxed)).collect())
        .unwrap_or_default()
}

/// 获取所有站点暂停状态
pub fn all_site_paused() -> Vec<bool> {
    SITE_PAUSED.get()
        .map(|vec| vec.iter().map(|a| a.load(Ordering::SeqCst)).collect())
        .unwrap_or_default()
}

/// 启动时间戳（Worker 启动时记录）
pub fn worker_start_time() -> Instant {
    static START_TIME: OnceLock<Instant> = OnceLock::new();
    *START_TIME.get_or_init(Instant::now)
}

/// 管理 API 认证 Token（通过 --manage-auth 设置）
pub static MANAGE_AUTH_TOKEN: OnceLock<String> = OnceLock::new();

/// 设置管理 API 认证 Token（在 fork 前调用一次）
pub fn set_manage_auth_token(token: &str) {
    let _ = MANAGE_AUTH_TOKEN.set(token.to_string());
}

/// 检查管理 API 是否启用
#[allow(dead_code)]
pub fn is_manage_enabled() -> bool {
    MANAGE_AUTH_TOKEN.get().is_some()
}

// ============================================================================
//  进程资源信息（Linux /proc 接口）
// ============================================================================

/// 获取当前进程内存占用（RSS, KB）
pub fn get_memory_kb() -> u64 {
    // 从 /proc/self/status 中读取 VmRSS
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(val) = line.strip_prefix("VmRSS:") {
            let kb: u64 = val.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            return kb;
        }
    }
    0
}

/// 获取当前进程 CPU 使用率（百分比，相对于单核）
/// 返回 (user_cpu_percent, system_cpu_percent)
pub fn get_cpu_percent() -> (f64, f64) {
    // 从 /proc/self/stat 读取 utime, stime（clock ticks）
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let parts: Vec<&str> = stat.split_whitespace().collect();
    if parts.len() < 15 {
        return (0.0, 0.0);
    }
    let utime: u64 = parts[13].parse().unwrap_or(0);
    let stime: u64 = parts[14].parse().unwrap_or(0);
    let hertz: f64 = 100.0; // 常见的 CLK_TCK
    let uptime_secs = worker_start_time().elapsed().as_secs_f64();
    if uptime_secs <= 0.0 {
        return (0.0, 0.0);
    }
    // CPU 百分比的估算：ticks / hertz / 运行秒数 * 100
    let user_pct = (utime as f64 / hertz) / uptime_secs * 100.0;
    let sys_pct = (stime as f64 / hertz) / uptime_secs * 100.0;
    (user_pct.min(100.0), sys_pct.min(100.0))
}

// ============================================================================
//  管理 API 处理器
// ============================================================================

/// 管理 API 处理器
#[derive(Clone)]
pub struct ManageHandler {
    app_config: AppConfig,
    /// 认证 Token（空=不需要认证）
    auth_token: String,
}

impl ManageHandler {
    /// 创建管理 API 处理器
    pub fn new(app_config: &AppConfig, auth_token: &str) -> Self {
        ManageHandler {
            app_config: app_config.clone(),
            auth_token: auth_token.to_string(),
        }
    }

    /// 入口：处理管理请求
    pub async fn handle_request(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        body_bytes: Bytes,
    ) -> Response<Full<Bytes>> {
        // 验证认证 Token
        if !self.auth_token.is_empty() && !self.verify_auth(headers) {
            return json_response(StatusCode::UNAUTHORIZED, &json!({
                "code": 401,
                "message": "Unauthorized: invalid or missing auth token",
                "error": "AUTH_FAILED"
            }));
        }

        // 路由分发
        let response = match (method, path) {
            // ─── 配置管理 ───
            (&Method::GET, "/_ohos/config") => self.handle_get_config().await,
            (&Method::PUT, "/_ohos/config") => {
                self.handle_put_config(body_bytes).await
            }

            // ─── 生命周期管理 ───
            (&Method::POST, "/_ohos/start") => self.handle_start().await,
            (&Method::POST, "/_ohos/pause") => self.handle_pause().await,
            (&Method::POST, "/_ohos/stop") => self.handle_stop().await,
            (&Method::POST, "/_ohos/restart") => self.handle_restart().await,

            // ─── 状态与指标 ───
            (&Method::GET, "/_ohos/status") => self.handle_status().await,
            (&Method::GET, "/_ohos/metrics") => self.handle_metrics().await,

            // ─── 站点管理 ───
            (&Method::GET, "/_ohos/sites") => self.handle_list_sites().await,

            // ─── 批量操作 ───
            (&Method::POST, "/_ohos/sites/batch/pause") => {
                self.handle_batch_pause(body_bytes).await
            }
            (&Method::POST, "/_ohos/sites/batch/start") => {
                self.handle_batch_start(body_bytes).await
            }

            // ─── 单站点操作（路径包含动态索引） ───
            _ if path.starts_with("/_ohos/sites/") && *method == Method::POST => {
                self.handle_site_action(path, body_bytes).await
            }

            // ─── 未知路由 ───
            _ => {
                if path.starts_with("/_ohos/") {
                    json_response(StatusCode::NOT_FOUND, &json!({
                        "code": 404,
                        "message": format!("Unknown management endpoint: {}", path),
                        "error": "NOT_FOUND"
                    }))
                } else {
                    // 不是管理路径，由主处理器处理
                    return json_response(StatusCode::NOT_FOUND, &json!({
                        "code": 404,
                        "message": "Not a management endpoint",
                        "error": "NOT_FOUND"
                    }));
                }
            }
        };

        response
    }

    // ──────── 认证 ────────

    /// 验证请求认证
    fn verify_auth(&self, headers: &HeaderMap) -> bool {
        if let Some(auth_header) = headers.get("Authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                // 支持 Bearer token
                if let Some(token) = auth_str.strip_prefix("Bearer ") {
                    return token == self.auth_token;
                }
                // 也支持直接使用 token 值
                return auth_str == self.auth_token;
            }
        }
        false
    }

    // ──────── GET /_ohos/config ────────

    /// 获取完整配置
    async fn handle_get_config(&self) -> Response<Full<Bytes>> {
        match serde_json::to_value(&self.app_config) {
            Ok(config_json) => {
                let response = json!({
                    "code": 0,
                    "message": "ok",
                    "data": {
                        "config": config_json,
                        "config_path": self.app_config.config_path,
                        "server_count": self.app_config.server.len(),
                    }
                });
                json_response(StatusCode::OK, &response)
            }
            Err(e) => {
                json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({
                    "code": 500,
                    "message": format!("Failed to serialize config: {}", e),
                    "error": "SERIALIZE_ERROR"
                }))
            }
        }
    }

    // ──────── PUT /_ohos/config ────────

    /// 更新配置
    async fn handle_put_config(&self, body_bytes: Bytes) -> Response<Full<Bytes>> {
        let body_str = match String::from_utf8(body_bytes.to_vec()) {
            Ok(s) => s,
            Err(_) => {
                return json_response(StatusCode::BAD_REQUEST, &json!({
                    "code": 400,
                    "message": "Invalid UTF-8 in request body",
                    "error": "INVALID_UTF8"
                }));
            }
        };

        // 解析请求体 — 支持 JSON 和 TOML 格式
        // 先尝试 JSON
        let config_path = &self.app_config.config_path;
        if config_path.is_empty() {
            return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400,
                "message": "No config file path available. Server was started from CLI args.",
                "error": "NO_CONFIG_FILE"
            }));
        }

        // 确定要写入的格式（与现有配置文件格式保持一致）
        let is_toml = config_path.ends_with(".toml");

        // 尝试解析并写入
        if is_toml {
            // TOML 格式 — 验证后写入文件
            match toml::from_str::<AppConfig>(&body_str) {
                Ok(ref new_cfg) => {
                    // 验证配置内容的安全性
                    if let Err(e) = validate_app_config(new_cfg) {
                        return json_response(StatusCode::BAD_REQUEST, &json!({
                            "code": 400,
                            "message": e,
                            "error": "VALIDATION_ERROR"
                        }));
                    }
                    // 写入配置文件
                    if let Err(e) = std::fs::write(config_path, &body_str) {
                        return json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({
                            "code": 500,
                            "message": format!("Failed to write config file: {}", e),
                            "error": "WRITE_ERROR"
                        }));
                    }
                    // 发送 SIGHUP 触发热重载
                    Self::send_sighup();
                    json_response(StatusCode::OK, &json!({
                        "code": 0,
                        "message": "Config updated and server reload triggered",
                        "data": {
                            "config_path": config_path,
                            "format": "toml"
                        }
                    }))
                }
                Err(e) => {
                    json_response(StatusCode::BAD_REQUEST, &json!({
                        "code": 400,
                        "message": format!("Invalid TOML config: {}", e),
                        "error": "PARSE_ERROR",
                        "details": e.to_string()
                    }))
                }
            }
        } else {
            // JSON 格式 — 直接写入文件（配置文件格式保持 TOML）
            // 如果传入的是 JSON 格式，我们尝试将其序列化为 TOML 字符串写入
            // 用户也可以直接发送 TOML 格式的内容
            match toml::from_str::<toml::Value>(&body_str) {
                Ok(_toml_val) => {
                    // 验证通过，直接写入
                    if let Err(e) = std::fs::write(config_path, &body_str) {
                        return json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({
                            "code": 500,
                            "message": format!("Failed to write config file: {}", e),
                            "error": "WRITE_ERROR"
                        }));
                    }
                    // 发送 SIGHUP 触发热重载
                    Self::send_sighup();
                    json_response(StatusCode::OK, &json!({
                        "code": 0,
                        "message": "Config updated and server reload triggered",
                        "data": {
                            "config_path": config_path,
                            "format": "toml"
                        }
                    }))
                }
                Err(_toml_err) => {
                    // 尝试作为 JSON 解析
                    match serde_json::from_str::<serde_json::Value>(&body_str) {
                        Ok(json_val) => {
                            // 尝试将 JSON 转换为 TOML 字符串
                            let toml_str = toml::to_string(&json_val)
                                .map_err(|e| format!("JSON to TOML conversion error: {}", e));
                            match toml_str {
                                Ok(toml_content) => {
                                    if let Err(e) = std::fs::write(config_path, &toml_content) {
                                        return json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({
                                            "code": 500,
                                            "message": format!("Failed to write config file: {}", e),
                                            "error": "WRITE_ERROR"
                                        }));
                                    }
                                    Self::send_sighup();
                                    json_response(StatusCode::OK, &json!({
                                        "code": 0,
                                        "message": "Config updated (JSON converted to TOML) and server reload triggered",
                                        "data": {
                                            "config_path": config_path,
                                            "format": "toml",
                                            "original_format": "json"
                                        }
                                    }))
                                }
                                Err(e) => {
                                    json_response(StatusCode::BAD_REQUEST, &json!({
                                        "code": 400,
                                        "message": format!("Cannot convert JSON to TOML config: {}", e),
                                        "error": "CONVERSION_ERROR"
                                    }))
                                }
                            }
                        }
                        Err(e) => {
                            json_response(StatusCode::BAD_REQUEST, &json!({
                                "code": 400,
                                "message": format!("Invalid config format (neither valid TOML nor JSON): {}", e),
                                "error": "PARSE_ERROR"
                            }))
                        }
                    }
                }
            }
        }
    }

    // ──────── POST /_ohos/start ────────

    /// 恢复服务（取消暂停）
    async fn handle_start(&self) -> Response<Full<Bytes>> {
        PAUSED.store(false, Ordering::SeqCst);
        json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": "Server resumed accepting requests",
            "data": {
                "paused": false
            }
        }))
    }

    // ──────── POST /_ohos/pause ────────

    /// 暂停服务
    async fn handle_pause(&self) -> Response<Full<Bytes>> {
        PAUSED.store(true, Ordering::SeqCst);
        json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": "Server paused (new requests will receive 503)",
            "data": {
                "paused": true
            }
        }))
    }

    // ──────── POST /_ohos/stop ────────

    /// 停止服务器
    async fn handle_stop(&self) -> Response<Full<Bytes>> {
        // 先把响应发送回去，再停止
        // 使用 tokio::spawn 延迟执行，确保响应先到达客户端
        let response = json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": "Server is shutting down gracefully"
        }));

        // 延迟发送 SIGTERM 到父进程
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // 发送 SIGTERM 到当前进程组（通知 Master 停止所有 Worker）
            let ppid = unsafe { libc::getppid() };
            if ppid > 1 {
                log::info!("Management API: sending SIGTERM to parent PID {}", ppid);
                unsafe { libc::kill(ppid, libc::SIGTERM); }
            } else {
                // 如果没有父进程（比如直接运行），也给自己发信号
                unsafe { libc::kill(0, libc::SIGTERM); }
            }
        });

        response
    }

    // ──────── POST /_ohos/restart ────────

    /// 热重启服务器
    async fn handle_restart(&self) -> Response<Full<Bytes>> {
        let response = json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": "Server is reloading config and restarting workers"
        }));

        // 延迟发送 SIGHUP 到父进程
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // 发送 SIGHUP 到父进程（通知 Master 热重启）
            let ppid = unsafe { libc::getppid() };
            if ppid > 1 {
                log::info!("Management API: sending SIGHUP to parent PID {} for reload", ppid);
                unsafe { libc::kill(ppid, libc::SIGHUP); }
            } else {
                // 如果没有父进程，直接触发 SIGHUP
                unsafe { libc::raise(libc::SIGHUP); }
            }
        });

        response
    }

    // ──────── 内部工具 ────────

    /// 发送 SIGHUP 信号给父进程触发热重载
    fn send_sighup() {
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let ppid = unsafe { libc::getppid() };
            if ppid > 1 {
                log::info!("Management API: sending SIGHUP to parent PID {}", ppid);
                unsafe { libc::kill(ppid, libc::SIGHUP); }
            } else {
                // 如果没有父进程，直接触发 SIGHUP
                unsafe { libc::raise(libc::SIGHUP); }
            }
        });
    }

    // ──────── GET /_ohos/status ────────

    /// 获取服务器状态（含每站点状态和请求数）
    async fn handle_status(&self) -> Response<Full<Bytes>> {
        let uptime = worker_start_time().elapsed();
        let uptime_secs = uptime.as_secs();
        let site_paused_vec = all_site_paused();
        let site_requests_vec = all_site_requests();

        let status = json!({
            "code": 0,
            "message": "ok",
            "data": {
                "server": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "name": env!("CARGO_PKG_NAME"),
                    "description": env!("CARGO_PKG_DESCRIPTION"),
                },
                "status": {
                    "paused": PAUSED.load(Ordering::SeqCst),
                    "uptime_secs": uptime_secs,
                    "uptime_human": format_human_duration(uptime_secs),
                    "pid": std::process::id(),
                    "ppid": unsafe { libc::getppid() },
                    "worker_memory_kb": get_memory_kb(),
                },
                "config": {
                    "config_path": self.app_config.config_path,
                    "server_count": self.app_config.server.len(),
                },
                "sites": self.app_config.server.iter().enumerate().map(|(i, s)| json!({
                    "index": i,
                    "bind": s.bind,
                    "root": s.root,
                    "domains": s.domains,
                    "https": s.cert.is_some(),
                    "workers": s.workers,
                    "paused": site_paused_vec.get(i).copied().unwrap_or(false),
                    "requests": site_requests_vec.get(i).copied().unwrap_or(0),
                })).collect::<Vec<_>>(),
            }
        });

        json_response(StatusCode::OK, &status)
    }

    // ──────── GET /_ohos/metrics ────────

    /// 获取运行时指标（含内存、CPU、每站点请求数）
    async fn handle_metrics(&self) -> Response<Full<Bytes>> {
        let uptime = worker_start_time().elapsed();
        let uptime_secs = uptime.as_secs();
        let total_requests = GLOBAL_REQUESTS.load(Ordering::Relaxed);
        let (user_cpu, sys_cpu) = get_cpu_percent();
        let memory_kb = get_memory_kb();
        let site_requests_vec = all_site_requests();

        let metrics = json!({
            "code": 0,
            "message": "ok",
            "data": {
                "requests": {
                    "total": total_requests,
                    "per_second": if uptime_secs > 0 { total_requests as f64 / uptime_secs as f64 } else { 0.0 },
                    "recent_per_second": get_recent_rps(),
                    "sites": self.app_config.server.iter().enumerate().map(|(i, s)| json!({
                        "index": i,
                        "bind": s.bind,
                        "root": s.root,
                        "requests": site_requests_vec.get(i).copied().unwrap_or(0),
                    })).collect::<Vec<_>>(),
                },
                "uptime": {
                    "seconds": uptime_secs,
                    "human": format_human_duration(uptime_secs),
                },
                "process": {
                    "pid": std::process::id(),
                    "worker_index": 0,
                },
                "memory": {
                    "rss_kb": memory_kb,
                    "rss_mb": format!("{:.1} MB", memory_kb as f64 / 1024.0),
                },
                "cpu": {
                    "user_percent": (user_cpu * 100.0).round() / 100.0,
                    "system_percent": (sys_cpu * 100.0).round() / 100.0,
                }
            }
        });

        json_response(StatusCode::OK, &metrics)
    }

    // ──────── GET /_ohos/sites ────────

    /// 列出所有站点及其状态
    async fn handle_list_sites(&self) -> Response<Full<Bytes>> {
        let site_paused_vec = all_site_paused();
        let site_requests_vec = all_site_requests();
        let (user_cpu, sys_cpu) = get_cpu_percent();

        let sites: Vec<_> = self.app_config.server.iter().enumerate().map(|(i, s)| json!({
            "index": i,
            "bind": s.bind,
            "root": s.root,
            "domains": s.domains,
            "https": s.cert.is_some(),
            "workers": s.workers,
            "paused": site_paused_vec.get(i).copied().unwrap_or(false),
            "requests": site_requests_vec.get(i).copied().unwrap_or(0),
            "memory_kb": get_memory_kb(),
            "cpu_user": user_cpu,
            "cpu_system": sys_cpu,
        })).collect();

        json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": "ok",
            "data": {
                "sites": sites,
                "server_count": self.app_config.server.len(),
            }
        }))
    }

    // ──────── POST /_ohos/sites/{index}/pause — 暂停单站点 ────────

    /// 处理单站点操作（暂停/恢复）
    async fn handle_site_action(&self, path: &str, _body: Bytes) -> Response<Full<Bytes>> {
        // 路径格式: /_ohos/sites/{index}/{action}
        let parts: Vec<&str> = path.split('/').collect();
        // parts = ["", "_ohos", "sites", "{index}", "{action}"]
        if parts.len() < 5 {
            return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400,
                "message": "Invalid site action path",
                "error": "INVALID_PATH"
            }));
        }

        let index: usize = match parts[3].parse() {
            Ok(i) => i,
            Err(_) => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400,
                "message": format!("Invalid site index: '{}'", parts[3]),
                "error": "INVALID_INDEX"
            })),
        };

        // 检查索引是否有效
        if index >= self.app_config.server.len() {
            return json_response(StatusCode::NOT_FOUND, &json!({
                "code": 404,
                "message": format!("Site index {} out of range (max: {})", index, self.app_config.server.len() - 1),
                "error": "INDEX_OUT_OF_RANGE"
            }));
        }

        let action = parts[4];
        match action {
            "pause" => {
                toggle_site_pause(index, true);
                json_response(StatusCode::OK, &json!({
                    "code": 0,
                    "message": format!("Site #{} paused", index),
                    "data": { "index": index, "paused": true }
                }))
            }
            "start" => {
                toggle_site_pause(index, false);
                json_response(StatusCode::OK, &json!({
                    "code": 0,
                    "message": format!("Site #{} started", index),
                    "data": { "index": index, "paused": false }
                }))
            }
            _ => json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400,
                "message": format!("Unknown action '{}'. Supported: pause, start", action),
                "error": "UNKNOWN_ACTION"
            })),
        }
    }

    // ──────── POST /_ohos/sites/batch/pause ────────

    /// 批量暂停站点
    async fn handle_batch_pause(&self, body_bytes: Bytes) -> Response<Full<Bytes>> {
        let body_str = match String::from_utf8(body_bytes.to_vec()) {
            Ok(s) => s,
            Err(_) => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Invalid UTF-8", "error": "INVALID_UTF8"
            })),
        };

        let parsed: serde_json::Value = match serde_json::from_str(&body_str) {
            Ok(v) => v,
            Err(_) => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Invalid JSON", "error": "INVALID_JSON"
            })),
        };

        let indices: Vec<usize> = match parsed["indices"].as_array() {
            Some(arr) => arr.iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .filter(|&i| i < self.app_config.server.len())
                .collect(),
            None => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Missing or invalid 'indices' field", "error": "MISSING_INDICES"
            })),
        };

        let mut paused_count = 0;
        for &idx in &indices {
            toggle_site_pause(idx, true);
            paused_count += 1;
        }

        json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": format!("Paused {} sites", paused_count),
            "data": { "paused_indices": indices, "count": paused_count }
        }))
    }

    // ──────── POST /_ohos/sites/batch/start ────────

    /// 批量恢复站点
    async fn handle_batch_start(&self, body_bytes: Bytes) -> Response<Full<Bytes>> {
        let body_str = match String::from_utf8(body_bytes.to_vec()) {
            Ok(s) => s,
            Err(_) => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Invalid UTF-8", "error": "INVALID_UTF8"
            })),
        };

        let parsed: serde_json::Value = match serde_json::from_str(&body_str) {
            Ok(v) => v,
            Err(_) => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Invalid JSON", "error": "INVALID_JSON"
            })),
        };

        let indices: Vec<usize> = match parsed["indices"].as_array() {
            Some(arr) => arr.iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .filter(|&i| i < self.app_config.server.len())
                .collect(),
            None => return json_response(StatusCode::BAD_REQUEST, &json!({
                "code": 400, "message": "Missing or invalid 'indices' field", "error": "MISSING_INDICES"
            })),
        };

        let mut started_count = 0;
        for &idx in &indices {
            toggle_site_pause(idx, false);
            started_count += 1;
        }

        json_response(StatusCode::OK, &json!({
            "code": 0,
            "message": format!("Started {} sites", started_count),
            "data": { "started_indices": indices, "count": started_count }
        }))
    }
}

// ============================================================================
//  工具函数
// ============================================================================

/// 构建 JSON 响应
fn json_response(status: StatusCode, value: &serde_json::Value) -> Response<Full<Bytes>> {
    let body_str = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let body = Full::from(Bytes::from(body_str));

    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
        .header("Access-Control-Allow-Headers", "Content-Type, Authorization")
        .body(body)
        .unwrap_or_else(|_| {
            Response::new(Full::from(Bytes::from("{}")))
        })
}

/// 格式化人类可读的持续时间
fn format_human_duration(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// 检查路径是否为管理 API 端点
#[allow(dead_code)]
pub fn is_manage_path(path: &str) -> bool {
    path.starts_with("/_ohos/")
}

/// 增加全局请求计数器 + 记录时间戳用于实时 QPS
pub fn inc_requests() {
    GLOBAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let tracker = REQUEST_TIMESTAMPS.get_or_init(|| Mutex::new(VecDeque::with_capacity(5000)));
    if let Ok(mut ts) = tracker.lock() {
        ts.push_back(Instant::now());
        let cutoff = Instant::now() - std::time::Duration::from_secs(5);
        while ts.front().map_or(false, |t| *t < cutoff) {
            ts.pop_front();
        }
    }
}

/// 获取最近5秒内的平均请求率 (req/s)
pub fn get_recent_rps() -> f64 {
    if let Some(tracker) = REQUEST_TIMESTAMPS.get() {
        if let Ok(ts) = tracker.lock() {
            let count = ts.len();
            if count > 0 {
                return count as f64 / 5.0;
            }
        }
    }
    0.0
}

/// 验证 AppConfig 配置内容的安全性
/// 在通过管理 API 写入配置时调用，防止危险配置
fn validate_app_config(cfg: &AppConfig) -> Result<(), String> {
    for (i, srv) in cfg.server.iter().enumerate() {
        // 根目录不能为空
        if srv.root.trim().is_empty() {
            return Err(format!("server[{}].root 不能为空", i));
        }

        // 检查 proxy_pass 是否指向内网地址
        for (j, loc) in srv.location.iter().enumerate() {
            if let Some(proxy_pass) = &loc.proxy_pass {
                // 检查是否为内网地址格式
                let lower = proxy_pass.to_lowercase();
                if lower.contains("://127.0.0.") || lower.contains("://localhost") ||
                   lower.contains("://10.") || lower.contains("://172.16.") ||
                   lower.contains("://192.168.") || lower.contains("://[::1]") {
                    return Err(format!(
                        "server[{}].location[{}].proxy_pass 不能指向内网地址: {}",
                        i, j, proxy_pass
                    ));
                }
            }

            // 检查 load_balance_targets 是否指向内网
            for (k, target) in loc.load_balance_targets.iter().enumerate() {
                let lower = target.url.to_lowercase();
                if lower.contains("://127.0.0.") || lower.contains("://localhost") ||
                   lower.contains("://10.") || lower.contains("://172.16.") ||
                   lower.contains("://192.168.") || lower.contains("://[::1]") {
                    return Err(format!(
                        "server[{}].location[{}].load_balance_targets[{}] 不能指向内网地址: {}",
                        i, j, k, target.url
                    ));
                }
            }
        }
    }
    Ok(())
}
