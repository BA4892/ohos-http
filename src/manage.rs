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

//! WEB 管理端服务器
//!
//! 提供可视化服务器管理界面，包括：
//! - 系统状态监控（CPU、内存、运行时间）
//! - 站点管理（新增、编辑、删除、启动、停止）
//! - 日志查看
//!
//! ## 启动方式
//!
//! ```bash
//! ohos-server start manage                      # 默认 0.0.0.0:随机端口
//! ohos-server start manage -a 0.0.0.0:1314      # 指定端口
//! ohos-server start manage -d                   # 守护进程模式
//! ohos-server start manage -a 0.0.0.0:1314 -d   # 指定端口 + 守护进程
//! ```

use std::collections::HashMap;
use std::convert::Infallible;
use std::io::BufRead;
use std::net::TcpListener;
use std::path::PathBuf;
use std::io::Read;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bytes::Bytes;
use chrono::Local;
use http::header::{CONTENT_TYPE, SET_COOKIE};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener as TokioTcpListener;

use crate::auth;
use crate::config::AppConfig;

// ============================================================================
//  常量
// ============================================================================

/// 会话过期时间（秒）
const SESSION_TTL: u64 = 86400; // 24 小时

/// 嵌入管理端 HTML 页面
const ADMIN_HTML: &str = include_str!("../www/admin.html");

/// 管理端口锁文件
const MANAGE_PORT_FILE: &str = ".manage.port";

// ============================================================================
//  数据结构
// ============================================================================

/// 会话信息
struct Session {
    username: String,
    expires: u64,
}

/// 管理服务器配置
#[derive(Clone)]
pub struct ManageConfig {
    pub addr: String,
    pub daemon: bool,
}

/// API 统一响应
#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T) -> Self {
        ApiResponse { success: true, message: "ok".to_string(), data: Some(data) }
    }
    fn ok_msg(msg: &str) -> Self where T: Default {
        ApiResponse { success: true, message: msg.to_string(), data: None }
    }
    fn err(msg: &str) -> Self where T: Default {
        ApiResponse { success: false, message: msg.to_string(), data: None }
    }
}

/// 系统状态
#[derive(Serialize)]
struct SystemStatus {
    uptime_seconds: u64,
    memory_total_mb: u64,
    memory_used_mb: u64,
    memory_percent: f64,
    cpu_percent: f64,
    cpu_cores: u32,
    hostname: String,
    os_version: String,
    server_version: String,
    sites_count: usize,
    running_sites: usize,
}

/// 站点信息
#[derive(Serialize, Deserialize, Clone)]
struct SiteInfo {
    id: usize,
    bind: String,
    root: String,
    domains: Vec<String>,
    workers: usize,
    upload_max_size: String,
    cache_enabled: bool,
    cache_ttl: String,
    cache_max_size: String,
    directory_listing: bool,
    access_log: Option<String>,
    log_rotate_size: String,
    ssl_enabled: bool,
    cert: Option<String>,
    key: Option<String>,
    http3_port: String,
    cors_origin: String,
    cors_methods: String,
    cors_headers: String,
    allow_ip_access: bool,
    allow_delete: bool,
    allow_upload: bool,
    forbidden_dirs: Vec<String>,
    forbidden_files: Vec<String>,
    status: String,
}

/// 登录响应数据
#[derive(Serialize)]
struct LoginData {
    token: String,
}

/// 登录请求
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// 站点创建/更新请求
#[derive(Deserialize)]
struct SiteCreateRequest {
    bind: String,
    root: String,
    domains: Option<Vec<String>>,
    workers: Option<usize>,
    upload_max_size: Option<String>,
    cache_enabled: Option<bool>,
    cache_ttl: Option<String>,
    cache_max_size: Option<String>,
    directory_listing: Option<bool>,
    access_log: Option<String>,
    log_rotate_size: Option<String>,
    ssl_cert: Option<String>,
    ssl_key: Option<String>,
    http3_port: Option<String>,
    cors_origin: Option<String>,
    cors_methods: Option<String>,
    cors_headers: Option<String>,
    allow_ip_access: Option<bool>,
    allow_delete: Option<bool>,
    allow_upload: Option<bool>,
    forbidden_dirs: Option<Vec<String>>,
    forbidden_files: Option<Vec<String>>,
}

// ============================================================================
//  全局状态
// ============================================================================

/// 会话存储（线程安全）
static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 最后一次 CPU 采样（用于计算使用率）
static LAST_CPU_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_CPU_IDLE: AtomicU64 = AtomicU64::new(0);

/// 管理服务器配置（全局可访问）
static MANAGE_CONFIG: LazyLock<Mutex<Option<ManageConfigInner>>> =
    LazyLock::new(|| Mutex::new(None));

