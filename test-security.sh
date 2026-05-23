#!/bin/sh
# 🔐 安全测试脚本 — ohos-server v1.4.0
cd /storage/Users/currentUser/Documents/Myapp/rust/ohos-server

PASS=0; FAIL=0

check() {
  local desc="$1" expected="$2" code; shift 2
  code=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout 3 --max-time 5 "$@" 2>/dev/null)
  if [ "$code" = "$expected" ]; then
    echo "  ✅ $desc → $code (expect $expected)"; PASS=$((PASS+1))
  else
    echo "  ❌ $desc → $code (expect $expected)"; FAIL=$((FAIL+1))
  fi
}

echo "╔══════════════════════════════════════════════╗"
echo "║    🔐 Security Test — ohos-server v1.4.0      ║"
echo "╚══════════════════════════════════════════════╝"
echo

# ─── [1] Path Traversal ───
echo "─── [1] Path Traversal ───"
check "basic ../.." "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/../../../etc/passwd"
check "deep ../../.." "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/../../../../etc/passwd"
check "urlencoded %2e%2e%2f" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/%2e%2e%2f%2e%2e%2fetc/passwd"
check "double encode %252e" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/%252e%252e%252fetc/passwd"
check "unicode %c0%ae" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/%c0%ae%c0%ae%c0%afetc/passwd"
check "mixed .." "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/foo/../../../etc/passwd"
check "double slash //" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089//etc/passwd"

# ─── [2] Sensitive Files ───
echo "─── [2] Sensitive Files ───"
check ".env" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/.env"
check ".git/config" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/.git/config"
check "Cargo.toml" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/Cargo.toml"
check "config-dual.toml" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/config-dual.toml"

# ─── [3] HTTP Methods ───
echo "─── [3] HTTP Methods ───"
check "TRACE" "405" -X TRACE -H "Host: site1.example.com" "http://127.0.0.1:8089/"
check "CONNECT" "405" -X CONNECT -H "Host: site1.example.com" "http://127.0.0.1:8089/"
check "OPTIONS" "204" -X OPTIONS -H "Host: site1.example.com" "http://127.0.0.1:8089/"
check "PUT" "405" -X PUT -H "Host: site1.example.com" -d "evil" "http://127.0.0.1:8089/evil.txt"
check "DELETE" "405" -X DELETE -H "Host: site1.example.com" "http://127.0.0.1:8089/index.html"
check "PATCH" "405" -X PATCH -H "Host: site1.example.com" "http://127.0.0.1:8089/"

# ─── [4] Directory Listing ───
echo "─── [4] Directory Listing ───"
check "root dir (listing=off)" "403" -H "Host: site1.example.com" "http://127.0.0.1:8089/"

# ─── [5] VHost Isolation ───
echo "─── [5] VHost Isolation ───"
c1=$(curl -s --max-time 3 -H "Host: site1.example.com" "http://127.0.0.1:8089/index.html")
c2=$(curl -s --max-time 3 -H "Host: blog.example.com" "http://127.0.0.1:8089/index.html")
echo "$c1" | grep -q "站点 1" && { echo "  ✅ site1 → site1 content"; PASS=$((PASS+1)); } || { echo "  ❌ site1 wrong"; FAIL=$((FAIL+1)); }
echo "$c2" | grep -q "站点 2" && { echo "  ✅ blog → site2 content"; PASS=$((PASS+1)); } || { echo "  ❌ blog wrong"; FAIL=$((FAIL+1)); }

# ─── [6] Normal request (sanity check) ───
echo "─── [6] Sanity ───"
check "normal GET" "200" -H "Host: site1.example.com" "http://127.0.0.1:8089/index.html"
check "nonexistent file" "404" -H "Host: site1.example.com" "http://127.0.0.1:8089/nonexistent.html"

echo
echo "══════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed"
echo "══════════════════════════════════════════"
