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

use serde::{Deserialize, Serialize};

/// 上传大小解析，如 "10MB" -> 10485760
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let (num_str, unit): (String, u64) = if s.ends_with("kb") {
        (s[..s.len() - 2].to_string(), 1024u64)
    } else if s.ends_with("mb") {
        (s[..s.len() - 2].to_string(), 1024u64 * 1024)
    } else if s.ends_with("gb") {
        (s[..s.len() - 2].to_string(), 1024u64 * 1024 * 1024)
    } else if s.ends_with('b') {
        (s[..s.len() - 1].to_string(), 1u64)
    } else {
        (s.to_string(), 1u64)
    };
    let num: u64 = num_str.trim().parse().ok()?;
    Some(num * unit)
}

/// 时间解析，如 "7d" -> 604800 秒
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let (num_str, unit) = if s.ends_with("d") {
        (&s[..s.len() - 1], 86400u64)
    } else if s.ends_with("h") {
        (&s[..s.len() - 1], 3600u64)
    } else if s.ends_with("m") {
        (&s[..s.len() - 1], 60u64)
    } else if s.ends_with('s') {
        (&s[..s.len() - 1], 1u64)
    } else {
        (s.as_str(), 1u64)
    };
    let num: u64 = num_str.trim().parse().ok()?;
    Some(num * unit)
}

/// 单个主机/站点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// 绑定的IP和端口，如 "0.0.0.0:8080"
    #[serde(default = "default_bind")]
    pub bind: String,

    /// 网站根目录
    #[serde(default = "default_root")]
    pub root: String,

    /// 绑定的域名列表（虚拟主机）
    #[serde(default)]
    pub domains: Vec<String>,

    /// 上传最大大小（如 "10MB"）
    #[serde(default = "default_upload_max_size")]
    pub upload_max_size: String,

    /// 上传最大大小（字节），由 parse_size 计算
    #[serde(skip)]
    pub upload_max_size_bytes: u64,

    /// 工作线程数
    #[serde(default = "default_threads")]
    pub threads: usize,

    /// 工作进程数（多进程模式，0=auto=CPU核数，1=单进程）
    #[serde(default = "default_workers")]
    pub workers: usize,

    /// 是否启用缓存
    #[serde(default = "default_cache_enabled")]
    pub cache_enabled: bool,

    /// 缓存TTL（秒），或格式如 "1h", "7d"
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: String,

    /// 缓存TTL（秒），由 parse_duration 计算
    #[serde(skip)]
    pub cache_ttl_seconds: u64,

    /// 缓存最大大小
    #[serde(default = "default_cache_max_size")]
    pub cache_max_size: String,

    /// 缓存最大大小（字节）
    #[serde(skip)]
    pub cache_max_size_bytes: u64,

    /// 目录列表
    #[serde(default = "default_directory_listing")]
    pub directory_listing: bool,

    /// 日志文件路径（可选）
    #[serde(default)]
    pub access_log: Option<String>,

    /// 日志轮转大小限制（如 "100MB", "1GB", 0=不限制）
    #[serde(default = "default_log_rotate_size")]
    pub log_rotate_size: String,

    /// 日志轮转大小（字节），由 parse_size 计算
    #[serde(skip)]
    pub log_rotate_size_bytes: u64,

    /// PID文件路径（守护进程模式）
    #[serde(default)]
    pub pid_file: Option<String>,

    /// URL重写规则（伪静态）
    #[serde(default)]
    pub rewrite: Vec<RewriteRule>,

    /// CGI解释器配置
    #[serde(default)]
    pub cgi: Vec<CgiConfig>,

    /// 路径规则（反向代理、静态文件等）
    #[serde(default)]
    pub location: Vec<LocationConfig>,

    /// CORS 允许的源（如 "*" 或 "https://example.com"，空=不启用CORS）
    #[serde(default)]
    pub cors_origin: String,

    /// CORS 允许的方法（逗号分隔，默认 "GET,POST,PUT,DELETE,PATCH,OPTIONS"）
    #[serde(default = "default_cors_methods")]
    pub cors_methods: String,

    /// CORS 允许的请求头（逗号分隔，默认 "*"）
    #[serde(default = "default_cors_headers")]
    pub cors_headers: String,

    /// TLS 证书路径（设置后自动启用 HTTPS）
    #[serde(default)]
    pub cert: Option<String>,

    /// TLS 私钥路径
    #[serde(default)]
    pub key: Option<String>,

    /// HTTP/3 (QUIC) 端口（0=不启用，如 "4433"）
    #[serde(default = "default_http3_port")]
    pub http3_port: String,

    /// 限流配置（可选）
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,

    /// IP 黑名单（完全屏蔽）
    #[serde(default)]
    pub blacklist: Vec<String>,

    /// 每个 IP 的自定义限流速率（覆盖全局 requests_per_second）
    #[serde(default)]
    pub per_ip_rates: std::collections::HashMap<String, u32>,

    /// Session 配置（可选）
    #[serde(default)]
    pub session: Option<SessionConfig>,

    /// 是否允许通过 IP 直接访问（false 则只允许绑定的域名访问）
    #[serde(default = "default_true")]
    pub allow_ip_access: bool,
}

