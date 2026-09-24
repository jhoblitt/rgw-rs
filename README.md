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
