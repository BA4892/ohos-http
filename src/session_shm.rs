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

//! 跨进程共享内存 Session 存储
//!
//! 使用 POSIX 共享内存 (shm_open + mmap) 实现多 Worker 进程间的 Session 数据共享。
//! 采用固定大小的开放地址哈希表，每个桶使用自旋锁保证原子性。
//!
//! ## 内存布局
//!
//! ```text
//! [Header 64B] [Bucket 0 (960B)] [Bucket 1 (960B)] ... [Bucket N-1]
//! ```
//!
//! Header:
//! - magic: u64 = 0x534553534F484F53 ("SESSOHOS")
//! - num_buckets: u32
//! - bucket_size: u32 = 960
//! - total_size: u64
//!
//! Bucket:
//! - lock: AtomicU8 (0=free, 1=locked)
//! - status: AtomicU8 (0=empty, 1=occupied, 2=deleted)
//! - key: [u8; 64]  (session_id, null-padded)
//! - data_len: u32
//! - data: [u8; 880]  (JSON-serialized session data)

use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicU8, Ordering};

use memmap2::MmapRaw;
use crate::session::Session;
const SHM_MAGIC: u64 = 0x534553534F484F53;
/// Session ID 最大长度
const KEY_MAX_LEN: usize = 64;
/// 每个桶的数据区大小
const DATA_MAX_LEN: usize = 880;
/// Bucket 总大小
const BUCKET_SIZE: usize = 960;
/// 默认桶数量 (2^16 = 65536)
const DEFAULT_NUM_BUCKETS: u32 = 65536;

// header 布局偏移
const HDR_MAGIC: usize = 0;          // 8 bytes
const HDR_NUM_BUCKETS: usize = 8;    // 4 bytes
const HDR_BUCKET_SIZE: usize = 12;   // 4 bytes
const HDR_TOTAL_SIZE: usize = 16;    // 8 bytes
const HDR_SIZE: usize = 64;          // 64 bytes

// bucket 内部偏移
const BUCKET_LOCK: usize = 0;       // 1 byte
const BUCKET_STATUS: usize = 1;     // 1 byte
const BUCKET_KEY: usize = 8;        // 64 bytes (aligned to 8)
const BUCKET_DATA_LEN: usize = 72;  // 4 bytes
const BUCKET_DATA: usize = 80;      // 880 bytes

/// 共享内存 Session 存储
///
/// 使用 `MmapRaw` 以支持通过 `&self` 进行读写操作（跨进程共享所需的内部可变性）。
pub struct ShmSessionStore {
    mmap: MmapRaw,
    num_buckets: u32,
    name: String,
}

unsafe impl Send for ShmSessionStore {}
unsafe impl Sync for ShmSessionStore {}

impl ShmSessionStore {
    /// 创建或打开共享内存 Session 存储
    ///
    /// * `shm_name` - POSIX 共享内存名称 (如 "/ohos_http_sessions")
    /// * `num_buckets` - 桶数量 (0 = 使用默认值 65536)
    pub fn new(shm_name: &str, num_buckets: u32) -> Result<Self, String> {
        let nb = if num_buckets == 0 { DEFAULT_NUM_BUCKETS } else { num_buckets };
        let total_size = HDR_SIZE + (nb as usize) * BUCKET_SIZE;

        // 创建/打开共享内存
        let fd = unsafe {
            let name_cstr = std::ffi::CString::new(shm_name).map_err(|e| e.to_string())?;
            let fd = libc::shm_open(
                name_cstr.as_ptr(),
                libc::O_CREAT | libc::O_RDWR,
                0o644,
            );
            if fd < 0 {
                return Err(format!("shm_open 失败: {}", std::io::Error::last_os_error()));
            }
            // 设置大小
            let ret = libc::ftruncate(fd, total_size as i64);
            if ret < 0 {
                libc::close(fd);
                return Err(format!("ftruncate 失败: {}", std::io::Error::last_os_error()));
            }
            fd
        };

        // mmap — 使用 File::from_raw_fd + MmapRaw
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let mmap = unsafe { MmapRaw::map_raw(&file) }
            .map_err(|e| format!("mmap 失败: {}", e))?;
        // file 在此处 drop 关闭 fd；mmap 映射仍然有效

        let store = ShmSessionStore {
            mmap,
            num_buckets: nb,
            name: shm_name.to_string(),
        };

        // 初始化 header (仅第一次)
        store.init_header_if_needed();

        Ok(store)
    }

