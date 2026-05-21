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

/// 多 IP 限流器，支持黑名单、CC/DDoS 防护、自动封禁
pub struct RateLimiter {
    // ─── 请求速率限制 ───
    buckets: Mutex<HashMap<String, TokenBucket>>,
    default_rps: f64,
    default_burst: f64,

    // ─── 连接速率限制（CC 防护：限制每 IP 新建连接数） ───
    conn_buckets: Mutex<HashMap<String, TokenBucket>>,
    conn_rps: f64,

    // ─── 并发连接数限制（DDoS 防护：限制每 IP 最大同时连接数） ───
    active_connections: Mutex<HashMap<String, u32>>,
    max_concurrent_connections: u32,

    // ─── 临时封禁（自动封禁恶意 IP） ───
    temp_bans: Mutex<HashMap<String, Instant>>,
    ban_duration: Duration,
    ban_threshold: u32,
    /// (违规次数, 窗口开始时间) — 30 秒滑动窗口
    violations: Mutex<HashMap<String, (u32, Instant)>>,

    // ─── 黑名单 / 白名单 ───
    blacklist: Vec<String>,
    whitelist: Vec<String>,
    per_ip_rates: HashMap<String, u32>,

    enabled: bool,
    cleanup_interval: Duration,
    last_cleanup: Mutex<Instant>,
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
            buckets: Mutex::new(HashMap::new()),
            default_rps: if requests_per_second > 0 { requests_per_second as f64 } else { f64::MAX },
            default_burst: burst_size as f64,
            conn_buckets: Mutex::new(HashMap::new()),
            conn_rps: if connections_per_second > 0 { connections_per_second as f64 } else { f64::MAX },
            active_connections: Mutex::new(HashMap::new()),
            max_concurrent_connections: if max_concurrent_connections > 0 { max_concurrent_connections } else { u32::MAX },
            temp_bans: Mutex::new(HashMap::new()),
            ban_duration: if ban_duration_seconds > 0 {
                Duration::from_secs(ban_duration_seconds)
            } else {
                Duration::from_secs(0)
            },
            ban_threshold: if ban_threshold > 0 { ban_threshold } else { u32::MAX },
            violations: Mutex::new(HashMap::new()),
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