/// 主服务器子进程（通过管理端启动/停止）
static MAIN_SERVER_CHILD: LazyLock<Mutex<Option<process::Child>>> = LazyLock::new(|| {
    Mutex::new(None)
});

struct ManageConfigInner {
    data_dir: PathBuf,
}

// ============================================================================
//  公共接口
// ============================================================================

/// 启动管理服务器
///
/// 解析参数，必要时代理化，然后启动 HTTP 服务。
pub fn start_manage(addr: &str, daemon: bool) {
    // 确定数据目录
    let data_dir = crate::init::data_dir();
    if !crate::init::data_dir_exists(&data_dir) {
        eprintln!("❌ ohos-server 尚未初始化，请先运行 `ohos-server init`。");
        process::exit(1);
    }

    // 检查管理员账号是否存在
    if !auth::is_admin_exists(&data_dir) {
        eprintln!("❌ WEB 管理端未配置管理员账号。");
        eprintln!("   请运行 `ohos-server init` 并选择开启 WEB 管理端。");
        process::exit(1);
    }

    // 解析地址
    let addr = if addr.is_empty() {
        // 使用随机端口
        random_addr()
    } else {
        addr.to_string()
    };

    // 存储全局配置
    {
        let mut cfg = MANAGE_CONFIG.lock().unwrap();
        *cfg = Some(ManageConfigInner { data_dir: data_dir.clone() });
    }

    if daemon {
        // 守护进程化
        match crate::daemonize("") {
            Ok(()) => {}
            Err(e) => {
                eprintln!("❌ 守护进程化失败: {}", e);
                process::exit(1);
            }
        }
    }

    eprintln!("\n🔧  ohos-server WEB 管理端启动中...");
    eprintln!("   绑定地址: {}", addr);

    // 写入管理端口文件（供主进程读取）
    let port_file = data_dir.join(MANAGE_PORT_FILE);
    let _ = std::fs::write(&port_file, &addr);

    // 启动异步 HTTP 服务
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("创建管理服务器运行时失败");

    rt.block_on(async {
        run_manage_server(&addr).await;
    });
}

/// 运行管理 HTTP 服务器（异步）
async fn run_manage_server(addr: &str) {
    // 解析地址
    let addr = if addr.starts_with("0.0.0.0:") || addr.starts_with("127.0.0.1:") || addr.starts_with("localhost:") {
        addr.to_string()
    } else {
        // 尝试解析主机名
        addr.to_string()
    };

    let listener = match TokioTcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("❌ 无法绑定地址 '{}': {}", addr, e);
            process::exit(1);
        }
    };

    let actual_addr = listener.local_addr().unwrap();
    // 输出友好地址（把 0.0.0.0 转为 127.0.0.1，方便浏览器直接访问）
    let friendly_addr = actual_addr.to_string().replace("0.0.0.0", "127.0.0.1");
    eprintln!("   ✅ 监听地址: http://{}", actual_addr);

    // 输出管理员信息
    let data_dir = {
        let cfg = MANAGE_CONFIG.lock().unwrap();
        cfg.as_ref().map(|c| c.data_dir.clone())
    };

    if let Some(ref d) = data_dir {
        if let Some(creds) = auth::load_credentials(d) {
            eprintln!("\n   🌐 管理界面访问地址:");
            eprintln!();
            eprintln!("      http://{}", friendly_addr);
            eprintln!();
            eprintln!("   🔑 管理员账号:");
            eprintln!("      用户名: {}", creds.username);
            eprintln!("      密码:   (您设置的密码)");
            eprintln!();
            eprintln!("   提示: 密码可通过 `ohos-server reset-admin` 重置。");
        }
    }

    // 初始化 CPU 采样
    sample_cpu();

    eprintln!("\n⏳  等待连接...\n");

    // 服务 HTTP 请求
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let svc = service_fn(|req| handle_request(req));
                    if let Err(e) = http1::Builder::new()
                        .serve_connection(io, svc)
                        .with_upgrades()
                        .await
                    {
                        // 连接关闭错误正常忽略
                        if !e.to_string().contains("connection closed") {
                            eprintln!("管理服务器连接错误: {}", e);
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("管理服务器接受连接失败: {}", e);
            }
        }
    }
}

// ============================================================================
//  HTTP 请求路由
// ============================================================================