    /// 获取 mmap 内存区域的指针（用于 unsafe 读写）
    fn ptr(&self) -> *mut u8 {
        self.mmap.as_mut_ptr()
    }

    /// 初始化 header（仅当第一次创建时）
    fn init_header_if_needed(&self) {
        let p = self.ptr();
        unsafe {
            let magic = *(p.add(HDR_MAGIC) as *const u64);
            if magic != SHM_MAGIC {
                // 写魔数
                *(p.add(HDR_MAGIC) as *mut u64) = SHM_MAGIC;
                // 写桶数量
                *(p.add(HDR_NUM_BUCKETS) as *mut u32) = self.num_buckets;
                // 写桶大小
                *(p.add(HDR_BUCKET_SIZE) as *mut u32) = BUCKET_SIZE as u32;
                // 写总大小
                let total = (HDR_SIZE + (self.num_buckets as usize) * BUCKET_SIZE) as u64;
                *(p.add(HDR_TOTAL_SIZE) as *mut u64) = total;
            }
        }
    }

    /// 计算 session_id 的哈希值
    fn hash(&self, session_id: &str) -> u32 {
        let mut h: u32 = 2166136261;
        for b in session_id.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        h % self.num_buckets
    }

    /// 获取桶的起始偏移
    fn bucket_offset(&self, idx: u32) -> usize {
        HDR_SIZE + (idx as usize) * BUCKET_SIZE
    }

