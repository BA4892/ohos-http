# ohosHttp 架构文档

## 概述

ohosHttp 采用 **Workerman 架构** — **进程 (Process) × 协程 (Coroutine)** 双层设计。

```
┌──────────────────────────────────────────────────────────────────┐
│                      Master 进程                                  │
│  信号管理 (SIGTERM/SIGHUP/SIGCHLD) | Worker 监控 | 零停机热重启    │
└────────────┬─────────────────────────────────────┬───────────────┘
             │ fork()                              │ fork()
      ┌──────▼──────┐                     ┌──────▼──────┐
      │  Worker 1   │                     │  Worker N   │
      │ ─────────── │                     │ ─────────── │
      │ PID: 12345  │    ...              │ PID: 1234N  │
      │             │                     │             │
      │ SO_REUSEPORT│                     │ SO_REUSEPORT│
      │ bind():8080 │                     │ bind():8080 │
      │             │                     │             │
      │ Tokio       │                     │ Tokio       │
      │ 单线程      │                     │ 单线程      │
      │ 事件循环    │                     │ 事件循环    │
      │ 协程 001    │                     │ 协程 001    │  ← 连接级
      │ 协程 002    │                     │ 协程 002    │    async
      │ 协程 003    │                     │ 协程 003    │    任务
      │ ...         │                     │ ...         │
      └─────────────┘                     └─────────────┘
```

### 核心理念

| 层 | 单位 | 并发方式 | 资源开销 |
|----|------|---------|---------|
| **进程层** | Worker 进程 | `fork()` | ~2MB/进程 |
| **协程层** | async 任务 | `tokio::spawn` | ~1KB/任务 |

---

## 第一层：进程层 (Process)

### Master 进程

Master 是轻量级管理进程，**不处理任何 I/O**：

```
[Master] → 信号处理 → Worker 管理 → 优雅关闭
           ↓
     ┌─────┴─────┐
     │ SIGTERM   │ → 通知所有 Worker 优雅退出
     │ SIGHUP    │ → 逐个重启 Worker（零停机热加载）
     │ SIGCHLD   │ → Worker 挂了？立即自动 respawn
     └───────────┘
```

工作流程：
```
main()
  ↓
load_config()
  ↓
daemonize()  (可选)
  ↓
print_startup_banner()   ← 仅打印一次
  ↓
fork() × N               ← 预创建 N 个 Worker
  ↓
Master 进入信号循环: pause() → 处理信号 → 管理 Worker
```

### Worker 进程

每个 Worker 是一个独立进程，彼此完全隔离：

```rust
// 通过 fork() 创建
let pid = unsafe { libc::fork() };
match pid {
    0 => run_worker(),    // Child
    n => master_loop(),   // Parent
}
```

Worker 特点：
- **进程隔离**：一个 Worker 崩溃不影响其他 Worker
- **无锁设计**：没有共享状态，不需要互斥锁
- **独占 CPU**：每个 Worker 走自己的事件循环
- **SO_REUSEPORT**：内核负责连接分发

### SO_REUSEPORT — 内核层负载均衡

```
                        ┌──────────────┐
    Client 1  ─────────▶│              │
    Client 2  ─────────▶│  Linux 内核   │
    Client 3  ─────────▶│  数据包调度器  │
    Client 4  ─────────▶│  (哈希分发)   │
                        └──────┬───────┘
                               │
                  ┌────────────┼────────────┐
                  ▼            ▼            ▼
            Worker 1      Worker 2     Worker N
          bind(:8080)   bind(:8080)   bind(:8080)
          SO_REUSEPORT  SO_REUSEPORT  SO_REUSEPORT
```

实现细节：

```rust
let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
socket.set_reuse_address(true)?;
// 启用 SO_REUSEPORT
libc::setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &1, size_of::<c_int>());
socket.bind(&addr)?;
socket.listen(1024)?;
```

---

## 第二层：协程层 (Coroutine)

### 单线程事件循环

每个 Worker 运行一个 Tokio **单线程** 运行时：

```rust
let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("创建 Worker 运行时失败");
```

- 只有一个 OS 线程在运行
- 所有 I/O 操作是非阻塞的
- 没有锁竞争
- 没有线程切换开销

### 连接处理模型

```
事件循环
  │
  ├─ epoll_wait() 等待事件
  │
  ├─ accept() → tokio::spawn(async { ... })  ← 协程 1
  ├─ accept() → tokio::spawn(async { ... })  ← 协程 2
  ├─ read() 完成 → 唤醒协程 1
  ├─ accept() → tokio::spawn(async { ... })  ← 协程 3
  ├─ read() 完成 → 唤醒协程 2
  │
  └─ 循环往复
```

