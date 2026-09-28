# rgw-rs design

Status: draft, 2026-09-27, for owner review before planning. The owner's
decisions are recorded in section 15; nothing else in it is approved, and
the adversarial review's edits are listed at the end. Companions:
rgw-go's `docs/exclusions.md`, canon for scope, coexistence obligations
and benchmark parity for both projects by owner decision of 2026-09-25;
the rados-rs RGW MVP design on branch
`design/rgw-mvp`, whose encoding-rule paragraph the owner adopted for
rgw-rs; and rgw-go's design spec, whose layer map, gates and phase order
this document mirrors wherever section 16 does not say otherwise. Claims
about C++ RGW were checked against ceph/ceph v19.2.6 and v20.2.4, about
Rook against v1.20.7 and main at dc7829268, about rados-rs against the
fork's `origin/main` at 0b5d1a2, about rgw-go at b0929ee, and about the
spike at 8621b5a. Each claim's evidence is in the appendix; anything
marked unverified there is a lead, not a fact.

## 1. Goal

rgw-rs is a reimplementation of Ceph's RADOS Gateway in Rust over
rados-rs, a pure-Rust RADOS client, with no librados, libceph-common or
foreign-function boundary anywhere in the process. Its goal is rgw-go's
goal: a drop-in replacement for C++ radosgw in a Rook cluster running
Squid or Tentacle, bit-compatible with radosgw's RADOS layout, able to
share a zone with radosgw during a rolling replacement or rollback, and
administered by the unmodified radosgw-admin.

The acceptance criterion is rgw-go's, word for word: rgw-rs, installed in
a derived Ceph image as `/usr/bin/radosgw`, passes Rook's object
integration suite, `TestCephObjectSuite`, in both of its passes, with and
without TLS, against current Rook main. The suite's shape and its
exclusions are as `docs/exclusions.md` records them, with one update that
document needs: at Rook main the suite also runs a user default-placement
test and a user default-storage-class test, and its zoned store declares
a second placement, `bar`; the `FOO` storage class predates main.
Placement targets and storage classes are therefore on the acceptance
path from phase 1.

Beyond the criterion, rgw-rs is benchmarked against radosgw and rgw-go on
the same cluster. rgw-go's question is what the cgo boundary costs. rgw-rs
asks the next one: what owning the whole client in one async runtime, the
messenger, cephx, CRUSH placement and the object-class encodings, buys or
costs against librados itself. The two projects share one layout and one
feature boundary so that the comparison isolates the language and the
client.

## 2. Objectives, in priority order

Same as rgw-go's: performance on the S3 data path, then scalability, then
a compact and maintainable code base that leans on the Rust ecosystem
wherever that does not cost the first objective.

Measurable forms: identical RADOS round-trip counts to radosgw on both
hot paths; an OS thread count under load equal to the tokio worker count
plus a small constant, independent of connection count, where rgw-go
starts from librados's own 49 to 58 threads before it issues an
operation; and payload copies between the socket and the OSD frame
counted and attributed, since a pure client can be measured where a C
boundary can only be estimated.

## 3. Decisions and reasons

- **Bit-compatible with radosgw's RADOS layout**, for rgw-go's reasons:
  the layout costs little on the hot path, a new one would confound the
  benchmark, the same layout isolates the variable under study, and
  radosgw is the correctness oracle. The unmodified radosgw-admin
  administers rgw-rs's metadata; rgw-rs ships one binary. The spike's
  `radosgw-admin` crate is dropped.
- **Ceph floor Squid, feature level Tentacle where gateway-side**, with
  the precise form in section 10.
- **rados-rs is the seam.** rgw-rs has no RADOS client abstraction of its
  own: the fork's `rados` crate is the client and its `rados-cls` crate
  holds the object-class encodings, by the owner's design for that fork.
  The consequence is a rule: a RADOS capability rgw-rs lacks is a rados-rs
  package, never a workaround in the gateway. Section 8 ends with the
  gaps this draft found, and section 14 schedules each before its first
  user.
- **Encoding primitives come from rados-rs.** `rados::denc` and its
  derive macros are the wire encoding; rgw-rs adds only RGW's stored
  types. rgw-go wrote its own `denc` because go-ceph has none; rgw-rs
  deliberately does not, which makes the framing gap in section 10 a
  rados-rs package rather than local code.
- **Own gateway core, borrowed edges.** An RGW-shaped op pipeline on
  hyper, one set of small store traits, one RADOS driver. No web
  framework between the socket and the hot path: the spike's axum
  router still needed a hand-written subresource chain inside every
  handler, which is the dispatcher, so the router only added a layer.
- **Non-RADOS storage exists only as a test double**, in memory, passing
  the same conformance suite the driver's fakes pass. The spike's SQLite
  driver bought only persistence across processes, which the dropped CLI
  was the only consumer of.
- **Exclusions** are decided by the three tests in `docs/exclusions.md`
  and recorded there; rgw-rs keeps no list of its own.
- **Upstream Ceph defects** that a gateway meets are recorded in an
  rgw-rs registry with rados-rs's entry format: stable IDs, evidence at a
  tag, the handling, the upstream status. Client- and class-level defects
  stay in rados-rs's registry; the two cross-reference each other and
  rgw-go's.

## 4. Architecture

One Cargo workspace. Dependencies point downward and the direction is
enforced by the crate graph, which is the reason the workspace has more
than one crate at all: a Rust module cannot forbid an import, a crate can.

| Crate | Provides | rgw-go counterpart |
|---|---|---|
| `rgw-meta` | RGW's stored types with their encodings over `rados::denc`: identity primitives (user, bucket, object key, pool with its name and namespace split, placement rule), user and account, bucket entry point, instance and layout, zone parameters, zonegroup, realm and period, manifest, compression info, ACL policy, cache-notify record; encode at the cluster's release, decode every version | `internal/meta`, `internal/acl` (encoding only) |
| `rgw-core` | the ops, one type per operation; the store traits they consume, split by concern, with in-memory fakes and the conformance suite; auth (SigV4 header, query and presigned, chunked and trailer readers, SigV2, anonymous), the policy language, ACL and quota evaluation; the S3, admin, IAM and STS protocol layers; the dispatch table; error documents | `internal/acl`; planned: `internal/op`, `auth`, `policy`, `s3`, `admin`, `iam` |
| `rgw-driver` | the RADOS store, implementing `rgw-core`'s traits over `rados` and `rados-cls`: the configuration bridge and release detection, zone and placement resolution, object naming, manifests and striping, atomic head writes, the index protocol, GC enqueue, the metadata cache with watch and notify, quotas, usage; the workers | `internal/cls/*` (which live in `rados-cls` here); planned: `internal/driver` |
| `rgwd` | the binary: radosgw-argv handling, the beast-spec frontend on hyper with TLS, deadlines and drain, mgr registration and perf reports, metrics, the admin socket, the ops log, build info | `cmd/rgw-go`, `internal/{cli,version}`; planned: `cephconf`, `frontend`, `metrics`, `asok`, `opslog` |
| `rgw-tests` | integration and cluster tests, the golden generator, the populator, the corpus harness and the gates; not published | `test/gate`, `hack/rooket`, `hack/goldens` (the generator; the goldens live at `<pkg>/testdata/goldens`) |

Rules inside the table:

- `rgw-meta` depends on `rados` for encoding and on nothing else of ours.
  `rgw-core` depends on `rgw-meta` and never on `rgw-driver`; the driver
  implements the core's traits, so `rgw-driver` depends on `rgw-core`.
  `rgwd` is the only crate that names both.
- The store traits are declared at their consumer, in `rgw-core`, and
  are small: one trait per concern, so an op's fake is a few dozen lines.
  The conformance suite is the executable form of each trait's error
  contract, the one piece of the spike whose idea survives intact.
- Types that carry semantics live with their semantics, in `rgw-core`;
  `rgw-meta` holds only what is stored and looked up. ACL is the one
  type split across both: its encoding is stored data, its grant
  evaluation is core.
- The op layer is protocol-neutral, so S3, the admin API, IAM and STS
  share the user, bucket and account operations, lifted above the
  driver, which is where the spike found they belong.
- S3 dispatch is a table matched on method, scope and subresource in
  radosgw's precedence, after Host, path and query are parsed once, with
  virtual-host addressing and tenants resolved before dispatch. No path
  router.
- Object identity carries the version instance from the first line of
  code, and bucket listing markers are `(tenant, name)`: the spike
  recorded both decisions and implemented neither, which is the failure
  mode to avoid.

Third-party dependencies in the shipped binary, complete: `rados` and
`rados-cls`; tokio; hyper, hyper-util, http, http-body-util and bytes;
rustls through tokio-rustls; quick-xml, serde and serde_json; the
RustCrypto hashes and MACs plus aes and cbc for SSE-C; the four
compression codecs, which `rados` already carries for msgr2; clap;
tracing and tracing-subscriber; thiserror; percent-encoding, base64,
hex, uuid and one time crate. Test-only: aws-sdk-s3, aws-sigv4, reqwest,
tempfile. Everything else is the standard library.

Gates, extending rados-rs's own (fmt, clippy `-D warnings` per feature
set, test): `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets --all-features -- -D warnings`, `cargo test --workspace
--all-targets --locked`, and `cargo deny check` for advisories and
licenses; `--all-features`, `--locked` and deny are additions. Workspace
lints deny `unwrap_used`, `expect_used` and `panic` outside tests, which
rados-rs states only in prose. The tree is LGPL-2.1-or-later, as the
spike is (decision 8), and `cargo deny`'s license policy admits the
licenses compatible with it, rados-rs's MIT among them.

## 5. Configuration and invocation

Rook launches `radosgw` with `--foreground`, `--fsid`, `--keyring`,
`--mon-host`, `--mon-initial-members`, `--id=rgw.<store>.<letter>`,
`--setuser`, `--setgroup`, six `--default-*` logging flags that send
every log to stderr and none to a file, `--host=$(POD_NAME)`,
`--rgw-frontends=beast port=8080 ...` with the TLS keys when a
certificate is set, `--rgw-mime-types-file`, `--rgw-realm`,
`--rgw-zonegroup` and `--rgw-zone`. Conditionally it adds
`--rgw-enable-apis`, the ops-log pair, `--rgw-dns-name`,
`--ms-bind-ipv4` or `--ms-bind-ipv6`, the SSE-KMS and SSE-S3 vault
flags, `--rados-replica-read-policy` and `--crush-location` in place of
`--host` for read affinity on Tentacle, through a `bash -c exec radosgw`
wrapper, and the store's `rgwCommandFlags`, appended last.
`--service-unique-id` exists only at main, gated on 19.2.4 and 20.2.1 or
later; v1.20.7 does not pass it.

Rook also writes `rgw_zone`, `rgw_zonegroup`, `rgw_run_sync_thread`,
`rgw_log_nonexistent_bucket`, `rgw_log_object_name_utc` and
`rgw_enable_usage_log` into the mon config store for the daemon's own
section through `config assimilate-conf`: those six keys and, when the
store spec asks, keystone, swift and any `rgwConfig` key; with wire
encryption on, it sets the `ms_*_mode` options to secure globally. The
daemon reads every key the store delivers for its section, not a fixed
list. `rgw_run_sync_thread` is written `true` unless the store disables
multisite sync traffic, so the gateway honors the option as a no-op on a
single-zone zonegroup rather than relying on Rook to turn it off; and
`rgw_enable_apis` reaches the daemon only when the store spec sets it,
disables S3 or sets the Swift URL prefix to `/`, so otherwise the
default list, swift included, is radosgw's own. `docs/exclusions.md`
records both since 4312a7a.

