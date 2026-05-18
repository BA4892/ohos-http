/*
 * Copyright 2025 ohosHttp Contributors
 *
 * 鸿蒙 ArkTS 管理 API 接口定义
 * 用于鸿蒙 PC/设备端 ArkTS 应用调用 ohosHttp 管理接口
 *
 * 使用前需确保 ohosHttp 已以 --manage-auth <TOKEN> 参数启动
 *
 * 示例:
 *   ohosHttp -a 0.0.0.0:8089 -r ./www --manage-auth mySecretToken
 */

// ============================================================================
//  接口基础类型定义
// ============================================================================

/** API 通用响应结构 */
export interface ApiResponse<T = any> {
  /** 状态码: 0 = 成功, 非0 = 错误 */
  code: number;
  /** 响应消息 */
  message: string;
  /** 响应数据 */
  data?: T;
  /** 错误信息（code 非0时存在） */
  error?: string;
}

// ============================================================================
//  配置相关类型
// ============================================================================

/** CGI 解释器配置 */
export interface CgiConfig {
  /** 解释器路径，如 /usr/bin/php-cgi */
  interpreter: string;
  /** 文件扩展名列表，默认 [".php"] */
  extensions: string[];
}

/** URL 重写规则（伪静态） */
export interface RewriteRule {
  /** 匹配正则，如 "^/(\\d+).html$" */
  from: string;
  /** 替换地址，支持 $1 捕获组，如 "/index.php?id=$1" */
  to: string;
}

/** 路径规则配置：反向代理、静态文件、负载均衡 */
export interface LocationConfig {
  /** 路径前缀，如 "/api" */
  path: string;
  /** 反向代理目标 URL，如 "http://127.0.0.1:8088" */
  proxy_pass?: string;
  /** 代理转发头 */
  proxy_headers?: string[];
  /** 静态文件根目录（可选） */
  root?: string;
  /** 缓存过期时间，如 "1h" */
  expires?: string;
  /** CGI 配置（可选） */
  cgi?: CgiConfig;
  /** 负载均衡后端目标列表 */
  load_balance_targets?: LoadBalanceTarget[];
}

/** 负载均衡后端目标 */
export interface LoadBalanceTarget {
  /** 后端 URL，如 "http://127.0.0.1:8081" */
  url: string;
  /** 权重，默认 1 */
  weight: number;
}

/** 限流配置 */
export interface RateLimitConfig {
  /** 是否启用 */
  enabled: boolean;
  /** 每 IP 每秒请求数 */
  requests_per_second: number;
  /** 突发大小 */
  burst_size: number;
}

/** Session 配置 */
export interface SessionConfig {
  /** 是否启用 */
  enabled: boolean;
  /** Cookie 名称，默认 "OHOS_SESSION" */
  cookie_name: string;
  /** 过期时间（秒），默认 3600 */
  ttl: number;
}

/** 单个站点配置 — 对应配置文件中 [[server]] */
export interface ServerConfig {
  /** 绑定地址，如 "0.0.0.0:8080" */
  bind: string;
  /** 网站根目录，如 "./www" */
  root: string;
  /** 绑定的域名列表（虚拟主机），如 ["example.com", "www.example.com"] */
  domains: string[];
  /** 上传最大大小，如 "10MB" */
  upload_max_size: string;
  /** Worker 进程数（0=自动=CPU核数） */
  workers: number;
  /** 是否启用缓存 */
  cache_enabled: boolean;
  /** 缓存 TTL，如 "1h", "7d" */
  cache_ttl: string;
  /** 缓存最大大小，如 "100MB" */
  cache_max_size: string;
  /** 是否启用目录列表 */
  directory_listing: boolean;
  /** 访问日志路径（可选） */
  access_log?: string;
  /** 日志轮转大小，如 "100MB"（0=不限制） */
  log_rotate_size: string;
  /** URL 重写规则 */
  rewrite: RewriteRule[];
  /** CGI 解释器配置 */
  cgi: CgiConfig[];
  /** 路径规则 */
  location: LocationConfig[];
  /** CORS 允许的源，如 "*" 或 "https://example.com" */
  cors_origin: string;
  /** CORS 允许的方法 */
  cors_methods: string;
  /** CORS 允许的请求头 */
  cors_headers: string;
  /** TLS 证书路径（可选） */
  cert?: string;
  /** TLS 私钥路径（可选） */
  key?: string;
  /** HTTP/3 (QUIC) 端口（0=不启用） */
  http3_port: string;
  /** 限流配置（可选） */
  rate_limit?: RateLimitConfig;
  /** IP 黑名单 */
  blacklist: string[];
  /** 自定义 IP 限流速率 */
  per_ip_rates: Record<string, number>;
  /** Session 配置（可选） */
  session?: SessionConfig;
  /** 是否允许 IP 直连 */
  allow_ip_access: boolean;
}

