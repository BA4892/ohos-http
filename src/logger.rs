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

use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// 日志条目 — 通过 mpsc 发送到后台写线程
struct LogEntry {
    remote_addr: String,
    method: String,
    path: String,
    status: u16,
    size: u64,
    referer: String,
    user_agent: String,
    duration_ms: u64,
}

/// 全局日志发送器（惰性初始化，首条日志时绑定）
static LOG_SENDER: OnceLock<mpsc::UnboundedSender<LogEntry>> = OnceLock::new();

/// 访问日志写入器，支持按日期和大小自动轮转
///
/// # 异步架构
/// `log()` 方法仅将日志条目发送到 mpsc 通道，不阻塞请求处理线程。
/// 后台 tokio 任务（`spawn_log_writer`）负责按序写入磁盘。
pub struct AccessLogger {
    log_path: String,
    max_size: u64,
}

impl AccessLogger {
    /// 创建一个新的访问日志写入器
    ///
    /// * `log_path` - 日志文件路径（如 `./logs/access.log`）
    /// * `max_size` - 单文件最大字节数（0=不限制）
    pub fn new(log_path: &str, max_size: u64) -> Self {
        let _ = fs::create_dir_all(PathBuf::from(log_path).parent().unwrap_or(Path::new(".")));

        // 预创建第一个日志文件，以便快速验证权限和路径
        let now = Local::now();
        let current_date = now.format("%Y-%m-%d").to_string();
        let _file = open_log_file(log_path, &current_date, 0);

        AccessLogger {
            log_path: log_path.to_string(),
            max_size,
        }
    }

    /// 写入一条访问日志（Combined Log Format 风格）
    ///
    /// 非阻塞：日志条目通过 mpsc 通道发送到后台写任务。
    /// 后台任务在首次调用时惰性启动。
    pub fn log(
        &self,
        remote_addr: &str,
        method: &str,
        path: &str,
        status: u16,
        size: u64,
        referer: &str,
        user_agent: &str,
        duration_ms: u64,
    ) {
        let entry = LogEntry {
            remote_addr: remote_addr.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            size,
            referer: referer.to_string(),
            user_agent: user_agent.to_string(),
            duration_ms,
        };

        // 惰性初始化后台写任务
        let sender = LOG_SENDER.get_or_init(|| {
            let (tx, rx) = mpsc::unbounded_channel::<LogEntry>();
            let log_path = self.log_path.clone();
            let max_size = self.max_size;
            // 在 tokio 运行时中启动后台写任务
            tokio::runtime::Handle::current().spawn(async move {
                run_log_writer(rx, &log_path, max_size).await;
            });
            tx
        });

        // 非阻塞发送 — 如果通道已关闭则丢弃
        let _ = sender.send(entry);
    }
}

/// 后台日志写任务：从 mpsc receiver 读取条目并按序写入磁盘
async fn run_log_writer(
    mut rx: mpsc::UnboundedReceiver<LogEntry>,
    log_path: &str,
    max_size: u64,
) {
    let now = Local::now();
    let mut current_date = now.format("%Y-%m-%d").to_string();
    let mut file = open_log_file(log_path, &current_date, 0);
    let mut current_size: u64 = file.metadata().map(|m| m.len()).unwrap_or(0);

    while let Some(entry) = rx.recv().await {
        let now = Local::now();
        let date_str = now.format("%d/%b/%Y:%H:%M:%S %z").to_string();
        let today = now.format("%Y-%m-%d").to_string();

        // Combined Log Format + duration
        let log_line = format!(
            "{} - - [{}] \"{} {} HTTP/1.1\" {} {} \"{}\" \"{}\" {}ms\n",
            entry.remote_addr, date_str, entry.method, entry.path,
            entry.status, entry.size, entry.referer, entry.user_agent, entry.duration_ms
        );
        let log_line_len = log_line.len() as u64;

        // 日期变更 → 新建文件
        if today != current_date {
            file = open_log_file(log_path, &today, 0);
            current_date = today;
            current_size = 0;
        }

        // 大小超限 → 轮转
        let needs_rotate = max_size > 0 && current_size + log_line_len > max_size;

        if needs_rotate {
            let date_for_rotate = current_date.clone();
            rotate_log_file(log_path, &date_for_rotate);
            file = open_log_file(log_path, &date_for_rotate, 0);
            current_size = 0;
        }

        let _ = file.write_all(log_line.as_bytes());
        let _ = file.flush();
        current_size += log_line_len;
    }
}

/// 打开日志文件（追加模式，自动创建）
fn open_log_file(log_path: &str, date: &str, rotate_index: u32) -> File {
    let dot = PathBuf::from(".");
    let stem = PathBuf::from(log_path);
    let ext = stem.extension().unwrap_or_default().to_str().unwrap_or("log").to_string();
    let name = stem.file_stem().unwrap_or_default().to_str().unwrap_or("access").to_string();
    let dir: &Path = stem.parent().unwrap_or(&dot);

    let path = if rotate_index > 0 {
        dir.join(format!("{}_{}.{}.{}", name, date, rotate_index, ext))
    } else {
        dir.join(format!("{}_{}.{}", name, date, ext))
    };

    let path_dir = path.parent().unwrap();
    let _ = fs::create_dir_all(path_dir);

    OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .expect("无法打开日志文件")
}

/// 轮转日志文件：将当前日志文件重命名为 .1 版本
fn rotate_log_file(log_path: &str, date: &str) {
    let path = open_log_file_path(log_path, date, 0);
    if path.exists() {
        let rotated_path = open_log_file_path(log_path, date, 1);
        if rotated_path.exists() {
            let _ = fs::remove_file(&rotated_path);
        }
        let _ = fs::rename(&path, &rotated_path);
    }
}

fn open_log_file_path(log_path: &str, date: &str, rotate_index: u32) -> PathBuf {
    let dot = PathBuf::from(".");
    let stem = PathBuf::from(log_path);
    let ext = stem.extension().unwrap_or_default().to_str().unwrap_or("log").to_string();
    let name = stem.file_stem().unwrap_or_default().to_str().unwrap_or("access").to_string();
    let dir: &Path = stem.parent().unwrap_or(&dot);

    if rotate_index > 0 {
        dir.join(format!("{}_{}.{}.{}", name, date, rotate_index, ext))
    } else {
        dir.join(format!("{}_{}.{}", name, date, ext))
    }
}
