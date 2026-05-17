use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 日志状态（在 Mutex 保护下可变）
struct LogState {
    file: File,
    current_date: String,
    current_size: u64,
}

/// 访问日志写入器，支持按日期和大小自动轮转
pub struct AccessLogger {
    log_path: String,
    max_size: u64, // 单文件最大字节（0=不限制大小）
    state: Mutex<LogState>,
}

impl AccessLogger {
    /// 创建一个新的访问日志写入器
    ///
    /// * `log_path` - 日志文件路径（如 `./logs/access.log`）
    /// * `max_size` - 单文件最大字节数（0=不限制）
    pub fn new(log_path: &str, max_size: u64) -> Self {
        let dir = PathBuf::from(log_path).parent().unwrap_or(Path::new(".")).to_path_buf();
        let _ = fs::create_dir_all(&dir);

        let now = Local::now();
        let current_date = now.format("%Y-%m-%d").to_string();

        let file = open_log_file(log_path, &current_date, 0);
        let current_size = file.metadata().map(|m| m.len()).unwrap_or(0);

        AccessLogger {
            log_path: log_path.to_string(),
            max_size,
            state: Mutex::new(LogState {
                file,
                current_date,
                current_size,
            }),
        }
    }

    /// 写入一条访问日志（Combined Log Format 风格）
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
        let now = Local::now();
        let date_str = now.format("%d/%b/%Y:%H:%M:%S %z").to_string();
        let today = now.format("%Y-%m-%d").to_string();

        // Combined Log Format + duration
        let log_line = format!(
            "{} - - [{}] \"{} {} HTTP/1.1\" {} {} \"{}\" \"{}\" {}ms\n",
            remote_addr, date_str, method, path, status, size, referer, user_agent, duration_ms
        );

        let log_line_len = log_line.len() as u64;

        // 在锁内检查并处理轮转
        let mut state = self.state.lock().unwrap();

        // 日期变更 → 新建文件
        if today != state.current_date {
            state.file = open_log_file(&self.log_path, &today, 0);
        }

        // 大小超限 → 轮转
        let needs_rotate = self.max_size > 0 && state.current_size + log_line_len > self.max_size;

        if needs_rotate {
            let date_for_rotate = state.current_date.clone();
            let path = self.log_path.clone();
            drop(state);
            rotate_log_file(&path, &date_for_rotate);
            // 重新获取锁并打开新文件
            let mut state = self.state.lock().unwrap();
            state.file = open_log_file(&self.log_path, &date_for_rotate, 0);
            let _ = state.file.write_all(log_line.as_bytes());
            let _ = state.file.flush();
            state.current_size = log_line_len;
            state.current_date = today;
        } else {
            let _ = state.file.write_all(log_line.as_bytes());
            let _ = state.file.flush();
            state.current_size += log_line_len;
            state.current_date = today;
        }
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