/** 完整应用配置 */
export interface AppConfig {
  /** 多个站点配置 */
  server: ServerConfig[];
  /** 配置文件路径（内部使用） */
  config_path: string;
}

// ============================================================================
//  状态相关类型
// ============================================================================

/** 服务器版本信息 */
export interface ServerInfo {
  version: string;
  name: string;
  description: string;
}

/** 服务器运行状态 */
export interface ServerStatus {
  /** 是否已暂停 */
  paused: boolean;
  /** 运行时长（秒） */
  uptime_secs: number;
  /** 可读格式的运行时长 */
  uptime_human: string;
  /** 当前进程 PID */
  pid: number;
  /** 父进程 PID */
  ppid: number;
}

/** 站点概览信息 */
export interface SiteInfo {
  bind: string;
  root: string;
  domains: string[];
  https: boolean;
  workers: number;
}

/** 状态响应数据 */
export interface StatusData {
  server: ServerInfo;
  status: ServerStatus;
  config: {
    config_path: string;
    server_count: number;
  };
  sites: SiteInfo[];
}

/** 配置响应数据 */
export interface ConfigData {
  config: AppConfig;
  config_path: string;
  server_count: number;
}

/** 配置更新结果 */
export interface ConfigUpdateData {
  config_path: string;
  format: string;
  original_format?: string;
}

// ============================================================================
//  指标相关类型
// ============================================================================

/** 请求统计 */
export interface RequestStats {
  total: number;
  per_second: number;
}

/** 运行时长信息 */
export interface UptimeInfo {
  seconds: number;
  human: string;
}

/** 进程信息 */
export interface ProcessInfo {
  pid: number;
  worker_index: number;
}

/** 指标响应数据 */
export interface MetricsData {
  requests: RequestStats;
  uptime: UptimeInfo;
  process: ProcessInfo;
  memory: Record<string, string>;
}

// ============================================================================
//  生命周期操作类型
// ============================================================================

/** 暂停/恢复操作结果 */
export interface PauseResult {
  paused: boolean;
}

// ============================================================================
//  管理 API 客户端
// ============================================================================

/**
 * ohosHttp 管理 API 客户端
 * 用于鸿蒙 ArkTS 应用调用 ohosHttp 管理接口
 *
 * 使用方式:
 * ```typescript
 * const client = new OhosHttpClient('http://192.168.1.100:8089', 'myToken');
 *
 * // 获取配置
 * const config = await client.getConfig();
 *
 * // 暂停服务
 * await client.pause();
 * ```
 */
export class OhosHttpClient {
  private baseUrl: string;
  private authToken: string;

  /**
   * @param baseUrl ohosHttp 服务器地址，如 'http://192.168.1.100:8089'
   * @param authToken 管理 API 认证 Token（--manage-auth 参数设置的值）
   */
  constructor(baseUrl: string, authToken: string) {
    // 确保 baseUrl 末尾无斜杠
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.authToken = authToken;
  }

  /**
   * 执行 HTTP 请求
   */
  private async request<T>(
    method: string,
    path: string,
    body?: object | string,
  ): Promise<ApiResponse<T>> {
    const url = `${this.baseUrl}${path}`;

    const headers: Record<string, string> = {
      'Authorization': `Bearer ${this.authToken}`,
    };

    let requestBody: string | undefined;

    if (body !== undefined) {
      if (typeof body === 'string') {
        headers['Content-Type'] = 'text/plain; charset=utf-8';
        requestBody = body;
      } else {
        headers['Content-Type'] = 'application/json; charset=utf-8';
        requestBody = JSON.stringify(body);
      }
    }

    try {
      const response = await fetch(url, {
        method,
        headers,
        body: requestBody,
      });

      const text = await response.text();

      try {
        return JSON.parse(text) as ApiResponse<T>;
      } catch {
        return {
          code: response.status,
          message: text,
        };
      }
    } catch (error) {
      return {
        code: -1,
        message: `Network error: ${error}`,
        error: 'NETWORK_ERROR',
      };
    }
  }