async fn handle_request(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // CORS 预检
    if method == http::Method::OPTIONS {
        return Ok(cors_response(Response::new(Full::new(Bytes::new()))));
    }

    // 路由匹配
    match (method.as_str(), path.as_str()) {
        // ─── 静态页面 ───
        ("GET", "/") | ("GET", "/index.html") | ("GET", "/admin.html") => {
            Ok(html_response(ADMIN_HTML))
        }

        // ─── API: 登录 ───
        ("POST", "/api/login") => {
            let body = collect_body(req).await;
            handle_login(&body).await
        }

        // ─── API: 登出 ───
        ("POST", "/api/logout") => {
            let token = extract_token(&req);
            handle_logout(token).await
        }

        // ─── API: 系统状态 ───
        ("GET", "/api/status") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            handle_status().await
        }

        // ─── API: 站点列表 ───
        ("GET", "/api/sites") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            handle_sites_list().await
        }

        // ─── API: 新增站点 ───
        ("POST", "/api/sites") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let body = collect_body(req).await;
            handle_site_create(&body).await
        }

        // ─── API: 站点操作（启动/停止/删除） ───
        ("POST", path) if path.starts_with("/api/sites/") && path.ends_with("/start") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = extract_id_from_path(path, "/api/sites/", "/start");
            handle_site_control(id, "start").await
        }
        ("POST", path) if path.starts_with("/api/sites/") && path.ends_with("/stop") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = extract_id_from_path(path, "/api/sites/", "/stop");
            handle_site_control(id, "stop").await
        }

        // ─── API: 获取单个站点 ───
        ("GET", path) if path.starts_with("/api/sites/") && path.matches('/').count() == 3 && !path.ends_with("/logs") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = path.trim_start_matches("/api/sites/");
            handle_site_get(id).await
        }

        // ─── API: 更新站点 ───
        ("PUT", path) if path.starts_with("/api/sites/") && path.matches('/').count() == 3 => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = path.trim_start_matches("/api/sites/");
            let body = collect_body(req).await;
            handle_site_update(id, &body).await
        }

        // ─── API: 删除站点 ───
        ("DELETE", path) if path.starts_with("/api/sites/") && path.matches('/').count() == 3 => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = path.trim_start_matches("/api/sites/");
            handle_site_delete(id).await
        }

        // ─── API: 查看日志 ───
        ("GET", path) if path.starts_with("/api/sites/") && path.ends_with("/logs") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            let id = extract_id_from_path(path, "/api/sites/", "/logs");
            handle_site_logs(id).await
        }

        // ─── API: 服务器日志 ───
        ("GET", "/api/logs") => {
            let token = extract_token(&req);
            if !check_auth(&token) {
                return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("未登录或会话已过期")));
            }
            handle_server_logs().await
        }

        // ─── 404 ───
        _ => Ok(json_response(StatusCode::NOT_FOUND, &ApiResponse::<()>::err("接口不存在"))),
    }
}

// ============================================================================
//  API 处理器
// ============================================================================

/// 处理登录请求
async fn handle_login(body: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    let login: LoginRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("请求格式错误")));
        }
    };

    let data_dir = {
        let cfg = MANAGE_CONFIG.lock().unwrap();
        cfg.as_ref().map(|c| c.data_dir.clone())
    };

    let data_dir = match data_dir {
        Some(d) => d,
        None => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err("服务器配置未初始化")));
        }
    };

    // 验证密码
    if !auth::verify_password(&data_dir, &login.password) {
        // 检查是否是用户名错误（提示不区分用户还是密码）
        return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("用户名或密码错误")));
    }

    // 验证用户名
    if let Some(creds) = auth::load_credentials(&data_dir) {
        if creds.username != login.username {
            return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("用户名或密码错误")));
        }
    } else {
        return Ok(json_response(StatusCode::UNAUTHORIZED, &ApiResponse::<()>::err("用户名或密码错误")));
    }

    // 生成会话 Token
    let token = generate_session_token();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    {
        let mut sessions = SESSIONS.lock().unwrap();
        sessions.insert(token.clone(), Session {
            username: login.username,
            expires: now + SESSION_TTL,
        });
    }

    // 构建 Cookie
    let cookie = format!(
        "manage_token={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        token, SESSION_TTL
    );

    let body = serde_json::to_string(&ApiResponse::ok(LoginData { token })).unwrap_or_default();
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(SET_COOKIE, cookie)
        .body(Full::new(Bytes::from(body)))
        .unwrap();
    Ok(cors_response(resp))
}

/// 处理登出请求
async fn handle_logout(token: Option<String>) -> Result<Response<Full<Bytes>>, Infallible> {
    if let Some(t) = token {
        let mut sessions = SESSIONS.lock().unwrap();
        sessions.remove(&t);
    }
    Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg("已登出")))
}

/// 处理系统状态查询
async fn handle_status() -> Result<Response<Full<Bytes>>, Infallible> {
    let status = get_system_status();
    Ok(json_response(StatusCode::OK, &ApiResponse::ok(status)))
}

/// 处理站点列表查询
async fn handle_sites_list() -> Result<Response<Full<Bytes>>, Infallible> {
    let sites = load_sites();
    let sites: Vec<SiteInfo> = match sites {
        Ok(s) => s,
        Err(e) => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
        }
    };
    Ok(json_response(StatusCode::OK, &ApiResponse::ok(sites)))
}

