# rgw-rs design

Status: draft, 2026-09-27, for owner review before planning. Nothing in
it is approved. Companions: rgw-go's `docs/exclusions.md`, canon for scope,
coexistence obligations and benchmark parity for both projects by owner
decision of 2026-09-25; the rados-rs RGW MVP design on branch
`design/rgw-mvp`, whose encoding-rule paragraph the owner adopted for
rgw-rs; and rgw-go's design spec, whose layer map, gates and phase order
this document mirrors wherever section 16 does not say otherwise. Claims
about C++ RGW were checked against ceph/ceph v19.2.6 and v20.2.4, about
Rook against v1.20.7 and main at dc7829268, about rados-rs against the
fork's `origin/main` at a511eca, about rgw-go at ca998e2, and about the
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
two placements and a second storage class, so placement targets and
storage classes are on the acceptance path from phase 1.

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
  gaps this draft found, and phase 0 has a package for them.
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
| `rgw-core` | the ops, one type per operation; the store traits they consume, split by concern, with in-memory fakes and the conformance suite; auth (SigV4 header, query and presigned, chunked and trailer readers, SigV2, anonymous), the policy language, ACL and quota evaluation; the S3, admin, IAM and STS protocol layers; the dispatch table; error documents | `internal/op`, `auth`, `policy`, `acl`, `s3`, `admin`, `iam` |
| `rgw-driver` | the RADOS store, implementing `rgw-core`'s traits over `rados` and `rados-cls`: the configuration bridge and release detection, zone and placement resolution, object naming, manifests and striping, atomic head writes, the index protocol, GC enqueue, the metadata cache with watch and notify, quotas, usage; the workers | `internal/driver`, `internal/cls/*` (which live in `rados-cls` here) |
| `rgwd` | the binary: radosgw-argv handling, the beast-spec frontend on hyper with TLS, deadlines and drain, metrics, the admin socket, the ops log, build info | `cmd/rgw-go`, `internal/cli`, `cephconf`, `frontend`, `metrics`, `asok`, `opslog` |
| `rgw-tests` | integration and cluster tests, the populator, the corpus harness and the gates; not published | `test/gate`, `hack/rooket`, `hack/goldens` |

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

