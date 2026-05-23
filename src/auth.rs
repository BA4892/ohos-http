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

//! WEB 管理端认证模块
//!
//! 提供管理员账号密码的加密存储、验证与暴力破解防护。
//!
//! ## 安全设计
//!
//! - **密码哈希**: PBKDF2-SHA256 (10 万次迭代)，基于 ring 实现
//! - **防暴力破解**: 连续失败达到阈值后锁定账户，锁定时间指数递增
//! - **凭证存储**: JSON 格式，存放在数据目录的 `.htadmin.json`（受文件权限保护）
//! - **锁定状态**: 存储在 `.htadmin.lock.json`，支持跨进程同步

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ring::pbkdf2;
use serde::{Deserialize, Serialize};

/// PBKDF2 迭代次数（10 万次，平衡安全性与性能）
const PBKDF2_ITERATIONS: u32 = 100_000;
/// 生成的哈希长度（字节）
const HASH_LENGTH: usize = 32;
/// 盐值长度（字节）
const SALT_LENGTH: usize = 16;

/// 凭证文件名
const ADMIN_FILE: &str = ".htadmin.json";
/// 锁定状态文件名
const LOCK_FILE: &str = ".htadmin.lock.json";

/// 最大连续失败次数
const MAX_FAILED_ATTEMPTS: u32 = 5;
/// 初始锁定时间（秒）
const BASE_LOCKOUT_SECS: u64 = 300; // 5 分钟
/// 锁定时间递增因子（每次翻倍）
const LOCKOUT_MULTIPLIER: u64 = 2;
/// 最大锁定时间（秒）
const MAX_LOCKOUT_SECS: u64 = 86400; // 24 小时

// ============================================================================
//  数据结构
// ============================================================================

/// 管理员凭证（加密存储）
#[derive(Serialize, Deserialize)]
pub struct AdminCredentials {
    /// 管理员用户名
    pub username: String,
    /// PBKDF2 密码哈希（Base64 编码）
    pub password_hash: String,
    /// 盐值（Base64 编码）
    pub salt: String,
    /// PBKDF2 迭代次数
    pub iterations: u32,
}

/// 锁定状态
#[derive(Serialize, Deserialize, Clone)]
struct LockoutState {
    /// 连续失败次数
    pub failed_attempts: u32,
    /// 最近一次失败时间（Unix 时间戳秒）
    pub last_failure: u64,
    /// 锁定截止时间（Unix 时间戳秒），None 表示未锁定
    pub locked_until: Option<u64>,
    /// 当前锁定期序号（用于指数递增）
    pub lockout_round: u32,
}

impl Default for LockoutState {
    fn default() -> Self {
        Self {
            failed_attempts: 0,
            last_failure: 0,
            locked_until: None,
            lockout_round: 0,
        }
    }
}

// ============================================================================
//  公开 API
// ============================================================================

/// 检查管理员是否已配置
pub fn is_admin_exists(data_dir: &Path) -> bool {
    credentials_path(data_dir).exists()
}