/// 处理单个站点查询
async fn handle_site_get(id: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    let site_id: usize = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("无效的站点 ID")));
        }
    };

    let sites = match load_sites() {
        Ok(s) => s,
        Err(e) => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
        }
    };

    match sites.into_iter().find(|s| s.id == site_id) {
        Some(site) => Ok(json_response(StatusCode::OK, &ApiResponse::ok(site))),
        None => Ok(json_response(StatusCode::NOT_FOUND, &ApiResponse::<()>::err("站点不存在"))),
    }
}

/// 处理新增站点
async fn handle_site_create(body: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    let create_req: SiteCreateRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("请求格式错误")));
        }
    };

    // 加载当前配置（如果配置文件不存在则创建新的）
    let (config_path, mut app_config) = match load_app_config() {
        Some(c) => c,
        None => {
            // 如果配置文件不存在，尝试创建一个新的 AppConfig
            let data_dir = {
                let cfg = MANAGE_CONFIG.lock().unwrap();
                cfg.as_ref().map(|c| c.data_dir.clone())
            };
            let config_path = match data_dir {
                Some(d) => d.join("config.toml"),
                None => {
                    return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err("无法获取数据目录")));
                }
            };
            (config_path.to_string_lossy().to_string(), AppConfig {
                server: Vec::new(),
                config_path: String::new(),
            })
        }
    };

    // 新增站点
    use crate::config::ServerConfig;
    let mut srv = ServerConfig {
        bind: create_req.bind,
        root: create_req.root,
        domains: create_req.domains.unwrap_or_default(),
        upload_max_size: create_req.upload_max_size.unwrap_or_else(|| "10MB".to_string()),
        upload_max_size_bytes: 0,
        workers: create_req.workers.unwrap_or(0),
        threads: 1,
        cache_enabled: create_req.cache_enabled.unwrap_or(false),
        cache_ttl: create_req.cache_ttl.unwrap_or_else(|| "1h".to_string()),
        cache_ttl_seconds: 0,
        cache_max_size: create_req.cache_max_size.unwrap_or_else(|| "100MB".to_string()),
        cache_max_size_bytes: 0,
        directory_listing: create_req.directory_listing.unwrap_or(false),
        access_log: create_req.access_log,
        log_rotate_size: create_req.log_rotate_size.unwrap_or_else(|| "0".to_string()),
        log_rotate_size_bytes: 0,
        pid_file: None,
        rewrite: Vec::new(),
        cgi: Vec::new(),
        location: Vec::new(),
        cors_origin: create_req.cors_origin.unwrap_or_default(),
        cors_methods: create_req.cors_methods.unwrap_or_else(|| "GET,POST,PUT,DELETE,PATCH,OPTIONS,HEAD".to_string()),
        cors_headers: create_req.cors_headers.unwrap_or_else(|| "*".to_string()),
        cert: create_req.ssl_cert,
        key: create_req.ssl_key,
        http3_port: create_req.http3_port.unwrap_or_else(|| "0".to_string()),
        rate_limit: None,
        blacklist: Vec::new(),
        per_ip_rates: std::collections::HashMap::new(),
        session: None,
        allow_ip_access: create_req.allow_ip_access.unwrap_or(true),
        forbidden_dirs: create_req.forbidden_dirs.unwrap_or_default(),
        forbidden_files: create_req.forbidden_files.unwrap_or_default(),
        allow_delete: create_req.allow_delete.unwrap_or(false),
        allow_upload: create_req.allow_upload.unwrap_or(false),
    };
    srv.finalize();

    app_config.server.push(srv);

    // 保存配置文件
    if let Err(e) = save_app_config(&config_path, &app_config) {
        return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
    }

    // 自动启动或热重载主服务器
    let extra_msg = if is_main_server_running() {
        match reload_main_server() {
            Ok(m) => format!("。{}", m),
            Err(_) => "。配置已保存。".to_string(),
        }
    } else {
        match start_main_server() {
            Ok(m) => format!("。{}", m),
            Err(_) => "。请点击「启动」按钮来启动服务器。".to_string(),
        }
    };

    let msg = format!("站点创建成功{}", extra_msg);
    Ok(json_response(StatusCode::CREATED, &ApiResponse::<()>::ok_msg(&msg)))
}

