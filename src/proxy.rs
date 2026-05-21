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
use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::Incoming;
use hyper::{Request, Response};
use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;

/// 反向代理客户端
pub struct ProxyClient {
    client: Client<HttpConnector, Full<Bytes>>,
}

impl ProxyClient {
    pub fn new() -> Self {
        let mut http_connector = HttpConnector::new();
        http_connector.set_connect_timeout(Some(Duration::from_secs(10)));

        let client = Client::builder(
            hyper_util::rt::TokioExecutor::new()
        )
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(16)
            .build(http_connector);
        ProxyClient { client }
    }

    /// 从已解析的请求部分执行反向代理
    pub async fn proxy_request_from_parts(
        &self,
        method: &hyper::Method,
        uri: &http::Uri,
        headers: &http::HeaderMap,
        body_bytes: Bytes,
        target_url: &str,
        proxy_headers: &[String],
    ) -> Result<Response<Incoming>, String> {
        let method = method.clone();
        let uri_path = uri.path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());
        let scheme = uri.scheme_str().unwrap_or("http").to_string();

        let orig_headers: Vec<(String, Vec<u8>)> = headers.iter()
            .filter(|(name, _)| {
                let n = name.as_str().to_lowercase();
                !HOP_BY_HOP.contains(&n.as_str())
            })
            .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
            .collect();

        let target = format!("{}{}", target_url.trim_end_matches('/'), uri_path);
        let proxy_uri: hyper::Uri = target.parse()
            .map_err(|e| format!("代理地址格式错误: {}", e))?;

        // SSRF防护：阻止代理到内部/私有地址的请求
        if let Some(host) = proxy_uri.host() {
            let addrs = (host, 0u16).to_socket_addrs()
                .map_err(|e| format!("DNS解析失败: {}", e))?;
            for addr in addrs {
                if is_private_ip(&addr.ip()) {
                    return Err("SSRF blocked: proxy to internal/private addresses is not allowed".to_string());
                }
            }
        }

        let mut proxy_req_builder = Request::builder()
            .uri(&proxy_uri)
            .method(&method);

        let proxy_headers_set: HashSet<String> = proxy_headers.iter().cloned().collect();

        for (name, value) in &orig_headers {
            if let Ok(header_name) = hyper::header::HeaderName::from_bytes(name.as_bytes()) {
                proxy_req_builder = proxy_req_builder.header(header_name, &value[..]);
            }
        }

        // Add proxy headers
        if let Some(addr) = uri.host() {
            if proxy_headers_set.contains("X-Forwarded-For") {
                proxy_req_builder = proxy_req_builder.header("X-Forwarded-For", addr);
            }
            if proxy_headers_set.contains("X-Forwarded-Proto") {
                proxy_req_builder = proxy_req_builder.header("X-Forwarded-Proto", scheme.as_str());
            }
        }

        let proxy_req = proxy_req_builder.body(Full::from(body_bytes))
            .map_err(|e| format!("构建代理请求失败: {}", e))?;

        let resp = self.client.request(proxy_req).await
            .map_err(|e| format!("代理请求失败: {}", e))?;

        Ok(resp)
    }
}

/// 检查IP地址是否为私有/内部地址（SSRF防护）
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.octets()[0] == 127                    // 127.0.0.0/8 loopback
                || ipv4.octets()[0] == 10              // 10.0.0.0/8
                || (ipv4.octets()[0] == 172            // 172.16.0.0/12
                    && ipv4.octets()[1] >= 16
                    && ipv4.octets()[1] <= 31)
                || (ipv4.octets()[0] == 192            // 192.168.0.0/16
                    && ipv4.octets()[1] == 168)
        }
        IpAddr::V6(ipv6) => ipv6 == &Ipv6Addr::LOCALHOST,  // ::1
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

impl Default for ProxyClient {
    fn default() -> Self {
        Self::new()
    }
}
