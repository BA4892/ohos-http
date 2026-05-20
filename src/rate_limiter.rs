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
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 令牌桶，用于单 IP 限流
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    fill_rate: f64, // tokens per second
}

impl TokenBucket {
    fn new(capacity: f64, fill_rate: f64) -> Self {
        TokenBucket {
            tokens: capacity,
            last_refill: Instant::now(),
            capacity,
            fill_rate,
        }
    }

    /// 尝试消费一个令牌，成功返回 true
    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.fill_rate).min(self.capacity);
            self.last_refill = now;
        }
    }
}

/// 多 IP 限流器，支持黑名单和按 IP 自定义速率
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, TokenBucket>>,
    default_rps: f64,
    default_burst: f64,
    blacklist: Vec<String>,
    whitelist: Vec<String>,
    per_ip_rates: HashMap<String, u32>,
    enabled: bool,
    cleanup_interval: Duration,
    last_cleanup: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(
        enabled: bool,
        requests_per_second: u32,
        burst_size: u32,
        blacklist: Vec<String>,
        per_ip_rates: HashMap<String, u32>,
    ) -> Self {
        RateLimiter {
            buckets: Mutex::new(HashMap::new()),
            default_rps: requests_per_second as f64,
            default_burst: burst_size as f64,
            blacklist,
            whitelist: Vec::new(),
            per_ip_rates,
            enabled,
            cleanup_interval: Duration::from_secs(60),
            last_cleanup: Mutex::new(Instant::now()),
        }
    }

    /// 添加白名单 IP（不受限流影响）
    #[allow(dead_code)]
    pub fn add_whitelist(&mut self, ips: Vec<String>) {
        self.whitelist = ips;
    }

    /// 检查 IP 是否被黑名单屏蔽
    pub fn is_blocked(&self, ip: &str) -> bool {
        if !self.enabled {
            return false;
        }
        // 检查完整 IP
        if self.blacklist.contains(&ip.to_string()) {
            return true;
        }
        // 检查 IP 段（简单前缀匹配：避免使用复杂的 CIDR 库）
        for entry in &self.blacklist {
            if entry.ends_with('*') {
                let prefix = entry.trim_end_matches('*');
                if ip.starts_with(prefix) {
                    return true;
                }
            }
        }
        false
    }

    /// 检查 IP 是否在白名单
    #[allow(dead_code)]
    pub fn is_whitelisted(&self, ip: &str) -> bool {
        self.whitelist.contains(&ip.to_string())
    }

    /// 尝试放行一次请求。返回 true = 允许，false = 限流
    #[allow(dead_code)]
    pub fn check(&self, ip: &str) -> bool {
        if !self.enabled {
            return true;
        }

        // 白名单直接放行
        if self.is_whitelisted(ip) {
            return true;
        }

        // 黑名单直接拒绝
        if self.is_blocked(ip) {
            return false;
        }

        // 获取该 IP 的速率配置
        let custom_rps = self.per_ip_rates.get(ip).copied();
        let (rps, burst) = if let Some(custom) = custom_rps {
            (custom as f64, custom as f64)
        } else {
            (self.default_rps, self.default_burst)
        };

        let ip_key = ip.to_string();
        let mut buckets = self.buckets.lock().unwrap();

        // 清理过期 bucket（防止内存泄漏）
        let mut last_cleanup = self.last_cleanup.lock().unwrap();
        if last_cleanup.elapsed() >= self.cleanup_interval {
            // 保留最近活跃的 IP
            if buckets.len() > 10000 {
                buckets.clear();
            }
            *last_cleanup = Instant::now();
        }
        drop(last_cleanup);

        let bucket = buckets.entry(ip_key).or_insert_with(|| {
            TokenBucket::new(burst, rps)
        });

        bucket.try_consume()
    }
}