  // ──────── 配置管理 ────────

  /**
   * 获取服务器完整配置
   */
  async getConfig(): Promise<ApiResponse<ConfigData>> {
    return this.request<ConfigData>('GET', '/_ohos/config');
  }

  /**
   * 更新服务器配置（支持 TOML 或 JSON 格式）
   * @param configBody 配置内容，可以是 TOML 字符串或 JSON 对象
   */
  async updateConfig(configBody: string | object): Promise<ApiResponse<ConfigUpdateData>> {
    return this.request<ConfigUpdateData>('PUT', '/_ohos/config', configBody);
  }

  // ──────── 生命周期管理 ────────

  /**
   * 恢复服务（取消暂停状态）
   */
  async start(): Promise<ApiResponse<PauseResult>> {
    return this.request<PauseResult>('POST', '/_ohos/start');
  }

  /**
   * 暂停服务（新请求返回 503）
   */
  async pause(): Promise<ApiResponse<PauseResult>> {
    return this.request<PauseResult>('POST', '/_ohos/pause');
  }

  /**
   * 停止服务器（优雅关闭所有 Worker）
   */
  async stop(): Promise<ApiResponse<void>> {
    return this.request<void>('POST', '/_ohos/stop');
  }

  /**
   * 热重启服务器（重新加载配置 + 零停机重启 Worker）
   */
  async restart(): Promise<ApiResponse<void>> {
    return this.request<void>('POST', '/_ohos/restart');
  }

  // ──────── 状态与指标 ────────

  /**
   * 获取服务器状态
   */
  async getStatus(): Promise<ApiResponse<StatusData>> {
    return this.request<StatusData>('GET', '/_ohos/status');
  }

  /**
   * 获取运行时指标
   */
  async getMetrics(): Promise<ApiResponse<MetricsData>> {
    return this.request<MetricsData>('GET', '/_ohos/metrics');
  }

  // ──────── 便捷方法 ────────

  /**
   * 检查服务器是否正在运行
   */
  async isRunning(): Promise<boolean> {
    try {
      const result = await this.getStatus();
      return result.code === 0;
    } catch {
      return false;
    }
  }

  /**
   * 检查服务器是否已暂停
   */
  async isPaused(): Promise<boolean> {
    const result = await this.getStatus();
    return result.data?.status.paused ?? false;
  }

  /**
   * 获取服务器运行时长（秒）
   */
  async getUptime(): Promise<number> {
    const result = await this.getStatus();
    return result.data?.status.uptime_secs ?? 0;
  }
}

// ============================================================================
//  使用示例
// ============================================================================

/**
 * 使用示例 — 鸿蒙 PC ArkTS 应用
 *
 * ```typescript
 * // 在鸿蒙应用的 EntryAbility 或页面中
 * import { OhosHttpClient } from './ohos-http-api';
 *
 * @Entry
 * @Component
 * struct ServerManagerPage {
 *   @State serverStatus: string = '未知';
 *   @State isPaused: boolean = false;
 *   private client = new OhosHttpClient(
 *     'http://192.168.1.100:8089',
 *     'mySecretToken'
 *   );
 *
 *   aboutToAppear() {
 *     this.refreshStatus();
 *   }
 *
 *   async refreshStatus() {
 *     const result = await this.client.getStatus();
 *     if (result.code === 0 && result.data) {
 *       this.serverStatus = result.data.status.paused ? '已暂停' : '运行中';
 *       this.isPaused = result.data.status.paused;
 *     }
 *   }
 *
 *   build() {
 *     Column() {
 *       Text(`服务器状态: ${this.serverStatus}`)
 *         .fontSize(20)
 *         .margin(10)
 *
 *       Row() {
 *         Button('启动')
 *           .onClick(async () => {
 *             await this.client.start();
 *             this.refreshStatus();
 *           })
 *
 *         Button('暂停')
 *           .onClick(async () => {
 *             await this.client.pause();
 *             this.refreshStatus();
 *           })
 *
 *         Button('重启')
 *           .onClick(async () => {
 *             await this.client.restart();
 *             setTimeout(() => this.refreshStatus(), 3000);
 *           })
 *
 *         Button('停止')
 *           .onClick(async () => {
 *             await this.client.stop();
 *           })
 *       }
 *       .padding(10)
 *     }
 *     .width('100%')
 *     .height('100%')
 *   }
 * }
 * ```
 */