/// 处理更新站点
async fn handle_site_update(id: &str, body: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    let site_id: usize = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("无效的站点 ID")));
        }
    };

    let update_req: SiteCreateRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("请求格式错误")));
        }
    };

    let (config_path, mut app_config) = match load_app_config() {
        Some(c) => c,
        None => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err("无法加载配置文件")));
        }
    };

    // 使用 0-based index 查找站点（配置文件中 N 个 [[server]] 对应 Vec 下标 N-1）
    if site_id == 0 || site_id > app_config.server.len() {
        return Ok(json_response(StatusCode::NOT_FOUND, &ApiResponse::<()>::err("站点不存在")));
    }

    let srv = &mut app_config.server[site_id - 1];
    srv.bind = update_req.bind;
    srv.root = update_req.root;
    if let Some(domains) = update_req.domains {
        srv.domains = domains;
    }
    if let Some(workers) = update_req.workers {
        srv.workers = workers;
    }
    if let Some(size) = update_req.upload_max_size {
        srv.upload_max_size = if size.is_empty() { "10MB".to_string() } else { size };
    }
    if let Some(cache) = update_req.cache_enabled {
        srv.cache_enabled = cache;
    }
    if let Some(ttl) = update_req.cache_ttl {
        srv.cache_ttl = if ttl.is_empty() { "1h".to_string() } else { ttl };
    }
    if let Some(max_size) = update_req.cache_max_size {
        srv.cache_max_size = if max_size.is_empty() { "100MB".to_string() } else { max_size };
    }
    if let Some(listing) = update_req.directory_listing {
        srv.directory_listing = listing;
    }
    if let Some(log) = update_req.access_log {
        srv.access_log = if log.is_empty() { None } else { Some(log) };
    }
    if let Some(rotate) = update_req.log_rotate_size {
        srv.log_rotate_size = if rotate.is_empty() { "0".to_string() } else { rotate };
    }
    if let Some(h3) = update_req.http3_port {
        srv.http3_port = if h3.is_empty() { "0".to_string() } else { h3 };
    }
    if let Some(origin) = update_req.cors_origin {
        srv.cors_origin = origin;
    }
    if let Some(methods) = update_req.cors_methods {
        srv.cors_methods = if methods.is_empty() { "GET,POST,PUT,DELETE,PATCH,OPTIONS,HEAD".to_string() } else { methods };
    }
    if let Some(headers) = update_req.cors_headers {
        srv.cors_headers = if headers.is_empty() { "*".to_string() } else { headers };
    }
    if let Some(cert) = update_req.ssl_cert {
        srv.cert = if cert.is_empty() { None } else { Some(cert) };
    }
    if let Some(key) = update_req.ssl_key {
        srv.key = if key.is_empty() { None } else { Some(key) };
    }
    if let Some(ip_access) = update_req.allow_ip_access {
        srv.allow_ip_access = ip_access;
    }
    if let Some(delete) = update_req.allow_delete {
        srv.allow_delete = delete;
    }
    if let Some(upload) = update_req.allow_upload {
        srv.allow_upload = upload;
    }
    if let Some(dirs) = update_req.forbidden_dirs {
        srv.forbidden_dirs = dirs;
    }
    if let Some(files) = update_req.forbidden_files {
        srv.forbidden_files = files;
    }
    srv.finalize();

    if let Err(e) = save_app_config(&config_path, &app_config) {
        return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
    }

    Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg("站点更新成功")))
}

/// 处理删除站点
async fn handle_site_delete(id: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    let site_id: usize = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("无效的站点 ID")));
        }
    };

    let (config_path, mut app_config) = match load_app_config() {
        Some(c) => c,
        None => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err("无法加载配置文件")));
        }
    };

    if site_id == 0 || site_id > app_config.server.len() {
        return Ok(json_response(StatusCode::NOT_FOUND, &ApiResponse::<()>::err("站点不存在")));
    }

    app_config.server.remove(site_id - 1);

    if let Err(e) = save_app_config(&config_path, &app_config) {
        return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
    }

    Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg("站点已删除")))
}

// ============================================================================
//  主服务器进程管理（子进程方式）
// ============================================================================

/// 检查主服务器是否在运行
fn is_main_server_running() -> bool {
    let mut guard = MAIN_SERVER_CHILD.lock().unwrap();
    if let Some(ref mut child) = *guard {
        match child.try_wait() {
            Ok(Some(_)) => {
                *guard = None; // 已退出，清理
                false
            }
            Ok(None) => true, // 仍在运行
            Err(_) => {
                *guard = None;
                false
            }
        }
    } else {
        false
    }
}

