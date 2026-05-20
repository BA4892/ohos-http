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
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};



pub fn default_now() -> std::time::Instant {
    std::time::Instant::now()
}

/// 单个 Session 数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub data: HashMap<String, String>,
    #[serde(skip, default = "crate::session::default_now")]
    #[allow(dead_code)]
    pub created_at: Instant,
    #[serde(skip, default = "crate::session::default_now")]
    pub last_access: Instant,
    /// 序列化用 — 自纪元以来的纳秒数
    #[serde(rename = "created_at_nanos")]
    created_at_nanos: u128,
    #[serde(rename = "last_access_nanos")]
    last_access_nanos: u128,
}

impl Session {
    fn new(id: String) -> Self {
        let now = Instant::now();
        Session {
            id,
            data: HashMap::new(),
            created_at: now,
            last_access: now,
            created_at_nanos: 0,
            last_access_nanos: 0,
        }
    }

    /// 获取 session 数据
    #[allow(dead_code)]
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    /// 设置 session 数据
    #[allow(dead_code)]
    pub fn set(&mut self, key: String, value: String) {
        self.data.insert(key, value);
        self.last_access = Instant::now();
    }

    /// 删除 session 数据
    #[allow(dead_code)]
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.last_access = Instant::now();
        self.data.remove(key)
    }
}

/// Session 管理器（线程安全，内存存储 + 可选跨进程共享内存）
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    cookie_name: String,
    ttl: Duration,
    cleanup_interval: Duration,
    last_cleanup: RwLock<Instant>,
}

impl SessionStore {
    pub fn new(cookie_name: &str, ttl_seconds: u64) -> Self {
        SessionStore {
            sessions: RwLock::new(HashMap::new()),
            cookie_name: cookie_name.to_string(),
            ttl: Duration::from_secs(ttl_seconds),
            cleanup_interval: Duration::from_secs(300), // 每5分钟清理一次
            last_cleanup: RwLock::new(Instant::now()),
        }
    }

    /// 获取 cookie 名称
    #[allow(dead_code)]
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    /// 创建新 session，返回 session ID
    pub fn create(&self) -> String {
        let session_id = generate_session_id();
        let session = Session::new(session_id.clone());

        // 写入本地内存
        {
            let mut sessions = self.sessions.write().unwrap();
            sessions.insert(session_id.clone(), session.clone());
        }

        session_id
    }

    /// 根据 session ID 获取 session
    #[allow(dead_code)]
    pub fn get(&self, session_id: &str) -> Option<Session> {
        // 查询本地内存
        {
            let sessions = self.sessions.read().unwrap();
            if let Some(s) = sessions.get(session_id) {
                return Some(s.clone());
            }
        }

        None
    }

    /// 获取可变 session
    #[allow(dead_code)]
    pub fn get_mut(&self, session_id: &str) -> Option<Session> {
        let mut session = self.get(session_id)?;
        session.last_access = Instant::now();
        Some(session)
    }

    /// 更新 session 数据
    #[allow(dead_code)]
    pub fn update(&self, session: Session) {
        // 更新本地
        {
            let mut sessions = self.sessions.write().unwrap();
            sessions.insert(session.id.clone(), session.clone());
        }
    }

    /// 删除 session
    #[allow(dead_code)]
    pub fn remove(&self, session_id: &str) {
        // 从本地删除
        {
            let mut sessions = self.sessions.write().unwrap();
            sessions.remove(session_id);
        }
    }

    /// 清理过期 session
    pub fn cleanup(&self) {
        let mut last = self.last_cleanup.write().unwrap();
        if last.elapsed() < self.cleanup_interval {
            return;
        }
        *last = Instant::now();
        drop(last);

        let mut sessions = self.sessions.write().unwrap();
        sessions.retain(|_, s| {
            s.last_access.elapsed() < self.ttl
        });
    }

    /// 从 Cookie 头中解析 session ID
    pub fn parse_session_id(&self, cookie_header: Option<&str>) -> Option<String> {
        let cookie_str = cookie_header?;
        let prefix = format!("{}=", self.cookie_name);

        for part in cookie_str.split(';') {
            let part = part.trim();
            if let Some(value) = part.strip_prefix(&prefix) {
                // 去掉可能的尾部属性（path=, domain= 等）
                let session_id = value.split(';').next().unwrap_or(value).trim();
                if !session_id.is_empty() {
                    return Some(session_id.to_string());
                }
            }
        }
        None
    }

    /// 生成 Set-Cookie 头值
    pub fn build_set_cookie(&self, session_id: &str) -> String {
        format!(
            "{}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
            self.cookie_name,
            session_id,
            self.ttl.as_secs(),
        )
    }

    /// 生成删除 Cookie 头值
    #[allow(dead_code)]
    pub fn build_delete_cookie(&self) -> String {
        format!("{}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0", self.cookie_name)
    }
}

/// 生成随机 session ID
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // 组合时间戳和随机数
    let random_part: u64 = {
        // 简单的随机数生成（不引入额外依赖）
        let ptr = &nanos as *const u128 as usize;
        (ptr.wrapping_mul(6364136223846793005)).wrapping_add(1442695040888963407) as u64
    };

    let combined = format!("{:x}{:x}{:x}", nanos, random_part, fast_hash(&nanos.to_ne_bytes()));
    // 截取 32 字符作为 session ID
    if combined.len() > 32 {
        combined[..32].to_string()
    } else {
        format!("{:0>32}", combined)
    }
}

fn fast_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_create_and_get() {
        let store = SessionStore::new("TEST_SESSION", 3600);
        let id = store.create();
        assert_eq!(id.len(), 32);

        let session = store.get(&id);
        assert!(session.is_some());
        assert_eq!(session.unwrap().id, id);
    }

    #[test]
    fn test_session_data() {
        let store = SessionStore::new("TEST_SESSION", 3600);
        let id = store.create();

        let mut session = store.get(&id).unwrap();
        session.set("username".to_string(), "alice".to_string());
        store.update(session);

        let session = store.get(&id).unwrap();
        assert_eq!(session.get("username").unwrap(), "alice");
    }

    #[test]
    fn test_parse_cookie() {
        let store = SessionStore::new("OHOS_SESSION", 3600);
        let id = store.create();

        let cookie = format!("OHOS_SESSION={}; Path=/", id);
        let parsed = store.parse_session_id(Some(&cookie));
        assert_eq!(parsed, Some(id));
    }

    #[test]
    fn test_invalid_cookie() {
        let store = SessionStore::new("OHOS_SESSION", 3600);
        let parsed = store.parse_session_id(Some("OTHER=abc123"));
        assert!(parsed.is_none());
    }
}