    /// 检查 IP 是否被黑名单屏蔽（包括临时封禁）
    pub fn is_blocked(&self, ip: &str) -> bool {
        if !self.enabled {
            return false;
        }
        // 检查临时封禁
        if self.is_temp_banned(ip) {
            return true;
        }
        // 检查永久黑名单
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
    pub fn is_temp_banned(&self, ip: &str) -> bool {
        let mut bans = self.temp_bans.lock().unwrap();
        if let Some(until) = bans.get(ip) {
            if Instant::now() < *until {
                return true;
            }
            // 封禁已过期，清理
            bans.remove(ip);
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
    pub fn try_connect(&self, ip: &str) -> bool {
        if !self.enabled {
            return true;
        }

        // 白名单直接放行
        if self.is_whitelisted(ip) {
            return true;
        }

        // 临时封禁或黑名单 → 拒绝
        if self.is_blocked(ip) {
            return false;
        }

        // 检查连接速率（新建连接/秒）
        {
            let mut cb = self.conn_buckets.lock().unwrap();
            self.cleanup_old(&mut cb);
            let bucket = cb.entry(ip.to_string()).or_insert_with(|| {
                TokenBucket::new(self.conn_rps, self.conn_rps)
            });
            if !bucket.try_consume() {
                // 连接速率超限 → 记录违规
                self.record_violation(ip);
                return false;
            }
        }

        // 检查并发连接数
        {
            let mut ac = self.active_connections.lock().unwrap();
            let count = ac.get(ip).copied().unwrap_or(0);
            if count >= self.max_concurrent_connections {
                // 并发连接超限 → 拒绝
                return false;
            }
            ac.insert(ip.to_string(), count + 1);
        }

        true
    }

    /// 断开连接时调用，减少并发计数
    pub fn disconnect(&self, ip: &str) {
        if !self.enabled {
            return;
        }
        let mut ac = self.active_connections.lock().unwrap();
        if let Some(count) = ac.get_mut(ip) {
            if *count > 1 {
                *count -= 1;
            } else {
                ac.remove(ip);
            }
        }
    }

    /// 获取当前 IP 的并发连接数
    #[allow(dead_code)]
    pub fn active_connections(&self, ip: &str) -> u32 {
        if !self.enabled {
            return 0;
        }
        let ac = self.active_connections.lock().unwrap();
        ac.get(ip).copied().unwrap_or(0)
    }

    // ═══════════════ 请求层防护 ═══════════════

    /// 尝试放行一次请求。返回 true = 允许，false = 限流
    pub fn check(&self, ip: &str) -> bool {
        if !self.enabled {
            return true;
        }

        // 白名单直接放行
        if self.is_whitelisted(ip) {
            return true;
        }

        // 临时封禁或黑名单 → 拒绝
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

        // 定期清理过期 bucket（防止内存泄漏）
        let mut last_cleanup = self.last_cleanup.lock().unwrap();
        if last_cleanup.elapsed() >= self.cleanup_interval {
            if buckets.len() > 10000 {
                buckets.clear();
            }
            *last_cleanup = Instant::now();
        }
        drop(last_cleanup);

        let bucket = buckets.entry(ip_key).or_insert_with(|| {
            TokenBucket::new(burst, rps)
        });

        let allowed = bucket.try_consume();

        if !allowed {
            // 请求被限流 → 记录违规
            self.record_violation(ip);
        }

        allowed
    }

    // ═══════════════ 违规与自动封禁 ═══════════════

    /// 记录一次违规，返回 true 表示该 IP 刚被触发自动封禁
    fn record_violation(&self, ip: &str) -> bool {
        if self.ban_duration.is_zero() || self.ban_threshold == u32::MAX {
            return false;
        }

        let mut violations = self.violations.lock().unwrap();
        let now = Instant::now();
        let window = Duration::from_secs(30);

        let entry = violations.entry(ip.to_string()).or_insert((0, now));
        // 如果窗口已过期，重置
        if now.duration_since(entry.1) > window {
            *entry = (1, now);
        } else {
            entry.0 += 1;
        }

        if entry.0 >= self.ban_threshold {
            // 触发自动封禁
            let mut bans = self.temp_bans.lock().unwrap();
            bans.insert(ip.to_string(), now + self.ban_duration);
            violations.remove(ip);
            log::warn!("自动封禁 IP {} (违规 {} 次, 封禁 {:?})", ip, self.ban_threshold, self.ban_duration);
            return true;
        }

        false
    }

    // ═══════════════ 工具方法 ═══════════════

    /// 清理过期 bucket
    fn cleanup_old(&self, buckets: &mut HashMap<String, TokenBucket>) {
        let mut last_cleanup = self.last_cleanup.lock().unwrap();
        if last_cleanup.elapsed() >= self.cleanup_interval {
            if buckets.len() > 10000 {
                buckets.clear();
            }
            *last_cleanup = Instant::now();
        }
    }

    /// 手动封禁一个 IP（返回是否成功封禁）
    #[allow(dead_code)]
    pub fn ban_ip(&self, ip: &str, duration_seconds: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let mut bans = self.temp_bans.lock().unwrap();
        bans.insert(ip.to_string(), Instant::now() + Duration::from_secs(duration_seconds));
        log::info!("手动封禁 IP {} (时长 {}s)", ip, duration_seconds);
        true
    }

    /// 手动解封一个 IP
    #[allow(dead_code)]
    pub fn unban_ip(&self, ip: &str) -> bool {
        let mut bans = self.temp_bans.lock().unwrap();
        bans.remove(ip);
        true
    }

    /// 获取已封禁的 IP 数量
    #[allow(dead_code)]
    pub fn banned_count(&self) -> usize {
        let bans = self.temp_bans.lock().unwrap();
        bans.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_limiter() -> RateLimiter {
        RateLimiter::new_full(
            true,
            100,   // rps
            200,   // burst
            50,    // conn_rps
            100,   // max_conn
            300,   // ban_duration
            3,     // ban_threshold
            vec![],
            HashMap::new(),
        )
    }

    #[test]
    fn test_basic_rate_limit() {
        let limiter = make_limiter();
        let ip = "192.168.1.1";

        // 前 200 次请求应该全部通过（burst=200）
        for _ in 0..200 {
            assert!(limiter.check(ip), "burst 内应通过");
        }
        // 第 201+ 次请求可能开始被限流（取决于时间精度）
        // 不做断言，仅验证不会 panic
    }

    #[test]
    fn test_blacklist() {
        let limiter = RateLimiter::new_full(
            true, 100, 200, 50, 100, 300, 3,
            vec!["10.0.0.1".to_string()],
            HashMap::new(),
        );
        assert!(limiter.is_blocked("10.0.0.1"));
        assert!(!limiter.is_blocked("10.0.0.2"));
    }

    #[test]
    fn test_blacklist_wildcard() {
        let limiter = RateLimiter::new_full(
            true, 100, 200, 50, 100, 300, 3,
            vec!["192.168.1.*".to_string()],
            HashMap::new(),
        );
        assert!(limiter.is_blocked("192.168.1.1"));
        assert!(limiter.is_blocked("192.168.1.100"));
        assert!(!limiter.is_blocked("192.168.2.1"));
    }

    #[test]
    fn test_connection_rate() {
        let limiter = make_limiter();
        let ip = "10.0.0.5";

        // 允许连接（在连接速率内）
        assert!(limiter.try_connect(ip));

        // 断开
        limiter.disconnect(ip);

        // 白名单测试
        assert!(!limiter.is_whitelisted(ip));
    }

    #[test]
    fn test_concurrent_connections() {
        let limiter = RateLimiter::new_full(
            true, 100, 200, 100, 5, 300, 3,
            vec![],
            HashMap::new(),
        );
        let ip = "10.0.0.10";

        // 建立 5 个连接（max=5）
        for i in 0..5 {
            assert!(limiter.try_connect(ip), "连接 {} 应允许", i + 1);
        }
        // 第 6 个应被拒绝
        assert!(!limiter.try_connect(ip), "第 6 个连接应被拒绝");

        // 断开一个后可以再连
        limiter.disconnect(ip);
        assert!(limiter.try_connect(ip), "断开后应能建立新连接");
    }

    #[test]
    fn test_temp_ban() {
        // 使用短封禁时间方便测试
        let limiter = RateLimiter::new_full(
            true, 1, 1,   // 极低 RPS
            50, 100,
            60,   // ban 60 秒
            2,    // 2 次违规就封禁
            vec![],
            HashMap::new(),
        );
        let ip = "10.0.0.99";

        // 先消耗掉 burst
        limiter.check(ip);  // 第 1 次，burst 内

        // 第 2 次触发限流（第一次违规）
        limiter.check(ip);  // 已没令牌，违规
        assert!(!limiter.is_blocked(ip), "1 次违规不应封禁");

        // 第 3 次触发限流（第二次违规 → 触发 auto-ban）
        limiter.check(ip);  // 再违规，达到阈值
        // 应该被封禁了
        assert!(limiter.is_blocked(ip), "达到阈值应被封禁");
    }

    #[test]
    fn test_disabled_rate_limiter() {
        let limiter = RateLimiter::new_full(
            false,  // disabled
            1, 1, 50, 100, 300, 3,
            vec!["10.0.0.1".to_string()],
            HashMap::new(),
        );

        // 禁用时所有检查应放行
        assert!(limiter.check("10.0.0.1"));
        assert!(!limiter.is_blocked("10.0.0.1"));
        assert!(limiter.try_connect("10.0.0.1"));
    }

    #[test]
    fn test_whitelist_bypass() {
        let mut limiter = make_limiter();
        limiter.add_whitelist(vec!["8.8.8.8".to_string()]);

        // 黑名单 + 白名单测试
        assert!(limiter.is_whitelisted("8.8.8.8"));
        assert!(!limiter.is_whitelisted("1.1.1.1"));

        // 白名单应绕过所有检查
        assert!(limiter.check("8.8.8.8"));
        assert!(!limiter.is_blocked("8.8.8.8"));
    }

    #[test]
    fn test_manual_ban_unban() {
        let limiter = make_limiter();
        let ip = "10.0.0.50";

        assert!(!limiter.is_blocked(ip));
        limiter.ban_ip(ip, 60);
        assert!(limiter.is_blocked(ip));
        limiter.unban_ip(ip);
        assert!(!limiter.is_blocked(ip));
    }
}
