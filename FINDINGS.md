# Findings: extracting RGW from ceph/ceph and porting it to Rust

Research spike, 2026-09-24. Measured against ceph/ceph `main` at
`e234256339f`. Everything under `crates/` is throwaway evidence for the
conclusions here, not a product.

## The question

Can RGW (`src/rgw` in ceph/ceph) be extracted into its own repository and
re-expressed in Rust while keeping its architecture, with a minimum viable
result that compiles and serves basic S3 and admin-API requests? Lua
scripting may be omitted.

## Short answer

Yes for the gateway core, with a clear seam and precedent on both counts,
and no for a drop-in replacement of `radosgw` on an existing cluster
within any short horizon. The two halves have very different costs:

- **The protocol and admin layers port cleanly.** RGW's request pipeline
  (frontend, REST parsing, `RGWOp`, storage abstraction layer) maps onto
  tokio, axum and one async trait. The spike's crates implement SigV4
  header authentication, a working S3 subset, backend-neutral user and
  bucket admin operations, and a `radosgw-admin` CLI over an SQLite driver,
  with no dependency on any ceph library.
- **The RADOS backend is the expensive part, and it is not about RADOS.**
  RGW on RADOS is a client of eleven object-class libraries whose logic
  runs inside the OSD, spoken over ceph's versioned binary encoding. A
  Rust RADOS driver that reads clusters written by C++ RGW must reproduce
  66 `cls_rgw` request/response encodings bit-exactly, plus the encoders of
  about a dozen central metadata types (`RGWBucketInfo` is at encoding
  version 24). That is mechanical but large, and it is the cost that
  decides whether a port is a rewrite of RGW or a new S3 gateway that
  happens to store on RADOS.

Recommended shape, if this proceeds: a standalone Rust repository owning
the gateway core, developed against the SQLite driver and conformance
suite; a RADOS driver added as a second implementor of the same trait,
scoped first to the object-data path (librados only) and only later to
the `cls_rgw` bucket index, GC, usage and lifecycle protocols; multisite
left in C++ indefinitely. The section "Staging" below argues the order.

## What the spike built

One Cargo workspace, eleven crates, about 6,300 lines of Rust including
tests, with no ceph package anywhere in the dependency graph (325 locked
packages, 44 direct). The layering follows RGW's; each crate's doc
comments name the C++ file or class it mirrors.

| Crate | Mirrors | Lines | What it does |
|---|---|---|---|
| `rgw-types` | `rgw_*_types.h`, `rgw_common.h` | 500 | `UserId`, `UserInfo`, `AccessKey`, `UserCaps`, `BucketInfo`, `ObjectInfo`, `Attrs`, and `RgwError` carrying the S3 code and HTTP status |
| `rgw-sal` | `rgw_sal.h` | 360 | the `Driver` trait (17 async methods) and a conformance suite any driver runs |
| `rgw-sal-sqlite` | `driver/dbstore` (14k lines of C++) | 615 | the trait on SQLite via rusqlite; passes the suite and a reopen test |
| `rgw-rest` | `rgw_rest.{h,cc}` | 95 | request ids, S3 XML and admin JSON error documents, date formats |
| `rgw-auth` | `rgw_auth_s3.{h,cc}` | 780 + 330 tests | SigV4 header auth, hashed and unsigned payloads, `aws-chunked` with chunk signatures and crc32 trailers; matches the AWS documentation vectors including the multi-chunk example |
| `rgw-rest-s3` | `rgw_rest_s3.cc`, `rgw_op.cc` | 1,630 + 470 tests | ListBuckets, Create/Head/Delete bucket, ListObjects V1 and V2 with paging and `encoding-type=url`, Put/Get/Head/Delete object with ranges and metadata, multi-object delete; 501 for every other subresource |
| `rgw-admin-ops` | `rgw_user.cc`, `rgw_bucket.cc` | 635 | user create/info/modify/remove/list, key and caps add/remove, bucket info/list/remove with purge, RGW-shaped JSON documents |
| `rgw-rest-admin` | `rgw_rest_admin.h`, `driver/rados/rgw_rest_{user,bucket}.cc` | 490 | `/admin/user`, `/admin/bucket`, `/admin/info`, cap-gated |
| `rgwd` | `rgw_main.cc`, `rgw_appmain.cc`, asio frontend | 50 | tokio + axum; the router is a library function so tests host it in-process |
| `radosgw-admin` | `radosgw-admin.cc` | 300 | the same operations as a CLI over the driver directly |
| `rgw-tests` | `qa/`, `s3-tests` | see Results | the AWS SDK and signed admin requests against an in-process daemon |

