#!/usr/bin/env bash
# Smoke-test a cost-report image: it must serve /healthz and /version, run
# as a non-root UID, and exit 0 promptly on SIGTERM (docker stop).
#
#   build/smoke-test.sh <image-ref> [expected-git-sha]
#
# The server runs in development mode (no cluster): only the probes are
# checked, the API is not.
set -euo pipefail

image="${1:?usage: $0 <image-ref> [expected-git-sha]}"
expected_sha="${2:-}"
name="cost-report-smoke-$$"
port="${SMOKE_PORT:-18080}"

cleanup() {
  docker logs "$name" 2>&1 | sed 's/^/[container] /' || true
  docker rm -f "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run --rm "$image" --version | grep -q '^cost-report ' || { echo "FAIL: --version" >&2; exit 1; }
echo "ok: --version"

docker run -d --name "$name" \
  -p "127.0.0.1:${port}:8080" \
  -e COST_REPORT_DEV_SCOPES='smoke=*' \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  "$image" server >/dev/null

body=""
for _ in $(seq 1 30); do
  if body="$(curl -fsS "http://127.0.0.1:${port}/healthz")"; then
    break
  fi
  sleep 1
done
if [ "$body" != "ok" ]; then
  echo "FAIL: /healthz returned '${body}', expected 'ok'" >&2
  exit 1
fi
echo "ok: /healthz"

version="$(curl -fsS "http://127.0.0.1:${port}/version")"
echo "/version: ${version}"
echo "$version" | jq -e '.name == "cost-report" and (.version | type == "string") and (.commit | type == "string")' >/dev/null
if [ -n "$expected_sha" ]; then
  echo "$version" | jq -e --arg sha "$expected_sha" '.commit == $sha' >/dev/null \
    || { echo "FAIL: commit does not match ${expected_sha}" >&2; exit 1; }
fi
echo "ok: /version"

pid="$(docker inspect -f '{{.State.Pid}}' "$name")"
uid="$(ps -o uid= -p "$pid" | tr -d ' ')"
if [ -z "$uid" ] || [ "$uid" = "0" ]; then
  echo "FAIL: container process runs as uid '${uid}'" >&2
  exit 1
fi
echo "ok: running as uid ${uid}"

start="$(date +%s)"
docker stop -t 10 "$name" >/dev/null
elapsed=$(( $(date +%s) - start ))
code="$(docker inspect -f '{{.State.ExitCode}}' "$name")"
if [ "$code" != "0" ] || [ "$elapsed" -ge 5 ]; then
  echo "FAIL: stop took ${elapsed}s with exit code ${code}" >&2
  exit 1
fi
echo "ok: graceful stop in ${elapsed}s"