fn default_bind() -> String { "0.0.0.0:8080".to_string() }
fn default_root() -> String { "./www".to_string() }
fn default_upload_max_size() -> String { "10MB".to_string() }
fn default_threads() -> usize { 0 }  // 0 = auto = num_cpus
fn default_workers() -> usize { 0 }  // 0 = auto = num_cpus
fn default_cache_enabled() -> bool { false }
fn default_cache_ttl() -> String { "1h".to_string() }
fn default_cache_max_size() -> String { "100MB".to_string() }
fn default_directory_listing() -> bool { false }

fn default_log_rotate_size() -> String { "0".to_string() }

fn default_cors_methods() -> String { "GET,POST,PUT,DELETE,PATCH,OPTIONS".to_string() }

fn default_cors_headers() -> String { "*".to_string() }

fn default_http3_port() -> String { "0".to_string() }

fn default_true() -> bool { true }

impl ServerConfig {
    pub fn finalize(&mut self) {
        self.upload_max_size_bytes = parse_size(&self.upload_max_size).unwrap_or(10 * 1024 * 1024);
        self.cache_ttl_seconds = parse_duration(&self.cache_ttl).unwrap_or(3600);
        self.cache_max_size_bytes = parse_size(&self.cache_max_size).unwrap_or(100 * 1024 * 1024);
        self.log_rotate_size_bytes = parse_size(&self.log_rotate_size).unwrap_or(0);
        if self.threads == 0 {
            self.threads = num_cpus();
        }

    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// CGI解析器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgiConfig {
    /// 文件扩展名列表，如 [".php", ".phtml"]
    #[serde(default)]
    pub extensions: Vec<String>,
    /// 解释器路径，如 "/usr/bin/php-cgi"
    #[serde(default)]
    pub interpreter: String,
}

/// URL重写规则（伪静态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRule {
    /// 正则匹配路径
    pub from: String,
    /// 替换目标，可使用 $1, $2 等捕获组
    pub to: String,
    /// 可选：如果源是正则标志
    #[serde(default = "default_true")]
    pub regex: bool,
}

/// 路径规则配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationConfig {
    /// 匹配路径前缀
    pub path: String,

    /// 反向代理目标，如 "http://127.0.0.1:3000"
    #[serde(default)]
    pub proxy_pass: Option<String>,

    /// 本地文件路径（覆盖根目录）
    #[serde(default)]
    pub root: Option<String>,

    /// 代理请求头
    #[serde(default)]
    pub proxy_headers: Vec<String>,

    /// 缓存过期时间
    #[serde(default)]
    pub expires: Option<String>,

    /// 本地CGI解释器配置（覆盖全局CGI）
    #[serde(default)]
    pub cgi: Option<CgiConfig>,

    /// 本地CGI解释器路径（旧版兼容）
    #[serde(default)]
    pub interpreter: Option<String>,

    /// 负载均衡后端目标列表（替代单个 proxy_pass）
    #[serde(default)]
    pub load_balance_targets: Vec<LoadBalanceTarget>,

    /// 负载均衡策略（"round_robin" / "random"）
    #[serde(default = "default_lb_strategy")]
    pub load_balance_strategy: String,
}

/// 限流配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// 是否启用限流
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    /// 每 IP 每秒允许的请求数
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,
    /// 突发大小（令牌桶容量）
    #[serde(default = "default_burst")]
    pub burst_size: u32,
}

/// Session 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// 是否启用 Session
    #[serde(default = "default_session_enabled")]
    pub enabled: bool,
    /// Cookie 名称
    #[serde(default = "default_session_cookie")]
    pub cookie_name: String,
    /// Session 过期时间（秒）
    #[serde(default = "default_session_ttl")]
    pub ttl: u64,
}

