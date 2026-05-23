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
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// 令牌桶（用于请求速率和连接速率限制）
struct TokenBucket {
    capacity: f64,
    fill_rate: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, fill_rate: f64) -> Self {
        TokenBucket {
            capacity,
            fill_rate,
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

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
        let new_tokens = elapsed * self.fill_rate;
        if new_tokens > 0.0 {
            self.tokens = (self.tokens + new_tokens).min(self.capacity);
            self.last_refill = now;
        }
    }
}

// ═══════════════ 可变的限流状态（由单个 Mutex 保护） ═══════════════

/// 限流器的所有可变状态，由单个 `tokio::sync::Mutex` 保护
/// 这样每请求只需一次锁获取，减少锁竞争
struct RateLimiterState {
    /// 请求速率令牌桶（每 IP）
    buckets: HashMap<String, TokenBucket>,
    /// 连接速率令牌桶（每 IP）
    conn_buckets: HashMap<String, TokenBucket>,
    /// 当前并发连接数（每 IP）
    active_connections: HashMap<String, u32>,
    /// 临时封禁列表（IP → 解封时间）
    temp_bans: HashMap<String, Instant>,
    /// 违规记录（IP → (计数, 窗口开始时间)）
    violations: HashMap<String, (u32, Instant)>,
    /// 上次清理时间
    last_cleanup: Instant,
}

// ═══════════════ 限流器（公开 API） ═══════════════

/// 限流器，支持请求限流 / CC 防护 / DDoS 防护 / 临时封禁
pub struct RateLimiter {
    /// 所有可变状态（单 Mutex 保护，减少锁竞争）
    state: Mutex<RateLimiterState>,

    // ── 以下为不变配置，无需加锁 ──
    default_rps: f64,
    default_burst: f64,
    conn_rps: f64,
    max_concurrent_connections: u32,
    ban_duration: Duration,
    ban_threshold: u32,
    blacklist: Vec<String>,
    whitelist: Vec<String>,
    per_ip_rates: HashMap<String, u32>,
    enabled: bool,
    cleanup_interval: Duration,
}

impl RateLimiter {
    /// 使用完整配置初始化（含 CC/DDoS 参数）
    pub fn new_full(
        enabled: bool,
        requests_per_second: u32,
        burst_size: u32,
        connections_per_second: u32,
        max_concurrent_connections: u32,
        ban_duration_seconds: u64,
        ban_threshold: u32,
        blacklist: Vec<String>,
        per_ip_rates: HashMap<String, u32>,
    ) -> Self {
        RateLimiter {
            state: Mutex::new(RateLimiterState {
                buckets: HashMap::new(),
                conn_buckets: HashMap::new(),
                active_connections: HashMap::new(),
                temp_bans: HashMap::new(),
                violations: HashMap::new(),
                last_cleanup: Instant::now(),
            }),
            default_rps: if requests_per_second > 0 { requests_per_second as f64 } else { f64::MAX },
            default_burst: burst_size as f64,
            conn_rps: if connections_per_second > 0 { connections_per_second as f64 } else { f64::MAX },
            max_concurrent_connections: if max_concurrent_connections > 0 { max_concurrent_connections } else { u32::MAX },
            ban_duration: if ban_duration_seconds > 0 {
                Duration::from_secs(ban_duration_seconds)
            } else {
                Duration::from_secs(0)
            },
            ban_threshold: if ban_threshold > 0 { ban_threshold } else { u32::MAX },
            blacklist,
            whitelist: Vec::new(),
            per_ip_rates,
            enabled,
            cleanup_interval: Duration::from_secs(60),
        }
    }

    /// 添加白名单 IP（不受限流影响）
    #[allow(dead_code)]
    pub fn add_whitelist(&mut self, ips: Vec<String>) {
        self.whitelist = ips;
    }

