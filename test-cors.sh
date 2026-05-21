#!/bin/sh
set -e

cd /storage/Users/currentUser/Documents/Myapp/rust/ohos-http

# Kill any existing server
pkill -f ohosHttp 2>/dev/null || true
sleep 1

# Start server
target/debug/ohosHttp -c test-dual/config-dual.toml &
PID=$!
sleep 3

echo "=== 测试1: 无 CORS 配置时的跨域请求 ==="
echo ""
echo "--- GET 带 Origin 头 ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://evil.com" -o /dev/null http://127.0.0.1:8089/index.html 2>&1 | grep -i "access-control"
[ $? -ne 0 ] && echo "(无 CORS 头 - 符合预期)"
echo ""

echo "--- POST 带 Origin 头 ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://evil.com" -X POST -d "test" -o /dev/null http://127.0.0.1:8089/ 2>&1 | grep -i "access-control"
[ $? -ne 0 ] && echo "(无 CORS 头 - 符合预期)"
echo ""

echo "--- OPTIONS 预检 ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://evil.com" -H "Access-Control-Request-Method: POST" -X OPTIONS -o /dev/null http://127.0.0.1:8089/ 2>&1 | grep -i "access-control"
[ $? -ne 0 ] && echo "(无 CORS 头 - 符合预期)"
echo ""

# Now test with CORS enabled
echo "=== 测试2: 启用 CORS 配置 ==="
pkill -f ohosHttp 2>/dev/null || true
sleep 1

# Use a modified config with CORS enabled
cat > /tmp/config-cors.toml << 'TOML'
[[server]]
bind = "0.0.0.0:8089"
root = "./test-dual/site1"
domains = ["site1.example.com", "www.site1.example.com"]
workers = 1

cors_origin = "*"
cors_methods = "GET,POST,PUT,DELETE,PATCH,OPTIONS"
cors_headers = "Content-Type,Authorization,X-Requested-With"
TOML

target/debug/ohosHttp -c /tmp/config-cors.toml &
PID=$!
sleep 3

echo "--- GET 带 Origin 头 ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://evil.com" -o /dev/null http://127.0.0.1:8089/index.html 2>&1 | grep -i "access-control\|200"
echo ""

echo "--- POST 带 Origin 头 ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://example.com" -X POST -d "test" -o /dev/null http://127.0.0.1:8089/ 2>&1 | grep -i "access-control\|200\|405"
echo ""

echo "--- OPTIONS 预检 (应返回 204) ---"
curl -s -w "\nHTTP_CODE: %{http_code}" -D- -H "Host: site1.example.com" -H "Origin: https://example.com" -H "Access-Control-Request-Method: POST" -X OPTIONS -o /dev/null http://127.0.0.1:8089/ 2>&1
echo ""

echo "--- 特定源 cors_origin=example.com ---"
pkill -f ohosHttp 2>/dev/null || true
sleep 1

cat > /tmp/config-cors2.toml << 'TOML'
[[server]]
bind = "0.0.0.0:8089"
root = "./test-dual/site1"
domains = ["site1.example.com", "www.site1.example.com"]
workers = 1

cors_origin = "https://myapp.example.com"
cors_methods = "GET,POST"
cors_headers = "Content-Type"
TOML

target/debug/ohosHttp -c /tmp/config-cors2.toml &
PID=$!
sleep 3

echo "--- 匹配 Origin ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://myapp.example.com" -o /dev/null http://127.0.0.1:8089/index.html 2>&1 | grep -i "access-control"
echo ""

echo "--- 不匹配 Origin (evil origin) ---"
curl -s -D- -H "Host: site1.example.com" -H "Origin: https://evil.com" -o /dev/null http://127.0.0.1:8089/index.html 2>&1 | grep -i "access-control"
# Should still return the configured cors_origin (not dynamic)
echo ""

pkill -f ohosHttp 2>/dev/null || true
echo ""
echo "=== 测试完成 ==="