/// 启动主服务器（作为子进程）
fn start_main_server() -> Result<String, String> {
    let mut guard = MAIN_SERVER_CHILD.lock().unwrap();

    // 检查是否已在运行
    if let Some(ref mut child) = *guard {
        match child.try_wait() {
            Ok(Some(_)) => { *guard = None; } // 已退出，清理
            Ok(None) => return Ok(format!("主服务器已在运行 (PID: {})", child.id())),
            Err(_) => { *guard = None; }
        }
    }

    let self_exe = std::env::current_exe().map_err(|e| format!("获取自身路径失败: {}", e))?;
    let data_dir = crate::init::data_dir();
    let config_path = data_dir.join("config.toml");

    if !config_path.exists() {
        return Err("配置文件不存在，请先添加站点。".to_string());
    }

    // 使用管道捕获 stderr，以便子进程启动失败时返回错误信息
    let child = process::Command::new(&self_exe)
        .arg("-c")
        .arg(config_path.to_string_lossy().to_string())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动主服务器失败: {}", e))?;

    let pid = child.id();

    // 等待一小段时间，验证子进程是否还活着
    let mut child = child;
    let start = std::time::Instant::now();
    let mut stderr_output = Vec::new();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // 子进程已退出
                let _ = child.stderr.take().map(|mut s| s.read_to_end(&mut stderr_output));
                let err_msg = if !stderr_output.is_empty() {
                    String::from_utf8_lossy(&stderr_output).trim().to_string()
                } else {
                    String::new()
                };
                let detail = if err_msg.is_empty() {
                    format!("子进程立即退出 (exit: {:?})", status.code())
                } else {
                    format!("子进程立即退出 (exit: {:?}): {}", status.code(), err_msg)
                };
                *guard = None;
                return Err(detail);
            }
            Ok(None) => {
                // 还在运行
                if start.elapsed() >= std::time::Duration::from_millis(500) {
                    // 已经存活超过 500ms，认为启动成功
                    *guard = Some(child);
                    return Ok(format!("主服务器已启动 (PID: {})", pid));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                *guard = None;
                return Err("等待子进程状态时出错".to_string());
            }
        }
    }
}

/// 停止主服务器
fn stop_main_server() -> Result<String, String> {
    let mut guard = MAIN_SERVER_CHILD.lock().unwrap();

    match guard.take() {
        Some(mut child) => {
            let pid = child.id() as i32;
            // SIGTERM 优雅退出
            let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
            // 等待退出（最多 3 秒）
            for _ in 0..30 {
                match child.try_wait() {
                    Ok(Some(_)) => return Ok(format!("主服务器已停止 (PID: {})", pid)),
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(_) => break,
                }
            }
            // 超时后强制杀死
            let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
            let _ = child.wait();
            Ok(format!("主服务器已强制停止 (PID: {})", pid))
        }
        None => Err("主服务器未在运行".to_string()),
    }
}

/// 热重启主服务器（SIGHUP）
fn reload_main_server() -> Result<String, String> {
    let guard = MAIN_SERVER_CHILD.lock().unwrap();

    match *guard {
        Some(ref child) => {
            let pid = child.id() as i32;
            let ret = unsafe { libc::kill(pid, libc::SIGHUP) };
            if ret == 0 {
                Ok(format!("已发送热重启信号 (PID: {})", pid))
            } else {
                Err("发送 SIGHUP 信号失败".to_string())
            }
        }
        None => Err("主服务器未在运行，请先启动。".to_string()),
    }
}

/// 处理站点启动/停止
async fn handle_site_control(_id: usize, action: &str) -> Result<Response<Full<Bytes>>, Infallible> {
    match action {
        "start" => {
            match start_main_server() {
                Ok(msg) => Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg(&msg))),
                Err(e) => Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e))),
            }
        }
        "stop" => {
            match stop_main_server() {
                Ok(msg) => Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg(&msg))),
                Err(e) => Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e))),
            }
        }
        "reload" => {
            match reload_main_server() {
                Ok(msg) => Ok(json_response(StatusCode::OK, &ApiResponse::<()>::ok_msg(&msg))),
                Err(e) => Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e))),
            }
        }
        _ => Ok(json_response(StatusCode::BAD_REQUEST, &ApiResponse::<()>::err("未知操作"))),
    }
}

/// 处理站点日志查询
async fn handle_site_logs(id: usize) -> Result<Response<Full<Bytes>>, Infallible> {
    let sites = match load_sites() {
        Ok(s) => s,
        Err(e) => {
            return Ok(json_response(StatusCode::INTERNAL_SERVER_ERROR, &ApiResponse::<()>::err(&e)));
        }
    };

    let site = match sites.into_iter().find(|s| s.id == id) {
        Some(s) => s,
        None => {
            return Ok(json_response(StatusCode::NOT_FOUND, &ApiResponse::<()>::err("站点不存在")));
        }
    };

    let log_content = if let Some(ref log_path) = site.access_log {
        read_log_file(log_path, 200)
    } else {
        "该站点未配置日志文件。".to_string()
    };

    let resp = ApiResponse::<String>::ok_msg(&log_content);
    Ok(json_response(StatusCode::OK, &resp))
}

/// 处理服务器日志查询
async fn handle_server_logs() -> Result<Response<Full<Bytes>>, Infallible> {
    let data_dir = crate::init::default_log_dir();
    let log_file = data_dir.join("access.log");

    let content = if log_file.exists() {
        read_log_file(&log_file.to_string_lossy(), 200)
    } else {
        "暂无服务器日志。".to_string()
    };

    let resp = ApiResponse::<String>::ok_msg(&content);
    Ok(json_response(StatusCode::OK, &resp))
}

