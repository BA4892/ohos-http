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
use crate::config::ServerConfig;
use chrono::Local;

/// 打印美观的启动画面
pub fn print_startup_banner(configs: &[ServerConfig], worker_count: usize) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let version = env!("CARGO_PKG_VERSION");

    let border = "═══════════════════════════════════════════════════════════════";

    println!();
    println!("  ╔{border}╗");
    println!("  ║                                                               ║");
    println!("  ║     ██████╗ ██╗  ██╗ ██████╗ ███████╗                        ║");
    println!("  ║    ██╔═══╝ ██║  ██║██╔═══╝ ██╔════╝                        ║");
    println!("  ║    ██║     ███████║██║     █████╗                          ║");
    println!("  ║    ██║     ██╔══██║██║     ██╔══╝                          ║");
    println!("  ║    ╚██████╗██║  ██║╚██████╗███████╗                        ║");
    println!("  ║     ╚═════╝╚═╝  ╚═╝ ╚═════╝╚══════╝                        ║");
    println!("  ║                                                               ║");
    println!("  ║   ohosHttp v{ver: <8} ─ 高性能 HTTP 服务器                    ║", ver = version);
    println!("  ║   启动时间 : {time:<47}║", time = now);
    println!("  ║   PID      : {pid:<47}║", pid = std::process::id());
    println!("  ╚{border}╝");

    for (i, cfg) in configs.iter().enumerate() {
        let cgi_count = cfg.cgi.len();
        let rewrite_count = cfg.rewrite.len();
        let proxy_count = cfg.location.iter().filter(|l| l.proxy_pass.is_some()).count();
        let location_count = cfg.location.len();

        let cache_status = if cfg.cache_enabled {
            format!("已启用 (TTL: {}, 最大: {})", cfg.cache_ttl, cfg.cache_max_size)
        } else {
            "未启用".to_string()
        };

        println!();
        println!("  ┌─── 站点 #{} ────────────────────────────────────────────────┐", i + 1);
        println!("  │                                                             │");
        println!("  │   绑定地址    │  {:<47}│", cfg.bind);
        println!("  │   根目录      │  {:<47}│", cfg.root);
        if !cfg.domains.is_empty() {
            println!("  │   域名        │  {:<47}│", cfg.domains.join(", "));
        }
        println!("  │   IP直连     │  {:<47}│", if cfg.allow_ip_access { "允许" } else { "仅域名" });
        println!("  │   Worker进程  │  {:<47}│", worker_count);
        println!("  │   上传大小    │  {:<47}│", cfg.upload_max_size);
        println!("  │   缓存        │  {:<47}│", cache_status);
        println!("  │   目录列表    │  {:<47}│", if cfg.directory_listing { "开启" } else { "关闭" });
        if !cfg.access_log.is_none() {
            println!("  │   访问日志    │  {:<47}│", cfg.access_log.as_deref().unwrap_or("-"));
        }
        if cfg.log_rotate_size_bytes > 0 {
            println!("  │   日志轮转    │  大小超过 {} 后自动轮转                               │", cfg.log_rotate_size);
        }
        if !cfg.cors_origin.is_empty() {
            println!("  │   CORS 源     │  {:<47}│", cfg.cors_origin);
            println!("  │   CORS 方法   │  {:<47}│", cfg.cors_methods);
        }
        if let Some(ref cert) = cfg.cert {
            println!("  │   HTTPS 证书  │  {:<47}│", if cert.len() > 43 { format!("{}...", &cert[..40]) } else { cert.clone() });
            println!("  │   HTTPS 状态  │  已启用 (ALPN: h2, http/1.1)                               │");
        }
        if cfg.http3_port != "0" && cfg.http3_port != "" {
            println!("  │   HTTP/3 端口 │  UDP {:<43}│", cfg.http3_port);
        }
        // 限流与安全
        if let Some(ref rl) = cfg.rate_limit {
            if rl.enabled {
                println!("  │   限流       │  每 IP {} req/s (突发: {})                               │", rl.requests_per_second, rl.burst_size);
            }
        }
        if !cfg.blacklist.is_empty() {
            println!("  │   黑名单 IP  │  {} 条                                              │", cfg.blacklist.len());
        }
        if !cfg.per_ip_rates.is_empty() {
            println!("  │   自定义限流  │  {} 个 IP                                              │", cfg.per_ip_rates.len());
        }
        // Session
        if let Some(ref sc) = cfg.session {
            if sc.enabled {
                println!("  │   Session    │  Cookie: {}, TTL: {}s                               │", sc.cookie_name, sc.ttl);
            }
        }
        // 负载均衡
        let lb_count = cfg.location.iter().filter(|l| !l.load_balance_targets.is_empty()).count();
        if lb_count > 0 {
            println!("  │   负载均衡    │  {} 条路径                                              │", lb_count);
        }
        // 管理 API
        if crate::manage::is_manage_enabled() {
            println!("  │   管理 API    │  已启用 (/_ohos/, 需要 Bearer Token 认证)                        │");
        }
        println!("  │   伪静态规则  │  {:<47}│", if rewrite_count > 0 { format!("{} 条", rewrite_count) } else { "无".to_string() });
        println!("  │   代理规则    │  {:<47}│", if proxy_count > 0 { format!("{} 条", proxy_count) } else { "无".to_string() });
        println!("  │   路径规则    │  {:<47}│", if location_count > 0 { format!("{} 条", location_count) } else { "无".to_string() });
        if cgi_count > 0 {
            let cgi_info: String = cfg.cgi.iter()
                .map(|c| format!("{} ({})", c.interpreter, c.extensions.join(", ")))
                .collect::<Vec<_>>()
                .join(" | ");
            println!("  │   CGI解释器   │  {:<47}│", cgi_info);
        }

        // 显示重写规则详情
        if rewrite_count > 0 {
            println!("  │  ─── 伪静态规则 ─────────────────────────────────         │");
            for (j, rule) in cfg.rewrite.iter().enumerate() {
                let from_display = if rule.from.len() > 38 {
                    format!("{}...", &rule.from[..35])
                } else {
                    rule.from.clone()
                };
                let to_display = if rule.to.len() > 38 {
                    format!("{}...", &rule.to[..35])
                } else {
                    rule.to.clone()
                };
                println!("  │    #{:<2}  {:>20}  →  {:<26}│", j + 1, from_display, to_display);
            }
        }

        // 显示代理/路径规则详情
        if proxy_count > 0 || location_count > 0 {
            println!("  │  ─── 路径规则 ─────────────────────────────────         │");
            for (j, loc) in cfg.location.iter().enumerate() {
                if let Some(proxy) = &loc.proxy_pass {
                    let p = if proxy.len() > 48 {
                        format!("{}...", &proxy[..45])
                    } else {
                        proxy.clone()
                    };
                    println!("  │    #{:<2}  {:<15}  →  proxy {:<31}│", j + 1, loc.path, p);
                }
                if let Some(root) = &loc.root {
                    let r = if root.len() > 42 {
                        format!("{}...", &root[..39])
                    } else {
                        root.clone()
                    };
                    println!("  │    #{:<2}  {:<15}  →  root  {:<31}│", j + 1, loc.path, r);
                }
            }
        }

        println!("  │                                                             │");
        println!("  └─────────────────────────────────────────────────────────────┘");
    }

    println!();
    println!("  ╔{border}╗");
    println!("  ║  服务已就绪，按 Ctrl+C 停止服务                                 ║");
    println!("  ╚{border}╝");
    println!();
}