Ninety-three unit and handler tests passed before any end-to-end run. On
the first assembled run, without any fix, the local `aws` CLI completed
`s3 mb`, a 1 MiB `s3 cp` with metadata, `s3 ls --recursive`, `s3api
head-object`, a download with a byte-exact compare, a ranged
`get-object`, `s3 rm --recursive` and `s3 rb` against `rgwd`, with the
user created by `radosgw-admin`. Section "Results" has the client
matrix.

Two RGW design choices were deliberately not carried over, and both are
findings rather than shortcuts: the SAL handle hierarchy became one flat
trait (see "The SAL is the seam"), and the user and bucket admin
operations moved above the driver (see "The admin API lives in the RADOS
driver today").

## What the C++ tree says

### Size: what the spike covers against what RGW is

RGW is 338k lines of C++ across 610 files. The spike's scope, a basic S3
subset plus user and bucket admin, touches a small fraction of it. Lines
of C++ by area (files under `src/rgw`, counted with `wc -l`):

| Area | Lines | In the spike's scope |
|---|---|---|
| `src/rgw` root (protocol, ops, types, auth, ...) | 162k | partly |
| of which `rgw_op` + `rgw_rest_s3` + `rgw_rest` | 26k | the S3 subset |
| of which IAM / STS / roles / OIDC | 16k | no |
| of which ACL / bucket policy / CORS / ARN | 7.5k | owner-only check |
| of which multisite sync + coroutine framework | 7.6k | no |
| of which crypto / SSE / KMS | 6.6k | no |
| of which lifecycle + restore | 6.2k | no |
| of which pubsub / notifications | 6.0k | no |
| of which Swift API | 5.7k | no |
| of which auth (SigV2/V4 + engines) | 5.7k | SigV4 header auth |
| of which zone / period / realm | 5.8k | no |
| of which Lua | 2.8k | omitted by decision |
| `driver/rados` | 93k | no |
| of which multisite sync, logs, trimming, reshard | 33k | no |
| `driver/dbstore` (SQLite) | 14k | replaced by 0.7k of Rust |
| `driver/posix` | 15k | no |
| `driver/d4n` (cache) | 7k | no |
| `services/` (RADOS-side metadata services) | 11k | no |
| `radosgw-admin/` | 15k | user/bucket subset |
| `src/cls/rgw` (object classes, OSD side + client) | 12k | no |

Lua is 2.8k lines and would port through the `mlua` crate; omitting it
changed nothing structural. The areas that would dominate a full port are
multisite (about 41k lines across root and driver), IAM/STS (16k) and the
RADOS driver's index, GC and lifecycle machinery.

### The SAL is the seam, and it collapses in Rust

`rgw_sal.h` defines the storage abstraction layer as a hierarchy of handle
classes: `Driver`, `User`, `Bucket`, `Object`, `MultipartUpload`,
`Writer`, `Zone`, `ZoneGroup`, `Lifecycle`, `Notification` and more, with
404 pure-virtual methods in total (`Driver` 101, `Object` 79, `Bucket` 59,
`User` 29). Every REST op reaches storage only through it, and six
backends implement it (`rados`, `dbstore`, `posix`, `d4n`, `daos`,
`motr`). This is the cut line for extraction.

In Rust the hierarchy flattens. The handles exist to carry cached state
(`RGWBucketInfo`, attrs, the object state) down a synchronous C++ call
chain, and to let a driver subclass each one. A Rust request holds those
values directly and a driver implements one object-safe async trait over
them. The spike's `rgw_sal::Driver` has 17 methods for the subset the
suite exercises; the equivalent C++ surface across `Driver`, `User`,
`Bucket` and `Object` is roughly 60 methods. The conformance suite in
`rgw-sal/src/testsuite.rs` is the executable form of the trait's error
contracts, which `rgw_sal.h` leaves to comments and to the RADOS
implementation's behaviour.

Two contract questions surfaced by the SQLite implementor are worth
carrying into a real design: bucket listing across tenants needs a
`(tenant, name)` marker, not `name` alone; and `ObjectKey::instance`
(versioning) has to be part of object identity from the start or every
driver silently drops it.

### The admin API lives in the RADOS driver today

`/admin/user`, `/admin/bucket` and `/admin/metadata` are registered from
`driver/rados/rgw_sal_rados.cc` (`register_admin_apis`), and their
handlers live in `driver/rados/rgw_rest_user.cc` and `rgw_rest_bucket.cc`.
The core only registers `/admin/info`, `/admin/usage`, `/admin/account`
and `/admin/restore` (`rgw_appmain.cc:462`). A dbstore or POSIX gateway
therefore has no REST user administration at all. The spike lifts the
user and bucket operations into `rgw-admin-ops`, written purely against
the trait, and both the REST layer and the CLI call it; this is a design
change relative to RGW, not just a port, and it is the one the standalone
C++ target would also need.

### Rust is already in the tree, synchronously

`src/rgw/Cargo.toml` is a three-crate workspace (`lancedb-c`,
`lancedb-rgw-store`, `rgw-lancedb`) added for S3 Vectors. CMake builds it
as an `ExternalProject` into `librgw_lancedb.a` and links it into
`rgw_common`. The direction is Rust calling C++: Rust imports a C shim over
the SAL (`rgw_sal_wrapper.cc`, 1.4k lines, exposing put/get/head/list/
delete/copy and multipart), and every call passes a null yield context, so
the C++ SAL call blocks inside the Rust `async fn`. Precedent for Rust in
RGW's build exists; precedent for async interop across the boundary does
not.

### Extraction has precedent: `WITH_RADOSGW_STANDALONE`

Commit `5f01d5c058c` (2026-03-16, the SAL maintainer) added
`WITH_RADOSGW_STANDALONE`, which builds `rgw-standalone` and
`rgw-standalone-admin` from the POSIX driver plus LMDB with no librados
(`src/rgw/CMakeLists.txt:344`, `:945`, `:1013`; option at top-level
`CMakeLists.txt:600`). It is undocumented under `doc/`. It is RADOS-free,
not ceph-free: it links `rgw_common_standalone`, which still carries
`global` and therefore `ceph-common` for config, logging, formatters and
encoding. Upstream has thus already decided that a RADOS-less RGW is a
supported shape; a standalone repository is the next step of the same
move, and the Rust question is separable from it.

### What a RADOS backend would demand

The analysis below is from reading `driver/rados` and `src/cls/rgw`; the
spike did not build any of it.

- **Two client APIs.** The driver is mostly on classic `librados::IoCtx`
  (593 `librados::` tokens in `driver/rados`, 21 files) with the newer log
  code (datalog, bilog, FIFO, restore) on `neorados` and C++20
  coroutines (96 tokens, 7 files, 135 `co_await`). Rust has `librados-sys`
  style bindings to the C API; there is no neorados equivalent, and the C
  API is what a port would target.
- **Eleven object classes.** `rgw_common` links `cls_rgw_client`,
  `cls_2pc_queue`, `cls_cmpomap`, `cls_lock`, `cls_log`, `cls_otp`,
  `cls_refcount`, `cls_rgw_gc`, `cls_timeindex`, `cls_user` and
  `cls_version` (`src/rgw/CMakeLists.txt:376`, `:451-464`). `cls_rgw`
  alone has 66 distinct client functions called from RGW, covering the
  bucket index (prepare/complete/list, `bi_*`, shard init, check and
  rebuild), OLH versioning, the bucket-index log, resharding, garbage
  collection, lifecycle, the usage log and multipart part info. The logic
  behind them executes inside the OSD; a Rust driver reproduces only the
  request and response encodings, but it must reproduce all of them for
  the operations it supports.
- **Versioned binary encoding.** `src/rgw` defines 186 `encode(bufferlist&)`
  methods and `src/cls/rgw` another 84, all on ceph's `ENCODE_START(v,
  compat)` framing. Central types and their versions: `RGWUserInfo` 23,
  `RGWBucketInfo` 24, `RGWBucketEntryPoint` 10, `RGWZoneParams` 19,
  `rgw_bucket_dir_entry` 8, `rgw_bucket_dir_header` 8, `RGWObjManifest`
  8. Nearly all also have `dump(Formatter*)` and most `decode_json`, so a
  greenfield layout could use JSON, but reading an existing cluster
  requires bit-exact decoding of every historical version each type still
  accepts. This, not librados, is the bulk of a compatible RADOS driver.
- **Layout.** Roughly twenty logical pools per zone, most of them
  namespaces in `.rgw.meta` (`users.uid`, `users.keys`, `users.email`,
  `root`, ...) and `.rgw.log` (`gc`, `lc`, `usage`, `reshard`, ...);
  bucket instances at `.bucket.meta.<tenant>:<name>:<id>`, index shards at
  `.dir.<id>[.<gen>].<shard>`, heads at `<marker>_<oid>` with a `_` escape,
  tails at `<marker>__shadow_<prefix><n>` and `__multipart_`. A compatible
  driver reproduces the naming; a greenfield one does not.

### Concurrency: `optional_yield` is everywhere, coroutines are multisite

The request path runs on stackful Boost.Asio coroutines threaded through
`optional_yield` (5,291 uses in 289 files). That is the C++ spelling of
"this function may suspend", and it maps one-to-one onto `async fn`. The
separate `RGWCoroutine` framework (5.9k lines across `rgw_coroutine.*`,
`rgw_cr_rados.*`, `rgw_cr_rest.*`; 174 subclasses) is used by multisite
sync, log trimming, period push and the metadata log, and by nothing on
the S3 op path, reshard, GC or lifecycle. Leaving multisite in C++
therefore also leaves the only part of RGW whose async model does not map
directly onto tokio.

### Ceph core coupling: what a standalone repository replaces

`src/rgw` includes 89 distinct headers from `common/` and 34 from
`include/`, uses `g_conf()` 177 times and `cct->_conf->` 691 times, and
`rgw.yaml.in` defines 459 `rgw_*` options. The facilities, with the
replacements the spike used or would use:

| Ceph facility | Uses | Replacement |
|---|---|---|
| `common/dout.h` logging with subsystem levels | 71 includes | `tracing` with per-target filters |
| `common/ceph_json.h`, `common/Formatter.h` (JSON/XML dump) | 103 includes | `serde` + `serde_json` + `quick-xml` |
| `include/encoding.h`, `include/buffer.h` (bufferlist, encode/decode) | 44 includes | not needed off RADOS; a `ceph-encoding` crate for a compatible RADOS driver |
| `CephContext`, `md_config_t`, `rgw.yaml.in` | 869 config reads | `clap` + a config file; the 459 options need triage, most are RADOS or multisite tuning |
| `common/async/*` (yield context, completions) | 84 includes | tokio |
| `common/ceph_crypto.h` (OpenSSL wrappers) | 13 includes | RustCrypto (`sha2`, `hmac`, `md-5`) |
| `global/global_init.h` (daemonization, signals) | 13 includes | tokio signal handling |
| `common/perf_counters.h` | 2 includes | `metrics` or Prometheus exporter |

None of these is a blocker. Config is the largest single item because it
is also where RGW's operational surface lives.

## Staging

If the recommendation is pursued, this order keeps each step
independently useful:

1. **Gateway core in its own repository**, developed against the SQLite
   driver and the conformance suite: SigV4 (add presigned URLs, SigV2 for
   `s3cmd` defaults, and chunked-trailer signatures), the full S3 object
   and bucket surface, multipart, ACLs and bucket policy, the admin API.
   Every piece of this is testable without a cluster, which is the
   property that makes a rewrite reviewable.
2. **RADOS data path only**: a driver that stores object data as RADOS
   objects via librados and keeps metadata in its own layout (or SQLite,
   as a bridge). This gives a RADOS-backed gateway without touching
   `cls_rgw`, at the price of no interoperability with `radosgw`-written
   buckets.
3. **`cls_rgw` compatibility**: a `ceph-encoding` crate and the bucket
   index protocol, then GC, usage and lifecycle. Only after this can a
   Rust gateway serve buckets an existing `radosgw` created, and only
   this step is a true port of `driver/rados`.
4. **Multisite** stays in C++, or is redesigned; porting `RGWCoroutine`
   code as written is not a mechanical translation.

## Left out of the spike and why

- Lua scripting: by decision; `mlua` makes it a bounded task later.
- Presigned URLs, SigV2, virtual-host addressing: protocol breadth, not
  architecture.
- Versioning, multipart, ACLs beyond owner-only, bucket policy, CORS,
  lifecycle, notifications, IAM/STS, Swift: feature breadth.
- Any RADOS access: see above; the spike was scoped to answer whether the
  layering ports, and it could not have exercised a cluster (the only
  reachable one is production).

## Risks the spike surfaced

- **Interop is a moving target, and clients differ by transport.** The
  AWS Rust SDK sends signed `aws-chunked` uploads with checksum trailers
  for file-backed bodies even over plain HTTP; the `aws` CLI does so only
  over HTTPS and signed whole payloads here; s3cmd signs requests for
  three different regions in one session. A server that only verifies
  hashed payloads passes the CLI and fails the SDK. The auth crate had to
  implement chunk decoding on day one, and the spike has no TLS listener,
  so the CLI's chunked path is still unexercised.
- **The trait is easy to under-specify.** The RGW SAL leaves error
  semantics to the RADOS implementation. Writing the conformance suite
  before the driver exposed several ambiguities (marker semantics across
  common prefixes, ranges on empty objects, key uniqueness across users)
  that the C++ resolves by accident of implementation.
- **A Rust RGW that cannot read existing buckets is a different product.**
  Steps 1 and 2 above deliver that product. Whether it is the wanted one
  is the decision to make before step 3.

## Results

Everything below ran on this machine against `127.0.0.1`, inside the
sandbox, with no cluster.

### Tests

| Suite | Tests | Passed |
|---|---|---|
| unit and handler tests across nine crates | 93 | 93 |
| `rgw-tests/tests/s3.rs` (AWS Rust SDK against in-process `rgwd`) | 17 | 17 |
| `rgw-tests/tests/admin.rs` (aws-sigv4-signed admin requests) | 9 | 9 |
| `scripts/smoke.sh` (aws CLI, s3cmd, botocore against the binary) | 20 steps | 20 |

`cargo check --workspace --all-targets` reports no warnings.

### Client matrix

What each client sent, captured by the integration worker with a logging
proxy, and whether it worked without server changes:

| Client | Payload signing | Checksums | Listing | Worked |
|---|---|---|---|---|
| AWS Rust SDK 1.149, in-memory body | hex SHA-256 | `x-amz-checksum-crc32` header | V2 | yes |
| AWS Rust SDK, file-backed body | `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER` (signed chunks, crc32 trailer, trailer signature) | trailer | V2 | yes |
| aws CLI 2.36.0 over HTTP | hex SHA-256, `Expect: 100-continue` | `x-amz-checksum-crc64nvme` header | V2 with `encoding-type=url` | yes |
| s3cmd 2.4.0 | hex SHA-256; scope regions `us-east-1`, `US`, then `default` in one session | none | V1 with delimiter | yes |
| botocore 1.43.90 `SigV4Auth` on `/admin/user` | none by default | none | n/a | after setting `x-amz-content-sha256` to the empty-body hash; the server rejects its absence as AWS does, C++ RGW would substitute `UNSIGNED-PAYLOAD` and fail the signature instead |

Keys containing spaces, `+`, `%`, `?&=`, XML metacharacters and non-ASCII
round-tripped through the CLI and the SDK.

### What integration found

- One server bug: GET of a zero-byte object returned 500, because
  SQLite's `substr()` on an empty BLOB yields NULL. The conformance suite
  stored an empty object but never read it back; it does now. Every other
  client interaction worked on the first assembled run.
- Accepted without validation: `x-amz-checksum-*` request headers (only a
  crc32 trailer is verified), the `x-amz-trailer-signature`, headers
  outside `SignedHeaders` that frame chunked bodies, any credential-scope
  region, and the `CreateBucketConfiguration` body. No checksum is
  returned on GET, so a client's `checksum-mode` validates nothing.

### Not verified

- Multipart upload: the CLI switches to it at 8 MiB and gets
  `NotImplemented`; the smoke test uploads 1 MiB.
- The `STREAMING-UNSIGNED-PAYLOAD-TRAILER` path from a real client, since
  SDKs only use it over HTTPS and `rgwd` has no TLS; a hand-framed test
  covers the server side.
- Presigned URLs, SigV2, SigV4A, checksum algorithms other than crc32,
  virtual-host addressing, tenanted buckets.
- Anything against a Ceph cluster.

## Method

- Toolchain: system `rustc`/`cargo` 1.98.1 (Fedora), edition 2024;
  `rustfmt` and `clippy` were not installed, so the quality gate was a
  warning-free `cargo check --all-targets` plus `cargo test`.
- The build ran inside the Claude Code sandbox with a project-local
  `CARGO_HOME` and `--offline`; the dependency fetch and the crates.io
  index queries were the only unsandboxed commands. `ccache` had to be
  disabled through `.cargo/config.toml` because its cache directory is
  read-only in the sandbox.
- The contract crates (`rgw-types`, `rgw-sal` with its suite, `rgw-rest`)
  were written first; the four leaf crates were then implemented in
  parallel by separate agents against fixed signatures, and integration
  tests were written last against the assembled daemon.