Gates, taken from rados-rs's own: `cargo fmt --check`, `cargo clippy
--workspace --all-targets --all-features -- -D warnings`, `cargo test
--workspace --all-targets --locked`, and `cargo deny check` for
advisories and licenses. Workspace lints deny `unwrap_used`,
`expect_used` and `panic` outside tests, as rados-rs's rule says in prose.

## 5. Configuration and invocation

Rook launches `radosgw` with `--foreground`, `--fsid`, `--keyring`,
`--mon-host`, `--id`, `--setuser`, `--setgroup`, the `--default-log-*`
flags, `--host=$(POD_NAME)`, `--rgw-frontends=beast port=8080 ...` with
the TLS keys when a certificate is set, `--rgw-mime-types-file`,
`--rgw-realm`, `--rgw-zonegroup`, `--rgw-zone`, and conditionally
`--rgw-enable-apis`, the ops-log pair, `--rgw-dns-name` and
`--service-unique-id`; and it writes `rgw_zone`, `rgw_zonegroup`,
`rgw_run_sync_thread`, `rgw_log_nonexistent_bucket`,
`rgw_log_object_name_utc` and `rgw_enable_usage_log` into the mon config
store for the daemon's own section through `config assimilate-conf`. Two
corrections to `docs/exclusions.md` follow from reading that code:
`rgw_run_sync_thread` is written `true` unless the store disables
multisite sync traffic, so the gateway honors the option as a no-op on a
single-zone zonegroup rather than relying on Rook to turn it off; and
`rgw_enable_apis` reaches the daemon only when the store spec sets it or
disables S3, so the default list, swift included, is radosgw's own.

rgw-rs therefore accepts radosgw's argv, as rgw-go does. What rgw-go gets
from librados here, rgw-rs must do itself, and that is the largest
single difference in cost between the two projects:

- Ceph's early arguments, the entity's `ceph.conf` sections (own name,
  then type, then global, which rados-rs already reads), `CEPH_ARGS`,
  which rados-rs does not read, and the generic `--<option>` and
  `--<option>=<value>` argv forms.
- The mon config store. rados-rs subscribes to `config` and decodes the
  `MConfig` message but keeps only four of its own client options and
  discards the rest; there is no way to read a value and no wait for the
  first message, where radosgw exits if it cannot fetch its config.
  Exposing the resolved values and the initial wait is the first rados-rs
  package in section 14. The mon sends values already resolved for the
  entity's section chain, so nothing beyond that is needed to see what
  Rook set.
- Ceph's documented precedence, last wins: compiled default, mon config
  store, local config file, environment, command line.

rgw-rs carries the defaults of the `rgw_*` options it honors, copied from
the floor release's option table with their source recorded; an option it
does not honor, the mime-types file among them, is logged once and
ignored. Privileges drop before the connection is made, as radosgw does.
The beast keys and their handling, and JSON logging to stderr through
`tracing`, are as rgw-go specifies them.

Rook's probes shape the first request the gateway ever answers: there is
no liveness probe, and the startup and readiness probes are exec probes
that curl `/` on the frontend port and pass on any status from 200 to
399, on 503, and for readiness on 500. rgw-rs answers an anonymous
`GET /` exactly as radosgw does. The operator marks the store ready at the
end of its reconcile without an HTTP check of its own; the suite's canary
is a CephObjectStoreUser becoming ready, which exercises the admin API
through the operator's `rgw-admin-ops-user`.

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
  tokio semaphore, defaulting to the objecter's 1024 operations and
  100 MiB, and a submit awaits its permit. rgw-rs keeps
  `rgw_max_concurrent_requests`, 1024, as a hard cap on requests and
  bounds body memory with a byte budget, as rgw-go does. Whether an
  operation larger than the byte budget can ever be admitted, as the
  C++ throttle admits it when nothing else is in flight, is unverified
  and belongs to the gap package.
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
  objects and sending the same notify on every metadata write. A watch
  that reports itself broken is delivered as an event on the watcher,
  which is the error channel rgw-go had to add; rados-rs reconnects it
  on session resets, map changes and every five seconds while in error,
  and after a not-connected error the driver watches again.
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
a colon with a backslash escape, and Rook's zoned store maps every shared
pool that way, `<pool>:<store>`. Every object is placed by pool,
namespace and locator key; in rados-rs, as in librados, namespace and
locator belong to the I/O context, not to the operation, and a context
clone is cheap, so the driver clones per namespace and, for the objects
that need one, per locator, until the gap package adds a per-operation
form.

**Object naming.** The bucket instance is `.bucket.meta.<bucket>:<id>`,
with the tenant and a colon before the bucket name when there is one, in
the domain root; the entry point is `<bucket>`, or `<tenant>/<bucket>`,
beside it. Index shards are `.dir.<id>.<shard>`, `.dir.<id>.<gen>.<shard>`
once a generation is above zero, and `.dir.<id>` unsharded, keyed on the
bucket id, in the placement's index pool. A head is `<marker>_<oid>` in
the data pool, where the oid is `_` plus the name when the name starts
with `_`, and then the locator is set to that name, and otherwise
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
usage shards `usage.<n>`, reshard logs `reshard.<n>`.

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

**Class calls by path.** The encodings are rados-cls's; the driver
decides which call goes on which path, as section 6 lists for the data
path. Beyond it: user statistics and quota through the user class,
keeping radosgw's semantics so `radosgw-admin user stats` agrees; GC per
shard, where the version class decides omap-era or queue-era and the
shard count is the smaller of `rgw_gc_max_objs`, default 32, and the
shard prime 65521; lifecycle shards capped at 7877; usage with 32 shards
and one per-user shard; the locks `gc_process`, `lc_process` and
`reshard_process` by name. Modifying methods that return data run with
the return-vector flag on the same operation; rados-cls already does this
for the user-stats reset and the 2pc queue reserve, so persistent
notifications are not gated on a client item here.

**Watch and notify.** The control pool holds `notify.<i>` for `i` below
`rgw_num_control_oids`, default 8; a metadata object's notify goes to the
one selected by hashing its pool, namespace and name; the payload is the
cache-notify record. rgw-rs watches all eight and notifies on every
metadata write it makes, so radosgw and radosgw-admin see its writes and
it sees theirs. Without this a shared zone is unsafe; with the cache
disabled it is merely slow.

**Gaps in rados-rs this map depends on**, each a rados-rs package before
the driver step that needs it:

1. The mon config store's values exposed and awaited (section 5).
2. An explicit modification time and write flag on a built operation: a
   class call sent alone is flagged as a read and carries mtime zero,
   where radosgw's class writes set the object's mtime; the OTP module's
   own docs already say a driver must.
3. Resend with the same transaction id after a connection loss: today a
   lost connection resubmits under a fresh id, which defeats the OSD's
   duplicate detection and can run a non-idempotent write twice, a
   refcount get, a usage add, a GC enqueue or a queue reserve among them,
   where the C++ objecter resends with the same id.
4. A per-operation namespace and locator on the I/O context's operations,
   or the clone pattern documented as the contract.
5. In `denc`: the legacy compat-length framing, and a decode mode that
   accepts a struct version above the type's own, skipping the tail, as
   the C++ decoders do (section 10).
6. Small items: a truncate step constructor; the oversized-operation
   throttle question; a bounded watch-event channel.

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
  and the zonegroup's placement tiers at 4 where Squid writes 1. The
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
its reservation id 0; the queue-registry listing that a Squid radosgw
never pages past 1024 entries; the stale epoch a cancelled index
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
  whose old versions carry no length field, is not implemented, and
  RGW's central types need it for their oldest corpus objects; it is gap
  5 above.
- The derive handles only a single fixed version; a type with
  version-gated fields implements the trait by hand, as rados-cls does
  for some thirty of its own. RGWUserInfo, RGWBucketInfo, the zone types
  and the manifest are all hand-written decoders with derived or
  hand-written encoders, and the goldens prove each.
- The crate's versioned decode refuses a struct version above the type's
  declared maximum, where C++ refuses only a compat above its own and
  skips the unknown tail. The rule to decode every version the C++
  accepts, and to survive the next release's bump, means `rgw-meta`
  keeps the trait's open maximum and skips trailing fields; the gap
  package makes that the documented mode rather than an accident of a
  default.
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
- The corpus archives from 0.61 through 19.2 carry RGWUserInfo,
  RGWBucketInfo, the manifest, the index entry and the ACL policy at
  every historical version; the zone parameters and the entry point from
  15.0. Tentacle encodings have no corpus and rely on generated vectors.

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
| Rook end to end | kind, Rook main, the derived image | `TestCephObjectSuite`, both passes | nightly and at gates |

Details and decisions:

- **Goldens.** rgw-go's goldens are language-neutral: the corpus object,
  ceph-dencoder's JSON dump and ceph-dencoder's re-encoding at v19.2.6,
  one triple per object, keyed by C++ type name, 98 types, 3376 triples.
  rgw-rs uses that format. rados-rs's own corpus harness is the other
  model: it compares against a live `ceph-dencoder`, taken from apt in
  CI, with a closed type registry inside a non-published binary, so
  rgw-rs cannot register its types there and copies the pattern into
  `rgw-tests` instead. How the goldens are produced is owner decision 4.
- **The cluster.** rooket, as rgw-go and rados-rs already use it: released
  Rook v1.20.7 with one worker, the `host-network` profile from the first
  `up`, `cephImage.tag` pinned per release, the dashboard off and rooket's
  `rgw` profile unused so that every user and bucket count is exact,
  `rooket wait`, and `rooket ceph-config --out` for the `ceph.conf` the
  tests read through `CEPH_CONF`, which is how rados-rs's cluster suites
  run today. The ambient cluster is never used. No make, bash or podman
  plumbing is added: the pins live in rooket configuration directories
  under `rgw-tests`, and the population and gate logic is Rust (owner
  decision 4).
- **The phase 0 gate**, rgw-go's concept, not its code: populate the
  zone through the toolbox's radosgw-admin and an S3 client, then read
  every metadata object from every pool and namespace, decode it with
  nothing left over, re-encode it for the cluster's release, and require
  the bytes radosgw wrote, or, for objects the operator's radosgw-admin
  wrote at a newer release, the bytes the toolbox's ceph-dencoder writes
  back; compare the decoded value, type-aware, with radosgw-admin's JSON;
  and require exact per-object counts so that no pass is vacuous. rgw-go
  reports that this is the order in which its real bugs surfaced.
- **The Rook suite.** Its installer selects the Ceph image from a fixed
  table keyed by `CEPH_SUITE_VERSION`, the floating `v19` and `v20` tags
  and the ceph-ci devel tags; there is no variable for an arbitrary
  image. Running it with a derived image therefore means either loading
  the derived image onto the kind nodes under the table's name for that
  release, or a one-line local patch to the framework, and the choice is
  recorded with the workflow. The suite brings its own three-mon cluster
  through Rook's installer with the dashboard on, reaches the gateway
  from the runner through a Service of its own, administers it with
  go-ceph's admin client as the `dashboard-admin` system user, and skips
  TLS verification; it does not run on a rooket cluster, it runs on kind
  as Rook's own CI does.
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

**Phase 0, foundations.** Gate: every metadata object a populated radosgw
wrote decodes and re-encodes byte-identically on a Squid and a Tentacle
rooket cluster, with exact counts.

1. **Scaffold.** The workspace as decided in owner decisions 1 and 2:
   the spike tagged and its crates removed, the five crates as empty
   shells with their dependency edges, `rados` and `rados-cls` as a git
   dependency pinned by revision, the gates in CI, workspace lints,
   commitlint and Dependabot kept, `docs/` with this spec, the
   exclusions pointer and the empty defect registry. No behaviour.
2. **rados-rs: `denc` framing and open-version decode** (gap 5), landed
   in the fork first because package 3 cannot decode the oldest corpus
   objects without it.
3. **`rgw-meta` primitives, the golden helper and the corpus harness.**
   The identity primitives and the small shared types, both framings
   proven against the corpus.
4. **User and account types.**
5. **Bucket entry point, instance and layout**, with the opaque
   sub-records.
6. **Zone parameters, zonegroup, realm, period and their defaults**,
   including the tier and placement-target shapes at both releases and
   the STANDARD quirk pinned by a dencoder oracle.
7. **Object attributes**: manifest, compression info, ACL policy
   encoding, OLH info, cache-notify record.
8. **rados-rs: the remaining gaps** (1 to 4 and 6), each its own fork
   package on its own branch, landed before package 9. This is where the
   seam's full surface is settled before the driver is written, which is
   rgw-go's first lesson; rgw-go's retrofits, the version and pool id on
   reads, the watch error channel, the local-versus-errno split and the
   return-vector output, are already present in the fork and are pinned
   by tests here rather than added.
9. **`rgw-driver` foundation.** Connect from radosgw's argv, ceph.conf
   and the mon config store; detect the release; load realm, zonegroup,
   zone and period; resolve pools and namespaces; read and write system
   objects with their versions.
10. **`rgw-tests` harness and the gate.** The rooket configuration
    directories per release, the Rust populator writing a manifest, and
    the gate test.
11. **Integration workflow**, nightly and on demand, one job per release,
    on rooket.

**Phase 1, data path, auth and core admin; first benchmark.** rgw-go's
phase 1 list, op for op, with three additions particular to rgw-rs: the
hyper frontend with the beast spec and TLS is built here rather than
inherited; the pipeline baseline replaces the seam microbenchmark; and
placement targets and storage classes are served, because the Rook
suite's zoned store declares two of each. Gates: s3-tests parity, Rook's
admin client suite, the first gateway comparison, and the Rook suite as
soon as the derived image serves a store.

**Phase 2, versioning, lifecycle and bucket configuration**, and
**phase 3, remaining services and workers**, are rgw-go's, with the
2pc-queue, lock and OTP work already in rados-cls rather than pending.

## 15. Owner decisions

**1. Evolve the spike or start fresh.**

- A. Evolve in place. Cost: the spike's shapes are load-bearing in every
  crate: axum routing, a body buffered to verify it, SQLite behind the
  trait, tenants hard-wired empty, no virtual hosts, no version instance
  honored, and AWS's rather than radosgw's signature behaviour. Reworking
  8.5k lines costs more than the parts worth keeping, and the tree keeps
  a README that calls itself throwaway.
- B. Fresh tree in the same repository: tag the spike, replace the tree
  in the scaffold package, and salvage by copying named pieces: the
  SigV4 core with its AWS vectors, the error table's shape, the XML
  structs and the whitespace-preserving multi-delete parser, range
  parsing, bucket-name validation, the in-process AWS SDK test harness,
  and the conformance-suite idea. FINDINGS.md stays under `docs/` as
  history. Cost: one large scaffold PR.
- C. A new repository and a renamed spike. Cost: loses the name, history
  and scorecard for no gain over B.

Recommendation: B.

**2. Repository layout and crate structure.**

- A. The spike's eleven crates, one per C++ file family. Cost: every op
  touches four crates; the split follows C++ files rather than build or
  test boundaries.
- B. Five crates cut on what tests without a cluster and what changes
  together: `rgw-meta`, `rgw-core`, `rgw-driver`, `rgwd`, `rgw-tests`
  (section 4). Cost: `rgw-core` is large; its modules carry the internal
  structure.
- C. One crate. Cost: no compile-time enforcement of dependency
  direction, and every test build compiles the driver.

Recommendation: B.

**3. How closely to mirror rgw-go's architecture and phasing.**

- A. Mirror closely: same package map, same phases, same gates, same op
  order. Benefit: plans transfer and the benchmark isolates language and
  client. Cost: inherits Go-shaped decisions that do not apply, the seam
  crate and the completion modes above all.
- B. Mirror the layer map, the phase order and the gates; differ exactly
  where rados-rs or Rust removes a layer or moves a cost, with the
  differences listed in section 16 and kept current.
- C. An independent design, for example the spike's data-path-first
  staging. Cost: discards rgw-go's evidence that the byte-exact gate is
  where the bugs are, and weakens the comparison.

Recommendation: B.

**4. The gate, and sharing rgw-go's harness.**

The acceptance gate:

- A. Rook's `TestCephObjectSuite` as the shared S3-level acceptance gate
  for both projects, with rgw-rs's own phase gates beneath it: the
  byte-exact metadata gate on rooket in phase 0, s3-tests parity from
  phase 1. Cost: the suite's image selection needs a retag or a local
  patch (section 12).
- B. rgw-go's gate reused. Not available as code: it is Go and drives
  rgw-go's encoders. Only its concept transfers.
- C. s3-tests parity alone. Cost: never exercises Rook's operator
  interactions, which is what "drop-in" means.

Recommendation: A.

Sharing rgw-go's language-neutral pieces, its `hack/rooket` scripts, the
populator with its manifest, and its goldens, touches the owner's rule
against new make, bash or podman cluster plumbing in this repository:

- A. Reference rgw-go's scripts and goldens in place, from a pinned
  checkout. Cost: a cross-repository test dependency that CI must clone;
  the scripts name the cluster `rgw-go-<release>` and rgw-go's gate
  asserts that name; goldens as a git dependency of tests.
- B. Port the population step to Rust in `rgw-tests` and copy the
  goldens once. Cost: about five hundred lines, and two manifests that
  drift unless their schema is shared; fifty-odd megabytes of goldens in
  the repository, regenerated only when rgw-go regenerates.
- C. rooket plus Rust for everything: the per-release rooket
  configuration directories live in `rgw-tests`, a small `xtask`-style
  binary drives `rooket up`, `wait` and `ceph-config`, a Rust populator
  writes metadata through the toolbox's radosgw-admin and data through
  the AWS SDK, and the goldens are generated by the same binary running
  ceph-dencoder through `rooket kubectl exec` into the toolbox, so no
  podman is added. The manifest schema is copied from rgw-go's so the
  two gates can compare. Cost: the largest port, and goldens depend on a
  running cluster to regenerate.
- D. As C for the cluster, but goldens as rados-rs does them: no
  committed goldens, a corpus job in CI that compares against the
  ceph-dencoder apt installs. Cost: the apt release decides the
  re-encode version, CI needs the network for the corpus, and `cargo
  test` no longer proves encodings offline.

Recommendation: C, with the manifest schema and the golden layout taken
from rgw-go verbatim.

**5. The first milestone's scope.**

- A. Phase 0 as above: stored types, the driver's connection and
  release detection, the byte-exact gate on both releases. Cost: no S3
  request is served until phase 1.
- B. A data-path hello: one PUT and GET through the index protocol into
  a real zone, with types modelled only as needed. Cost: encoding
  shortcuts taken to reach it become the foundation, which is the
  failure rgw-go's gate was built to catch.
- C. Scaffold and `rgw-meta` only, no cluster. Cost: the rados-rs gaps
  surface a milestone later.

Recommendation: A, with package 1 as the first PR to plan in full.

**6. Async runtime and HTTP stack.**

The runtime is tokio: rados-rs is tokio and there is no second choice.

- A. hyper 1.x with hyper-util's server, an own dispatcher, HTTP/1.1
  only, TLS through rustls. Cost: the beast-spec server, per-request
  deadlines, header timeouts and drain are written by hand, about what
  rgw-go writes on net/http.
- B. axum 0.8, as the spike, with a fallback-only router. Cost: a tower
  and extractor layer between the socket and the hot path; the router
  cannot express subresource dispatch, so the dispatcher is written
  anyway; `axum::serve` still needs the same hand-configured timeouts.
- C. actix-web. Cost: its own runtime model beside tokio-native rados-rs.

Recommendation: A. HTTP/2 stays off because radosgw's beast frontend is
HTTP/1.1 and every client in the gate speaks it. The rustls crypto
provider is a build-time choice recorded in the scaffold package.

**7. How rgw-rs tracks rados-rs.**

- A. A git dependency on the fork's `main`, pinned by revision and bumped
  by hand, for both `rados` and `rados-cls`; Dependabot does not follow
  git revisions, which is rgw-go's situation with its go-ceph replace.
- B. crates.io. Cost: the fork's publish workflow is upstream's and
  publishes only `rados` and `rados-denc-macros`; `rados-cls` is not
  published anywhere, and the fork's packages are not upstream yet.
- C. A git submodule with path dependencies. Cost: submodule hygiene in
  every checkout and CI step, for no benefit over A.

Recommendation: A, revisited when the fork's packages are upstream. The
one git revision the gateway depends on is the fork's `main`, which is
what the rados-rs design says that branch is for.

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
  installed client headers and go-ceph would not take them. The owner's
  rados-rs design takes them, one class per feature, for upstreaming.
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
  config store, `CEPH_ARGS` and every `rgw_*` option during connect;
  rgw-go reads them back. rgw-rs implements the config sources and their
  precedence itself, and the mon store first needs a fork package. This
  is the one place where rgw-rs's foundations are larger than rgw-go's.
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
- **The derived image is the release artifact**, not a binary archive:
  no goreleaser equivalent is adopted, and the release workflow builds
  the image per supported Ceph release with rgw-rs as `/usr/bin/radosgw`
  and the original beside it, as rgw-go plans and has not yet built.

What is mirrored on purpose: the layer map, the round-trip tables, the
op-per-type pipeline with store traits at the consumer, the dispatch
table with no router, the metadata cache design, the worker set per
phase, the phase order and every gate, the goldens format, the rooket
harness shape, the encoding rule, the exclusions, and the defect
registry.

## 17. Risks and items to verify at implementation

- **rados-rs gaps found late.** Six are known (section 8) and two of
  them, the fresh transaction id on reconnect and the missing write
  mtime, are correctness rather than convenience; anything else the
  driver needs is found by package 8, before the driver depends on it.
- **Fork drift.** Every package of rados-rs is an upstream candidate;
  what upstream declines lives on at a rebase cost per upstream commit,
  and rgw-rs's pin follows the fork's `main`, not upstream's.
- **Encoding versions**, as for rgw-go and rados-rs: a misjudged floor or
  framing decodes nothing from a real cluster, and only the gate on a
  populated cluster catches the ones the corpus lacks; Tentacle
  encodings have no corpus and rely on generated vectors.
- **The Rook suite's image selection**, which needs a retag or a patch
  and is not yet exercised by rgw-go either, whose derived image is not
  built; and the suite itself grows with Rook main, so the acceptance
  target moves.
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
  them; secrets never derive `Debug`; the gateway's own cephx caps stay as
  narrow as radosgw's need, because the control pool's notify channel is
  trusted by every gateway in the zone. These are requirements of phase
  1, and the reason the spike's auth crate is salvaged as an algorithm,
  not as a middleware.

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
  `v19.2.6:src/rgw/rgw_zone_types.h:624-626`.
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
  `v19.2.6:src/rgw/driver/rados/rgw_tools.h:35-36,40-43` (7877, 65521,
  `rgw_shards_max`); `v19.2.6:src/rgw/rgw_lc.cc:238,244-246`;
  `v19.2.6:src/rgw/driver/rados/rgw_rados.cc:120,1626`;
  `v19.2.6:src/rgw/driver/rados/rgw_reshard.cc:29-30,1326`.
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
  `v19.2.6:doc/rados/configuration/ceph-conf.rst:47-60`.
- Unsigned-payload fallback: `v19.2.6:src/rgw/rgw_auth_s3.h:641-662`.
- Corpus: `~/github/ceph/ceph-object-corpus` at 9670a0ef, archives listed
  with `ls archive/*/objects/<type>`.

Rook (`~/github/rook`):

- Which radosgw-admin: `v1.20.7:pkg/operator/ceph/object/admin.go:239-274`
  (Multus `:245-271`, local `:273-274`);
  `v1.20.7:pkg/daemon/ceph/client/command.go:53-55`. Operator image:
  `v1.20.7:images/ceph/Makefile:19` `CEPH_VERSION ?= v20.2.4-20260818`,
  `:22`, `Dockerfile:16`.
- Launch: `dc7829268:pkg/operator/ceph/object/spec.go:446-459`
  (`radosgw`, `--foreground`, frontends, mime types, realm, zonegroup,
  zone), `:474-483` (`--service-unique-id`), `:530-536` (ops log),
  `:556-558` (`--rgw-enable-apis`), `:606-608` (`--host`);
  `pkg/operator/ceph/controller/spec.go:366-380` (daemon flags);
  `pkg/operator/ceph/object/config.go:65-125` (frontend string),
  `:1180-1213`. Same flags at `v1.20.7:pkg/operator/ceph/object/spec.go:440-462`.
- Mon store: `dc7829268:pkg/operator/ceph/object/config.go:236-265`
  (`rgw_run_sync_thread` `"true"` unless `DisableMultisiteSyncTraffic`,
  `rgw_log_nonexistent_bucket`, `rgw_log_object_name_utc`,
  `rgw_enable_usage_log`, `rgw_zone`, `rgw_zonegroup`);
  `pkg/operator/ceph/config/monstore.go:286-345` (`assimilate-conf`);
  same at `v1.20.7:...config.go:256-265,300`.
- Probes: `dc7829268:pkg/operator/ceph/object/spec.go:634-642` (no
  liveness), `:644-680`, `:714-752`, `:683-712`, `:754-775`;
  `pkg/operator/ceph/object/rgw-probe.sh:17,25,43-67`. Health checker
  removed in `a7c0c7ee9`; ready at `controller.go:523-527`.
- Admin ops user: `pkg/operator/ceph/object/admin.go:114,117,484-535`,
  `user.go:113-175`. No per-daemon image: `pkg/apis/ceph.rook.io/v1/types.go:1911-1981,2091-2217`;
  every RGW container uses `c.clusterSpec.CephVersion.Image`
  (`spec.go:362,398,411,446`).
- Suite: `dc7829268:tests/integration/ceph_object_test.go:44-144`
  (entry points `:124-143`, TLS `:87-101`);
  `tests/integration/object/util/sharedstore/sharedstore.go:96-107`
  (zoned and classic), `:175-205` (placements `default` with storage
  class `FOO`, and `bar`), `:259-296` (Service), `:370-374,386-392`;
  `tests/integration/object/util/client/{s3.go:33-34,73,85, admin.go:35-63,
  tls.go:38-45,52-85}`; `tests/scripts/generate-tls-config.sh:45,54`;
  `pkg/operator/ceph/object/objectstore.go:955-973` (`<pool>:<store>`
  namespaces), `:75,1130-1135` (`dashboard-admin`);
  `tests/framework/installer/ceph_installer.go:46-51,96-116`,
  `ceph_manifests.go:170-171`; go-ceph `rgw/admin/radosgw.go:94-95`
  (`UNSIGNED-PAYLOAD`, module cache v0.41.0). CI invocation
  `.github/workflows/ceph-suite-integration-test.yml:133-137`. Chart
  keys `deploy/charts/rook-ceph-cluster/values.yaml:110-116`.

rados-rs (`scratchpad/rados-rs`, `origin/main` = a511eca):

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
  only when WRITE-flagged); `rados-cls/src/otp.rs:18-20`.
- Throttle and retry: `rados/src/osdclient/throttle.rs:12-14,70-107,163-184`;
  `rados/src/osdclient/client.rs:191,233-239,470-505,1891-1892,1911-1968`
  (`:1961-1965` "allocates a fresh tid"), `:2166-2179`;
  `rados/src/osdclient/session.rs:863-868,1198-1215`;
  `rados/src/osdclient/tracker.rs:22-27`.
- Watch: `rados/src/osdclient/watch.rs:26-42,53,361-430,472-487,505-553,
  596-600`; `rados/src/osdclient/ioctx.rs:766,772,791`;
  `rados/src/osdclient/client.rs:33-37,579-595,649-681,999-1036,1147-1151,
  1270-1278`.
- Encoding: `rados/src/denc/codec.rs:53,315,373,408,416,458-547,641-851`;
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
- CI and publish: `.github/workflows/ci.yml:29,55-68,87,89-136`;
  `.github/workflows/test-with-ceph.yml:21-22,57-62,76-129`;
  `docker/docker-compose.ceph.yml:3` (v19.2.2);
  `.github/workflows/publish.yml:76-84` (`rados-denc-macros`, `rados`
  only). rooket instructions: `rados/tests/common/mod.rs:12-59,84-99`.
- Duplicate map keys: rados-rs `design/rgw-mvp`
  `docs/superpowers/plans/2026-09-25-rgw-mvp-16-release-shapes.md:864-883`;
  `docs/superpowers/ceph-upstream-bugs.md:593-597`. The Rook paragraph:
  `docs/superpowers/specs/2026-09-24-rados-rs-rgw-mvp-design.md:92-104`
  (commit 15389c2).

rgw-go (`~/github/rgw-go` at ca998e2, clean tree):

- Packages present: `internal/{denc,meta,acl,radosclient,radosclient/goceph,cls/*}`,
  `test/gate`; absent: `internal/{driver,op,s3,auth}`.
- rooket harness: `hack/rooket/up.sh:22-23,57`; `hack/rooket/lib.sh:8,23,46-54`;
  `hack/rooket/{squid,tentacle}/values/rook-ceph-cluster.yaml:5` (v19.2.6,
  v20.2.4; `dashboard.enabled: false`); `hack/rooket/{squid,tentacle}/config.yaml:4`;
  `hack/rooket/README.md:57-59,96-103`; podman path retired in `fa9ac12`;
  rooket pinned at 9e9ab38e, `.github/workflows/integration.yml:21,32-33,127`.
- Gate: `test/gate/phase0_test.go:93-103,116-139,167-197,199-219,399,
  1319-1470`; `hack/rooket/populate.sh:23-34,43-53,124-135,190-211`.
- Goldens: `hack/goldens/gen.sh:5-8,14-15,39-44`;
  `internal/denc/goldentest/goldentest.go:32-59,62-71,89-94`;
  `hack/goldens/types.txt:1-98`; 3376 triples (count of `.bin`).
- go-ceph pin `go.mod:41`; no Dockerfile; `.goreleaser.yaml:8-12`;
  thread counts `docs/cgo-limitations.md:23-25,33-34,39-41`; registry
  `docs/ceph-upstream-bugs.md` (18 entries; verdicts at `:38,56,73,91,111,
  131,150,170,187,203,218,233,251,279,302`); spec sections 4, 6 to 10, 12
  to 14; plan `:30-38,66-88`.

rgw-rs spike (`~/github/rgw-rs` at 8621b5a, clean tree):

- `Cargo.toml:2-9`; `Cargo.lock` (hyper 1.11.1, axum 0.8.9, tokio 1.53.1,
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
in a values file reaches the chart unchanged.

Unverified in this draft: whether an operation larger than rados-rs's
byte budget is ever admitted (an inference from tokio semaphore
semantics, not a test); whether the fork holds a crates.io token; the
exact behaviour of Rook's S3 test client's checksum middleware.
