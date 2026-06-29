#!/bin/sh
# Healthcheck — exits 0 if all services respond
TIMEOUT=5
check() {
  curl -sf --max-time "$TIMEOUT" "$1" >/dev/null 2>&1 || {
    echo "FAIL: $1"; return 1
  }
  echo " OK:  $1"
}
check "http://localhost:8080/health"
check "http://localhost:5432"
check "http://localhost:6379/ping"
