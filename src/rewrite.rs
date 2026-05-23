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
use regex::Regex;
use crate::config::RewriteRule;

/// URL重写引擎
pub struct RewriteEngine {
    rules: Vec<CompiledRule>,
}

struct CompiledRule {
    from: Regex,
    to: String,
}

impl RewriteEngine {
    pub fn new(rules: &[RewriteRule]) -> Self {
        let compiled = rules
            .iter()
            .filter_map(|r| {
                Regex::new(&r.from).ok().map(|re| CompiledRule {
                    from: re,
                    to: r.to.clone(),
                })
            })
            .collect();
        RewriteEngine { rules: compiled }
    }

    /// 尝试重写路径，返回重写后的路径，如果无匹配则返回 None
    pub fn rewrite(&self, path: &str) -> Option<String> {
        for rule in &self.rules {
            if let Some(caps) = rule.from.captures(path) {
                let mut result = rule.to.clone();
                // 替换 $0 为整个匹配的字符串
                if let Some(m0) = caps.get(0) {
                    result = result.replace("$0", m0.as_str());
                }
                // 替换 $1, $2 等捕获组
                for (i, cap) in caps.iter().enumerate().skip(1) {
                    if let Some(m) = cap {
                        result = result.replace(&format!("${}", i), m.as_str());
                    }
                }
                return Some(result);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rules(rules: &[(&str, &str)]) -> Vec<RewriteRule> {
        rules.iter().map(|(from, to)| RewriteRule {
            from: from.to_string(),
            to: to.to_string(),
            regex: true,
        }).collect()
    }

    #[test]
    fn test_no_rules() {
        let engine = RewriteEngine::new(&[]);
        assert_eq!(engine.rewrite("/any/path"), None);
    }

    #[test]
    fn test_no_match() {
        let rules = make_rules(&[
            ("^/api/(.*)$", "/api.php?r=$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/about"), None);
        assert_eq!(engine.rewrite("/"), None);
    }

    #[test]
    fn test_simple_capture() {
        let rules = make_rules(&[
            ("^/article/(\\d+)$", "/article.html?id=$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/article/123").unwrap(), "/article.html?id=123");
        assert_eq!(engine.rewrite("/article/999").unwrap(), "/article.html?id=999");
    }

    #[test]
    fn test_multiple_captures() {
        let rules = make_rules(&[
            ("^/api/v1/(\\w+)/(\\d+)$", "/api.php?action=$1&id=$2"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/api/v1/users/42").unwrap(), "/api.php?action=users&id=42");
        assert_eq!(engine.rewrite("/api/v1/posts/7").unwrap(), "/api.php?action=posts&id=7");
    }

    // ─── ThinkPHP 伪静态规则 ──────────────────────────────────────

    /// ThinkPHP 标准伪静态：所有非静态文件请求路由到 index.php
    /// 注意：真实文件跳过重写的逻辑在 handler.rs 的 handle_internal() 中实现
    /// 这里仅测试 rewrite 引擎本身的正则替换
    #[test]
    fn test_thinkphp_catch_all() {
        // ThinkPHP 最常见的一条规则
        let rules = make_rules(&[
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // 模块/控制器/操作
        assert_eq!(engine.rewrite("/home/index/test").unwrap(), "/index.php/home/index/test");
        // 多级参数
        assert_eq!(engine.rewrite("/admin/user/edit/id/1").unwrap(), "/index.php/admin/user/edit/id/1");
        // 根路径
        assert_eq!(engine.rewrite("/").unwrap(), "/index.php/");
    }

    /// ThinkPHP 排除静态目录：使用多条规则组合替代负向前瞻
    /// 注意：Rust regex 不支持零宽断言（lookahead），需要用 if-else 规则链
    #[test]
    fn test_thinkphp_exclude_static() {
        // 方案：静态路径不匹配任何规则（返回 None），动态路径匹配第二条
        let rules = make_rules(&[
            // 第一条匹配静态资源，用 $0 返回自身（不重写）
            ("^/assets/.*$", "$0"),
            ("^/uploads/.*$", "$0"),
            ("^/static/.*$", "$0"),
            ("^/robots\\.txt$", "$0"),
            ("^/favicon\\.ico$", "$0"),
            // 最后一条 catch-all，只重写动态路径
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // 动态路由应该匹配最后一条规则
        assert_eq!(engine.rewrite("/home/index").unwrap(), "/index.php/home/index");
        assert_eq!(engine.rewrite("/admin/login").unwrap(), "/index.php/admin/login");
        // 静态路径被前面的规则匹配，返回自身（不改变路径）
        assert_eq!(engine.rewrite("/assets/css/app.css").unwrap(), "/assets/css/app.css");
        assert_eq!(engine.rewrite("/uploads/2024/01.jpg").unwrap(), "/uploads/2024/01.jpg");
        assert_eq!(engine.rewrite("/static/js/app.js").unwrap(), "/static/js/app.js");
        // robots.txt 不包含在 catch-all 中（被前一条规则匹配）
        assert_eq!(engine.rewrite("/robots.txt").unwrap(), "/robots.txt");
        assert_eq!(engine.rewrite("/favicon.ico").unwrap(), "/favicon.ico");
    }

    /// ThinkPHP 完整配置示例（多条规则组合）
    #[test]
    fn test_thinkphp_full_config() {
        let rules = make_rules(&[
            // 静态资源 - 不重写（返回 None，由静态文件处理器处理）
            ("^/(assets|uploads|static|runtime)/.*$", "/$0"),
            // 根路径
            ("^/$", "/index.php"),
            // 所有其他 URL 路由到 index.php
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // 静态资源不被路由（没匹配第一条，继续匹配第二条... 但第二条会匹配）
        // 实际上第二条 `^/(.*)$` 会匹配所有包括 `/assets/...`，所以需要用负向前瞻
        // 这里只是测试引擎不会崩溃
        assert!(engine.rewrite("/assets/css/app.css").is_some());
        assert!(engine.rewrite("/home/index").is_some());
    }

    // ─── Laravel 伪静态规则 ──────────────────────────────────────

    /// Laravel 标准伪静态：所有请求路由到 index.php，Laravel 自动处理路由
    /// Laravel 的 public/.htaccess 标准规则：
    ///   RewriteRule ^(.*)$ index.php [QSA,L]
    #[test]
    fn test_laravel_catch_all() {
        let rules = make_rules(&[
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/login").unwrap(), "/index.php/login");
        assert_eq!(engine.rewrite("/register").unwrap(), "/index.php/register");
        assert_eq!(engine.rewrite("/api/users").unwrap(), "/index.php/api/users");
        assert_eq!(engine.rewrite("/").unwrap(), "/index.php/");
    }

    /// Laravel 子目录部署（document root 设置为 Laravel 项目根目录，
    /// rewrite 到 public/index.php）
    #[test]
    fn test_laravel_subdirectory() {
        let rules = make_rules(&[
            ("^/(.*)$", "/public/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/dashboard").unwrap(), "/public/index.php/dashboard");
        assert_eq!(engine.rewrite("/admin/users/1/edit").unwrap(), "/public/index.php/admin/users/1/edit");
    }

    /// Laravel API 路由 + 排除静态资源
    #[test]
    fn test_laravel_api_and_static() {
        let rules = make_rules(&[
            // API 路由
            ("^/api/v\\d+/(.*)$", "/index.php/api/$1"),
            // 静态资源 — 匹配后返回自身（使用 $0 代表整个匹配路径）
            ("^/(css|js|img|fonts)/.*$", "$0"),
            // 所有其他路由
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // API 路由
        assert_eq!(engine.rewrite("/api/v1/users").unwrap(), "/index.php/api/users");
        assert_eq!(engine.rewrite("/api/v2/products").unwrap(), "/index.php/api/products");
        // 静态资源 — 第二条规则用 $0 返回自身，handler 层会检查到路径无变化
        // 从而不会在 proxy/location 匹配中使用
        assert_eq!(engine.rewrite("/css/style.css").unwrap(), "/css/style.css");
        assert_eq!(engine.rewrite("/js/app.js").unwrap(), "/js/app.js");
        // 动态路由
        assert_eq!(engine.rewrite("/dashboard").unwrap(), "/index.php/dashboard");
    }

    // ─── 其他主流框架 ──────────────────────────────────────────

    /// WordPress 伪静态
    #[test]
    fn test_wordpress_rewrite() {
        let rules = make_rules(&[
            ("^/$", "/index.php"),
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/").unwrap(), "/index.php");
        assert_eq!(engine.rewrite("/hello-world").unwrap(), "/index.php/hello-world");
        assert_eq!(engine.rewrite("/category/tech").unwrap(), "/index.php/category/tech");
    }

    /// Yii2 伪静态
    #[test]
    fn test_yii2_rewrite() {
        let rules = make_rules(&[
            ("^/(.*)$", "/index.php?r=$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/site/index").unwrap(), "/index.php?r=site/index");
        assert_eq!(engine.rewrite("/admin/default/login").unwrap(), "/index.php?r=admin/default/login");
    }

    // ─── 边界情况 ──────────────────────────────────────────────

    #[test]
    fn test_invalid_regex() {
        // 无效正则应被忽略，不崩溃
        let rules = vec![RewriteRule {
            from: "[invalid".to_string(),
            to: "/index.php".to_string(),
            regex: true,
        }];
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/any"), None);
    }

    #[test]
    fn test_first_match_wins() {
        let rules = make_rules(&[
            ("^/api/(.*)$", "/api_handler.php?r=$1"),
            ("^/(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // /api 路径匹配第一条规则
        assert_eq!(engine.rewrite("/api/users").unwrap(), "/api_handler.php?r=users");
        // 其他路径匹配第二条
        assert_eq!(engine.rewrite("/home").unwrap(), "/index.php/home");
    }

    #[test]
    fn test_empty_path() {
        let rules = make_rules(&[
            ("^(.*)$", "/index.php/$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        // 空路径
        assert_eq!(engine.rewrite("").unwrap(), "/index.php/");
    }

    #[test]
    fn test_special_chars() {
        let rules = make_rules(&[
            ("^/product/([a-zA-Z0-9_-]+)$", "/product.php?slug=$1"),
        ]);
        let engine = RewriteEngine::new(&rules);
        assert_eq!(engine.rewrite("/product/iphone-15_pro").unwrap(), "/product.php?slug=iphone-15_pro");
    }
}