/// 负载均衡后端目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadBalanceTarget {
    /// 后端服务 URL，如 "http://127.0.0.1:3000"
    pub url: String,
    /// 权重（越大分配越多请求）
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_rate_limit_enabled() -> bool { true }
fn default_rps() -> u32 { 100 }
fn default_burst() -> u32 { 200 }
fn default_session_enabled() -> bool { true }
fn default_session_cookie() -> String { "OHOS_SESSION".to_string() }
fn default_session_ttl() -> u64 { 3600 }
fn default_weight() -> u32 { 1 }
fn default_lb_strategy() -> String { "round_robin".to_string() }

/// 全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 多个服务器站点配置
    #[serde(default)]
    pub server: Vec<ServerConfig>,
    /// 配置文件路径（内部使用）
    #[serde(skip)]
    pub config_path: String,
}

impl AppConfig {
    /// 从文件路径加载配置
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取配置文件失败: {}", e))?;
        let mut cfg: AppConfig = toml::from_str(&content)
            .map_err(|e| format!("解析配置文件失败: {}", e))?;
        for s in &mut cfg.server {
            s.finalize();
        }
        cfg.config_path = path.to_string();
        Ok(cfg)
    }

    /// 从命令行参数创建单个服务器配置
    pub fn from_cli(addr: &str, root: &str) -> Self {
        let mut srv = ServerConfig {
            bind: addr.to_string(),
            root: root.to_string(),
            domains: Vec::new(),
            upload_max_size: default_upload_max_size(),
            upload_max_size_bytes: 0,
            workers: default_workers(),
            threads: default_threads(),
            cache_enabled: default_cache_enabled(),
            cache_ttl: default_cache_ttl(),
            cache_ttl_seconds: 0,
            cache_max_size: default_cache_max_size(),
            cache_max_size_bytes: 0,
            directory_listing: default_directory_listing(),
            access_log: None,
            log_rotate_size: default_log_rotate_size(),
            log_rotate_size_bytes: 0,
            pid_file: None,
            rewrite: Vec::new(),
            cgi: Vec::new(),
            location: Vec::new(),
            cors_origin: String::new(),
            cors_methods: default_cors_methods(),
            cors_headers: default_cors_headers(),
            cert: None,
            key: None,
            http3_port: default_http3_port(),
            rate_limit: None,
            blacklist: Vec::new(),
            per_ip_rates: std::collections::HashMap::new(),
            session: None,
            allow_ip_access: true,
        };
        srv.finalize();
        AppConfig { server: vec![srv], config_path: String::new() }
    }
}

/// 默认配置文件内容，作为文档
pub const DEFAULT_CONFIG: &str = r#"# ═══════════════════════════════════════════════
# ohosHttp 服务器配置文件
# ═══════════════════════════════════════════════
# 使用方式：
#   ohosHttp -c config.toml          # 从配置文件启动
#   ohosHttp -c config.toml -d       # 守护进程模式启动
#   ohosHttp -c config.toml --all    # 启动配置中所有站点
#
# 生成本配置：
#   ohosHttp --gen-config > config.toml
# ═══════════════════════════════════════════════

# =============================================
# 站点 #1 - 主站
# =============================================
[[server]]
# 绑定地址和端口
bind = "0.0.0.0:8080"

# 网站根目录
root = "./www"

# 绑定的域名列表（虚拟主机）
domains = ["example.com", "www.example.com"]

# 上传最大大小（KB/MB/GB）
upload_max_size = "10MB"

# 工作线程数（0=自动检测CPU核心数）
threads = 4

# 是否启用文件缓存
cache_enabled = true

# 缓存TTL（秒/h/d）
cache_ttl = "1h"

# 缓存最大大小
cache_max_size = "100MB"

# 是否开启目录列表（无 index.html 时显示）
directory_listing = false

# 访问日志文件路径（可选，不设置则不记录）
# access_log = "./logs/access.log"

# 日志轮转大小限制（如 "100MB", "1GB", 0=不限制大小，仅按日期轮转）
# log_rotate_size = "100MB"

# PID文件路径（守护进程模式）
# pid_file = "/var/run/ohoshttp.pid"

# CORS 跨域配置（可选，不设置则不启用CORS）
# cors_origin = "*"
# cors_methods = "GET,POST,PUT,DELETE,PATCH,OPTIONS"
# cors_headers = "Content-Type,Authorization,X-Requested-With"


# =============================================
# 限流与安全配置（可选）
# =============================================

# 限流控制：每 IP 每秒允许的请求数
# rate_limit = { enabled = true, requests_per_second = 100, burst_size = 200 }

# IP 黑名单：完全屏蔽的 IP（支持 * 通配符）
# blacklist = ["10.0.0.1", "192.168.1.*", "203.0.113.0"]