Rook's `ceph.conf` is the `config` key of the `rook-config-override`
ConfigMap, mounted at `/etc/ceph/ceph.conf`: usually an empty `[global]`,
and where a user's overrides arrive. The container's environment carries
`POD_NAME`, `POD_NAMESPACE`, `NODE_NAME`, `CONTAINER_IMAGE`, the resource
limits, `ROOK_CEPH_MON_HOST`, `ROOK_CEPH_MON_INITIAL_MEMBERS`,
`ROOK_MSGR2`, `CURL_CA_BUNDLE` when a CA bundle is referenced, and
`CEPH_USE_RANDOM_NONCE=true`, which in Ceph changes only a daemon's
messenger, a client's nonce being random already; rados-rs chooses its
client nonce differently from librados, and one cluster test covers it.

rgw-rs therefore accepts radosgw's argv, as rgw-go does. rgw-go parses
the early arguments and `CEPH_ARGS` itself and gets the rest from
librados: the generic `--<option>` forms, every `rgw_*` option, and the
mon config store applied during connect; rgw-rs must do all of it, and
that is the largest single difference in cost between the two projects.
Under Rook the minimum is:

- The generic `--<option>` and `--<option>=<value>` argv forms, with `-`
  and `_` equivalent, which every flag Rook passes is; among them `--id`,
  `--keyring`, `--mon-host`, `--mon-initial-members`, `--setuser` and
  `--setgroup`.
- The mounted `ceph.conf`, with the entity's sections (own name, then
  type, then global), which rados-rs already reads.
- The mon config store for the daemon's section. rados-rs subscribes to
  `config` and decodes the `MConfig` message but keeps only four of its
  own client options and discards the rest; there is no way to read a
  value and no wait for the first message, where radosgw exits if it
  cannot fetch its config. Exposing the resolved values and the initial
  wait is gap 1, a rados-rs package that lands before phase 1's first
  `rgwd` package (section 14). The mon sends values already resolved for
  the entity's section chain, so nothing beyond that is needed to see
  what Rook set.
- Ceph's documented precedence, last wins: compiled default, mon config
  store, local config file, environment, command line; a mon value never
  replaces one set locally.

`CEPH_ARGS`, `CEPH_CONF` and `CEPH_KEYRING`, which Ceph reads from the
environment and rados-rs does not, are not set by Rook: they are parity
items, not requirements.

rgw-rs carries the defaults of the `rgw_*` options it honors, copied from
the floor release's option table with their source recorded, and
radosgw's own default overrides in `rgw_main.cc`, which set the
objecter's in-flight cap to 24576 and require a secure monitor
connection; the messenger-mode package, gap 8, is what lets the client
honor the latter. An option it does not honor, the mime-types file among
them, is logged once and ignored. Configuration load refuses a GC,
lifecycle or usage shard count below 1 (`rgw_gc_max_objs`,
`rgw_lc_max_objs`, `rgw_usage_max_shards`) rather than faulting on first
use (rados-rs registry, CEPH-BUG-019). radosgw connects to the monitors
and RADOS as the launching user, binds its frontends, and only then drops
to `--setuser`/`--setgroup` inside the beast frontend's init, so
privileged ports still bind; rgw-rs keeps that order. JSON logging goes
to stderr through `tracing`.

The frontend parses beast's keys, pinned here rather than deferred to
rgw-go: `port`, `endpoint`, `ssl_port`, `ssl_endpoint`,
`ssl_certificate`, `ssl_private_key`, `ssl_options`, `ssl_ciphers`,
`ssl_ciphersuites`, `ssl_reload`, `prefix`, `tcp_nodelay`,
`request_timeout_ms`, `max_connection_backlog` and `max_header_size`,
with `config://` values for the TLS keys; `ssl_reload` is Squid's alone
and Tentacle adds `so_reuseport`. Rook mounts its certificate Secret's
`cert` key, the key, certificate and CA concatenated, and passes
`ssl_certificate` with no `ssl_private_key` unless the Secret is
TLS-typed, plus optional `ssl_options`, `ssl_ciphers`,
`ssl_ciphersuites` and `tls_groups`, the last of which neither release's
beast reads and rgw-rs treats as radosgw does. Phase 1 therefore loads a
combined PEM from `ssl_certificate` alone.

Rook's probes shape the first request the gateway ever answers: there is
no liveness probe, and the startup and readiness probes are exec probes
that curl `/` on the frontend port, or `/swift/info` when S3 is off, and
pass on any status from 200 to 399, on 503, and for readiness on 500.
rgw-rs answers an anonymous `GET /` as radosgw does: a 200 whose body is
radosgw's empty `ListAllMyBucketsResult` and carries nothing internal.
The operator marks the store ready at the end of its reconcile without an
HTTP check of its own; the suite's canary is a CephObjectStoreUser
becoming ready, which exercises the admin API through the operator's
`rgw-admin-ops-user`.

Metrics reach Rook through ceph-exporter, which reads the admin sockets
in `/run/ceph`, mounted into the RGW pod from the host; Rook defines no
RGW ServiceMonitor. rgw-rs answers on an admin socket at
`/run/ceph/ceph-client.rgw.<store>.<letter>.<pid>.asok` with
`counter dump` and `counter schema`, and `perf dump` for the toolbox, in
phase 3, where rgw-go places its admin-socket counters. The mgr's service
map and perf reports are gap 7, in phase 1.

## 6. Data path

One tokio task per request; no request owns a thread. The frontend
stamps radosgw's transaction-id format; the dispatcher selects the op;
auth resolves the credential through the metadata cache and, for
chunked uploads, wraps the body in a reader that verifies each chunk
signature and the trailer signature as bytes stream, never buffering the
body to verify it; the op runs radosgw's lifecycle in order.

The round trips on the latency path are radosgw's and therefore
rgw-go's, table for table, with radosgw's sizes: a 4 MiB head and 4 MiB
stripes, a put window of 16 MiB rising to 64 MiB, a get window of 16 MiB
in 4 MiB requests. Reading the C++ paths for this draft adds three
class calls rgw-go's table does not show, all on the same round trip:

- PUT: the guarded index prepare, then the head write, which also carries
  a compare on the id tag, the class call that stores the PG version
  attribute and, on overwrite, the class's own head removal; then the
  guarded index complete, issued through the completion manager and not
  awaited. On the busy-resharding code the driver re-reads the layout
  and retries.
- GET: one read op composing the version class's read through the
  version tracker, xattrs, stat and the first chunk; later head reads
  compare the id tag; tail reads are plain reads.
- List: the bucket-list call per shard, then suggest-changes for stale
  entries, fire-and-forget, guarded on Tentacle and unguarded on Squid as
  `docs/exclusions.md` records.
- Delete: prepare for delete, the class's head removal rather than a
  plain remove, then complete; tails go to GC synchronously.
- Copy: a refcount get on every tail under the new tag with the
  NUL-terminated reference radosgw writes, and only the head rewritten;
  data streams only where placement, storage class, encryption or head
  geometry force it.

CompleteMultipart takes radosgw's `RGWCompleteMultipart` lock on the
multipart meta object in the data-extra pool for `rgw_mp_lock_max_time`,
ten minutes. Listing performs radosgw's pending-entry reconciliation. A
failed head write cancels the prepare; a guard failure retries the
sequence as radosgw's atomic write does.

## 7. Concurrency

- **No completion modes.** rgw-go builds three ways to wake a goroutine
  from a librados completion and benchmarks them. rados-rs completes an
  operation by resolving its future from the messenger's reader task;
  there is one mechanism and nothing to select. rgw-go's seam
  microbenchmark has no counterpart here; its question is rados-rs's own
  A/B benchmarks against librados, which the fork carries.
- **Buffers are owned, not pinned.** Reads return client-owned `Bytes`
  and every write step takes `Bytes`, so a body chunk reaches the OSD
  frame by reference counting rather than by a copy across a boundary.
  rgw-go pays librados's copy in each direction plus pinning; rgw-rs pays
  whatever the messenger's framing demands, which the benchmark measures.
- **Submission backpressure is a future, not a blocked thread.**
  rados-rs bounds in-flight operations and bytes at the client with a
  tokio semaphore, defaulting to the objecter's compiled 1024 operations
  and 100 MiB, where radosgw overrides the operation cap to 24576, which
  rgw-rs sets for parity; a submit awaits its permit. rgw-rs keeps
  `rgw_max_concurrent_requests`, 1024, as a hard cap on requests and
  bounds body memory with a byte budget, as rgw-go does. An operation
  larger than the byte budget is never admitted today (tokio's
  `acquire_many` past the semaphore's total permits never completes),
  where the C++ throttle admits it once nothing else is in flight. That
  is a rados-rs defect, fixed in gap 6's package, which clamps or admits
  it. Four-mebibyte chunks never approach it.
- **Cancellation is drop, and drop does not cancel.** A client
  disconnect drops the request's future and every op future it owns.
  rados-rs releases the throttle permit at once but keeps the operation
  tracked until its reply or timeout, and sends nothing to the OSD, so a
  write already sent may still apply, as with librados, and a dropped
  operation no longer counts against the throttle. The driver orders
  each multi-op sequence so a cancelled prefix leaves the layout in a
  state radosgw's own recovery repairs: a pending prepare is reconciled
  at the next listing, an orphaned tail by GC.
- **Metadata cache.** Decoded users, bucket entry points, bucket
  instances with attributes and zone configuration, radosgw's 25000
  entries with its 900 second expiry, watching the control pool's notify
  objects and sending the same notify, with its full record, on every
  metadata write. The cache handles an UPDATE_OBJ notify by invalidating
  the named entry and re-reading it, never by applying the payload it
  carries, a deliberate difference from radosgw (section 16). A watch
  that reports itself broken is delivered as an event on the watcher,
  which rgw-go's seam forwards from go-ceph's own channel; rados-rs
  reconnects it on session resets, map changes and every five seconds
  while in error, and after a not-connected error the driver watches
  again.
- **Workers**, each a tokio task under one cancellation token, with
  rgw-go's per-phase set: phase 1 has the GC processor with radosgw's
  locks, the usage-log flush, the index-completion retry worker, watch
  re-registration and quota statistics; phase 2 adds lifecycle; phase 3
  adds the notification deliverer and the persistent-queue expiry pass.

## 8. The RADOS driver's layer map

Everything below is a constraint proven by the oracle, not a string
format to be typed in: the gate in section 12 reads what a populated
radosgw wrote and the driver must produce the same names and bytes.

**Pools and namespaces.** The zone parameters name every pool. The
defaults a zone `<z>` gets are the root pool `.rgw.root`; the meta pool
`<z>.rgw.meta` with namespaces `root` (the domain root), `users.uid`,
`users.keys`, `users.email`, `users.swift`, `roles`, `oidc`, `topics`,
`accounts` and `groups`; the log pool `<z>.rgw.log` bare and with
namespaces `gc`, `lc`, `intent`, `usage`, `reshard` and `notif`; the
control pool `<z>.rgw.control`; a separate OTP pool `<z>.rgw.otp`; and per
placement `<z>.rgw.buckets.index`, `.data` and `.non-ec` for data-extra.
Tentacle adds a dedup pool and `restore` and `logging` log namespaces,
which is the zone's version bump. A pool name carries its namespace after
a colon with a backslash escape, and Rook's zoned store maps every zone
pool field to `<pool>:<store>.<suffix>` with a suffix table of its own
that differs from Ceph's defaults (`account` for `accounts`,
`bucket-logging` for `logging`), so the driver takes every pool and
namespace from the zone record and never from the defaults. Every object
is placed by pool, namespace and locator key; in rados-rs, as in
librados, namespace and locator belong to the I/O context, not to the
operation, and a context clone costs an `Arc` and two strings, so the
driver clones per namespace and, for the objects that need one, per
locator, until the gap package adds a per-operation form.