    /// 尝试锁定桶 (spinlock, 最多重试 10000 次)
    fn lock_bucket(&self, offset: usize) -> bool {
        let p = self.ptr();
        let lock_ptr = unsafe { &mut *(p.add(offset + BUCKET_LOCK) as *mut AtomicU8) };
        for _ in 0..10000 {
            if lock_ptr.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return true;
            }
            std::hint::spin_loop();
        }
        false
    }

    /// 解锁桶
    fn unlock_bucket(&self, offset: usize) {
        let p = self.ptr();
        let lock_ptr = unsafe { &mut *(p.add(offset + BUCKET_LOCK) as *mut AtomicU8) };
        lock_ptr.store(0, Ordering::Release);
    }

    /// 读取桶的状态
    fn bucket_status(&self, offset: usize) -> u8 {
        let p = self.ptr();
        let status_ptr = unsafe { &mut *(p.add(offset + BUCKET_STATUS) as *mut AtomicU8) };
        status_ptr.load(Ordering::Acquire)
    }

    /// 设置桶的状态
    fn set_bucket_status(&self, offset: usize, status: u8) {
        let p = self.ptr();
        let status_ptr = unsafe { &mut *(p.add(offset + BUCKET_STATUS) as *mut AtomicU8) };
        status_ptr.store(status, Ordering::Release);
    }

    /// 读取桶中的 key
    fn bucket_key(&self, offset: usize) -> String {
        let p = self.ptr();
        let key_ptr = unsafe { p.add(offset + BUCKET_KEY) as *const u8 };
        let mut bytes = [0u8; KEY_MAX_LEN];
        unsafe {
            std::ptr::copy_nonoverlapping(key_ptr, bytes.as_mut_ptr(), KEY_MAX_LEN);
        }
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(KEY_MAX_LEN);
        String::from_utf8_lossy(&bytes[..end]).to_string()
    }

    /// 写入 key 到桶
    fn set_bucket_key(&self, offset: usize, key: &str) {
        let p = self.ptr();
        let key_ptr = unsafe { p.add(offset + BUCKET_KEY) };
        let bytes = key.as_bytes();
        let len = bytes.len().min(KEY_MAX_LEN);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), key_ptr, len);
            // null padding
            if len < KEY_MAX_LEN {
                std::ptr::write_bytes(key_ptr.add(len), 0, KEY_MAX_LEN - len);
            }
        }
    }

    /// 读取桶中数据长度
    fn bucket_data_len(&self, offset: usize) -> u32 {
        let p = self.ptr();
        unsafe { *(p.add(offset + BUCKET_DATA_LEN) as *const u32) }
    }

    /// 设置桶中数据长度
    fn set_bucket_data_len(&self, offset: usize, len: u32) {
        let p = self.ptr();
        unsafe { *(p.add(offset + BUCKET_DATA_LEN) as *mut u32) = len; }
    }

    /// 写入 session 到共享内存
    pub fn put(&self, session_id: &str, session: &Session) -> bool {
        let data_json = serde_json::to_string(session).unwrap_or_default();
        if data_json.len() > DATA_MAX_LEN {
            return false;
        }

        let mut idx = self.hash(session_id);
        let start_idx = idx;

        loop {
            let offset = self.bucket_offset(idx);
            if !self.lock_bucket(offset) {
                return false;
            }

            let status = self.bucket_status(offset);
            if status == 0 || status == 2 {
                // 空桶或已删除 — 写入
                self.set_bucket_key(offset, session_id);
                let data_bytes = data_json.as_bytes();
                let p = self.ptr();
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data_bytes.as_ptr(),
                        p.add(offset + BUCKET_DATA),
                        data_bytes.len(),
                    );
                }
                self.set_bucket_data_len(offset, data_bytes.len() as u32);
                self.set_bucket_status(offset, 1);
                self.unlock_bucket(offset);
                return true;
            }

            if status == 1 {
                let existing_key = self.bucket_key(offset);
                if existing_key == session_id {
                    // 更新
                    let data_bytes = data_json.as_bytes();
                    let p = self.ptr();
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data_bytes.as_ptr(),
                            p.add(offset + BUCKET_DATA),
                            data_bytes.len(),
                        );
                    }
                    self.set_bucket_data_len(offset, data_bytes.len() as u32);
                    self.unlock_bucket(offset);
                    return true;
                }
            }

            self.unlock_bucket(offset);

            idx = (idx + 1) % self.num_buckets;
            if idx == start_idx {
                return false;
            }
        }
    }

    /// 从共享内存读取 session
    pub fn get(&self, session_id: &str) -> Option<Session> {
        let mut idx = self.hash(session_id);
        let start_idx = idx;

        loop {
            let offset = self.bucket_offset(idx);
            if !self.lock_bucket(offset) {
                return None;
            }

            let status = self.bucket_status(offset);
            if status == 0 {
                self.unlock_bucket(offset);
                return None;
            }

            if status == 1 {
                let existing_key = self.bucket_key(offset);
                if existing_key == session_id {
                    let data_len = self.bucket_data_len(offset) as usize;
                    let p = self.ptr();
                    let data_slice = unsafe {
                        std::slice::from_raw_parts(p.add(offset + BUCKET_DATA), data_len)
                    };
                    let session: Session = serde_json::from_slice(data_slice).ok()?;
                    self.unlock_bucket(offset);
                    return Some(session);
                }
            }

            self.unlock_bucket(offset);

            idx = (idx + 1) % self.num_buckets;
            if idx == start_idx {
                return None;
            }
        }
    }

    /// 从共享内存删除 session
    pub fn remove(&self, session_id: &str) -> bool {
        let mut idx = self.hash(session_id);
        let start_idx = idx;

        loop {
            let offset = self.bucket_offset(idx);
            if !self.lock_bucket(offset) {
                return false;
            }

            let status = self.bucket_status(offset);
            if status == 0 {
                self.unlock_bucket(offset);
                return false;
            }

            if status == 1 {
                let existing_key = self.bucket_key(offset);
                if existing_key == session_id {
                    self.set_bucket_status(offset, 2); // deleted
                    self.unlock_bucket(offset);
                    return true;
                }
            }

            self.unlock_bucket(offset);

            idx = (idx + 1) % self.num_buckets;
            if idx == start_idx {
                return false;
            }
        }
    }

    /// 获取共享内存名称
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for ShmSessionStore {
    fn drop(&mut self) {
        // MmapRaw's drop will unmap. The shm object persists for other processes.
    }
}

/// 清理共享内存对象（由 master 进程在退出时调用）
pub fn cleanup_shm(shm_name: &str) {
    if let Ok(name_cstr) = std::ffi::CString::new(shm_name) {
        unsafe {
            libc::shm_unlink(name_cstr.as_ptr());
        }
    }
}
