# ohosHttp 架构文档

## 概述

ohosHttp 采用 **进程 (Process) × 线程 (Thread) × 协程 (Coroutine)** 三层架构设计，充分释放多核 CPU 的并行计算能力。

```
┌─────────────────────────────────────────────────────────┐
│                    Master 进程                            │
│  信号管理 | 进程监控 | 共享内存生命周期                     │
└────────────────────────┬────────────────────────────────┘
                         │ fork()
         ┌───────────────┼───────────────┐
         │               │               │
    ┌────▼────┐    ┌────▼────┐    ┌────▼────┐
    │Worker 0  │    │Worker 1  │    │Worker N  │  ← 进程层
    │CPU Core 0│    │CPU Core 1│    │CPU Core N│     (OS 进程隔离)
    └────┬────┘    └────┬────┘    └────┬────┘
         │              │              │
    ┌────▼────┐    ┌────▼────┐    ┌────▼────┐
    │Tokio    │    │Tokio    │    │Tokio    │  ← 线程层
    │Multi-   │    │Multi-   │    │Multi-   │     (Tokio 工作线程)
    │Thread   │    │Thread   │    │Thread   │
    └────┬────┘    └────┬────┘    └────┬────┘
         │              │              │
    ┌────▼────┐    ┌────▼────┐    ┌────▼────┐
    │协程池   │    │协程池   │    │协程池   │  ← 协程层
    │Task 001 │    │Task 001 │    │Task 001 │     (异步任务)
    │Task 002 │    │Task 002 │    │Task 002 │
    │...      │    │...      │    │...      │
    └─────────┘    └─────────┘    └─────────┘
```

---

## 第一层：进程层 (Process)

### 架构模型

采用 **Master-Worker 多进程模型**，类似于 Nginx 的架构设计：

| 角色 | 职责 | 数量 |
|------|------|------|
| **Master** | 进程管理、信号处理、共享内存生命周期 | 1 |
| **Worker** | HTTP 请求处理 | N (默认 = CPU 核数) |

### Master 进程

- 通过 `fork()` 系统调用创建 Worker 子进程
- 使用 `waitpid(-1)` 等待 Worker 退出
- 管理 POSIX 共享内存 (`shm_open` / `shm_unlink`)
- 不参与任何网络 I/O 处理

### Worker 进程

- 每个 Worker 绑定到特定 CPU 核心（CPU Affinity）
- 使用 `SO_REUSEPORT` 套接字选项共享同一监听端口
- 由内核在多个 Worker 间分发连接请求（内核级负载均衡）
- 独立的进程地址空间，天然隔离

### CPU 亲和性 (CPU Affinity)

```rust
fn set_cpu_affinity(worker_id: usize, total_cpus: usize) {
    let cpu_id = worker_id % total_cpus;
    // sched_setaffinity 将当前进程绑定到指定 CPU
    libc::sched_setaffinity(0, ...)
}
```

**收益**：
- 消除 CPU 缓存抖动 (Cache Thrashing)
- 减少上下文切换
- 最大化 CPU L1/L2 缓存命中率

### SO_REUSEPORT

```rust
let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP));
socket.set_reuse_address(true)?;
socket.set_reuse_port(true)?;  // SO_REUSEPORT (仅 Linux)
socket.bind(&addr.into())?;
socket.listen(1024)?;
```

**内核负载均衡**：连接到达时，内核根据哈希算法（源 IP/端口）将连接分发到最空闲的 Worker 进程。

---

## 第二层：线程层 (Thread)

### Tokio 多线程运行时

每个 Worker 进程内部运行一个 Tokio 多线程运行时：

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(num_cpus::get())  // 线程数 = CPU 核数
    .enable_all()                      // 启用所有 I/O 驱动
    .build()
    .expect("创建 Tokio 运行时失败");