# 按 IP 自定义限流速率（覆盖全局 requests_per_second）
# [server.per_ip_rates]
# "192.168.1.50" = 50     # 该 IP 每秒只允许 50 个请求
# "10.0.0.2" = 1000       # 该 IP 每秒允许 1000 个请求


# =============================================
# Session 配置（可选）
# =============================================

# session = { enabled = true, cookie_name = "OHOS_SESSION", ttl = 3600 }


# =============================================
# URL 重写规则（伪静态）
# =============================================
# 格式：from（正则匹配路径）→ to（替换目标，支持 $1 $2 捕获组）

# 文章详情页：/article/123 → /article.html?id=123
[[server.rewrite]]
from = "^/article/(\\d+)$"
to = "/article.html?id=$1"

# 分类页：/category/tech → /category.html?name=tech
[[server.rewrite]]
from = "^/category/(\\w+)$"
to = "/category.html?name=$1"

# 用户主页：/user/john → /profile.php?user=john
[[server.rewrite]]
from = "^/user/([^/]+)$"
to = "/profile.php?user=$1"

# 标签页：/tag/rust → /tag.php?name=rust
[[server.rewrite]]
from = "^/tag/([^/]+)$"
to = "/tag.php?name=$1"

# 文章分页：/list/2 → /list.php?page=2
[[server.rewrite]]
from = "^/list/(\\d+)$"
to = "/list.php?page=$1"

# 产品详情：/product/iphone-15 → /product.php?slug=$1
[[server.rewrite]]
from = "^/product/([a-zA-Z0-9_-]+)$"
to = "/product.php?slug=$1"

# RESTful API 风格：/api/v1/users/42 → /api.php?action=users&id=42
[[server.rewrite]]
from = "^/api/v1/(\\w+)/(\\d+)$"
to = "/api.php?action=$1&id=$2"


# =============================================
# CGI 解释器配置（执行 PHP/Python 等脚本）
# =============================================
# 当请求的文件扩展名匹配时，自动调用指定的解释器执行

# PHP 解释器
[[server.cgi]]
extensions = [".php", ".phtml", ".php5"]
interpreter = "/usr/bin/php-cgi"

# Python 解释器
[[server.cgi]]
extensions = [".py", ".cgi"]
interpreter = "/usr/bin/python3"

# Perl 解释器
[[server.cgi]]
extensions = [".pl"]
interpreter = "/usr/bin/perl"


# =============================================
# 路径规则（Location）
# =============================================

# 反向代理：将 /api 请求转发到后端服务
[[server.location]]
path = "/api"
proxy_pass = "http://127.0.0.1:3000"
proxy_headers = ["X-Real-IP", "X-Forwarded-For", "X-Forwarded-Proto"]

# 负载均衡：将 /api/v2 请求分发到多个后端（替代单个 proxy_pass）
# [[server.location]]
# path = "/api/v2"
# load_balance_strategy = "round_robin"
# proxy_headers = ["X-Forwarded-For"]
# 
# [[server.location.load_balance_targets]]
# url = "http://127.0.0.1:3001"
# weight = 5
# 
# [[server.location.load_balance_targets]]
# url = "http://127.0.0.1:3002"
# weight = 3
# 
# [[server.location.load_balance_targets]]
# url = "http://127.0.0.1:3003"
# weight = 2

# 静态文件：/static 路径映射到自定义目录
[[server.location]]
path = "/static"
root = "/var/www/static"
expires = "7d"

# 上传目录：/uploads 映射并设置长缓存
[[server.location]]
path = "/uploads"
root = "/data/uploads"
expires = "30d"

# 管理后台：/admin 使用独立解释器
[[server.location]]
path = "/admin"
root = "./admin"
cgi = { interpreter = "/usr/bin/php-cgi", extensions = [".php", ".phtml"] }


# =============================================
# 站点 #2 - 博客
# =============================================
[[server]]
bind = "0.0.0.0:8081"
root = "./www2"
domains = ["blog.example.com"]
threads = 2
upload_max_size = "50MB"
cache_enabled = true
cache_ttl = "2h"
directory_listing = true

# 博客的伪静态规则
[[server.rewrite]]
from = "^/post/(\\d+)/([^/]+)$"
to = "/post.php?id=$1"

[[server.rewrite]]
from = "^/archive/(\\d{4})/(\\d{2})$"
to = "/archive.php?year=$1&month=$2"

[[server.rewrite]]
from = "^/feed$"
to = "/feed.xml"

# 博客的去哪儿都用PHP
[[server.cgi]]
extensions = [".php", ".phtml"]
interpreter = "/usr/bin/php-cgi"
"#;