/// 交互式设置管理员账号
///
/// 提示用户输入用户名和密码（两次确认），加密存储在数据目录中。
/// 如果已存在会覆盖。
pub fn setup_admin(data_dir: &Path) {
    eprintln!("\n🔐  WEB 管理端配置");
    eprintln!("    ────────────────────────────────────");

    // 如果已存在，提示覆盖
    if is_admin_exists(data_dir) {
        eprintln!("  ⚠️   管理员账号已存在，将覆盖现有配置。");
        eprint!("  是否继续？(y/N): ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            eprintln!("  已取消。");
            return;
        }
    }

    // 输入用户名
    eprintln!("\n  请输入管理员账号信息：");

    let username = loop {
        eprint!("  用户名 (默认 admin): ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();
        let input = input.trim().to_string();
        if input.is_empty() {
            break "admin".to_string();
        }
        // 用户名仅允许字母、数字、下划线、短横线
        if !input.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            eprintln!("  ❌ 用户名只能包含字母、数字、下划线和短横线");
            continue;
        }
        if input.len() < 3 {
            eprintln!("  ❌ 用户名至少 3 个字符");
            continue;
        }
        break input;
    };

    // 输入密码（两次确认）
    let password = loop {
        eprint!("  密码 (至少 6 位): ");
        io::stdout().flush().ok();
        let pw1 = read_password();

        if pw1.len() < 6 {
            eprintln!("  ❌ 密码至少 6 位");
            continue;
        }

        eprint!("  再次输入密码确认: ");
        io::stdout().flush().ok();
        let pw2 = read_password();

        if pw1 != pw2 {
            eprintln!("  ❌ 两次输入的密码不一致，请重新设置");
            continue;
        }

        break pw1;
    };

    // 生成盐值和哈希
    let salt = generate_salt();
    let hash = hash_password(&password, &salt);

    let credentials = AdminCredentials {
        username,
        password_hash: encode_base64(&hash),
        salt: encode_base64(&salt),
        iterations: PBKDF2_ITERATIONS,
    };

    // 写入文件
    let path = credentials_path(data_dir);
    match serde_json::to_string_pretty(&credentials) {
        Ok(json) => match std::fs::write(&path, json) {
            Ok(_) => {
                // 设置文件权限为 600（仅所有者可读写）
                set_private_permissions(&path);
                eprintln!("\n  ✅ WEB 管理端已启用");
                eprintln!("     管理员账号配置完成");
            }
            Err(e) => {
                eprintln!("\n  ❌ 写入凭证文件失败 '{}': {}", path.display(), e);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("\n  ❌ 序列化凭证失败: {}", e);
            process::exit(1);
        }
    }

    // 清除锁定状态（如果有残留）
    clear_lockout(data_dir);
}

/// 验证密码（带暴力破解保护）
///
/// 返回 `true` 表示验证通过，`false` 表示密码错误或账户被锁定。
pub fn verify_password(data_dir: &Path, password: &str) -> bool {
    // 1. 检查锁定状态
    if let Some(remaining) = check_lockout(data_dir) {
        let mins = remaining / 60;
        let secs = remaining % 60;
        eprintln!("  🔒 账户已被临时锁定，请 {} 分 {} 秒后重试", mins, secs);
        return false;
    }

    // 2. 读取凭证
    let credentials = match load_credentials(data_dir) {
        Some(c) => c,
        None => {
            eprintln!("  ❌ 管理员账号未配置，请先运行 `ohos-server init`");
            return false;
        }
    };

    // 3. 验证密码
    let salt = match decode_base64(&credentials.salt) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("  ❌ 凭证数据损坏");
            return false;
        }
    };

    let stored_hash = match decode_base64(&credentials.password_hash) {
        Ok(h) => h,
        Err(_) => {
            eprintln!("  ❌ 凭证数据损坏");
            return false;
        }
    };

    let is_valid = verify_hash(password, &salt, &stored_hash);

    // 4. 更新锁定状态
    if !is_valid {
        record_failure(data_dir);
    } else {
        // 验证成功，清除锁定
        clear_lockout(data_dir);
    }

    is_valid
}

/// 交互式重置管理员密码
///
/// 流程：验证旧密码 → 输入新密码（两次确认）→ 更新凭证
///
/// 如果 `force` 为 `true`，跳过旧密码验证，直接设置新密码。
pub fn reset_password(data_dir: &Path, force: bool) {
    if !is_admin_exists(data_dir) {
        eprintln!("❌ 管理员账号未配置，请先运行 `ohos-server init` 完成初始化。");
        return;
    }

    // 读取当前用户名
    let username = match load_credentials(data_dir) {
        Some(c) => c.username,
        None => {
            eprintln!("❌ 无法读取管理员账号信息");
            return;
        }
    };

    eprintln!("\n🔑  重置管理员密码");
    eprintln!("   当前管理员: {}", username);
    eprintln!("    ────────────────────────────────────");

    if force {
        eprintln!("  ⚠️   强制重置模式：跳过旧密码验证。");
    }

    // 验证旧密码（非强制模式）
    if !force {
        // 先检查是否被锁定
        if let Some(remaining) = check_lockout(data_dir) {
            let mins = remaining / 60;
            let secs = remaining % 60;
            eprintln!("  🔒 账户已被临时锁定，请 {} 分 {} 秒后重试。", mins, secs);
            eprintln!("  提示: 如需强制重置，请运行 `ohos-server reset-admin --force`");
            return;
        }

        eprint!("  请输入当前密码: ");
        io::stdout().flush().ok();
        let old_pw = read_password();

        if !verify_password(data_dir, &old_pw) {
            eprintln!("  ❌ 密码验证失败，操作已取消。");
            return;
        }
    }

    // 输入新密码（两次确认）
    let new_password = loop {
        eprint!("\n  新密码 (至少 6 位): ");
        io::stdout().flush().ok();
        let pw1 = read_password();

        if pw1.len() < 6 {
            eprintln!("  ❌ 密码至少 6 位");
            continue;
        }

        eprint!("  再次输入新密码确认: ");
        io::stdout().flush().ok();
        let pw2 = read_password();

        if pw1 != pw2 {
            eprintln!("  ❌ 两次输入的密码不一致，请重新设置");
            continue;
        }

        break pw1;
    };

    // 生成新的盐值和哈希
    let salt = generate_salt();
    let hash = hash_password(&new_password, &salt);

    let credentials = AdminCredentials {
        username,
        password_hash: encode_base64(&hash),
        salt: encode_base64(&salt),
        iterations: PBKDF2_ITERATIONS,
    };

    // 写入文件
    let path = credentials_path(data_dir);
    match serde_json::to_string_pretty(&credentials) {
        Ok(json) => match std::fs::write(&path, json) {
            Ok(_) => {
                set_private_permissions(&path);
                eprintln!("\n  ✅ 密码已更新");
            }
            Err(e) => {
                eprintln!("\n  ❌ 写入凭证文件失败 '{}': {}", path.display(), e);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("\n  ❌ 序列化凭证失败: {}", e);
            process::exit(1);
        }
    }

    // 清除锁定状态
    clear_lockout(data_dir);
}

/// 检查账户是否被锁定
///
/// 返回 `Some(remaining_secs)` 表示仍在锁定中，`None` 表示未被锁定。
pub fn check_lockout(data_dir: &Path) -> Option<u64> {
    let lock_path = lock_path(data_dir);
    let content = std::fs::read_to_string(lock_path).ok()?;

    let state: LockoutState = serde_json::from_str(&content).ok()?;
    let now = unix_now();

    match state.locked_until {
        Some(until) if until > now => Some(until - now),
        _ => None,
    }
}

/// 获取管理员用户名
pub fn get_username(data_dir: &Path) -> Option<String> {
    load_credentials(data_dir).map(|c| c.username)
}

// ============================================================================
//  内部函数
// ============================================================================

/// 生成随机盐值
fn generate_salt() -> Vec<u8> {
    use rand::Rng;
    let mut salt = vec![0u8; SALT_LENGTH];
    rand::thread_rng().fill(&mut salt[..]);
    salt
}

/// PBKDF2-SHA256 密码哈希
fn hash_password(password: &str, salt: &[u8]) -> Vec<u8> {
    let mut hash = vec![0u8; HASH_LENGTH];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).unwrap(),
        salt,
        password.as_bytes(),
        &mut hash,
    );
    hash
}

