use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 负载均衡后端目标
#[derive(Debug, Clone)]
pub struct BalanceTarget {
    pub url: String,
    pub weight: u32,
}

/// 负载均衡器（支持加权轮询）
pub struct LoadBalancer {
    targets: Vec<Arc<BalanceTarget>>,
    /// 加权轮询的虚拟索引
    index: AtomicUsize,
    client: Client<HttpConnector, Full<Bytes>>,
    /// 后端健康状态（true=健康）
    healthy: Vec<Arc<AtomicUsize>>, // 0=未知, 1=健康, 2=不健康
}

impl LoadBalancer {
    pub fn new(targets: Vec<BalanceTarget>) -> Self {
        let client = Client::builder(
            hyper_util::rt::TokioExecutor::new()
        ).build(HttpConnector::new());

        let total_weight: u32 = targets.iter().map(|t| t.weight.max(1)).sum();

        // 按权重构建 expanded target 列表用于加权轮询
        let mut expanded = Vec::new();
        let mut healthy = Vec::new();
        for t in &targets {
            let w = t.weight.max(1);
            for _ in 0..w {
                expanded.push(Arc::new(t.clone()));
            }
            healthy.push(Arc::new(AtomicUsize::new(0))); // 默认未知
        }

        // 如果权重总和超过100，缩减（防止内存膨胀）
        let actual_targets: Vec<Arc<BalanceTarget>> = if total_weight > 100 {
            // 使用原始列表，纯轮询
            targets.into_iter().map(|t| Arc::new(t)).collect()
        } else {
            expanded
        };

        LoadBalancer {
            targets: actual_targets,
            index: AtomicUsize::new(0),
            client,
            healthy,
        }
    }

    /// 获取下一个后端 URL（轮询）
    pub fn next_url(&self) -> Option<String> {
        if self.targets.is_empty() {
            return None;
        }
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % self.targets.len();
        Some(self.targets[idx].url.clone())
    }

    /// 执行负载均衡代理请求
    pub async fn proxy_request(
        &self,
        method: &hyper::Method,
        uri: &http::Uri,
        headers: &http::HeaderMap,
        body_bytes: Bytes,
        proxy_headers: &[String],
    ) -> Result<Response<Incoming>, String> {
        let target_url = self.next_url()
            .ok_or_else(|| "没有可用的后端服务器".to_string())?;

        // 复用后端的一致性 URL 构建
        let uri_path = uri.path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        let full_url = format!("{}{}", target_url.trim_end_matches('/'), uri_path);
        let proxy_uri: hyper::Uri = full_url.parse()
            .map_err(|e| format!("代理地址格式错误: {}", e))?;

        // 构建代理请求
        let mut proxy_req_builder = Request::builder()
            .uri(&proxy_uri)
            .method(method.clone());

        // 复制请求头（过滤逐跳头）
        for (name, value) in headers.iter() {
            let name_lower = name.as_str().to_lowercase();
            if HOP_BY_HOP.contains(&name_lower.as_str()) {
                continue;
            }
            proxy_req_builder = proxy_req_builder.header(name.as_str(), value.as_bytes());
        }

        // 添加代理头
        let proxy_headers_set: std::collections::HashSet<String> =
            proxy_headers.iter().cloned().collect();

        if let Some(host) = uri.host() {
            if proxy_headers_set.contains("X-Forwarded-For") {
                proxy_req_builder = proxy_req_builder.header("X-Forwarded-For", host);
            }
            if proxy_headers_set.contains("X-Forwarded-Proto") {
                let scheme = uri.scheme_str().unwrap_or("http");
                proxy_req_builder = proxy_req_builder.header("X-Forwarded-Proto", scheme);
            }
        }

        let proxy_req = proxy_req_builder.body(Full::from(body_bytes))
            .map_err(|e| format!("构建代理请求失败: {}", e))?;

        self.client.request(proxy_req).await
            .map_err(|e| format!("代理请求失败: {}", e))
    }

    /// 简单健康检查（标记后端状态）
    pub fn mark_healthy(&self, idx: usize) {
        if idx < self.healthy.len() {
            self.healthy[idx].store(1, Ordering::Relaxed);
        }
    }

    pub fn mark_unhealthy(&self, idx: usize) {
        if idx < self.healthy.len() {
            self.healthy[idx].store(2, Ordering::Relaxed);
        }
    }
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

impl std::fmt::Debug for LoadBalancer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadBalancer")
            .field("targets", &self.targets)
            .field("healthy_count", &self.healthy.len())
            .finish()
    }
}
