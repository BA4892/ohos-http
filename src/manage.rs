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
//! | 方法   | 路径              | 说明                     |
//! |--------|-------------------|--------------------------|
//! | GET    | /_ohos/config     | 获取完整服务器配置        |
//! | PUT    | /_ohos/config     | 更新配置（写入文件+热重载）|
//! | POST   | /_ohos/start      | 恢复服务（取消暂停）      |
//! | POST   | /_ohos/pause      | 暂停服务（返回 503）      |
//! | POST   | /_ohos/stop       | 停止服务器                |
//! | POST   | /_ohos/restart    | 热重启服务器              |
//! | GET    | /_ohos/status     | 获取服务器状态            |
//! | GET    | /_ohos/metrics    | 获取运行时指标            |

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock};
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
                Ok(_new_cfg) => {
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

    // ──────── GET /_ohos/status ────────

    /// 获取服务器状态
    async fn handle_status(&self) -> Response<Full<Bytes>> {
        let uptime = worker_start_time().elapsed();
        let uptime_secs = uptime.as_secs();

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
                },
                "config": {
                    "config_path": self.app_config.config_path,
                    "server_count": self.app_config.server.len(),
                },
                "sites": self.app_config.server.iter().map(|s| json!({
                    "bind": s.bind,
                    "root": s.root,
                    "domains": s.domains,
                    "https": s.cert.is_some(),
                    "workers": s.workers,
                })).collect::<Vec<_>>(),
            }
        });

        json_response(StatusCode::OK, &status)
    }

    // ──────── GET /_ohos/metrics ────────

    /// 获取运行时指标
    async fn handle_metrics(&self) -> Response<Full<Bytes>> {
        let uptime = worker_start_time().elapsed();
        let uptime_secs = uptime.as_secs();
        let total_requests = GLOBAL_REQUESTS.load(Ordering::Relaxed);

        let metrics = json!({
            "code": 0,
            "message": "ok",
            "data": {
                "requests": {
                    "total": total_requests,
                    "per_second": if uptime_secs > 0 { total_requests as f64 / uptime_secs as f64 } else { 0.0 },
                },
                "uptime": {
                    "seconds": uptime_secs,
                    "human": format_human_duration(uptime_secs),
                },
                "process": {
                    "pid": std::process::id(),
                    "worker_index": 0,
                },
                // 这些是占位符，生产环境中可通过更细粒度的统计扩展
                "memory": {
                    "note": "Memory stats available via OS tools (e.g., /proc/self/status on Linux)"
                }
            }
        });

        json_response(StatusCode::OK, &metrics)
    }

    // ──────── 内部工具 ────────

    /// 发送 SIGHUP 信号给父进程触发热重载
    fn send_sighup() {
        let ppid = unsafe { libc::getppid() };
        if ppid > 1 {
            log::info!("Management API: sending SIGHUP to parent PID {}", ppid);
            unsafe { libc::kill(ppid, libc::SIGHUP); }
        } else {
            unsafe { libc::raise(libc::SIGHUP); }
        }
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

/// 增加全局请求计数器
pub fn inc_requests() {
    GLOBAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
}