/// 验证 PBKDF2 哈希
fn verify_hash(password: &str, salt: &[u8], stored_hash: &[u8]) -> bool {
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).unwrap(),
        salt,
        password.as_bytes(),
        stored_hash,
    )
    .is_ok()
}

/// 记录一次失败尝试
fn record_failure(data_dir: &Path) {
    let lock_path = lock_path(data_dir);
    let now = unix_now();

    // 读取当前状态
    let mut state = load_lockout(data_dir).unwrap_or_default();

    state.failed_attempts += 1;
    state.last_failure = now;

    // 计算锁定时间（指数递增）
    if state.failed_attempts >= MAX_FAILED_ATTEMPTS {
        let lockout_secs = calculate_lockout_duration(state.lockout_round);
        state.locked_until = Some(now + lockout_secs);
        state.lockout_round += 1;
        state.failed_attempts = 0; // 重置计数，下一次从新 round 开始
    }

    // 写入文件
    if let Ok(json) = serde_json::to_string(&state) {
        let _ = std::fs::write(&lock_path, json);
    }
}

/// 清除锁定状态
fn clear_lockout(data_dir: &Path) {
    let lock_path = lock_path(data_dir);
    let _ = std::fs::remove_file(lock_path);
}

/// 计算锁定时间（指数递增）
fn calculate_lockout_duration(round: u32) -> u64 {
    let secs = BASE_LOCKOUT_SECS * LOCKOUT_MULTIPLIER.pow(round);
    secs.min(MAX_LOCKOUT_SECS)
}

/// 加载凭证
pub fn load_credentials(data_dir: &Path) -> Option<AdminCredentials> {
    let path = credentials_path(data_dir);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 加载锁定状态
fn load_lockout(data_dir: &Path) -> Option<LockoutState> {
    let path = lock_path(data_dir);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 凭证文件路径
fn credentials_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ADMIN_FILE)
}

/// 锁定状态文件路径
fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK_FILE)
}

/// Base64 编码
fn encode_base64(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(data)
}

/// Base64 解码
fn decode_base64(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.decode(data)
}

/// 当前 Unix 时间戳（秒）
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

/// 读取密码（不回显）
///
/// 在标准 UNIX 系统上尝试关闭回显，若不支持则直接读取。
fn read_password() -> String {
    // 尝试关闭终端回显
    let _ = disable_echo();
    let mut password = String::new();
    io::stdin().read_line(&mut password).ok();
    let _ = enable_echo();
    eprintln!(); // 换行
    password.trim().to_string()
}

#[cfg(unix)]
fn disable_echo() -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = io::stdin().as_raw_fd();
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios as *mut libc::termios) } != 0 {
        // tcgetattr 失败，静默忽略（可能不是 TTY）
        return Ok(());
    }
    let mut raw = termios;
    raw.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw as *const libc::termios) };
    Ok(())
}

#[cfg(unix)]
fn enable_echo() -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = io::stdin().as_raw_fd();
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios as *mut libc::termios) } != 0 {
        return Ok(());
    }
    termios.c_lflag |= libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios as *const libc::termios) };
    Ok(())
}

#[cfg(not(unix))]
fn disable_echo() -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn enable_echo() -> std::io::Result<()> {
    Ok(())
}

/// 设置文件权限为 600（仅所有者可读写）
///
/// UNIX 系统上使用 chmod，其他平台静默忽略。
fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(()) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            // okay
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

use std::process;
