#!/usr/bin/env bash
# Interop smoke test: start rgwd on a scratch database and drive it with
# the aws CLI, s3cmd and a botocore-signed admin request.
# Honors CARGO_HOME / CARGO_TARGET_DIR. Everything runs on 127.0.0.1 with
# a random free port and a throwaway database.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; exit 1; }
# step <name> <command...>: run quietly, show the output only on failure.
step() {
    local name=$1; shift
    local out
    if out=$("$@" 2>&1); then
        pass "$name"
    else
        printf '%s\n' "$out" >&2
        fail "$name"
    fi
}

(cd "$ROOT" && cargo build -q -p rgwd -p radosgw-admin --offline) || fail "cargo build"
pass "cargo build rgwd radosgw-admin"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/rgw-smoke.XXXXXX")
RGWD_PID=
cleanup() {
    local rc=$?
    if [[ -n $RGWD_PID ]]; then
        kill "$RGWD_PID" 2>/dev/null || true
        wait "$RGWD_PID" 2>/dev/null || true
    fi
    if [[ $rc -ne 0 && -f $TMP/rgwd.log ]]; then
        echo "--- rgwd log (last 40 lines) ---" >&2
        tail -n 40 "$TMP/rgwd.log" >&2
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
ENDPOINT=http://127.0.0.1:$PORT

RUST_LOG=${RUST_LOG:-info} "$TARGET_DIR/debug/rgwd" --listen "127.0.0.1:$PORT" --db "$TMP/rgw.db" \
    >"$TMP/rgwd.log" 2>&1 &
RGWD_PID=$!
for _ in $(seq 100); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then break; fi
    kill -0 "$RGWD_PID" 2>/dev/null || fail "rgwd exited during startup"
    sleep 0.1
done
(exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null || fail "rgwd did not listen on $PORT"
pass "rgwd listening on $ENDPOINT"

step "radosgw-admin user create" \
    "$TARGET_DIR/debug/radosgw-admin" --db "$TMP/rgw.db" user create --uid smoke --display-name Smoke \
    --access-key SMOKEKEY --secret-key smokesecret --caps 'users=*;buckets=*'

# Keep every client away from the invoking user's config and credentials.
export AWS_ACCESS_KEY_ID=SMOKEKEY AWS_SECRET_ACCESS_KEY=smokesecret AWS_DEFAULT_REGION=default
export AWS_CONFIG_FILE=$TMP/aws-config AWS_SHARED_CREDENTIALS_FILE=$TMP/aws-credentials
export AWS_EC2_METADATA_DISABLED=true AWS_PAGER=""
: >"$AWS_CONFIG_FILE"
: >"$AWS_SHARED_CREDENTIALS_FILE"
aws() { command aws --endpoint-url "$ENDPOINT" "$@"; }

head -c 1048576 /dev/urandom >"$TMP/blob.bin"

# ---- aws CLI ----------------------------------------------------------
step "aws s3 mb" aws s3 mb s3://smoke
step "aws s3 cp (upload, 1 MiB)" aws s3 cp "$TMP/blob.bin" s3://smoke/dir/blob.bin --metadata color=blue
ls_out=$(aws s3 ls s3://smoke/ --recursive) || fail "aws s3 ls"
grep -q ' 1048576 dir/blob.bin$' <<<"$ls_out" || { echo "$ls_out"; fail "aws s3 ls shows dir/blob.bin"; }
pass "aws s3 ls --recursive"
head_out=$(aws s3api head-object --bucket smoke --key dir/blob.bin) || fail "aws s3api head-object"
python3 -c '
import json, sys
h = json.load(sys.stdin)
assert h["Metadata"] == {"color": "blue"}, h
assert h["ContentLength"] == 1048576, h
' <<<"$head_out" || { echo "$head_out"; fail "head-object metadata"; }
pass "aws s3api head-object (metadata color=blue, 1 MiB)"
step "aws s3 cp (download)" aws s3 cp s3://smoke/dir/blob.bin "$TMP/back.bin"
step "cmp aws round trip" cmp "$TMP/blob.bin" "$TMP/back.bin"
range_out=$(aws s3api get-object --bucket smoke --key dir/blob.bin --range bytes=0-9 "$TMP/range.bin") \
    || fail "aws s3api get-object --range"
grep -q '"ContentRange": "bytes 0-9/1048576"' <<<"$range_out" || { echo "$range_out"; fail "range ContentRange"; }
cmp "$TMP/range.bin" <(head -c 10 "$TMP/blob.bin") || fail "range bytes"
pass "aws s3api get-object --range bytes=0-9"
step "aws s3 rm --recursive" aws s3 rm s3://smoke --recursive
step "aws s3 rb" aws s3 rb s3://smoke

# ---- s3cmd ------------------------------------------------------------
: >"$TMP/s3cfg"
s3c() {
    s3cmd -c "$TMP/s3cfg" --no-ssl --host="127.0.0.1:$PORT" --host-bucket="127.0.0.1:$PORT" \
        --access_key=SMOKEKEY --secret_key=smokesecret "$@"
}
step "s3cmd mb" s3c mb s3://smoke2
step "s3cmd put" s3c put "$TMP/blob.bin" s3://smoke2/dir/blob.bin
s3ls=$(s3c ls s3://smoke2/dir/) || fail "s3cmd ls"
grep -q 's3://smoke2/dir/blob.bin$' <<<"$s3ls" || { echo "$s3ls"; fail "s3cmd ls shows the object"; }
pass "s3cmd ls"
step "s3cmd get" s3c get s3://smoke2/dir/blob.bin "$TMP/back2.bin"
step "cmp s3cmd round trip" cmp "$TMP/blob.bin" "$TMP/back2.bin"
step "s3cmd rm" s3c rm s3://smoke2/dir/blob.bin
step "s3cmd rb" s3c rb s3://smoke2

# ---- admin API signed by botocore --------------------------------------
admin_out=$(PORT=$PORT python3 - <<'PY'
import hashlib, json, os, urllib.error, urllib.request
import botocore.session
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest

url = f"http://127.0.0.1:{os.environ['PORT']}/admin/user?uid=smoke"
creds = botocore.session.get_session().get_credentials()
req = AWSRequest(method="GET", url=url)
# Plain SigV4Auth (unlike botocore's S3SigV4Auth) sends no
# x-amz-content-sha256, which S3 and rgwd require; set it so the signer
# signs it and uses it as the payload hash.
req.headers["x-amz-content-sha256"] = hashlib.sha256(b"").hexdigest()
SigV4Auth(creds, "s3", "default").add_auth(req)
prepared = req.prepare()
try:
    with urllib.request.urlopen(urllib.request.Request(url, headers=dict(prepared.headers))) as resp:
        doc = json.load(resp)
except urllib.error.HTTPError as e:
    raise SystemExit(f"HTTP {e.code}: {e.read().decode()}")
print(json.dumps({k: doc[k] for k in ("user_id", "display_name", "caps")}))
assert doc["user_id"] == "smoke", doc
assert doc["keys"][0]["access_key"] == "SMOKEKEY", doc
PY
) || fail "botocore-signed GET /admin/user?uid=smoke"
echo "      $admin_out"
pass "botocore-signed GET /admin/user?uid=smoke"

echo "ALL PASS"