### 协程 vs 线程

| 特性 | 进程 | 协程 (async task) |
|------|------|-------------------|
| 创建开销 | ~10μs fork | ~10ns spawn |
| 内存占用 | ~2MB | ~1KB |
| 切换成本 | 内核上下文切换 | 状态机跳转 |
| 隔离性 | 完全隔离 | 同一进程内共享 |
| 并发数量 | 数千 (受限内存) | 数十万 |
| 适用场景 | CPU 密集型 / 强隔离 | I/O 密集型 |

---

## 生命周期

### 启动流程

```
Master
  │
  ├─ 加载配置
  ├─ 打印 Banner (一次)
  ├─ block SIGTERM/SIGHUP/SIGINT (fork 前)
  │
  ├─ fork() → Worker 1 ──┬── unblock signals
  │                       ├── create single-thread runtime
  │                       ├── SO_REUSEPORT bind
  │                       ├── signal handlers (Tokio)
  │                       └── accept loop
  │
  ├─ fork() → Worker 2 ──┬── (同上)
  │                       └── ...
  │
  ├─ unblock signals (Master)
  └─ signal loop (libc::pause)
```

### 优雅关闭

```
收到 SIGTERM/SIGINT
        │
        ▼
   ┌─ Master ────────────────────────┐
   │ Kill Worker #0 (SIGTERM)        │
   │ Kill Worker #1 (SIGTERM)        │
   │ ...                             │
   │ Wait for all Workers (waitpid)  │
   │ Exit                            │
   └─────────────────────────────────┘
                │
     ┌──────────▼──────────┐
     │  Worker 收到 SIGTERM │
     │                     │
     │ Stop accepting      │
     │ Wait for active     │
     │ requests (~30s max) │
     │ Exit                │
     └─────────────────────┘
```

### 零停机热重启 (SIGHUP)

```
Master 收到 SIGHUP
  │
  ├─ 重新加载配置（如果使用配置文件）
  │
  ├─ Worker #0: SIGTERM → waitpid → fork 新 Worker
  ├─ Worker #1: SIGTERM → waitpid → fork 新 Worker
  ├─ ...
  │
  └─ 热重启完成
       │
       任何时候都有 N-1 个 Worker 在运行
```

### 自动恢复

```
Worker 因 SIGSEGV/OOM 等意外崩溃
        │
        ▼
SIGCHLD → Master 收到
  │
  ├─ waitpid() 获取退出状态
  ├─ 找到 worker ID
  └─ fork() 新 Worker → 自动替换
```

---

## Session 管理

每个 Worker 有独立的 Session 存储（进程内）：

```rust
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    // ...
}
```

- 无跨进程共享状态（Workerman 风格）
- 适合**无状态**或**后端 Session 存储**（如 Redis/Memcached）的场景
- 默认内存存储，如需跨 Worker 共享 → 配置外部 Session 存储

---

## 配置说明

```toml
[[server]]
bind = "0.0.0.0:8080"
root = "/var/www/html"
workers = 4                    # Worker 进程数（0 = CPU 核心数）
```

### 配置参考

| 场景 | 推荐 Workers | 说明 |
|------|-------------|------|
| 普通 Web 服务 | CPU 核心数 | I/O 密集，进程隔离 |
| 高并发 API | CPU 核心数 | 内核负载均衡到各 Worker |
| 静态文件服务 | CPU 核心数 | SO_REUSEPORT 高效分发 |
| CGI/PHP | CPU 核心数 或 ×2 | 进程隔离保护 Master |
| 单进程调试 | 1 | `-w 1` 即单进程回退 |

### 性能调优

1. **Worker 数量**：通常 = CPU 核心数（超线程也算核心）
2. **过多 Worker**：增加上下文切换和内存开销
3. **过少 Worker**：CPU 无法充分利用
4. **SO_REUSEPORT 哈希不均**：使用 `--reuseport` 的 `np` (negotiated port) 或增加 Worker 数量

---

## 与标准 Workerman 的差异

| 特性 | PHP Workerman | ohosHttp (Rust) |
|------|--------------|-----------------|
| 运行时 | PHP + event extension | Tokio 单线程运行时 |
| 协程 | Generator/yield | async/await + Future |
| 连接处理 | callback | async task |
| 类型安全 | 动态类型 | Rust 静态类型 |
| 内存安全 | PHP GC | Rust 所有权系统 |