// ============================================================================
//  辅助函数
// ============================================================================

/// 获取系统状态
fn get_system_status() -> SystemStatus {
    let sys = sysinfo::System::new_all();

    let uptime = sysinfo::System::uptime(); // seconds

    let hostname = sysinfo::System::host_name().unwrap_or_else(|| "unknown".to_string());
    let os_version = format!(
        "{} {}",
        sysinfo::System::name().unwrap_or_else(|| "unknown".to_string()),
        sysinfo::System::kernel_version().unwrap_or_else(|| "".to_string()),
    );

    let cpu_cores = sys.physical_core_count().unwrap_or(0) as u32;

    // CPU 使用率
    let cpu_percent = sample_cpu();

    // 内存
    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let mem_percent = if total_mem > 0 {
        (used_mem as f64 / total_mem as f64) * 100.0
    } else {
        0.0
    };

    // 站点数量
    let sites = match load_sites() {
        Ok(s) => s,
        Err(_) => Vec::new(),
    };
    let sites_count = sites.len();
    let running_sites = if is_main_server_running() { sites_count } else { 0 };

    SystemStatus {
        uptime_seconds: uptime,
        memory_total_mb: total_mem / 1024 / 1024,
        memory_used_mb: used_mem / 1024 / 1024,
        memory_percent: (mem_percent * 100.0).round() / 100.0,
        cpu_percent: (cpu_percent * 100.0).round() / 100.0,
        cpu_cores,
        hostname,
        os_version,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        sites_count,
        running_sites,
    }
}

/// 采样 CPU 使用率
fn sample_cpu() -> f64 {
    // 读取 /proc/stat 计算 CPU 使用率（兼容 Linux）
    let stat_content = match std::fs::read_to_string("/proc/stat") {
        Ok(c) => c,
        Err(_) => return 0.0,
    };

    let cpu_line = match stat_content.lines().next() {
        Some(l) => l,
        None => return 0.0,
    };

    let parts: Vec<&str> = cpu_line.split_whitespace().collect();
    if parts.len() < 5 || parts[0] != "cpu" {
        return 0.0;
    }

    let values: Vec<u64> = parts[1..]
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();

    if values.len() < 3 {
        return 0.0;
    }

    let total: u64 = values.iter().sum();
    let idle = values[3]; // idle time

    let prev_total = LAST_CPU_TOTAL.swap(total, Ordering::SeqCst);
    let prev_idle = LAST_CPU_IDLE.swap(idle, Ordering::SeqCst);

    if prev_total == 0 || prev_idle == 0 {
        return 0.0;
    }

    let dtotal = total.saturating_sub(prev_total);
    let didle = idle.saturating_sub(prev_idle);

    if dtotal == 0 {
        return 0.0;
    }

    (dtotal.saturating_sub(didle)) as f64 / dtotal as f64 * 100.0
}

/// 读取日志文件（获取最后 N 行）
fn read_log_file(path: &str, max_lines: usize) -> String {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return format!("读取日志文件失败: {}", e),
    };

    let reader = std::io::BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .collect();

    let total = lines.len();
    let start = if total > max_lines { total - max_lines } else { 0 };
    lines[start..].join("\n")
}

/// 从配置中加载站点列表
fn load_sites() -> Result<Vec<SiteInfo>, String> {
    let (_, app_config) = match load_app_config() {
        Some(c) => c,
        None => return Err("无法加载配置".to_string()),
    };

    let server_running = is_main_server_running();

    let sites: Vec<SiteInfo> = app_config.server.iter().enumerate().map(|(i, s)| {
        SiteInfo {
            id: i + 1,
            bind: s.bind.clone(),
            root: s.root.clone(),
            domains: s.domains.clone(),
            workers: s.workers,
            upload_max_size: s.upload_max_size.clone(),
            cache_enabled: s.cache_enabled,
            cache_ttl: s.cache_ttl.clone(),
            cache_max_size: s.cache_max_size.clone(),
            directory_listing: s.directory_listing,
            access_log: s.access_log.clone(),
            log_rotate_size: s.log_rotate_size.clone(),
            ssl_enabled: s.cert.is_some() && s.key.is_some(),
            cert: s.cert.clone(),
            key: s.key.clone(),
            http3_port: s.http3_port.clone(),
            cors_origin: s.cors_origin.clone(),
            cors_methods: s.cors_methods.clone(),
            cors_headers: s.cors_headers.clone(),
            allow_ip_access: s.allow_ip_access,
            allow_delete: s.allow_delete,
            allow_upload: s.allow_upload,
            forbidden_dirs: s.forbidden_dirs.clone(),
            forbidden_files: s.forbidden_files.clone(),
            status: if server_running { "running" } else { "stopped" }.to_string(),
        }
    }).collect();

    Ok(sites)
}

