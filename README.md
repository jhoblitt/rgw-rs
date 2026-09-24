# rgw-rs

A research spike: can Ceph's RADOS Gateway (`src/rgw` in ceph/ceph) be
extracted into its own repository and re-expressed in Rust, keeping its
REST -> op -> storage-abstraction layering, without depending on the rest
of Ceph?

This is throwaway proof-of-concept code. `FINDINGS.md` holds the answer;
the crates hold the evidence.

## Layout

One Cargo workspace, one crate per layer that RGW keeps as a directory or
a file family today:

| crate | mirrors in ceph/ceph `src/rgw` |
|---|---|
| `rgw-types` | `rgw_user_types.h`, `rgw_bucket_types.h`, `rgw_obj_types.h`, `rgw_common.h` |
| `rgw-sal` | `rgw_sal.h` (the storage abstraction layer, as a trait) |
| `rgw-sal-sqlite` | `driver/dbstore` |
| `rgw-rest` | `rgw_rest.{h,cc}` (shared REST plumbing, error rendering) |
| `rgw-auth` | `rgw_auth_s3.{h,cc}` (SigV4) |
| `rgw-rest-s3` | `rgw_rest_s3.{h,cc}` + the `RGWOp` subclasses in `rgw_op.cc` |
| `rgw-admin-ops` | `rgw_user.cc` / `rgw_bucket.cc` admin operations |
| `rgw-rest-admin` | `rgw_rest_admin.h`, `driver/rados/rgw_rest_{user,bucket}.cc` |
| `rgwd` | `rgw_main.cc`, `rgw_appmain.cc`, the asio frontend |
| `radosgw-admin` | `radosgw-admin/radosgw-admin.cc` |
| `rgw-tests` | integration tests against a real S3 client |

## Building

The crate registry is kept project-local so builds work inside a sandbox
whose `~/.cargo` is read-only:

```sh
export CARGO_HOME="$PWD/.cargo-home"
cargo build
cargo test
```

## Status

Every crate builds warning-free; 119 tests pass; `scripts/smoke.sh`
drives the built `rgwd` with the aws CLI, s3cmd and a botocore-signed
admin request and reports `ALL PASS`. Supported: SigV4 header auth,
ListBuckets, Create/Head/Delete bucket, ListObjects V1 and V2,
Put/Get/Head/Delete object, multi-object delete, and the `/admin/user`,
`/admin/bucket` and `/admin/info` APIs. Not supported: multipart,
versioning, ACLs beyond owner-only, presigned URLs, SigV2, TLS, Lua, and
any RADOS backend. See `FINDINGS.md`.

## Running it

```sh
export CARGO_HOME="$PWD/.cargo-home"   # or drop this to use ~/.cargo online
cargo test --workspace
bash scripts/smoke.sh                  # needs aws, s3cmd, python3 with botocore

cargo run -p rgwd -- --listen 127.0.0.1:7480 --db rgw.db
cargo run -p radosgw-admin -- --db rgw.db user create --uid me \
    --display-name Me --access-key AK --secret-key SK --caps 'users=*;buckets=*'
```
