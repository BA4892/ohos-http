# ohosHttp 架构文档

## 概述

ohosHttp 采用 **线程 (Thread) × 协程 (Coroutine)** 双层架构设计，基于 Tokio 异步运行时实现全异步非阻塞 I/O。

```
┌─────────────────────────────────────────────────────┐
│                   ohosHttp 进程                       │
│  信号管理 | 优雅关闭 | 配置文件热重载                   │
└──────────────────────┬──────────────────────────────┘
                       │
                  ┌────▼────┐
                  │Tokio    │
                  │Multi-   │  ← 线程层 (OS 线程)
                  │Thread   │     Work-Stealing 调度
                  │Runtime  │
                  └────┬────┘
                       │
                  ┌────▼────┐
                  │协程池   │
                  │Task 001 │  ← 协程层 (异步任务)
                  │Task 002 │     每连接一个 async 任务
                  │Task 003 │    零成本上下文切换
                  │...      │
                  └─────────┘
```

---

## 第一层：线程层 (Thread)

### Tokio 多线程运行时

使用 `tokio::runtime::Builder::new_multi_thread()` 创建多线程运行时：

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(n)   // 可通过 -t/--threads 配置
    .enable_all()
    .build()
    .expect("创建 Tokio 运行时失败");
```

- 每个工作线程 (worker thread) 运行一个独立的 `tokio` 事件循环
- 线程数通过 `--threads` 或配置文件的 `threads` 字段设置
- 默认值 = CPU 核心数 (通过 `std::thread::available_parallelism()` 检测)
- 最小值为 2

### Work-Stealing 调度

Tokio 的多线程运行时采用 **Work-Stealing** 调度策略：

```
线程 0: [Task A] [Task B] [Task C] ████████████████
线程 1: [Task D] █████████████████████████████████
线程 2: ██████████████████████████████████████████
线�3: [Task E] [Task F] ██████████████████████████
```

- 每个线程维护自己的就绪任务队列
- 当某线程队列为空时，从其他线程"偷取"任务
- 自动均衡负载，无需手动分配

### 收益

- **真正的并行执行**：多核 CPU 同时处理多个连接
- **自动负载均衡**：Work-Stealing 确保所有核心充分利用
- **无需多进程**：单进程内即可充分利用多核

---

## 第二层：协程层 (Coroutine)

### 异步任务模型

每个 TCP 连接对应一个异步任务 (Task)：

```rust
// 每连接一个独立异步任务（协程）
tokio::spawn(async move {
    let service = service_fn(|req| handler.handle(req, remote));
    
    if let Err(err) = conn.await {
        error!("连接处理错误: {}", err);
    }
});
```

### 协程 vs 线程

| 特性 | 线程 (OS Thread) | 协程 (async Task) |
|------|-----------------|-------------------|
| 创建开销 | ~1μs | ~10ns |
| 内存占用 | ~1MB 栈 | ~1KB 栈 (heap) |
| 切换成本 | 系统调用 ~100ns | 状态机跳转 ~1ns |
| 并发数量 | 数千 | 数百万 |
| 上下文保存 | 内核完整保存寄存器 | 仅保存局部变量 |

### 异步 I/O 流程

```
┌─────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ Client  │────▶│  Accept   │────▶│  读取     │────▶│  处理     │
│  请求    │     │  连接     │     │  请求体   │     │  请求     │
└─────────┘     └──────────┘     └──────────┘     └──────────┘
                                                    │
                                                    ▼
                 ┌──────────┐     ┌──────────┐
                 │ Client   │◀────│  写入     │
                 │  响应     │     │  响应体   │
                 └──────────┘     └──────────┘
```

每个步骤都是异步非阻塞的：
- `accept()` → 不阻塞，连接到达时被唤醒
- `read()` → 不阻塞，数据到达时被唤醒
- `write()` → 不阻塞，缓冲区可写时被唤醒

### 并发控制

使用 `Semaphore` 限制最大并发连接数：

```rust
let semaphore = Arc::new(Semaphore::new(self.config.threads));
let permit = semaphore.clone().acquire_owned().await;
```

---

## 双层协同工作原理

### 请求处理路径

```
客户端连接
   │
   ▼
线程 0 的 EventLoop 获得连接
   │
   ├─ tokio::spawn(协程 1)    ──▶  线程 0 处理
   ├─ tokio::spawn(协程 2)    ──▶  线程 1 偷取
   ├─ tokio::spawn(协程 3)    ──▶  线程 2 偷取
   │
   ▼
协程处理请求 (异步 I/O)
   │
   ├─ read() 遇到 EAGAIN     ──▶  挂起，交出CPU
   ├─ 其他就绪协程开始执行   ──▶  充分利用CPU
   ├─ 数据到达，协程被唤醒   ──▶  继续处理
   │
   ▼
返回响应 → 关闭连接
```

### 并发数学模型

$$C = T \times \frac{R + W}{R}$$

其中：
- $C$ = 最大并发连接数
- $T$ = 线程数
- $R$ = 计算时间
- $W$ = 等待时间 (I/O)

当 $W >> R$ 时（典型 Web 场景），并发量远大于线程数。

---

## Session 管理

Session 采用内存存储，线程安全：

```rust
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    cookie_name: String,
    ttl: Duration,
    // ...
}
```

- 使用 `RwLock` 实现并发安全的读写
- 定时清理过期 Session（每 5 分钟）
- HttpOnly + SameSite 安全 Cookie
- 单进程多线程模式下所有请求共享同一 Session 存储

---

## 性能优化技术总结

### 1. 零拷贝 (Zero Copy)

使用 `Bytes` 和 `Full<Bytes>` 避免不必要的数据复制：
- 文件读取 → `tokio::fs::read()` → `Bytes`
- 响应体构造 → `Full::new(bytes)`
- 流式响应 → `BodyExt::collect()`

### 2. 异步文件 I/O

使用 `tokio::fs` 进行非阻塞文件操作：
- 不阻塞工作线程
- 内部使用 `io_uring` (Linux) 或 `evented I/O`

### 3. 内存缓存

可选的 LRU 文件缓存：
- 缓存热点文件内容
- 自动过期和大小限制
- 使用 `RwLock` 实现并发安全

### 4. 优雅关闭 (Graceful Shutdown)

```
收到 Ctrl+C / SIGTERM / SIGQUIT
   │
   ▼
停止接受新连接
   │
   ▼
等待进行中的请求完成（最多 30 秒）
   │
   ▼
关闭所有连接
   │
   ▼
清理资源 → 退出
```

### 5. 配置文件热重载

```
收到 SIGHUP 信号
   │
   ▼
重新读取配置文件
   │
   ▼
启动新配置的服务器
   │
   ▼
关闭旧服务器
   │
   ▼
继续服务
```

---

## 配置示例

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "/var/www/html"
threads = 4           # Tokio 运行时线程数（非阻塞 IO 线程）
```

### 配置建议

| 场景 | 推荐线程数 | 说明 |
|------|-----------|------|
| 普通 Web 服务 | CPU 核心数 | I/O 密集，再多会徒增切换开销 |
| 高并发 API | CPU 核心数 × 2 | 计算与 I/O 混合 |
| 静态文件服务 | CPU 核心数 | I/O 密集，tokio 异步即可 |
| CGI/PHP | CPU 核心数 | 计算密集，多核优势 |