/// 加载完整配置
fn load_app_config() -> Option<(String, AppConfig)> {
    let data_dir = {
        let cfg = MANAGE_CONFIG.lock().unwrap();
        cfg.as_ref().map(|c| c.data_dir.clone())
    }?;

    let config_path = data_dir.join("config.toml");
    if !config_path.exists() {
        return None;
    }

    match AppConfig::from_file(&config_path.to_string_lossy()) {
        Ok(cfg) => Some((config_path.to_string_lossy().to_string(), cfg)),
        Err(_) => None,
    }
}

/// 保存配置到文件
fn save_app_config(path: &str, app_config: &AppConfig) -> Result<(), String> {
    // 将 AppConfig 序列化为 TOML
    // 由于 AppConfig 使用了 serde Serialize，我们可以直接用 toml crate
    let toml_str = toml::to_string_pretty(&app_config).map_err(|e| format!("序列化配置失败: {}", e))?;

    // 添加注释头
    let content = format!(
        "# ═══════════════════════════════════════════════\n\
         # ohos-server 服务器配置文件\n\
         # 由 WEB 管理端保存\n\
         # 保存时间: {}\n\
         # ═══════════════════════════════════════════════\n\n{}",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        toml_str
    );

    std::fs::write(path, &content).map_err(|e| format!("写入配置文件失败: {}", e))?;

    Ok(())
}

/// 生成随机地址（绑定 0.0.0.0:随机端口）
fn random_addr() -> String {
    let port = get_random_port();
    format!("0.0.0.0:{}", port)
}

/// 获取随机可用端口
fn get_random_port() -> u16 {
    // 尝试绑定端口 0（系统分配），获取实际端口
    match TcpListener::bind("0.0.0.0:0") {
        Ok(listener) => {
            let port = listener.local_addr().unwrap().port();
            // drop listener 释放端口
            drop(listener);
            port
        }
        Err(_) => {
            // 如果失败，使用随机端口
            let mut rng = rand::thread_rng();
            rng.gen_range(49152..65535)
        }
    }
}

/// 生成会话 Token
fn generate_session_token() -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    // 使用 base64 URL-safe 编码（无填充）
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 从请求中提取 Token
fn extract_token(req: &Request<Incoming>) -> Option<String> {
    // 优先从 Cookie 提取
    if let Some(cookie) = req.headers().get(http::header::COOKIE) {
        let cookie_str = cookie.to_str().unwrap_or("");
        for part in cookie_str.split(';') {
            let part = part.trim();
            if let Some(value) = part.strip_prefix("manage_token=") {
                return Some(value.to_string());
            }
        }
    }

    // 其次从 Authorization header
    if let Some(auth) = req.headers().get(http::header::AUTHORIZATION) {
        let auth_str = auth.to_str().unwrap_or("");
        if let Some(token) = auth_str.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }

    // 最后从 URL 参数
    let uri = req.uri();
    if let Some(query) = uri.query() {
        for param in query.split('&') {
            if let Some(value) = param.strip_prefix("token=") {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// 检查会话是否有效
fn check_auth(token: &Option<String>) -> bool {
    let token = match token {
        Some(t) => t,
        None => return false,
    };

    let mut sessions = SESSIONS.lock().unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    // 清理过期会话
    sessions.retain(|_, s| s.expires > now);

    // 检查当前会话
    if let Some(session) = sessions.get(token) {
        session.expires > now
    } else {
        false
    }
}

/// 收集请求体内容
async fn collect_body(req: Request<Incoming>) -> String {
    let body = req.collect().await.unwrap_or_default().to_bytes();
    String::from_utf8_lossy(&body).to_string()
}

/// 从路径中提取 ID（如 /api/sites/1/start → 1）
fn extract_id_from_path(path: &str, prefix: &str, suffix: &str) -> usize {
    let trimmed = path.trim_start_matches(prefix);
    let trimmed = trimmed.trim_end_matches(suffix);
    trimmed.parse().unwrap_or(0)
}

// ============================================================================
//  HTTP 响应构建
// ============================================================================

/// 构建 HTML 响应
fn html_response(content: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(content.to_string())))
        .unwrap()
}

/// 构建 JSON 响应
fn json_response<T: Serialize>(status: StatusCode, data: &T) -> Response<Full<Bytes>> {
    let json = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Full::new(Bytes::from(json)))
        .unwrap()
}

/// 添加 CORS 头
fn cors_response(resp: Response<Full<Bytes>>) -> Response<Full<Bytes>> {
    let (mut parts, body) = resp.into_parts();
    parts.headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        "*".parse().unwrap(),
    );
    parts.headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, PUT, DELETE, OPTIONS".parse().unwrap(),
    );
    parts.headers.insert(
        http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        "Content-Type, Authorization".parse().unwrap(),
    );
    Response::from_parts(parts, body)
}