```

| 参数 | 说明 |
|------|------|
| `worker_threads` | Tokio 工作线程数 = num_cpus，最大化 CPU 利用率 |
| `enable_all` | 启用 IO、Time、Signal 所有驱动 |

### 线程调度策略

Tokio 的 `multi_thread` 运行时使用 **work-stealing 调度器**：

```
Worker Thread 0     Worker Thread 1     Worker Thread N
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│ Local Queue  │    │ Local Queue  │    │ Local Queue  │
├──────────────┤    ├──────────────┤    ├──────────────┤
│ Task A       │    │ Task D       │    │ Task G       │
│ Task B       │    │ Task E       │    │ wait()       │
│ wait()       │    │ wait()       │    │              │
└──────────────┘    └──────────────┘    └──────────────┘
        │                   │                   │
        └───────────────┬───┘                   │
                        │ steal                 │
                        ▼                       │
                Global Queue ◄───────────────────┘
```

- 空闲线程从繁忙线程的本地队列 **窃取** (steal) 任务
- 全局队列作为后备调度器
- 零成本任务迁移，无需操作系统上下文切换

### 收益

- **真正的并行执行**：N 个线程利用 N 个 CPU 核心
- **无锁调度**：work-stealing 减少锁竞争
- **缓存亲和**：任务尽量在创建它的线程上执行

---

## 第三层：协程层 (Coroutine)

### 异步任务模型

每个 HTTP 连接被包装为一个轻量级 Tokio 异步任务（协程）：

```rust
tokio::spawn(async move {
    let service = service_fn(move |req| {
        handler.handle(req, remote.clone())
    });
    
    // HTTP/1.1 或 HTTP/2
    if use_http2 {
        http2::Builder::new(executor)
            .serve_connection(io, service)
            .await;
    } else {
        http1::Builder::new()
            .keep_alive(true)
            .serve_connection(io, service)
            .await;
    }
});
```

### 协程 vs 线程

| 特性 | OS 线程 | Tokio 协程 |
|------|---------|-----------|
| 创建开销 | ~1MB 栈内存 | ~2KB 栈内存 |
| 上下文切换 | 微秒级 (内核态) | 纳秒级 (用户态) |
| 最大并发 | ~10^3 | ~10^7 |
| 切换成本 | 系统调用 | 函数调用 |

### 异步 I/O 流程

```
请求进入 → TCP accept → 创建协程 → 解析 HTTP 头
    ↓
处理请求 (异步)
    ├── 读文件 → tokio::fs::read (异步文件 I/O)
    ├── 代理请求 → hyper 客户端 (异步网络 I/O)
    ├── CGI 执行 → tokio::process (异步子进程)
    └── 限流检查 → 内存操作 (即时返回)
    ↓
序列化响应 → 写回 socket → 协程结束
```

### 并发控制

使用 Semaphore 限制最大并发连接数，防止资源耗尽：

```rust
let semaphore = Arc::new(Semaphore::new(config.threads));
// 每个连接获取一个许可
let permit = semaphore.clone().acquire_owned().await;
```

---

## 三层协同工作原理

### 请求处理路径

```
                    ┌───────────────────┐
                    │    客户端请求      │
                    └────────┬──────────┘
                             │
                     ▼ 内核分发 (SO_REUSEPORT)
                             │
             ┌───────────────┼───────────────┐
             ▼               ▼               ▼
      Worker 0          Worker 1        Worker N     进程层
      CPU Core 0        CPU Core 1      CPU Core N
             │               │               │
      Tokio 接收连接, 分配协程                     线程层
             │               │               │
      ┌──────┴──────┐  ┌────┴────┐  ┌──────┴──────┐
      │解析 HTTP    │  │解析 HTTP│  │解析 HTTP    │
      │处理请求     │  │处理请求 │  │处理请求     │ 协程层
      │生成响应     │  │生成响应 │  │生成响应     │
      └─────────────┘  └─────────┘  └─────────────┘
```

### 并发数学模型

假设系统有 **16 核 CPU**，配置如下：

- 进程数: 16 (每核 1 个 Worker)
- Tokio 线程数: 16 (每 Worker 16 线程)
- 协程数: 无上限 (受限于内存)

```
理论最大并发 = 进程数 × 线程数 × (内存限制的协程数)
             = 16 × 16 × N