**Object naming.** The bucket instance is `.bucket.meta.<bucket>:<id>`,
with the tenant and a colon before the bucket name when there is one, in
the domain root; the entry point is `<bucket>`, or `<tenant>/<bucket>`,
beside it. Index shards are `.dir.<id>.<shard>`, `.dir.<id>.<gen>.<shard>`
once a generation is above zero, and `.dir.<id>` unsharded, keyed on the
bucket id, in the placement's index pool. A head is `<marker>_<oid>` in
the data pool. With an empty namespace and no instance encoded, the oid
is the name, or `_` plus the name when the name starts with `_`, and
then the locator is `<marker>_<name>`; otherwise the oid is
`_<ns>[:<instance>]_<name>`. Tails are `<marker>__shadow_.<32 random
characters>_<n>` for a plain object and `<marker>__multipart_<name>.<upload
id>.<n>` for a part's first stripe, with `.<n>_<m>` shadow stripes after
it; upload ids start with `2~`. The multipart meta object is
`<name>.<upload id>.meta` in the multipart namespace of the data-extra
pool. Users are `<uid>` or `<tenant>$<uid>` in `users.uid` with a user-id
record followed by the user info as data, `<uid>.buckets` beside them for
the user class, the access key as the object name in `users.keys`, the
lowercased email in `users.email`, and the Swift key in `users.swift`,
each index holding a bare user-id record. The root pool holds
`zone_info.<id>`, `zone_names.<name>`, `zonegroup_info.<id>`,
`zonegroups_names.<name>`, `realms.<id>`, `realms_names.<name>`,
`realms.<id>.control`, `periods.<id>.<epoch>`, `periods.<id>.latest_epoch`,
`period_config.<realm>`, `default.zone.<realm>`, `default.zonegroup.<realm>`
and `default.realm`. GC shards are `gc.<n>`, lifecycle shards `lc.<n>`,
usage shards `usage.<n>`, reshard logs `reshard.<n>` with `<n>`
zero-padded to ten digits, `reshard.0000000000` to `reshard.0000000015`
by default.

**Metadata types and their versions.** The gateway itself reads and
writes these, so the encoding rule applies in full:

| Type | Squid v19.2.6 | Tentacle v20.2.4 | Where it lives |
|---|---|---|---|
| RGWUserInfo | 23 | 23 | user object data in `users.uid` |
| RGWBucketEntryPoint | 10 | 10 | entry point in the domain root |
| RGWBucketInfo | 24 | 24 | bucket instance in the domain root |
| RGWZoneParams | 15 | 18 | `zone_info.<id>` |
| RGWZoneGroup | 6 | 6 | `zonegroup_info.<id>` |
| RGWZoneGroupPlacementTier | 1 | 4 | inside the zonegroup |
| RGWZoneGroupPlacementTarget | 3 | 3 | inside the zonegroup; inserts STANDARD on decode |
| RGWRealm, RGWPeriod | 1 | 1 | `realms.<id>`, `periods.<id>.<epoch>` |
| RGWObjManifest | 8 | 8 | head xattr |
| RGWCacheNotifyInfo | 2 | 2 | the notify payload |
| rgw_bucket_dir_entry | 8 | 8 | index omap value, written by the class |
| rgw_bucket_dir_header | 7 | 8 | index omap header, written by the class |

The last two are class-written and belong to rados-cls; they are in the
table because a Tentacle OSD re-encodes the header at 8 while Squid
corpus objects are 7, and the driver's listing sees both on one cluster.
Three Squid types declare a decode maximum one below their own encoder:
RGWObjManifest encodes at 8 and declares 7, rgw_bucket_dir_header 7 and
6, and the index entry's metadata record 7 and 6; section 10 gives the
consequence. RGWCacheNotifyInfo, like the central types, uses the legacy
compat-length framing.

**Class calls by path.** The encodings are rados-cls's; the driver
decides which call goes on which path, as section 6 lists for the data
path. Beyond it: user statistics and quota through the user class,
keeping radosgw's semantics so `radosgw-admin user stats` agrees; GC per
shard, where the version class decides omap-era or queue-era and the
shard count is the smaller of `rgw_gc_max_objs`, default 32, and the
shard prime 65521; lifecycle shards capped at 7877; usage with 32 shards
and one per-user shard; the locks `gc_process`, `lc_process` and
`reshard_process` by name. Modifying methods that return data run with
the return-vector flag on the same operation, whose reply the OSD caps
at 64 bytes per op (`osd_max_write_op_reply_len`), failing with
EOVERFLOW beyond it; rados-cls already does this for the user-stats
reset and the 2pc queue reserve, so persistent notifications are not
gated on a client item here.

**Watch and notify.** The control pool holds `notify.<i>` for `i` below
`rgw_num_control_oids`, default 8; a metadata object's notify goes to the
one selected by hashing its pool, namespace and name; the payload is the
cache-notify record. rgw-rs watches all eight and notifies on every
metadata write it makes, so radosgw and radosgw-admin see its writes and
it sees theirs. Without this a shared zone is unsafe; with the cache
disabled it is merely slow.

**Gaps in rados-rs this map depends on**, each a rados-rs package landed
before the gateway package that first needs it; section 14 schedules
them:

1. The mon config store's values exposed and awaited (section 5).
2. An explicit modification time on a built operation, and a write marker
   on rados-cls's modifying calls: flags come from opcodes and CALL is a
   read opcode, so a class call sent alone goes out READ-flagged with
   mtime zero and the OSD leaves the object's mtime unchanged;
   `OpBuilder::flags(WRITE)` already forces the flag, but no rados-cls
   helper sets it and no API takes the mtime radosgw passes on copies and
   OTP writes. The OTP module's own docs already say a driver must.
