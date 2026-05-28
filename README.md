# ohos-server - High-Performance HTTP Server

A cross-platform HTTP server supporting HarmonyOS (OpenHarmony) and Linux x86_64 platforms.

## Features

- ✅ **Static File Service** - Provides access to static files in the website root directory
- ✅ **Multi-Site Configuration** - Supports multiple virtual hosts via configuration files
- ✅ **Virtual Hosts** - Request routing based on domain names
- ✅ **URL Rewriting (Pseudo-Static)** - URL rewriting rules using regular expressions
- ✅ **Reverse Proxy** - Forwards requests to backend services, with support for path prefix matching
- ✅ **CGI Support** - Supports execution of CGI scripts (e.g., PHP)
- ✅ **Access Logs** - Automatically rotated access logs with support for date and size-based rotation
- ✅ **CORS (Cross-Origin Resource Sharing)** - Configurable support for Cross-Origin Resource Sharing
- ✅ **Multiple Request Methods** - Supports GET, POST, PUT, DELETE, HEAD, OPTIONS, and PATCH
- ✅ **HTTPS (TLS)** - Enable TLS encryption via the `--cert` and `--key` parameters
- ✅ **HTTP/2 Support** - Automatic ALPN negotiation; automatically upgrades to HTTP/2 over TLS connections
- ✅ **HTTP/3 (QUIC)** - Enable UDP/QUIC connections via the `--http3-port` parameter
- ✅ **Signal Handling** - Graceful shutdown via SIGTERM, hot restart via SIGHUP
- ✅ **Daemon Mode** - Runs in the background and manages PID files
- ✅ **Path Security** - Prevents path traversal attacks
- ✅ **Cross-Platform** - Supports HarmonyOS and Linux
- ✅ **IP Throttling** - Token bucket algorithm, independent throttling per IP, configurable rate and burst size
- ✅ **IP Blacklist** - Completely block specified IPs or IP ranges, supports `*` wildcards
- ✅ **Custom Throttling** - Configure different throttling rates per IP
- ✅ **IP Access Control** - Disable direct IP connections; allow access only from bound domains
- ✅ **Session Support** - Cookie-based in-memory session management with TTL expiration support
- ✅ **Access Blocking** - Block access based on directories and file types; automatically bypass reverse proxy sites
- ✅ **Load Balancing** - Distributes requests to multiple backend servers using weighted round-robin

## Complete Documentation

Please refer to **[USAGE.md](USAGE.md)** for the complete user documentation, including:

- Explanations of all command-line options
- Daemon mode (runs in the background)
- CGI interpreter configuration (PHP, Python, Perl, etc.)
- Detailed explanation of all configuration file parameters
- Examples of pseudo-static rewrite rules
- Reverse proxy and path rules
- Rate limiting and blacklist configuration
- IP access control
- Session management
- Load balancing configuration
- Multi-site configuration
- Caching configuration
- Troubleshooting

---

## Quick Start

### Method 0: One-click installation (Recommended)

```bash
# Download and install with one click
chmod +x install.sh
./install.sh
```

### Method 1: Quick Start via Command Line

```bash
# Bind address and port, specify website directory
./ohos-server -a 127.0.0.1:8080 -r ./www

# Specify the number of worker threads
./ohos-server --addr=0.0.0.0:8089 --root=/var/www --threads=4
```

### Method 2: Start via Configuration File

```bash
# Generate the default configuration file