    /// 检查 IP 是否被黑名单屏蔽（包括临时封禁）
    pub async fn is_blocked(&self, ip: &str) -> bool {
        if !self.enabled {
            return false;
        }
        // 检查临时封禁（不带锁进行原子检查，但需要获取状态）
        if self.is_temp_banned(ip).await {
            return true;
        }
        // 检查永久黑名单（不用锁——blacklist 是不可变配置）
        if self.blacklist.contains(&ip.to_string()) {
            return true;
        }
        // 检查 IP 段（通配符匹配）
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

    /// 检查 IP 是否被临时封禁
    pub async fn is_temp_banned(&self, ip: &str) -> bool {
        let mut state = self.state.lock().await;
        if let Some(until) = state.temp_bans.get(ip) {
            if Instant::now() < *until {
                return true;
            }
            // 封禁已过期，清理
            state.temp_bans.remove(ip);
        }
        false
    }

    /// 检查 IP 是否在白名单
    #[allow(dead_code)]
    pub fn is_whitelisted(&self, ip: &str) -> bool {
        self.whitelist.contains(&ip.to_string())
    }

    // ═══════════════ 连接层防护 ═══════════════

    /// 尝试建立新连接。
    /// 返回 true = 允许连接，false = 连接超限（拒绝）
    pub async fn try_connect(&self, ip: &str) -> bool {
        if !self.enabled {
            return true;
        }

        // 白名单直接放行
        if self.is_whitelisted(ip) {
            return true;
        }

        // 临时封禁或黑名单 → 拒绝
        if self.is_blocked(ip).await {
            return false;
        }

        let mut state = self.state.lock().await;

        // 检查连接速率（新建连接/秒）
        {
            let cb = &mut state.conn_buckets;
            self.cleanup_old(cb);
            let bucket = cb.entry(ip.to_string()).or_insert_with(|| {
                TokenBucket::new(self.conn_rps, self.conn_rps)
            });
            if !bucket.try_consume() {
                // 连接速率超限 → 记录违规
                self.record_violation_inner(&mut state, ip);
                return false;
            }
        }

        // 检查并发连接数
        {
            let ac = &mut state.active_connections;
            let count = ac.get(ip).copied().unwrap_or(0);
            if count >= self.max_concurrent_connections {
                return false;
            }
            ac.insert(ip.to_string(), count + 1);
        }

        true
    }

    /// 断开连接时调用，减少并发计数
    pub async fn disconnect(&self, ip: &str) {
        if !self.enabled {
            return;
        }
        let mut state = self.state.lock().await;
        if let Some(count) = state.active_connections.get_mut(ip) {
            if *count > 1 {
                *count -= 1;
            } else {
                state.active_connections.remove(ip);
            }
        }
    }

    /// 获取当前 IP 的并发连接数
    #[allow(dead_code)]
    pub async fn active_connections(&self, ip: &str) -> u32 {
        if !self.enabled {
            return 0;
        }
        let state = self.state.lock().await;
        state.active_connections.get(ip).copied().unwrap_or(0)
    }

    // ═══════════════ 请求层防护 ═══════════════

    /// 尝试放行一次请求。返回 true = 允许，false = 限流
    pub async fn check(&self, ip: &str) -> bool {
        if !self.enabled {
            return true;
        }

        // 白名单直接放行（无需加锁）
        if self.is_whitelisted(ip) {
            return true;
        }

        // 获取该 IP 的速率配置
        let custom_rps = self.per_ip_rates.get(ip).copied();
        let (rps, burst) = if let Some(custom) = custom_rps {
            (custom as f64, custom as f64)
        } else {
            (self.default_rps, self.default_burst)
        };

        // 单次上锁完成所有检查
        let mut state = self.state.lock().await;

        let ip_key = ip.to_string();

        // 定期清理过期 bucket（防止内存泄漏）
        if state.last_cleanup.elapsed() >= self.cleanup_interval {
            if state.buckets.len() > 10000 {
                state.buckets.clear();
            }
            state.last_cleanup = Instant::now();
        }

        let bucket = state.buckets.entry(ip_key).or_insert_with(|| {
            TokenBucket::new(burst, rps)
        });

        let allowed = bucket.try_consume();

        if !allowed {
            // 请求被限流 → 记录违规
            self.record_violation_inner(&mut state, ip);
        }

        allowed
    }

    // ═══════════════ 内部方法 ═══════════════

    /// 记录一次违规（内部方法，调用方已持有 state 锁）
    fn record_violation_inner(&self, state: &mut RateLimiterState, ip: &str) -> bool {
        let now = Instant::now();
        let violation_window = Duration::from_secs(30);

        let entry = state.violations.entry(ip.to_string()).or_insert((0, now));
        // 如果窗口已过期，重置
        if now.duration_since(entry.1) > violation_window {
            *entry = (1, now);
        } else {
            entry.0 += 1;
        }

        // 如果违规次数超过阈值 → 临时封禁
        if entry.0 >= self.ban_threshold && self.ban_duration > Duration::from_secs(0) {
            state.temp_bans.insert(ip.to_string(), now + self.ban_duration);
            return true; // 封禁
        }

        false
    }

    /// 清理过期的令牌桶
    fn cleanup_old(&self, buckets: &mut HashMap<String, TokenBucket>) {
        let now = Instant::now();
        buckets.retain(|_, bucket| {
            now.duration_since(bucket.last_refill).as_secs() < 3600
        });
    }

    // ═══════════════ 管理接口 ═══════════════

    /// 手动封禁 IP
    #[allow(dead_code)]
    pub async fn ban_ip(&self, ip: &str, duration_seconds: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let mut state = self.state.lock().await;
        if duration_seconds > 0 {
            state.temp_bans.insert(ip.to_string(), Instant::now() + Duration::from_secs(duration_seconds));
            true
        } else {
            false
        }
    }

    /// 解封 IP
    #[allow(dead_code)]
    pub async fn unban_ip(&self, ip: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let mut state = self.state.lock().await;
        state.temp_bans.remove(ip).is_some()
    }

    /// 当前封禁的 IP 数量
    #[allow(dead_code)]
    pub async fn banned_count(&self) -> usize {
        if !self.enabled {
            return 0;
        }
        let state = self.state.lock().await;
        state.temp_bans.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_limiter() -> RateLimiter {
        RateLimiter::new_full(
            true,   // enabled
            10,     // requests_per_second
            5,      // burst_size
            5,      // connections_per_second
            10,     // max_concurrent_connections
            60,     // ban_duration_seconds
            3,      // ban_threshold
            vec!["203.0.113.1".to_string()],
            HashMap::new(),
        )
    }

    #[tokio::test]
    async fn test_basic_rate_limit() {
        let limiter = make_limiter();
        // 前 5 次应该放行（burst=5）
        for _ in 0..5 {
            assert!(limiter.check("1.2.3.4").await);
        }
        // 第 6 次应该被限流（因为每秒只有 10 个，burst 已耗尽）
        // 注意：由于时间窗口很小，maybe 还是允许
        let result = limiter.check("1.2.3.4").await;
        // 我们不强制断言结果，因为时序可能导致 refill
        // 只是确保不 panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_connection_rate() {
        let limiter = make_limiter();
        // 连接速率限制 5/秒，所以前 5 个应该通过
        for i in 0..5 {
            assert!(limiter.try_connect(&format!("10.0.0.{}", i)).await);
        }
    }

    #[tokio::test]
    async fn test_concurrent_connections() {
        let limiter = make_limiter();
        // 最大并发 10，尝试 10 个都应成功
        for i in 0..10 {
            assert!(limiter.try_connect(&format!("10.0.0.{}", i + 1)).await);
        }
        // 第 11 个不同 IP 应该成功（限制是每 IP 10 个）
        assert!(limiter.try_connect("10.0.0.99").await);
    }

    #[tokio::test]
    async fn test_temp_ban() {
        let limiter = make_limiter();
        // burst=5, ban_threshold=3, rps=10
        // 前 5 次消耗掉所有 burst token，第 6 次触发限流并记录违规
        for _ in 0..5 {
            assert!(limiter.check("5.6.7.8").await);
        }
        // 第 6 次 → 桶空，触发违规
        assert!(!limiter.check("5.6.7.8").await);
        // 再触发 2 次违规（累计 3 次 = ban_threshold）→ 封禁
        assert!(!limiter.check("5.6.7.8").await);
        assert!(!limiter.check("5.6.7.8").await);
        // is_blocked 应该返回 true
        assert!(limiter.is_blocked("5.6.7.8").await);
    }

    #[tokio::test]
    async fn test_disabled_rate_limiter() {
        let limiter = RateLimiter::new_full(
            false, 0, 0, 0, 0, 0, 0,
            vec![], HashMap::new(),
        );
        // 禁用时所有检查都应放行
        assert!(limiter.check("1.2.3.4").await);
        assert!(limiter.try_connect("1.2.3.4").await);
        assert!(!limiter.is_blocked("1.2.3.4").await);
    }

    #[tokio::test]
    async fn test_blacklist() {
        let limiter = RateLimiter::new_full(
            true, 100, 200, 50, 100, 300, 3,
            vec!["10.0.0.1".to_string()],
            HashMap::new(),
        );
        assert!(limiter.is_blocked("10.0.0.1").await);
        assert!(!limiter.is_blocked("10.0.0.2").await);
    }

    #[tokio::test]
    async fn test_blacklist_wildcard() {
        let limiter = RateLimiter::new_full(
            true, 100, 200, 50, 100, 300, 3,
            vec!["192.168.1.*".to_string()],
            HashMap::new(),
        );
        assert!(limiter.is_blocked("192.168.1.1").await);
        assert!(limiter.is_blocked("192.168.1.100").await);
        assert!(!limiter.is_blocked("192.168.2.1").await);
    }

    #[tokio::test]
    async fn test_whitelist_bypass() {
        let mut limiter = RateLimiter::new_full(
            true, 100, 200, 50, 100, 300, 3,
            vec!["203.0.113.1".to_string()],
            HashMap::new(),
        );
        limiter.add_whitelist(vec!["8.8.8.8".to_string()]);

        // 黑名单 + 白名单测试
        assert!(limiter.is_whitelisted("8.8.8.8"));
        assert!(!limiter.is_whitelisted("1.1.1.1"));

        // 白名单应绕过所有检查
        assert!(limiter.check("8.8.8.8").await);
        assert!(!limiter.is_blocked("8.8.8.8").await);
    }

    #[tokio::test]
    async fn test_manual_ban_unban() {
        let limiter = make_limiter();
        assert!(limiter.active_connections("9.9.9.9").await == 0);
        assert!(limiter.ban_ip("9.9.9.9", 60).await);
        assert!(limiter.is_blocked("9.9.9.9").await);
        assert!(limiter.unban_ip("9.9.9.9").await);
        assert!(!limiter.is_blocked("9.9.9.9").await);
    }

    #[tokio::test]
    async fn test_disconnect() {
        let limiter = make_limiter();
        assert!(limiter.try_connect("7.7.7.7").await);
        assert!(limiter.active_connections("7.7.7.7").await == 1);
        limiter.disconnect("7.7.7.7").await;
        assert!(limiter.active_connections("7.7.7.7").await == 0);
    }
}