3. In-flight operations must be resent under their original transaction
   id after a connection loss, as the C++ objecter does, so the OSD's
   duplicate detection applies; a rados-rs fix is in flight
   ([jhoblitt/rados-rs#28](https://github.com/jhoblitt/rados-rs/pull/28),
   in review), and rgw-rs pins it with a test rather than planning it.
4. A per-operation namespace and locator on the I/O context's operations,
   or the clone pattern documented as the contract. The raw form exists:
   the client's submission by object id is public, and the id carries
   namespace and key. What is missing is the I/O-context form that
   rados-cls's helpers, which take a context, can use.
5. In `denc`: a decode mode that separates the compat guard from the
   accepted struct version, accepting a struct version above the type's
   own and skipping the tail as the C++ decoders do (needed by package 8
   for Squid's own manifests); and the legacy compat-length framing,
   needed by the rule only (section 10).
6. Small items: a truncate step constructor; builder steps for xattr get,
   set, remove and list and for a class call, which today go through raw
   op constructors; a `stat2` step, which radosgw's head read composes; a
   class call charged to the operation budget, which today charges it
   nothing; a bounded watch-event channel; and admission of an operation
   larger than the byte budget, a defect today (section 7).
7. A mgr client: `MMgrOpen`/`MMgrReport` with service-daemon registration
   and status, so rgw-rs appears in the service map with radosgw's
   metadata and reports its perf counters. Without it `ceph status`, the
   dashboard's RGW discovery and the prometheus module's `ceph_rgw_*`
   metrics see no rgw, and rgw-go's harness, which finds the daemon
   through `ceph service dump`, fails. rgw-rs's harness is rooket plus
   Rust and does not depend on `ceph service dump`.
8. The messenger modes, `ms_client_mode` and `ms_mon_client_mode`,
   honored by the client, with radosgw's defaults, which require a secure
   monitor connection. In flight in rados-rs, and a dependency of the
   driver's connect.

## 9. Coexistence with C++ radosgw on the same cluster

The coexistence obligations in `docs/exclusions.md` are requirements of
this design without restatement: cache invalidation both ways, the GC
shard formats, queue initialization and reservation accounting, the
return-vector rule, OLH epochs by OSD release, all four compression
codecs on read, exact user statistics, radosgw's lock names, radosgw's
signature quirks including the unsigned-payload substitution for a
missing `x-amz-content-sha256`, which Rook's own admin client depends
on (it signs `UNSIGNED-PAYLOAD` and never sends the header), opaque
sub-records in phase 0, and request shapes keyed on the cluster's
release. Two facts sharpen them here:

- **Who writes the zone metadata under Rook.** Rook writes none itself.
  It runs `radosgw-admin`, and which binary that is depends on the
  cluster's networking: without Multus the command runs locally in the
  operator pod, so the realm, zonegroup, zone and periods are written by
  the operator image's Ceph, 20.2.4 in Rook v1.20.7; with Multus it is
  proxied into the mgr's command-proxy container, which runs the
  cluster's own image. On a Squid cluster without Multus `zone_info` is
  therefore Tentacle-encoded, RGWZoneParams at 18 where Squid writes 15,
  and the zonegroup's placement tiers, which Rook's stores do not define,
  would be at 4 where Squid writes 1. The
  decode rule covers it; a gateway that rewrites either object writes
  the cluster release's encoding, as the cluster's own radosgw would,
  and the gate holds it to that.
- **radosgw-admin must be told the site.** Rook names the realm,
  zonegroup and zone after the store, and a radosgw-admin invoked
  without those flags creates and works in a global `default` zone the
  gateway never serves. Every invocation in rgw-rs's tests derives the
  names from the running cluster and passes all three.

Upstream defects the gateway must handle, from the two existing
registries, all verified there at v19.2.6 or later: the packed-value
encoding of exactly 65536 as 0, reproduced for byte identity; the usage
trim that never finishes on payer-keyed or bucket-filtered records,
so every trim loop is bounded; the 2pc queue's inexact reserved size and
its reservation id 0; the queue-registry listing that a radosgw before
20.2.3, every Squid release included, never pages past 1024 entries; the
stale epoch a cancelled index
completion writes; the delete-marker refusal on OSDs before 19.2.3;
the version class returning ECANCELED where its header says EAGAIN; the
lock class's EIO on an expired ephemeral lock; the OTP class's
unvalidated step size and replay index; and radosgw's insertion of
STANDARD into an empty placement target on decode, which makes a
re-encode longer than the stored bytes and which rgw-rs does not
reproduce, matching ceph-dencoder rather than radosgw.

## 10. Encoding and releases

The rule, adopted from rgw-go and rados-rs: driver-level persistent
types, the ones the gateway reads and writes as object data, xattrs or
omap values, decode every struct version the C++ decoders accept, since
never-rewritten metadata sits at its original encoding, and encode at
the version the cluster's own radosgw release writes, detected once from
the OSD map's required OSD release and overridable by flag. Class
requests encode at that release's shape; class replies decode from the
Squid version up to what main writes. rados-cls already keys the three
request shapes that changed, update-stats at Tentacle and the OLH-log
read and entry metadata at Umbrella, on the release the `rados` crate
exposes, and radosgw does not key on the release at all, which makes
rgw-rs stricter during a rolling upgrade by design.

Consequences for `rgw-meta`, each checked against what rados-rs offers:

- The `rados` crate's versioned framing is the standard one: a version
  byte, a compat byte and a length. The legacy compat-length framing,
  whose pre-length versions carry no compat or length byte, is not
  implemented. No object in the corpus at any archive needs it (every
  RGW type's oldest corpus version is at or above its length version),
  so it is required by the encoding rule for metadata written before
  those versions, proven by generated vectors, and never by the gate; it
  is the second half of gap 5.
- The derive handles only a single fixed version; a type with
  version-gated fields implements the trait by hand, as rados-cls does
  for some thirty of its own. RGWUserInfo, RGWBucketInfo, the zone types
  and the manifest are all hand-written decoders with derived or
  hand-written encoders, and the goldens prove each.
- The crate's versioned decode refuses a struct version above the type's
  declared maximum, where C++ refuses only a compat above its own and
  skips the unknown tail. One maximum guards both checks, so leaving it
  open drops the compat guard as well. The rule to decode every version
  the C++ accepts, and to survive the next release's bump, means
  `rgw-meta` needs a mode that keeps the compat guard, accepts a newer
  struct version and skips trailing fields; the first half of gap 5 adds
  it. The mode is `rgw-meta`'s only: rados-cls's decoders stay closed by
  the fork's rule, since class replies decode at the OSD's version.
  Squid itself is the proof: RGWObjManifest, rgw_bucket_dir_header and
  rgw_bucket_dir_entry_meta encode one version above the number in their
  own decode macro, so a decoder that treats that number as a maximum
  rejects a Squid cluster's bytes.
- The required release is a lossless byte in rados-rs, with the named
  constants Squid, Tentacle and Umbrella and any newer byte kept as is;
  `rgw-meta`'s encoders map a byte newer than they know to the newest
  release they know, with a warning, never failing closed.
- A website configuration, object-lock configuration and sync policy
  inside a bucket instance are carried as the encoded bytes radosgw
  wrote until the phase that serves them models them, as rgw-go does.
- Duplicate keys in a decoded map are a parity note, not a behaviour:
  Ceph's own encoders never repeat a key, and the settled semantics,
  last wins for struct-keyed maps and first wins for denc-traits maps and
  every unordered map, are documented on the decoders that could meet
  them.
- The corpus carries RGWUserInfo, RGWBucketInfo, the manifest, the index
  entry and the ACL policy in eight archives from 0.61.4 through 19.2,
  and the zone parameters and the entry point in five from 15.0.0.
  Tentacle encodings have no corpus and rely on generated vectors.

## 11. Error handling

- rados-rs distinguishes an OSD's errno, carried as a return code with
  its message, from local failures: connection, timeout, decode,
  encoding, cancellation, blocklisting. The distinction rgw-go had to
  retrofit into its seam exists at the source here. The driver folds
  the client's convenience variants for a missing object or pool into the
  same not-found the errno form produces, so no path depends on which
  the client chose. Class-specific codes arrive as per-operation return
  codes, not errnos; the driver names them, starting with the guard's
  busy-resharding value, minus 2300.
- The driver maps errno to RGW semantics exactly where the C++ does:
  ENOENT, EEXIST, ECANCELED from a failed guard, EBUSY from a lock, ENOSPC
  from a full queue to SlowDown.
- One gateway error type carries the S3 code and HTTP status, the shape
  the spike got right, plus its source for logging. What reaches a client
  is radosgw's error document; an internal error's detail reaches only
  the log and the request id joins the two.
- Malformed bytes from RADOS are an internal error with the object named
  in the log, never a panic; malformed input from a client is the 4xx
  radosgw returns. The workspace lints make a panic in production code a
  build failure.
- A watch's failure is an event on the watcher, never an error on an
  unrelated operation; a notify that times out reports which watchers did
  not ack and is not an error.

## 12. Testing

| Layer | Needs | Covers | Runs |
|---|---|---|---|
| Unit tests | nothing | `rgw-meta` encoders, `rgw-core` ops against fakes with the conformance suite, auth, policy, ACL, dispatch, the frontend in-process through hyper | `cargo test`, every PR |
| Corpus and vector goldens | committed files only | every encoding in scope: each corpus object decodes to ceph-dencoder's JSON and re-encodes to its bytes at the Squid release; generated vectors for Tentacle encodings and for types the corpus lacks; unordered types compare after decoding | `cargo test`, every PR |
| Cluster tests, `#[ignore]` | a rooket cluster | the driver's layout and protocols against a radosgw oracle, the live coexistence matrix, the admin API through Rook's own client, ceph/s3-tests comparative | on demand and at gates |
| Rook end to end | a bare kind cluster, Rook main, the derived image | `TestCephObjectSuite`, both passes | nightly and at gates |

Details and decisions:

- **Goldens.** rgw-go's goldens are language-neutral: the corpus object,
  ceph-dencoder's JSON dump and ceph-dencoder's re-encoding at v19.2.6,
  one triple per object, keyed by C++ type name, 98 types, 3376 triples,
  54 MB across seven packages, each at
  `<pkg>/testdata/goldens/<Type>/<archive>/<object>.{bin,json,reenc}`.
  rgw-rs uses that format and layout, per crate. The goldens are
  generated by the `rgw-tests` binary: for each corpus object it runs the
  toolbox's `ceph-dencoder` through `rooket kubectl exec` to produce the
  JSON dump and the re-encoding at the cluster's release, and commits the
  triple. Regeneration needs a rooket cluster per release; `cargo test`
  needs only the committed files. The corpus is read at the commit Ceph
  v19.2.6 pins. rados-rs's own corpus harness compares against a live
  `ceph-dencoder` from apt in CI, with a closed type registry inside a
  non-published binary; rgw-rs can register nothing there, and decision
  4 keeps `cargo test` offline instead.
- **The cluster.** rooket, as rgw-go and rados-rs already use it: released
  Rook v1.20.7 with one worker, the `host-network` profile from the first
  `up`, `cephImage.tag` pinned per release, the dashboard off and rooket's
  `rgw` profile unused so that every user and bucket count is exact,
  `rooket up --wait`, and `rooket ceph-config --out` for the `ceph.conf`
  the tests read through `CEPH_CONF`, which is how rados-rs's cluster
  suites run today. The ambient cluster is never used. No make, bash or
  podman plumbing is added: the pins live in rooket configuration
  directories under `rgw-tests`, and the population and gate logic is
  Rust (decision 4).
- **The phase 0 gate**, rgw-go's concept, not its code: populate the
  zone through the toolbox's radosgw-admin and an S3 client, then read
  the objects the layout names in each known namespace, and list the
  root, index and data pools; decode each with nothing left over,
  re-encode it for the cluster's release, and require the bytes radosgw
  wrote, or, for objects the operator's radosgw-admin wrote at a newer
  release, the bytes the toolbox's ceph-dencoder writes back; compare the
  decoded value, type-aware, with radosgw-admin's JSON; and require
  counts equal to the manifest's so that no pass is vacuous. rgw-go's
  registry records its placement-target quirk as found by this gate.
- **The Rook suite.** Its installer selects the Ceph image from a fixed
  table keyed by `CEPH_SUITE_VERSION`, `quay.io/ceph/ceph:v19`, `v20` and
  `v21` and the ceph-ci devel tags, and sets no pull policy, so a
  non-`latest` tag already on a node is used as is; there is no variable
  for an arbitrary image. The derived image is therefore loaded onto the
  nodes under the table's name for its release, with the Rook checkout's
  own image-import helper, as Rook's CI imports its images. The suite
  brings its own three-mon cluster through Rook's installer with the
  dashboard on, reaches the gateway from the runner through a Service of
  its own, administers it with go-ceph's admin client as the
  `dashboard-admin` system user, which exists because the dashboard is
  on, and skips TLS verification. It runs every `radosgw-admin` in the
  toolbox, so the derived image keeps the original `radosgw-admin`; its
  first admin call is `GET /admin/info`. It runs on a bare kind cluster
  from `rooket cluster create`, never on a rooket-deployed one (the chart
  operator watches every namespace), through Rook's own installer and
  image-import helper, with Rook main's operator built from the same
  checkout, all driven from an rgw-rs workflow; nothing is added in make,
  bash or podman.
- **s3-tests** is comparative, as in rgw-go: the same suite against
  radosgw and rgw-rs on the same cluster with the same configuration, and
  the gate is an equal pass-and-fail set for the phase's groups.

## 13. Benchmarking

rgw-go's three questions become two. The cgo question is rados-rs's
question, answered by its A/B benchmarks against librados and reported
there. The pipeline baseline, a no-op handler and a one-round-trip
handler under the real load, answers how much of a request is ours. The
gateway comparison, radosgw, rgw-go and rgw-rs one at a time on the same
rooket cluster through their derived images under the parity settings in
`docs/exclusions.md`, answers the project's question. The load generator,
metrics recorded and the results layout are rgw-go's, so the three sets
of numbers are comparable.

## 14. Phasing

Each package is one pull request, reviewed and merged before the next
starts unless it says otherwise. Phase 0 is the plan to write next; its
first package is small enough to plan in full.

**Phase 0, foundations.** Gate: the metadata objects a populated radosgw
wrote decode and re-encode byte-identically, or, for the objects the
operator's newer radosgw-admin wrote, to the toolbox dencoder's
re-encoding, on a Squid and a Tentacle rooket cluster, with counts equal
to the populator's manifest. Phase 0 is rgw-go's with one deviation from
decision 3: package 10, the driver's connection and release detection,
is here rather than in phase 1, because the owner put the driver
connection in the first milestone (decision 5), and the gate then
exercises driver code. The gate still loads the zone in test code, as
rgw-go's does.

1. **Scaffold.** The workspace as decided in decisions 1, 2 and 8: the
   spike tagged and its crates removed from `main`, with a README line
   naming the tagged spike an insecure research prototype and
   `FINDINGS.md` moved to `docs/`; the five crates as empty shells with
   their dependency edges; `rados` and `rados-cls` as git dependencies
   pinned by revision; the license, LGPL-2.1-or-later, with `cargo
   deny`'s license policy to match; commitlint and Dependabot kept; the
   gates and workspace lints added; `docs/` with this spec, the
   exclusions pointer and the empty defect registry. No behaviour.
2. **rados-rs: `denc` open-version decode** (gap 5, first half), landed
   in the fork first because package 8's manifest decoder rejects Squid's
   own bytes without it.
3. **`rgw-tests` golden generator and rooket configuration.** The
   per-release rooket configuration directories (`config.yaml` with
   `profiles: [host-network]`, `values/rook-ceph-cluster.yaml` pinning
   `cephImage.tag` and `dashboard.enabled: false`), the binary that
   drives `rooket up --rook-version v1.20.7 --workers 1 --config-dir …
   --wait` and `rooket ceph-config --out`, and the generator that writes
   the goldens in rgw-go's layout. Each `rgw-meta` package below commits
   the goldens for its own types.
4. **`rgw-meta` primitives and the corpus harness.** The identity
   primitives and the small shared types, proven against their goldens.
5. **User and account types.**
6. **Bucket entry point, instance and layout**, with the opaque
   sub-records.
7. **Zone parameters, zonegroup, realm, period and their defaults**,
   including the tier and placement-target shapes at both releases and
   the STANDARD quirk pinned by a dencoder oracle.
8. **Object attributes**: manifest, compression info, ACL policy
   encoding, cache-notify record. OLH info arrives with versioning in
   phase 2, as in rgw-go.
9. **rados-rs gap packages, scheduled to their first user.** Gap 1 (mon
   config store, exposed and awaited) before phase 1's first `rgwd`
   package; gap 2 (explicit mtime and a write marker on rados-cls's
   modifying calls) before the index protocol; gap 4 and gap 6 before
   the driver's head writes and workers; gap 7 (the mgr client) in phase
   1, before the Rook suite first runs; gap 8 (the messenger modes), in
   flight, before package 10; gap 5's second half, the legacy framing,
   before phase 0 closes, with generated vectors for each type that
   declares it; gap 3 (same-tid resend) is being fixed in rados-rs
   separately and is pinned here by a test, not planned. rgw-go's
   retrofits — version and pool id on reads, the watch error channel,
   the local-versus-errno split and return-vector output — are already in
   the fork and are pinned by tests in package 10.
10. **`rgw-driver` connection and release.** Connect from a `ceph.conf`
    and keyring as rados-rs reads them today, detect the release, expose
    the override flag.
11. **`rgw-tests` populator and the gate.** The Rust populator writing
    rgw-go's manifest, and the gate test.
12. **Integration workflow**, nightly and on demand, one job per release,
    on rooket.

**Phase 1, data path, auth and core admin; first benchmark.** It opens
with the site and system objects, as rgw-go's phase 1 does: load realm,
zonegroup, zone and period; resolve pools and namespaces from the zone
record; read and write system objects with their versions. Gap 1 lands
before the first `rgwd` package, which brings radosgw's argv, the
`ceph.conf` sections and the mon config store (section 5); gap 7 lands
before the Rook suite first runs. The rest is rgw-go's phase 1 list, op
for op, with three additions particular to rgw-rs: the hyper frontend
with the beast spec and TLS, loading a combined PEM from
`ssl_certificate` alone, is built here rather than inherited; the
pipeline baseline replaces the seam microbenchmark; and placement targets
and storage classes are served, because the Rook suite's store declares
a second placement and a second storage class. The admin surface
includes the suite's first call, `GET /admin/info`, and every call the
operator makes: user get, create, modify and remove, user caps add and
remove, user quota, bucket info, bucket listing, and the four account
calls. Gates: s3-tests parity, Rook's admin client suite, the first
gateway comparison, and the Rook suite as soon as the derived image
serves a store.

**Phase 2, versioning, lifecycle and bucket configuration**, and
**phase 3, remaining services and workers**, are rgw-go's, with the
2pc-queue, lock and OTP work already in rados-cls rather than pending.
Phase 3 includes the admin socket with the counters ceph-exporter
scrapes, as rgw-go places them (section 5).

## 15. Owner decisions

Recorded by the owner on 2026-09-27. Each decision states the chosen
option, then the options rejected, one line each.

**1. A fresh tree in the same repository.** The scaffold package tags the
spike and removes it from `main`, with a README line naming the tagged
spike an insecure research prototype; `FINDINGS.md` moves from the
repository root to `docs/` as history. Named pieces are salvaged by
copying: the SigV4 core with its AWS vectors, the error table's shape,
the XML structs and the whitespace-preserving multi-delete parser, range
parsing, bucket-name validation, the in-process AWS SDK test harness,
and the conformance-suite idea. The cost is one large scaffold PR.
Rejected:

- Evolve in place: the spike's shapes are load-bearing in every crate
  and cost more to rework than the parts worth keeping.
- A new repository and a renamed spike: loses the name, history and
  scorecard for no gain.

**2. Five crates**, cut on what tests without a cluster and what changes
together: `rgw-meta`, `rgw-core`, `rgw-driver`, `rgwd`, `rgw-tests`
(section 4). `rgw-core` is large, and its modules carry the internal
structure. Rejected:

- The spike's eleven crates, one per C++ file family: every op touches
  four crates.
- One crate: no compile-time enforcement of dependency direction, and
  every test build compiles the driver.

**3. Mirror rgw-go's layer map, phase order and gates**, differing only
where rados-rs or Rust removes a layer or moves a cost, with the
differences listed in section 16 and kept current, plus one deviation:
the driver's connection and release detection, package 10, is in phase
0 (decision 5). Rejected:

- Mirror package for package: inherits Go-shaped decisions that do not
  apply, the seam crate and the completion modes above all.
- An independent design, such as the spike's data-path-first staging:
  gives up the byte-exact gate before the data path and weakens the
  comparison.

**4. Rook's `TestCephObjectSuite` is the shared acceptance gate**, the
S3-level gate for both projects, with rgw-rs's own phase gates beneath
it: the byte-exact metadata gate on rooket in phase 0, s3-tests parity
from phase 1. The harness is rooket plus Rust only: the per-release
rooket configuration directories live in `rgw-tests`, a small binary
drives `rooket up --wait` and `rooket ceph-config`, a Rust populator
writes metadata through the toolbox's radosgw-admin and data through the
AWS SDK, and the goldens are generated by the same binary running
ceph-dencoder through `rooket kubectl exec` into the toolbox, so no make,
bash or podman plumbing is added. rgw-go's manifest schema and golden
layout are taken verbatim, so the two gates can compare. Rejected, for
the gate:

- rgw-go's gate reused: it is Go and drives rgw-go's encoders; only its
  concept transfers.
- s3-tests parity alone: never exercises Rook's operator interactions,
  which is what "drop-in" means.

Rejected, for the harness:

- rgw-go's scripts and goldens referenced from a pinned checkout: a
  cross-repository test dependency, with scripts and a gate that assert
  rgw-go's own cluster name.
- The populator ported to Rust and the goldens copied once: two
  manifests that drift, and goldens regenerated only when rgw-go
  regenerates them.
- No committed goldens, a CI corpus job against the apt ceph-dencoder as
  rados-rs does: the apt release decides the re-encode version, and
  `cargo test` no longer proves encodings offline.

**5. The first milestone is rgw-go's phase 0 plus package 10**: stored
types, the driver's connection and release detection, and the byte-exact
gate on both releases, with package 1 as the first PR to plan in full.
No S3 request is served until phase 1. Rejected:

- A data-path hello, one PUT and GET through the index protocol: the
  encoding shortcuts taken to reach it become the foundation, the
  failure rgw-go's gate was built to catch.
- Scaffold and `rgw-meta` only, no cluster: the rados-rs gaps surface a
  milestone later.

**6. tokio and hyper 1.x, with an own dispatcher, HTTP/1.1 only and TLS
through rustls.** The runtime is tokio because rados-rs is. hyper-util's
server carries the connections; the beast-spec server, per-request
deadlines, header timeouts and drain are written by hand, about what
rgw-go writes on net/http. HTTP/2 stays off because radosgw's beast
frontend is HTTP/1.1 and every client in the gate speaks it. The rustls
crypto provider is a build-time choice recorded in the scaffold package.
Rejected:

- axum 0.8, as the spike: a tower and extractor layer on the hot path,
  and a router that cannot express subresource dispatch, so the
  dispatcher is written anyway.
- actix-web: its own runtime model beside tokio-native rados-rs.

**7. rados-rs as Cargo git dependencies**, `git =
"https://github.com/jhoblitt/rados-rs"` with a `rev = ...` on the fork's
`main`, for both `rados` and `rados-cls`, bumped by hand; Dependabot does
not follow git revisions, which is rgw-go's situation with its go-ceph
replace. The fork's `main` is what the rados-rs design says that branch
is for; the choice is revisited when the fork's packages are upstream.
Rejected:

- crates.io: the fork's publish workflow publishes only `rados` and
  `rados-denc-macros`, not `rados-cls`, and the fork's packages are not
  upstream yet.
- A git submodule with path dependencies: submodule hygiene in every
  checkout and CI step, for no benefit.

**8. The license is LGPL-2.1-or-later**, the spike's, kept for the new
tree and stated in the scaffold package. `cargo deny`'s license policy
admits the licenses compatible with it; rados-rs's MIT is one. No
alternative was recorded.

## 16. Learn from rgw-go: where rgw-rs deliberately differs

rgw-go's spec, its phase 0 plan and `docs/cgo-limitations.md` are the
evidence. Each difference names what changes and why.

- **No seam crate and no completion modes.** rgw-go's seam exists to hide
  go-ceph and to compare three wake-up mechanisms. rados-rs is native
  async; the seam is the client, and there is one completion path.
- **No `denc` of its own.** rgw-go's `internal/denc` is rgw-rs's
  `rados::denc`, which is why its two missing pieces are fork packages.
  What remains is RGW's types, which is `rgw-meta`.
- **Class clients live in rados-cls, not in the gateway.** rgw-go decided
  the opposite, keeping class packages in rgw-go because Ceph ships no
  installed client headers, and only librados bindings go to the go-ceph
  fork. The owner's rados-rs design takes them, one class per feature,
  for upstreaming.
- **Ownership instead of pinning.** go-ceph pins Go buffers for the life
  of an asynchronous operation and reaps them on cancellation, which is
  the class of silent-until-corrupting bug its seam tests run under the
  race detector to find. rados-rs owns its buffers, so there is no reaper
  and no pinned lifetime.
- **Version per operation; locator still per context.** librados reports
  the version per I/O context, so rgw-go borrows a private context per
  synchronous write; rados-rs returns it on every result. The locator is
  per context in both, and rados-rs's contexts clone cheaply where
  rgw-go had to pool them.
- **Return-vector on any operation**, so a modifying class method's
  output is not a fork item gating persistent notifications.
- **Configuration is a cost, not a gift.** librados applies the mon
  config store, the generic option forms and every `rgw_*` option during
  connect; rgw-go parses only the early arguments and `CEPH_ARGS` and
  reads the rest back. rgw-rs implements the config sources and their
  precedence itself, and the mon store first needs a fork package. This
  is the one place where rgw-rs's phase 1 is larger than rgw-go's.
- **The cephx floor moves.** rgw-go supports only a librados at 19.2.6 or
  20.2.4 and later because those parse AES256KRB5 keys. rados-rs
  implements that cipher, so rgw-rs has no library floor and instead
  carries the cipher and every future change to it.
- **Threads.** rgw-go's target is GOMAXPROCS plus librados's own threads
  plus a handful; rgw-rs's is the tokio workers plus a handful.
- **Errors are enums, not sentinels.** The errno-versus-local split, the
  class-specific codes and the S3 code-plus-status shape are types the
  compiler checks, not conventions.
- **Tests are `#[test]` and `#[tokio::test]`, cluster tests are
  `#[ignore]`**, as rados-rs's are; there is no suite framework, and the
  conformance suite over the store traits is the analogue of rgw-go's
  counterfeiter fakes.

What is mirrored on purpose: the layer map, the round-trip tables, the
op-per-type pipeline with store traits at the consumer, the dispatch
table with no router, the metadata cache design, the worker set per
phase, the phase order and every gate, the goldens format, the rooket
harness shape, the encoding rule, the exclusions, the defect registry,
and the derived image as the release artifact, built per supported Ceph
release with rgw-rs as `/usr/bin/radosgw` and the original beside it, as
rgw-go plans and has not yet built. rgw-go also keeps goreleaser
archives; rgw-rs adopts no equivalent.

Where rgw-rs deliberately differs from radosgw itself, beyond the
STANDARD insertion section 9 declines to reproduce:

- **An UPDATE_OBJ notify invalidates.** The metadata cache handles an
  UPDATE_OBJ notify by invalidating the named entry and re-reading it,
  never by applying the payload it carries. rgw-rs still sends the full
  record on its own metadata writes, because radosgw applies it (rados-rs
  registry, CEPH-BUG-017).
- **Shard counts below 1 are refused.** Configuration load refuses a GC,
  lifecycle or usage shard count below 1 (`rgw_gc_max_objs`,
  `rgw_lc_max_objs`, `rgw_usage_max_shards`) rather than faulting on
  first use (rados-rs registry, CEPH-BUG-019).

## 17. Risks and items to verify at implementation

- **rados-rs gaps found late.** Eight are known (section 8) and two of
  them, resending under the original transaction id and the write mtime,
  are correctness rather than convenience; anything else the gateway
  needs lands in the fork before the package that first needs it.
- **Fork drift.** Every package of rados-rs is an upstream candidate;
  what upstream declines lives on at a rebase cost per upstream commit,
  and rgw-rs's pin follows the fork's `main`, not upstream's.
- **Encoding versions**, as for rgw-go and rados-rs: a misjudged floor or
  framing decodes nothing from a real cluster, and only the gate on a
  populated cluster catches the ones the corpus lacks; Tentacle
  encodings have no corpus and rely on generated vectors.
- **The Rook suite's image selection**, which needs the derived image
  loaded under the table's name and is not yet exercised by rgw-go
  either, whose derived image is not built; and the suite itself grows
  with Rook main, so the acceptance target moves.
- **Release-keyed request shapes** wait for the operator to raise the
  required release during a rolling upgrade; accepted by the encoding
  rule, to be stated in the operator-facing docs.
- **Two gateways, one owner.** rgw-go is ahead by a phase; the value of
  rgw-rs is the comparison, which holds only while section 16 stays
  honest and the parity settings are enforced in every benchmark run.
- **Build time.** Five crates, hyper, rustls and an AWS SDK in tests make
  `cargo test --workspace` slower than `go test`; the cluster tests are
  `#[ignore]` so the every-PR gate stays under a few minutes, and this is
  measured in the scaffold package.
- **Security surface.** Public repository, network-facing daemon, auth as
  the boundary. Every `x-amz-*` header present must be in
  `SignedHeaders`, as radosgw enforces; trailer signatures are verified;
  bodies stream under a budget and are never buffered to verify; the
  system and admin flags are set only by callers radosgw allows to set
  them, and the admin API's capability and system-flag checks match
  radosgw's exactly; secrets never derive `Debug`; the anonymous `GET /`
  answer is radosgw's empty `ListAllMyBucketsResult` and carries nothing
  internal; the metadata cache handles an UPDATE_OBJ notify by
  invalidating the named entry and never by applying the payload it
  carries, and the gateway's cephx caps stay as narrow as radosgw's need.
  These are requirements of phase 1, and the reason the spike's auth
  crate is salvaged as an algorithm, not as a middleware.

## Appendix: evidence

`tag:path:line` cites a git tag; `path:line` cites the checkout named at
the top. A line quoted was read at that line.

Ceph (`~/github/ceph`):

- RGWZoneParams: `v19.2.6:src/rgw/driver/rados/rgw_zone.h:156`
  `ENCODE_START(15, 1, bl)`; `v20.2.4:...:160` `ENCODE_START(18, 1, bl)`.
  RGWZoneGroup 6: `v19.2.6:...:372`, `v20.2.4:...:394`. RGWRealm 1:
  `v19.2.6:...:550`. RGWPeriod 1: `v19.2.6:...:784`. RGWPeriodConfig 2:
  `:496`. RGWPeriodLatestEpochInfo 1: `:609`.
- RGWZoneGroupPlacementTier: `v19.2.6:src/rgw/rgw_zone_types.h:558`
  `ENCODE_START(1, 1, bl)` (struct at :545); `v20.2.4:...:612`
  `ENCODE_START(4, 1, bl)` (struct at :591). RGWZoneGroupPlacementTarget
  3: `v19.2.6:...:609`, `v20.2.4:...:702`; STANDARD inserted on decode
  `v19.2.6:src/rgw/rgw_zone_types.h:625`, `v20.2.4:...:717-719`.
- RGWUserInfo `(23, 9)`: `v19.2.6:src/rgw/rgw_common.h:624`,
  `v20.2.4:...:648`. RGWBucketEntryPoint `(10, 8)`:
  `v19.2.6:src/rgw/rgw_common.h:1119`, `v20.2.4:...:1162`.
  RGWBucketInfo::encode `(24, 4)`: `v19.2.6:src/rgw/rgw_common.cc:2268`,
  `v20.2.4:...:2331`. RGWObjManifest `(8, 6)`:
  `v19.2.6:src/rgw/driver/rados/rgw_obj_manifest.h:273`, same at v20.2.4.
  RGWCacheNotifyInfo `(2, 2)`: `v19.2.6:src/rgw/rgw_cache.h:96,106`.
- rgw_bucket_dir_entry `(8, 3)`: `v19.2.6:src/cls/rgw/cls_rgw_types.h:400`;
  rgw_bucket_dir_entry_meta `(7, 3)`: `:216`; rgw_bucket_dir_header
  `(7, 2)`: `v19.2.6:...:808`, `(8, 2)`: `v20.2.4:...:835`.
- Decode maxima below the encoder, Squid: RGWObjManifest
  `DECODE_START_LEGACY_COMPAT_LEN_32(7, 2, 2)`
  `v19.2.6:src/rgw/driver/rados/rgw_obj_manifest.h:300`;
  rgw_bucket_dir_entry_meta `(6, 3, 3)` `v19.2.6:src/cls/rgw/cls_rgw_types.h:232`;
  rgw_bucket_dir_header `(6, 2, 2)` `:819`. C++ checks only the compat
  version, `v19.2.6:src/include/encoding.h:1510-1521,1586-1595`, and
  `DECODE_FINISH` skips to the end, `:1635-1641`. RGWCacheNotifyInfo's
  legacy framing `v19.2.6:src/rgw/rgw_cache.h:115`; the full list of
  legacy-framed types is `git grep DECODE_START_LEGACY_COMPAT_LEN v19.2.6
  -- src/rgw src/cls/rgw`.
- Pools and namespaces: `v19.2.6:src/rgw/rgw_zone.cc:1243-1262`
  (`".rgw.meta:root"` ... `".rgw.otp"` ... `".rgw.meta:groups"`),
  suffixes `:20-21,36`; v20.2.4 additions `:1285,1288,1301`. Root pool
  default `.rgw.root`: `v19.2.6:src/common/options/rgw.yaml.in:1321`.
  `rgw_pool::from_str` with the `\` escape: `v19.2.6:src/rgw/rgw_common.cc:2112-2121`.
- Naming: `v19.2.6:src/rgw/services/svc_bucket_sobj.cc:20`
  `".bucket.meta."`, `:89-94`; `v19.2.6:src/rgw/services/svc_bi_rados.cc:18`
  `".dir."`, `:89` (bucket id), `:122` `"%s.%" PRIu64 ".%d"`, `:127`
  `"%s.%d"`, `:137`; `v19.2.6:src/rgw/rgw_obj_types.h:215` (locator),
  `:243` (`"_" + name`), `:246-249`; `v19.2.6:src/rgw/driver/rados/rgw_rados.h:77-91`
  (`<marker>_`); `v19.2.6:src/rgw/rgw_obj_manifest.cc:232,239,241`,
  `v19.2.6:src/rgw/driver/rados/rgw_obj_manifest.cc:241-245` (random
  prefix), `v19.2.6:src/rgw/driver/rados/rgw_putobj_processor.cc:486,559-560`;
  `v19.2.6:src/rgw/services/svc_bi_rados.h:38-39` (`multipart`, `shadow`);
  `v19.2.6:src/rgw/rgw_multipart_meta_filter.cc:6` (`.meta`),
  `v19.2.6:src/rgw/rgw_multi.h:16` (`2~`). Root-pool names:
  `v19.2.6:src/rgw/driver/rados/config/{zone.cc:25-26,39, zonegroup.cc:25-27,42,
  realm.cc:26-29, period.cc:25-27,35, period_config.cc:23}`. Users:
  `v19.2.6:src/rgw/rgw_user_types.h:82` (`tenant$id`),
  `v19.2.6:src/rgw/services/svc_user_rados.cc:29,111,297-298,320-321,335,347`,
  `v19.2.6:src/rgw/driver/rados/rgw_user.h:48-58` (bare RGWUID). Entry
  point: `v19.2.6:src/rgw/rgw_bucket.cc:93-102`.
- Notify objects: `v19.2.6:src/rgw/services/svc_notify.cc:19,185,201`,
  selection `:191-196`; `rgw_num_control_oids` default 8
  `v19.2.6:src/common/options/rgw.yaml.in:1115,1126`. Cache:
  `rgw_cache_lru_size` 25000 `:309,318`; `rgw_cache_expiry_interval` 900
  `:3336,3346`.
- Sizes and limits (`v19.2.6:src/common/options/rgw.yaml.in`):
  `rgw_max_chunk_size` 4_M `:95,104`; `rgw_obj_stripe_size` 4_M `:1869`;
  `rgw_put_obj_min_window_size` 16_M `:107`; `rgw_put_obj_max_window_size`
  64_M `:120`; `rgw_get_obj_window_size` 16_M `:1904`;
  `rgw_get_obj_max_req_size` 4_M `:1915`; `rgw_max_concurrent_requests`
  1024 `:3471,3480`; `rgw_thread_pool_size` 512 `:1111`;
  `rgw_mp_lock_max_time` 10_min `:466,475`; `rgw_enable_apis` default
  `:284,293`; `rgw_run_sync_thread` `:2492`; `rgw_gc_max_objs` 32
  `:1692,1701`; `rgw_lc_max_objs` 32 `:433`; `rgw_usage_max_shards` 32
  `:1522`; `rgw_usage_max_user_shards` 1 `:1535`; `rgw_reshard_num_logs`
  16 `:2717`.
- Shards and caps: `v19.2.6:src/rgw/driver/rados/rgw_gc.cc:28-29,35,42`;
  `v19.2.6:src/rgw/driver/rados/rgw_tools.h:35-36,40-43` (65521,
  `rgw_shards_max`); lifecycle `HASH_PRIME` 7877 `v19.2.6:src/rgw/rgw_lc.h:27`,
  cap `v19.2.6:src/rgw/rgw_lc.cc:237-239`, names `:244-246`;
  `v19.2.6:src/rgw/driver/rados/rgw_rados.cc:120,1626`;
  `v19.2.6:src/rgw/driver/rados/rgw_reshard.cc:29-30`, `reshard.%010u`
  `:1323-1329`.
- Return-vector reply cap: `v19.2.6:src/osd/PrimaryLogPG.cc:4210-4225`;
  `osd_max_write_op_reply_len` 64 `v19.2.6:src/common/options/global.yaml.in:3768`.
  A zero mtime leaves the object's mtime unchanged:
  `v19.2.6:src/osd/PrimaryLogPG.cc:8978-8983`.
- Locks: `v19.2.6:src/rgw/rgw_op.cc:6431-6434` (`"RGWCompleteMultipart"`),
  `:6640`; `v19.2.6:src/rgw/driver/rados/rgw_sal_rados.cc:3737-3759`;
  `v19.2.6:src/rgw/rgw_lc.h:30` (`lc_process`);
  `v19.2.6:src/rgw/driver/rados/rgw_reshard.cc:30` (`reshard_process`).
- Guard and paths: `v19.2.6:src/rgw/rgw_common.h:331` `ERR_BUSY_RESHARDING 2300`;
  `v19.2.6:src/rgw/driver/rados/rgw_rados.cc:917` (completion-manager
  retry guarded), `:3124-3374` (`_do_write_meta`: prepare `:3291`, head
  `:3300`, complete `:3320`, cancel `:3374`), `:3249`
  (`cls_rgw_obj_store_pg_ver`), `:6511` (id-tag compare), `:6549,5722-5726`
  (head removal on overwrite), `:8827-8854` (`raw_obj_stat` with
  `prepare_op_for_read`, `getxattrs`, `stat2`, first chunk), `:164-167`
  (`cls_version_read`), `:6467`, `:7434`, `:9456-9475` (prepare with the
  guard), `:9511-9517` (complete, `aio_operate`), `:9710`, `:9902`
  (suggest-changes), `:5931-5961` (delete: prepare, `remove_rgw_head_obj`,
  complete), `:5400`, `:4922` (`cls_refcount_get`), `:783,905-918`
  (`RGWIndexCompletionManager`). User class: `v19.2.6:src/rgw/driver/rados/buckets.cc:41,166,216,271`.
- Mon config: `v19.2.6:src/rgw/rgw_main.cc:104`,
  `v19.2.6:src/global/global_init.cc:369-375`,
  `v19.2.6:src/mon/MonClient.cc:125,220,397-398,480,497,614`,
  `v19.2.6:src/msg/Message.h:69` (`MSG_CONFIG 62`),
  `v19.2.6:src/mon/ConfigMap.cc:145-163` (section chain),
  `v19.2.6:src/librados/RadosClient.cc:232`. Precedence:
  `v19.2.6:doc/rados/configuration/ceph-conf.rst:47-60`,
  `v19.2.6:src/common/config.h:31-37`; mon values ignored for locally set
  options `v19.2.6:src/common/config.cc:277-330`. Environment:
  `parse_env` `v19.2.6:src/common/config.cc:473-500` (`CEPH_ARGS`,
  `CEPH_KEYRING`), called from `v19.2.6:src/global/global_init.cc:165`;
  `CEPH_CONF` `config.cc:428`.
- radosgw's defaults and start-up order: `v19.2.6:src/rgw/rgw_main.cc:79-86`
  (`objecter_inflight_ops` 24576, `ms_mon_client_mode` secure,
  `auth_client_required` cephx), `:100-102`
  (`CINIT_FLAG_DEFER_DROP_PRIVILEGES`), `:143` (`init_storage`), `:165`
  (`init_frontends2`); `v19.2.6:src/global/global_init.cc:320`; the drop
  after bind `v19.2.6:src/rgw/rgw_asio_frontend.cc:598-615,770`.
- Beast keys: `v19.2.6:src/rgw/rgw_asio_frontend.cc` (`config.find` and
  `get_val` calls; `config://` `:775`); `so_reuseport`
  `v20.2.4:src/rgw/rgw_asio_frontend.cc:652`, and no `ssl_reload` key
  there; no `tls_groups` key in either (`git grep`).
- Service map: `v19.2.6:src/rgw/driver/rados/rgw_rados.cc:1141-1171`
  (`register_to_service_map`), `:1175` (status);
  `v19.2.6:src/rgw/rgw_appmain.cc:421,493-494` (frontend metadata);
  `v19.2.6:src/librados/RadosClient.cc:266,306-309` (MgrClient). Client
  nonce: `v19.2.6:src/msg/Messenger.cc:33`.
- Anonymous `GET /`: `v19.2.6:src/rgw/rgw_rest_s3.cc:4596-4602`,
  `v19.2.6:src/rgw/rgw_common.cc:1290-1291`,
  `v19.2.6:src/rgw/rgw_op.cc:2562` ("skipping list_buckets() for
  anonymous user").
- Unsigned-payload fallback: `v19.2.6:src/rgw/rgw_auth_s3.h:641-662`.
- `ceph-dencoder` in `ceph-common`: `v19.2.6:ceph.spec.in:1718`,
  `debian/ceph-common.install`.
- Corpus: `~/github/ceph/ceph-object-corpus`, archives listed with
  `ls archive/*/objects/<type>`. The goldens use the commit v19.2.6 pins,
  6b15dbab. The evidence was read at the working tree's 9670a0ef, four
  commits later, whose diff touches no RGW object; the checkout's HEAD
  pins 44b11dd5, which is not used.

Rook (`~/github/rook`):

- Which radosgw-admin: `v1.20.7:pkg/operator/ceph/object/admin.go:235-278`;
  `v1.20.7:pkg/daemon/ceph/client/command.go:52-55`. Operator image:
  `v1.20.7:images/ceph/Makefile:19` `CEPH_VERSION ?= v20.2.4-20260818`,
  `:22`, `Dockerfile:16`.
- Launch: `dc7829268:pkg/operator/ceph/object/spec.go:446-459`
  (`radosgw`, `--foreground`, frontends, mime types, realm, zonegroup,
  zone), `:474-483` (`--service-unique-id`, gated on 19.2.4 and 20.2.1),
  `:518-528,560-576` (vault flags), `:530-536` (ops log), `:556-558`
  (`--rgw-enable-apis`), `:589-604` (read-affinity wrapper), `:606-608`
  (`--host`), `:611-613` (`rgwCommandFlags`), `:1180-1213`
  (`rgw_enable_apis` forced by a `/` Swift prefix);
  `pkg/operator/ceph/controller/spec.go:366-380` (daemon flags, `--id`
  `:366-369`), `:380-398` (`--ms-bind-ipv4/6`);
  `pkg/operator/ceph/object/config.go:65-125` (frontend string), `:77-96`
  (TLS frontend keys, `ssl_private_key` only for a TLS Secret), `:99-127`
  (TLS options), `:161-164` (`rgw.<store>.<letter>`);
  `pkg/operator/ceph/config/store.go:148-152` (`--mon-initial-members`);
  `pkg/operator/ceph/config/defaults.go:40-50` (logging flags).
  `v1.20.7:pkg/operator/ceph/object/spec.go:436-462` has no
  `--service-unique-id`.
- `ceph.conf` and environment: `dc7829268:pkg/operator/ceph/controller/spec.go:133-160,314-318`
  (`rook-config-override` at `/etc/ceph/ceph.conf`);
  `pkg/operator/k8sutil/pod.go` (`ClusterDaemonEnvVars`),
  `pkg/operator/ceph/controller/spec.go` (`ApplyNetworkEnv`).
- Mon store: `dc7829268:pkg/operator/ceph/object/config.go:236-265`
  (`rgw_run_sync_thread` `"true"` unless `DisableMultisiteSyncTraffic`,
  `rgw_log_nonexistent_bucket`, `rgw_log_object_name_utc`,
  `rgw_enable_usage_log`, `rgw_zone`, `rgw_zonegroup`), `:272-276`
  (`rgw_s3_auth_use_keystone`), `:278-288` (swift), `:290-319`
  (`rgwConfig`, `rgwConfigFromSecret`), `:324-360` (keystone);
  `pkg/operator/ceph/config/monstore.go:286-345` (`assimilate-conf`);
  `pkg/operator/ceph/cluster/cluster.go:824-840` (`ms_*_mode` secure with
  encryption); same six keys at `v1.20.7:...config.go:256-265,300`.
- Probes: `dc7829268:pkg/operator/ceph/object/spec.go:640-642` (no
  liveness), `:644-680`, `:714-752`, `:683-712`, `:754-775`;
  `pkg/operator/ceph/object/rgw-probe.sh:17,25,43-67`. Health checker
  removed in `a7c0c7ee9`; ready at `controller.go:527`.
- Exporter: `dc7829268:pkg/operator/ceph/nodedaemon/exporter.go:46,202`;
  `pkg/operator/ceph/controller/spec.go:246-256,314-320` (the RGW pod's
  `/run/ceph` mount).
- Admin ops user: `pkg/operator/ceph/object/admin.go:114,117,484-535`,
  `user.go:113-175`. No per-daemon image: `pkg/apis/ceph.rook.io/v1/types.go:1911-1981,2091-2217`;
  every RGW container uses `c.clusterSpec.CephVersion.Image`
  (`spec.go:362,398,411,446`).
- Suite: `dc7829268:tests/integration/ceph_object_test.go:44-144`
  (entry points `:124-143`, TLS `:87-101`);
  `tests/integration/object/util/sharedstore/sharedstore.go:96-107`
  (zoned and classic), `:175-204` (placement `bar`; `FOO` on `default`
  already at `v1.20.7:...sharedstore.go:123-125`), `:259-296` (Service),
  `:370-374,386-392`;
  `tests/integration/object/util/client/{s3.go:33-36,73,85, admin.go:35-63,
  tls.go:38-45,52-85}` (`radosgw-admin` in the toolbox `s3.go:33-36`;
  `GET /admin/info` `admin.go:57-60`);
  `tests/scripts/generate-tls-config.sh:45,54`;
  `pkg/operator/ceph/object/objectstore.go:77-99` (namespace suffix
  table), `:948-975` (applied), `:75,1130-1220` and `rgw.go:77-101`
  (`dashboard-admin`); the operator's admin calls
  `pkg/operator/ceph/object/admin.go:171-205`,
  `pkg/operator/ceph/object/user/controller.go:437-504`,
  `pkg/operator/ceph/object/bucket/provisioner.go:921-938`;
  `tests/framework/installer/ceph_installer.go:44-53` (image table),
  `:96-116`, `:214` (toolbox routing), `ceph_manifests.go:159-161` (no
  pull policy), `:170-171`, `:190-193`; go-ceph
  `rgw/admin/radosgw.go:94-95` (`UNSIGNED-PAYLOAD`, module cache
  v0.41.0). CI invocation
  `.github/workflows/ceph-suite-integration-test.yml:133-137`; kind
  `.github/workflows/integration-test-setup-cluster-resources/action.yaml:29-38`,
  `tests/config/kind-config.yaml`; image import
  `tests/scripts/github-action-helper.sh:348-363`
  (`load_image_into_cluster`). Chart keys
  `deploy/charts/rook-ceph-cluster/values.yaml:110-116`; the chart
  operator's scope `v1.20.7:deploy/charts/rook-ceph/values.yaml:45`
  (`currentNamespaceOnly: false`).

rados-rs (`scratchpad/rados-rs`, `origin/main` = 0b5d1a2; the draft read
a511eca, and a511eca..0b5d1a2 is three test-only commits):

- Workspace: `Cargo.toml` (members, version 0.1.4, rust-version 1.88,
  edition 2024, MIT); `rados/Cargo.toml` (snap, zstd, lz4, flate2;
  `bench-librados`, `ab_*`); `rados-cls/Cargo.toml` (features).
- Config: `rados/src/client.rs:63,96-116,131,286,309-330,434-446`;
  `rados/src/cephconfig/config.rs:97-107` (section chain); no `CEPH_ARGS`
  reader under `rados/src` (grep). Mon config partial:
  `rados/src/monclient/client.rs:411-412` (subscribe), `:183-191`,
  `:1326-1337` (`handle_config` keeps four keys, sends key names),
  `:1737-1740`; `rados/src/monclient/messages.rs:184-188` (`MConfig`).
- Release: `rados/src/osdclient/osdmap.rs:1382-1395` (`CephRelease(pub u8)`,
  unknown bytes kept), `:1430-1444`; `rados/src/osdclient/ioctx.rs:115`;
  `rados-cls/src/rgw/mod.rs:7-26`.
- IoCtx: `rados/src/osdclient/ioctx.rs:71-78` (namespace and locator per
  context, "Mirrors `IoCtxImpl::oloc`"), `:123,132,177-186,1114-1125`;
  `rados/src/osdclient/client.rs:1846` (per-op `ObjectId` below IoCtx);
  `rados/src/osdclient/operation.rs:116-427` (builder steps; `write`
  `:127` and `write_full` `:137` take `impl Into<Bytes>`; `returnvec`
  `:262`); `rados/src/osdclient/types.rs:173-178,203-217,589-590,616,935,
  1146-1168,1389-1410,1530-1562,1583`; `rados/src/osdclient/messages.rs:117-139`
  (flags from opcode); `rados/src/osdclient/client.rs:2054-2062` (mtime
  only when WRITE-flagged); `rados-cls/src/otp.rs:18-20`. Gap 2:
  `rados/src/osdclient/operation.rs:419-422` (`flags`),
  `rados/src/osdclient/client.rs:1857-1858,2027`; `rados-cls/src/call.rs:36-66`
  and `rados/src/osdclient/ioctx.rs:1081-1083` (class calls set no flag).
  Gap 4: `client.rs:1846` (`execute_built_op_with_id`, public),
  `types.rs:203-217` (`ObjectId` with namespace and key). Gap 6: builder
  steps through `OSDOp::` constructors only, `types.rs:935,1146-1222`;
  `Stat` only, `types.rs:826-832`; `calc_op_budget` `types.rs:1359-1379`.
- Throttle and retry: `rados/src/osdclient/throttle.rs:12-14,70-107,163-184`
  (`acquire_many` with no clamp to the budget `:70-93`);
  `rados/src/osdclient/client.rs:191,233-239,470-505,1891-1892,1911-1968`,
  `:2166-2179` (resend paths; the fix is
  [jhoblitt/rados-rs#28](https://github.com/jhoblitt/rados-rs/pull/28));
  `rados/src/osdclient/session.rs:863-868,1198-1215`;
  `rados/src/osdclient/tracker.rs:22-27`.
- Messenger and mgr: mode lists `rados/src/msgr2/mod.rs:327,358`, no
  `ms_client_mode` plumbing into `ClientBuilder`; client address
  `rados/src/msgr2/protocol.rs:1007`. No mgr client: the only hits are
  the `mgrmap` subscription name, `rados/src/monclient/subscription.rs:21`,
  `rados/src/monclient/client.rs:178,1742`.
- Watch: `rados/src/osdclient/watch.rs:26-42,53,361-430,472-487,505-553,
  596-600`; `rados/src/osdclient/ioctx.rs:766,772,791`;
  `rados/src/osdclient/client.rs:33-37,579-595,649-681,999-1036,1147-1151,
  1270-1278`.
- Encoding: `rados/src/denc/codec.rs:53,315,373,408,416,458-547,641-851`
  (one maximum for both checks `:493-508`, tail skipped `:543-544`);
  `rados-denc-macros/src/lib.rs:38,76-85,139-145,225,280,401-405,406,467,565`;
  `rados/src/denc/macros.rs:28-41,66-74`; `rados/src/denc/types.rs:42-70`
  (`UTime`); no legacy compat-length framing under `rados/src/denc` (grep);
  `rados-cls/src/lock.rs:140`, `rados-cls/src/rgw/index.rs:124-125,2238-2252`.
- Corpus harness: `rados-dencoder/tests/dencoder_corpus_comparison_test.rs:88-232,
  235-274,303-341,464-489,541,580,795-808`; `rados-dencoder/src/main.rs:197-353`
  (closed registry, RGW class types at `:264-323`); `rados-dencoder/Cargo.toml`
  (`publish = false`, bin only).
- rados-cls surface: `rados-cls/src/rgw/index.rs` (`guard_bucket_resharding_op`
  `:1860`, `guard_op` `:1870`, `get_bucket_resharding_op` `:1876`,
  `update_stats_op` `:1748`, `DirEntryMeta::for_release` `:116`);
  `rados-cls/src/rgw/olh.rs:460,530`; `rados-cls/src/call.rs:1-6,47-66`;
  `rados-cls/src/two_pc_queue.rs:528`; `rados-cls/src/user.rs:666`.
- Errors: `rados/src/osdclient/error.rs:31-69`.
- Cephx: `rados/src/auth/aes256krb5.rs:1-3`; `rados/src/auth/types.rs:54-65`;
  `rados/tests/cephx_aes256k.rs:153-207`.
- CI and publish: `.github/workflows/ci.yml:29` (fmt), `:55-68` (clippy
  per feature, no `--all-features`), `:87` (test, no `--locked`),
  `:89-136` (`corpus-test`, the apt ceph-dencoder comparison); no
  `cargo deny` anywhere and no `[workspace.lints]`, the unwrap and expect
  rule is prose in `.claude/CLAUDE.md:75,85`;
  `.github/workflows/test-with-ceph.yml:21-22,57-62,76-129`;
  `docker/docker-compose.ceph.yml:3` (v19.2.2);
  `.github/workflows/publish.yml:76-84` (`rados-denc-macros`, `rados`
  only; `rados-cls/Cargo.toml` has no `publish = false`). rooket
  instructions: `rados/tests/common/mod.rs:12-59,84-99`.
- Duplicate map keys: rados-rs `design/rgw-mvp`
  `docs/superpowers/plans/2026-09-25-rgw-mvp-16-release-shapes.md:864-883`;
  `docs/superpowers/ceph-upstream-bugs.md:593-597`. The Rook paragraph:
  `docs/superpowers/specs/2026-09-24-rados-rs-rgw-mvp-design.md:92-104`
  (commit 15389c2). Registry entries this spec answers, on
  `design/rgw-mvp` at b056c4f: CEPH-BUG-017 (`ceph-upstream-bugs.md:591`,
  UPDATE_OBJ handling) and CEPH-BUG-019 (`:767`, shard counts).

rgw-go (`~/github/rgw-go` at b0929ee; the draft read ca998e2, and the
one merge between touches only `docs/exclusions.md`):

- Packages present: `cmd/rgw-go`, `internal/{denc,meta,acl,radosclient,radosclient/goceph,cls/*,cli,version,testutil}`,
  `test/gate`, `hack/{rooket,goldens}`; absent: `internal/{driver,op,s3,auth,policy,admin,iam,cephconf,frontend,metrics,asok,opslog}`.
- Spec `docs/superpowers/specs/2026-09-25-rgw-go-design.md`: `:68-71`
  (only librados bindings go to the go-ceph fork), `:140-147` (`cephconf`
  parses the early arguments and `CEPH_ARGS`), `:326-328` (admin-socket
  counters in phase 3), `:417-423` (the derived image). The exclusions
  corrections: `docs/exclusions.md:100-103,133-136`, commit 4312a7a. The
  watch error channel forwarded from go-ceph's `Watcher.Errors()`:
  `d8efe8a`. The gate's finding: `docs/ceph-upstream-bugs.md:222`.
- rooket harness: `hack/rooket/up.sh:22-23,57`; `hack/rooket/lib.sh:8,23,46-54`;
  `hack/rooket/{squid,tentacle}/values/rook-ceph-cluster.yaml:5` (v19.2.6,
  v20.2.4; `dashboard.enabled: false`); `hack/rooket/{squid,tentacle}/config.yaml:4`;
  `hack/rooket/README.md:57-59,96-103`; podman path retired in `fa9ac12`;
  rooket pinned at 9e9ab38e, `.github/workflows/integration.yml:21,32-33,127`;
  the service-map lookups `hack/rooket/lib.sh:64-79`,
  `hack/rooket/populate.sh:23-28`, `hack/rooket/up.sh:42-47`.
- Gate: `test/gate/phase0_test.go:93-103,116-139,167-197,199-219,399,
  1319-1470`; connect from `ceph.conf` `:414`; layout reads and pool
  listings `:286-295,1191`; counts against the manifest
  `:982-983,1185-1186,1444`; `hack/rooket/populate.sh:23-34,43-53,124-135,190-211`.
  Phase 0 plan task index `docs/superpowers/plans/2026-09-26-phase-0-foundations.md:66-87`.
- Goldens: `hack/goldens/gen.sh:5-8,14-15,29,37,39-44`;
  `internal/denc/goldentest/goldentest.go:32-59,62-71,89-94`;
  `hack/goldens/types.txt:1-98`; 3376 triples (count of `.bin`), 54 MB
  across seven packages.
- go-ceph pin `go.mod:41`; no Dockerfile; `.goreleaser.yaml:8-12`;
  thread counts `docs/cgo-limitations.md:23-25,33-34,39-41`; registry
  `docs/ceph-upstream-bugs.md` (18 entries; verdicts at `:38,56,73,91,111,
  131,150,170,187,203,218,233,251,279,302`); spec sections 4, 6 to 10, 12
  to 14; plan `:30-38,66-88`.

rgw-rs spike (`~/github/rgw-rs` at 8621b5a, clean tree):

- `Cargo.toml:2-9` (`:9` LGPL-2.1-or-later); `FINDINGS.md` at the
  repository root; README `:6,11` ("research spike", "throwaway");
  `Cargo.lock` (hyper 1.11.1, axum 0.8.9, tokio 1.53.1,
  rustls 0.23.45, tokio-rustls 0.26.5, quick-xml 0.42.0, aws-sdk-s3
  1.149.0); `.cargo/config.toml:4-5`; workflows ci, codeql, commitlint,
  dependency-review, scorecard, workflow-lint; 119 tests.
- `crates/rgw-sal/src/lib.rs:20,30,85,90-186` (16 async methods under
  `#[async_trait]`, marker `&str` at `:132-137`); `testsuite.rs:17-23,
  68-327`; instance dropped `crates/rgw-sal-sqlite/src/lib.rs:56,601-613`,
  `crates/rgw-rest-s3/src/xml.rs:174-180`; name-only paging
  `crates/rgw-sal-sqlite/src/lib.rs:465-479`.
- Dispatch `crates/rgw-rest-s3/src/lib.rs:5-6,54-75`, `src/handler.rs:47-73`,
  `src/perm.rs:41`. Auth `crates/rgw-auth/src/lib.rs:133-146,165-167,184,
  241-254`, `chunked.rs:3,55-56`; vectors `sigv4.rs:353-439`.
- Salvage: `crates/rgw-auth/src/sigv4.rs`; `crates/rgw-types/src/error.rs:10-134`;
  `crates/rgw-rest-s3/src/{xml.rs:192-274,range.rs,bucket.rs:24-54,list.rs:42-74}`;
  `crates/rgw-tests/src/lib.rs:44-168`; `crates/rgwd/src/lib.rs:24-32`.

rooket (`scratchpad/rooket-src`, README "Using rooket as a test harness"):
the `up`, `ceph-config` and `wait` commands, the `host-network` rule,
`cephImage.tag` and `allowUnsupported`, the users the cluster adds; no
`cephImage` special-casing in its Go sources (grep), so `cephImage.repository`
in a values file reaches the chart unchanged. A bare kind cluster:
`rooket cluster create`, or `rooket up --skip-build --skip-deploy`
(`cmd/up.go:54`, README `:425`); `rooket wait` `cmd/wait.go:42-55`;
toolbox access `rooket k -n rook-ceph exec deploy/rook-ceph-tools --`
(README `:83,180-183`), with no `rooket exec`.

Unverified in this draft: whether the fork holds a crates.io token; the
exact behaviour of Rook's S3 test client's checksum middleware.

## Review edits applied (2026-09-27)

The adversarial review of draft 2bc2099, with the owner's rulings, one
line per finding:

1. The golden generator moves ahead of its users, to package 3; section
   12 names how goldens are made and rgw-go's layout; the phase 0 gate
   admits the toolbox dencoder's re-encoding.
2. The privilege drop order is corrected: connect, bind, then drop.
3. The legacy framing is required by the rule only; package 2 is the
   open-version decode, which Squid's own manifests need, and the mode is
   `rgw-meta`'s alone.
4. The driver foundation is split: connection and release detection stay
   in phase 0 as the one deviation from decision 3; site and system
   objects open phase 1; gap packages are scheduled to their first user.
5. Gap 7, the mgr client, is added and placed in phase 1 before the Rook
   suite; rgw-rs's harness does not read the service map.
6. Section 5 is rewritten: Rook's full argv, environment, `ceph.conf` and
   mon-store writes, the minimum under Rook, and radosgw's own default
   overrides; the exclusions corrections are recorded as landed.
7. Throttle parity is 24576 operations; the byte throttle's refusal of an
   oversized operation is a rados-rs defect in gap 6.
8. Layer-map corrections: reshard log names, the head locator, Rook's
   namespace suffixes, decoder maxima, the return-vector reply cap, the
   tier nit, the appendix cites and the corpus pin.
9. The Rook suite runs on a bare kind cluster with the derived image
   loaded under the table's name and `radosgw-admin` kept; `GET
   /admin/info` and the operator's admin calls join phase 1; section 1's
   placement sentence is corrected.
10. The gates are attributed to rados-rs correctly, with the additions
    named; the license is LGPL-2.1-or-later, with a `cargo deny` policy to
    match.
11. Section 15 records the owner's decisions, with the license as decision
    8; the README line, the `FINDINGS.md` move, the messenger-mode package
    (gap 8, in flight) and gap 3's in-flight fix are added; the derived
    image moves to what is mirrored on purpose.
12. Gaps 2 and 4 are made precise, and gap 6 gains the builder, `stat2`
    and budget items.
13. Text corrections in sections 7, 9, 12 and 16 and the appendix; the
    rgw-go pin moves to b0929ee and the rados-rs pin to 0b5d1a2.
14. The TLS combined PEM and the beast key list are pinned; admin-socket
    counters are placed in phase 3; the anonymous `GET /` body is
    required.

Security: section 17 and gap 3 are stated as requirements, and no caps or
check shapes are described. Owner-agreed additions: UPDATE_OBJ handling by
invalidation (sections 7, 16 and 17) and the refusal of shard counts
below 1 (sections 5 and 16).