```

实际瓶颈通常在网络带宽、磁盘 I/O 或业务逻辑，而非 CPU。

---

## 会话一致性方案

### 问题

多进程模式下，同一用户的请求可能被分发到不同 Worker 进程。
进程内存互不共享，需要跨进程 Session 数据同步。

### 解决方案：POSIX 共享内存哈希表

```
Worker 0 ←──┬──→ 共享内存 ←──┬──→ Worker 1
            │                 │
            ▼                 ▼
        shm_open + mmap (MAP_SHARED)
            │                 │
        同一物理内存         同一物理内存
```

### 内存布局

使用固定大小的开放地址哈希表：

```
[Header 64B]
  ├── magic: u64       = 0x534553534F484F53 ("SESSOHOS")
  ├── num_buckets: u32 = 65536
  ├── bucket_size: u32 = 960
  └── total_size: u64  = ~64MB

[Bucket 0] [Bucket 1] ... [Bucket 65535]
  每个桶:
  ├── lock: AtomicU8    (0=free, 1=locked)  ← 自旋锁
  ├── status: AtomicU8  (0=empty, 1=occupied, 2=deleted)
  ├── key: [u8; 64]     ← Session ID
  ├── data_len: u32     ← 数据长度
  └── data: [u8; 880]   ← JSON 序列化的 Session 数据
```

### 自旋锁实现

```rust
fn lock_bucket(&self, offset: usize) -> bool {
    // CAS 原子操作，最多重试 10000 次
    lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
}
```

- `compare_exchange` 是 CPU 原子指令 (CMPXCHG on x86)
- 自旋等待期间使用 `spin_loop()` 提示 CPU 降低功耗
- 无操作系统介入（对比 mutex 的内核态切换）

### 本地缓存 + 共享内存双通道

```
SessionStore.get(session_id)
    │
    ├── 1. 查本地 HashMap (RwLock 保护) → 命中直接返回
    │
    └── 2. 查共享内存哈希表 → 命中后缓存到本地 HashMap
```

**读操作**：本地缓存优先，共享内存次之，写入本地回填

```
SessionStore.update(session)
    │
    ├── 1. 写入本地 HashMap
    │
    └── 2. 写入共享内存哈希表
```

**写操作**：双写保证一致性

---

## 性能优化技术总结

### 1. 零拷贝 (Zero Copy)

- Session 序列化使用 `serde_json` 直接在 mmap 内存中读写
- 响应体使用 `hyper::body::Full<Bytes>` 避免数据拷贝

### 2. 无锁数据结构 (Lock-Free)

- 共享内存哈希表使用自旋锁 (CPU 原子指令)
- 无操作系统上下文切换
- 适用于短临界区操作

### 3. 内核级负载均衡

- `SO_REUSEPORT` 让内核分发连接
- 减少用户态调度开销
- 天然支持 NUMA 亲和

### 4. 缓存局部性 (Cache Locality)

- CPU 亲和性绑定 Worker 到固定核心
- 每个 Worker 拥有独立的内存分配器
- Tokio work-stealing 保持任务局部性

### 5. 异步文件 I/O

- `tokio::fs` 使用 `io_uring` (Linux 5.1+) 或线程池
- 不阻塞事件循环

### 6. 优雅关闭 (Graceful Shutdown)

- `SIGTERM` → 停止接受新连接 → 等待活跃请求完成 → 退出
- `SIGHUP` → 重新加载配置 → 旧 Worker 优雅退出 → 新 Worker 启动

---

## 配置示例

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "./www"
threads = 0    # 0 = 自动 = CPU 核数
workers = 0    # 0 = 自动 = CPU 核数

[session]
enabled = true
cookie_name = "OHOS_SESSION"
ttl = 3600
```

### 进程/线程/协程配置建议

| 场景 | workers | threads | 说明 |
|------|---------|---------|------|
| 开发机 (4核) | 0 (4) | 0 (4) | 自动检测 |
| 生产服务器 (16核) | 0 (16) | 0 (16) | 最大化并行 |
| 高隔离需求 | N | 2 | 进程隔离，较少线程 |
| 内存受限 | 1 | 4 | 单进程，多线程 |
