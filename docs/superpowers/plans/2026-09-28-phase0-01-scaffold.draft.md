# rgw-rs phase 0, package 1 of 12: `scaffold`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `jhoblitt/rgw-rs` from the research spike into the tree
the design spec builds on: the spike tagged and removed from `main`, its
findings kept under `docs/`, five empty crates with the spec's dependency
edges, `rados` and `rados-cls` pinned to the fork by revision, the
LGPL-2.1-or-later license with a `cargo deny` policy to match, workspace
lints that deny `unwrap`, `expect` and `panic` outside tests, the 1.98
toolchain pinned, CI running the four gates, and `docs/` holding the spec,
the exclusions pointer and an empty defect registry. No behaviour.

**Architecture:** One Cargo workspace, `crates/<name>`, dependencies
pointing downward and enforced by the crate graph (spec section 4):
`rgw-meta` depends on `rados` and nothing else of ours; `rgw-core` on
`rgw-meta` and never on `rgw-driver`; `rgw-driver` on `rgw-core`,
`rgw-meta`, `rados` and `rados-cls`; `rgwd` is the one crate naming both
`rgw-core` and `rgw-driver`; `rgw-tests` has no edges yet (its first
arrive with packages 3, 4 and 11). Each crate is a lib (or `rgwd`, a bin)
whose only content is its crate doc. The `rados` and `rados-cls` edges
are declared now even though nothing calls them, because the edges are
the deliverable and because CI then proves the git pin resolves, the
lockfile locks it, and `cargo deny` sees the whole graph. The rustls
crypto provider decision (spec decision 6) is recorded in `README.md`,
`CLAUDE.md` and mechanically in `deny.toml`, which bans `ring`.

**Tech Stack:** Rust 1.98 (edition 2024, resolver 3), cargo-deny 0.20.2,
GitHub Actions on `ubuntu-latest` with the runner's rustup. No tokio,
hyper, serde or any other third-party crate until the package that first
needs it.

**Spec:** `docs/superpowers/specs/2026-09-27-rgw-rs-design.md` at
231cdd3 (the `design/rgw-rs` branch): section 4 (crate table `:106-112`,
rules from `:114`, gates and lints `:149-157`), section 14 package 1
(`:753-761`), section 15 decisions 1, 2, 6, 7 and 8 (from `:836`, `:851`,
`:914`, `:928` and `:942-945`), section 3's exclusions and registry
bullets (`:92-98`), and section 17's "Build time" item (`:1041-1044`; the
every-PR gate is measured here).

## Global Constraints

- **Branch and worktree.** `EnterWorktree` with name `scaffold` creates
  `~/github/rgw-rs/.claude/worktrees/scaffold` on branch
  `worktree-scaffold` from `origin/main` (8621b5a) and moves the session
  cwd there; `.claude/worktrees/` is already in `.git/info/exclude`
  (`~/github/rgw-rs/.git/info/exclude:7`). Push as `git push origin
  HEAD:scaffold`; the PR is `scaffold` into `main`. `origin` is
  `git@github.com:jhoblitt/rgw-rs.git`; the repository is public, allows
  merge commits, and its ruleset `protect-default-branch` (active, branch
  target) does not touch tags. Any git command whose result is acted on
  runs with the sandbox off (sandboxed git can serve a stale `.git/`).
- **Commits** are Conventional Commits (`.commitlintrc.yml:1-8`), each
  one logical unit that builds, every message ending with exactly
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` and nothing
  after it; no session link. The PR opens as a draft assigned to the
  author, with the body in Task 7 (under 100 words, the Claude Code line
  last).
- **The spike stays addressable and gets no new description.** The
  repository is public: the README line, the tag message, the commit
  messages and the PR body say "insecure research prototype, not for
  use" and nothing about how. `docs/FINDINGS.md` is moved byte-identical
  (`git mv`), never edited. Salvage happens later, by `git show
  spike-2026-09-24:<path>` (Roadmap).
- **No cluster, no plumbing.** Nothing in this package touches a cluster,
  and no make, bash or podman file is added to the repository:
  `scripts/smoke.sh` goes and nothing replaces it. The container recipe
  below is a local practice, not a repository file.
- **Machine facts, verified 2026-09-27.** The host has Fedora's cargo
  1.98.1 (`/usr/bin/cargo`) and no rustup, rustfmt, clippy or cargo-deny
  (`FINDINGS.md:361-363` says the same); `gcc` and `cc` resolve through
  ccache (`/usr/lib64/ccache/gcc`), which is why `.cargo/config.toml`
  sets `CCACHE_DISABLE`. Two images exist locally:
  `localhost/rust-tools:1.98` (cargo 1.98.1, rustfmt 1.9.0, clippy
  0.1.98, no cargo-deny) and `docker.io/library/rust:1.98` (rustup
  1.29.1). fmt and clippy therefore run in the container, as the rados-rs
  plans do (`~/github/rados-rs/.claude/worktrees/design/docs/superpowers/plans/2026-09-25-rgw-mvp-03-cls-crate.md:46-53,1687-1688`),
  and cargo-deny runs from its release binary. actionlint v1.7.7,
  shellcheck 0.11.0 and pinact are on the host.
- **Sandbox facts.** Inside the sandbox `github.com` and
  `objects.githubusercontent.com` are reachable; `crates.io`,
  `api.github.com` and `rustsec.org` are not, and Rust programs cannot
  resolve names at all. So `cargo generate-lockfile`, `cargo fetch` and
  `cargo deny fetch all` run with the sandbox off and
  `CARGO_HOME=$PWD/.cargo-home` (gitignored; the README already documents
  it, `README.md:35-42`); every build, test, lint and `cargo deny --offline
  check` runs sandboxed. `gh` writes and `pinact run` (needs
  `GITHUB_TOKEN=$(gh auth token)`) run with the sandbox off.
- **The container gate**, with `R` the worktree:

  ```sh
  podman run --rm --user "$(id -u):$(id -g)" -v "$R:/src" -v "$R/.cargo-home:/src/.cargo-home" -w /src -e CARGO_HOME=/src/.cargo-home localhost/rust-tools:1.98 cargo fmt --all --check
  podman run --rm -v "$R:/src" -v "$R/.cargo-home:/src/.cargo-home" -w /src -e CARGO_HOME=/src/.cargo-home -e CARGO_TARGET_DIR=/src/target/tools localhost/rust-tools:1.98 cargo clippy --workspace --all-targets --all-features --offline -- -D warnings
  ```

  The image's rustup reads `rust-toolchain.toml` and auto-installs 1.98
  with rustfmt and clippy on first use (observed: "the missing active
  toolchain `1.98-x86_64-unknown-linux-gnu` has been auto-installed").
  `target/tools` keeps the container's artifacts under the gitignored
  `target/`.
- **cargo-deny locally.** Release 0.20.2, the
  `x86_64-unknown-linux-musl` tarball, sha256
  `9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f`
  (the `.sha256` asset beside it says the same), into
  `$TMPDIR/bin/cargo-deny` or `~/.local/bin`. `cargo deny fetch all`
  once, sandbox off, then `cargo deny --offline check`.
- **rados-rs facts** at `origin/main` 0b5d1a21455d270e83a6fc236b78d4603e23826c
  (scratch clone `scratchpad/rados-rs`): workspace version 0.1.4,
  `rust-version = "1.88"`, MIT (`Cargo.toml:12-15`); `Cargo.lock` is
  gitignored (`.gitignore:4`), so our lockfile is the first to lock its
  graph; `rados/build.rs` is a no-op without the `bench-librados`
  feature (`rados/build.rs:3-7,21-24`) and `--all-features` on our
  workspace does not enable a dependency's features; `rados-cls` turns
  every class on by default (`rados-cls/Cargo.toml:15`); `rados` pulls
  `paste` (`rados/Cargo.toml:70`), `hickory-resolver 0.24` (`:89`),
  `zstd` (`:98`) and `lz4` (`:99`). Its CI runs fmt, clippy with `-D
  warnings -D clippy::uninlined_format_args` and test on `stable`
  (`.github/workflows/ci.yml:24,29,55-58,87`), and its unwrap/expect
  rule is prose (`.claude/CLAUDE.md:75,85`).
- **MSRV and toolchain.** 1.98 is the spike's pin (`Cargo.toml:8`,
  `.github/workflows/ci.yml:29-30`) and the host's cargo; rados-rs's MSRV
  is 1.88, so 1.98 is compatible. `rust-toolchain.toml` pins it for
  rustup users and CI; the host's system cargo ignores the file and
  matches anyway.
- **Verified by a dry run** (scratchpad `dryrun/`, host cargo 1.98.1,
  the files exactly as Tasks 4 and 5 write them): 201 packages lock with
  `rados` at the git revision; `cargo build --workspace --all-targets`
  takes 18 s wall and 98 s CPU here (`zstd-sys` and `lz4-sys` compile C;
  nothing else does); the five test binaries run with 0 tests; fmt and
  clippy are clean; an `unwrap()` added outside a test fails clippy with
  `-D clippy::unwrap-used` while `unwrap`, `expect` and `panic!` inside
  a `#[test]` pass; `cargo deny check` reports `advisories ok, bans ok,
  licenses ok, sources ok`. The debug `target/` is 1.6 GB, which is why
  CI caches only the registry and git checkouts.
- **Two advisories are inherited from rados** and are ignored with
  reasons rather than fixed here, because the fix is a rados-rs change:
  RUSTSEC-2026-0119 (`hickory-proto 0.24.4`, fixed in 0.26.1) and
  RUSTSEC-2024-0436 (`paste 1.0.15`, unmaintained). The Roadmap carries
  the fork bump that removes both ignores.

## Review Focus

1. **Dependency direction is the spec's, no more.** `cargo tree
   --workspace --depth 1 --offline` shows exactly: `rgw-meta` → `rados`;
   `rgw-core` → `rgw-meta`; `rgw-driver` → `rgw-core`, `rgw-meta`,
   `rados`, `rados-cls`; `rgwd` → `rgw-core`, `rgw-driver`; `rgw-tests`
   → nothing. No crate names tokio, hyper or serde. A grep for
   `[dependencies]` across `crates/*/Cargo.toml` is the check.
2. **The lints bite outside tests only.** `[workspace.lints.clippy]`
   denies `unwrap_used`, `expect_used`, `panic` (and rados-rs's
   `uninlined_format_args`); every crate carries `[lints] workspace =
   true`; `clippy.toml` sets `allow-unwrap-in-tests`,
   `allow-expect-in-tests` and `allow-panic-in-tests` (the three options
   exist in 1.98's clippy and affect exactly those three lints). The probe
   in Task 4 Step 4 is the proof.
3. **`cargo deny` is a policy, not a pass.** Licenses admitted are the
   LGPL-2.1-or-later-compatible set; `unused-allowed-license = "allow"`
   makes the list the policy rather than an inventory; the two advisory
   ignores name their crate and reason; `ring` is banned; the only git
   source is the fork. OpenSSL-licensed code (`aws-lc-sys` declares `ISC
   AND (Apache-2.0 OR ISC) AND OpenSSL`) is not admitted yet and is an
   owner decision for the frontend package (Roadmap).
4. **Nothing of the spike survives on `main`** except `docs/FINDINGS.md`
   (byte-identical: `git diff spike-2026-09-24:FINDINGS.md
   HEAD:docs/FINDINGS.md` is empty), `LICENSE` (unchanged), the five
   github-conventions-managed workflows and `.commitlintrc.yml`,
   `.github/dependabot.yml`, `.github/tools/breaking-footer/main.go` and
   `.github/codeql/codeql-config.yml`. `git ls-files` shows no
   `crates/rgw-types`, `rgw-sal*`, `rgw-rest*`, `rgw-auth`,
   `rgw-admin-ops`, `radosgw-admin`, and no `scripts/`.
5. **Workflow hygiene** per github-conventions `references/workflows.md`:
   every `uses:` a 40-hex SHA with a `# vX.Y.Z` comment; top-level
   `permissions: contents: read`; the concurrency block keyed on the PR
   number or SHA with `cancel-in-progress: true`; `timeout-minutes` on
   every job; `persist-credentials: false` on every checkout; actionlint
   and `pinact run --check` clean. The managed workflows (`codeql`,
   `commitlint`, `dependency-review`, `scorecard`, `workflow-lint`) are
   untouched.
6. **Public-repository wording.** No commit message, tag message, README
   line, doc or PR body describes a weakness of the spike; the phrase is
   "an insecure research prototype, not for use".
7. **The gate's cost is measured.** The CI run's job durations (fmt,
   clippy, test, deny) are read from the first green run and reported in
   chat, which is where spec section 17's "measured in the scaffold
   package" lands.

## File structure

```
Cargo.toml                          workspace: members, package defaults, git pins, lints
Cargo.lock                          regenerated; 201 packages
rust-toolchain.toml                 channel 1.98, rustfmt and clippy, minimal profile
clippy.toml                         unwrap, expect and panic allowed in tests
deny.toml                           advisories, licenses, bans (ring), sources (the fork)
.cargo/config.toml                  kept: CCACHE_DISABLE, comment retargeted
.gitignore                          target/, .cargo-home/
README.md                           rewritten: description, spike line, layout, gates
CLAUDE.md                           repository facts (new)
LICENSE                             unchanged (LGPL-2.1, identical to rgw-go's)
crates/rgw-meta/{Cargo.toml,src/lib.rs}
crates/rgw-core/{Cargo.toml,src/lib.rs}
crates/rgw-driver/{Cargo.toml,src/lib.rs}
crates/rgwd/{Cargo.toml,src/main.rs}
crates/rgw-tests/{Cargo.toml,src/lib.rs}   publish = false
docs/FINDINGS.md                    moved from the root, byte-identical
docs/exclusions.md                  pointer to rgw-go's canonical list
docs/ceph-upstream-bugs.md          empty registry in rados-rs's entry format
docs/superpowers/specs/2026-09-27-rgw-rs-design.md   cherry-picked from design/rgw-rs
.github/workflows/ci.yml            rewritten: fmt, clippy, test, deny
.github/codeql/codeql-config.yml    kept, comment retargeted
removed: crates/{rgw-types,rgw-sal,rgw-sal-sqlite,rgw-rest,rgw-auth,rgw-rest-s3,rgw-admin-ops,rgw-rest-admin,radosgw-admin}, scripts/smoke.sh
```

## Task index

| # | Task | Depends on |
|---|---|---|
| 0 | Branch and worktree (controller) | none |
| 1 | Tag the spike (controller; the one push outside the PR) | none |
| 2 | Cherry-pick the design spec | 0 |
| 3 | Move the findings under `docs/` | 2 |
| 4 | Replace the spike with the five-crate workspace | 3 |
| 5 | CI gates and the `cargo deny` policy | 4 |
| 6 | The exclusions pointer, the defect registry and `CLAUDE.md` | 5 |
| 7 | Gate, push, draft PR, watch CI, merge (controller) | 1, 6 |

Tasks 2 to 6 are one sequential series on one branch; Task 1 is
independent and can run first or in parallel. Tasks 3 to 6 are one
code-worker's job on the `opus` tier; Tasks 0, 1 and 7 are the
controller's.

---

### Task 0: Branch and worktree (controller)

- [ ] **Step 1: Enter the worktree**

`EnterWorktree` with name `scaffold`. Expected: cwd
`~/github/rgw-rs/.claude/worktrees/scaffold`, branch `worktree-scaffold`
at 8621b5a, `git status` clean. The main clone at `~/github/rgw-rs` is not
touched.

- [ ] **Step 2: Seed the project-local cargo home** (sandbox off, once)

```sh
R="$(pwd)"; export CARGO_HOME="$R/.cargo-home"
```

Nothing to fetch yet; the variable is exported for the worker's shell.
Each Bash call is a fresh shell, so the worker re-exports it per call.

---

### Task 1: Tag the spike (controller)

**Files:** none in the tree. The tag is the salvage source and the README
target; it is pushed before the PR merges so the README link resolves.

- [ ] **Step 1: Tag 8621b5a** (sandbox off)

```sh
git -C ~/github/rgw-rs tag -a spike-2026-09-24 8621b5a -m 'Research spike of 2026-09-24: an insecure research prototype, not for use.

The workspace, findings and smoke script as they stood before the fresh
tree. Salvage by git show spike-2026-09-24:<path>; the findings live on as
docs/FINDINGS.md on main.'
git -C ~/github/rgw-rs push origin spike-2026-09-24
```

Expected: `git ls-remote --tags origin` lists
`refs/tags/spike-2026-09-24`. The name is the date `FINDINGS.md:3` gives
the spike; it cannot be mistaken for a release. The repository had no tags
before this.

---

### Task 2: Cherry-pick the design spec

**Files:**
- Create: `docs/superpowers/specs/2026-09-27-rgw-rs-design.md`

The `design/rgw-rs` branch is two commits on top of `main`, 2bc2099
("docs: rgw-rs design spec, first draft") and 231cdd3 ("docs: rgw-rs spec
takes its review and the owner's decisions"), each touching only the spec
file and each already carrying the `Co-Authored-By: Claude Opus 5.5`
trailer. They are cherry-picked so authorship and the draft-to-review
history survive and the branch still starts at `origin/main`.

- [ ] **Step 1: Cherry-pick both commits** (sandbox off)

```sh
git cherry-pick 2bc2099 231cdd3
git diff design/rgw-rs -- docs   # expected: empty
```

Expected: two commits, `docs/superpowers/specs/2026-09-27-rgw-rs-design.md`
identical to the branch's.

---

### Task 3: Move the findings under `docs/`

**Files:**
- Move: `FINDINGS.md` → `docs/FINDINGS.md` (byte-identical)
- Modify: `README.md:11` (the one reference, `FINDINGS.md` →
  `docs/FINDINGS.md`)

- [ ] **Step 1: Move and re-point**

```sh
git mv FINDINGS.md docs/FINDINGS.md
sed -i '11s|`FINDINGS.md`|`docs/FINDINGS.md`|' README.md
git diff --cached --stat; git diff --stat
git diff HEAD:FINDINGS.md docs/FINDINGS.md   # expected: empty
```

- [ ] **Step 2: Commit**

```
docs: move the spike findings under docs/

FINDINGS.md is the spike's answer to its question and stays as history;
docs/ is where the design spec and the registries live, so it moves there
unchanged.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 4: Replace the spike with the five-crate workspace

**Files:**
- Delete: `crates/radosgw-admin`, `crates/rgw-admin-ops`, `crates/rgw-auth`,
  `crates/rgw-rest`, `crates/rgw-rest-admin`, `crates/rgw-rest-s3`,
  `crates/rgw-sal`, `crates/rgw-sal-sqlite`, `crates/rgw-types`, and the
  contents of `crates/rgwd` and `crates/rgw-tests`; `scripts/smoke.sh`.
- Create: `crates/rgw-meta/{Cargo.toml,src/lib.rs}`,
  `crates/rgw-core/{Cargo.toml,src/lib.rs}`,
  `crates/rgw-driver/{Cargo.toml,src/lib.rs}`,
  `crates/rgwd/{Cargo.toml,src/main.rs}`,
  `crates/rgw-tests/{Cargo.toml,src/lib.rs}`, `rust-toolchain.toml`,
  `clippy.toml`.
- Rewrite: `Cargo.toml`, `Cargo.lock`, `README.md`, `.gitignore`.
- Modify: `.cargo/config.toml` (comment), `.github/codeql/codeql-config.yml`
  (comment).
- Keep untouched: `LICENSE` (LGPL-2.1, `LICENSE:1-2`; github-conventions
  never relicenses, `references/new-repo.md`, "LICENSE"),
  `.commitlintrc.yml`, `.github/dependabot.yml` (its `cargo` entry,
  `:12-18`, keeps working; Dependabot already titles its PRs
  `chore(deps): ...` here, PR #2, so no `commit-message` prefix is
  needed), `.github/tools/breaking-footer/main.go`, the five managed
  workflows.

**Interfaces:** none. Each crate exports nothing; its `//!` doc is the
spec's crate-table row.

- [ ] **Step 1: Remove the spike's code**

```sh
git rm -r -q crates scripts
mkdir -p crates/{rgw-meta,rgw-core,rgw-driver,rgwd,rgw-tests}/src
```

- [ ] **Step 2: Write the workspace files**

`Cargo.toml` (the spike's shape, `Cargo.toml:1-9`, kept: resolver 3,
`crates/*`, version 0.1.0, edition 2024, rust-version 1.98, the license;
`repository` added as rados-rs has it):

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.98"
license = "LGPL-2.1-or-later"
repository = "https://github.com/jhoblitt/rgw-rs"

[workspace.dependencies]
rgw-meta = { path = "crates/rgw-meta" }
rgw-core = { path = "crates/rgw-core" }
rgw-driver = { path = "crates/rgw-driver" }

# rados-rs comes from the fork's main, pinned by revision and bumped by
# hand: Dependabot does not follow git revisions.
rados = { git = "https://github.com/jhoblitt/rados-rs", rev = "0b5d1a21455d270e83a6fc236b78d4603e23826c" }
rados-cls = { git = "https://github.com/jhoblitt/rados-rs", rev = "0b5d1a21455d270e83a6fc236b78d4603e23826c" }

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
uninlined_format_args = "deny"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.98"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

`clippy.toml`:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
```

`.cargo/config.toml` (the spike's `:1-5`, comment retargeted: the C
builds are now `zstd-sys` and `lz4-sys`, which `rados` pulls for msgr2
compression):

```toml
# rados's zstd-sys and lz4-sys shell out to a C compiler. The development
# machine puts ccache first in PATH, and a sandboxed build cannot write
# its cache, so disable it for this workspace.
[env]
CCACHE_DISABLE = "1"
```

`.gitignore` (the spike's `:1-2`; the SQLite entries go with the driver):

```
target/
.cargo-home/
```

`.github/codeql/codeql-config.yml` (the spike's `:1-5`, path unchanged,
comment in the tense the tree now has):

```yaml
# Referenced from .github/workflows/codeql.yml. rgw-tests drives the
# gateway in-process over plain HTTP with throwaway credentials by design;
# code scanning otherwise reports that as cleartext transmission.
paths-ignore:
  - crates/rgw-tests
```

- [ ] **Step 3: Write the five crates**

Each manifest has the spike's shape (`crates/rgwd/Cargo.toml:1-7`) plus
`repository.workspace = true` and `[lints] workspace = true`. Only the
`name`, `description`, `[dependencies]` block and, for `rgw-tests`,
`publish = false` (`crates/rgw-tests/Cargo.toml:8` had it) differ:

```toml
[package]
name = "rgw-meta"
description = "RGW's stored types and their Ceph encodings"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lints]
workspace = true

[dependencies]
rados.workspace = true
```

| Crate | `description` | `[dependencies]` |
|---|---|---|
| `rgw-meta` | RGW's stored types and their Ceph encodings | `rados` |
| `rgw-core` | The gateway's operations, store traits, auth, policy and protocol layers | `rgw-meta` |
| `rgw-driver` | The RADOS store behind rgw-core's traits | `rgw-core`, `rgw-meta`, `rados`, `rados-cls` |
| `rgwd` | The RADOS Gateway daemon | `rgw-core`, `rgw-driver` |
| `rgw-tests` | Integration and cluster tests, the golden generator, the populator and the gates (`publish = false`) | none |

`crates/rgw-meta/src/lib.rs`:

```rust
//! RGW's stored types with their Ceph encodings over `rados::denc`:
//! identity primitives, user and account, bucket entry point, instance
//! and layout, zone parameters, zonegroup, realm and period, manifest,
//! compression info, ACL policy and the cache-notify record. Encodes at
//! the cluster's release and decodes every version. Depends on `rados`
//! and on nothing else of this workspace.
```

`crates/rgw-core/src/lib.rs`:

```rust
//! The gateway's protocol-neutral core: one type per operation; the store
//! traits those operations consume, declared here at their consumer, one
//! per concern, with in-memory fakes and the conformance suite; auth, the
//! policy language, ACL and quota evaluation; the S3, admin, IAM and STS
//! protocol layers; the dispatch table; the error documents. Depends on
//! `rgw-meta` and never on `rgw-driver`.
```

`crates/rgw-driver/src/lib.rs`:

```rust
//! The RADOS store: `rgw-core`'s store traits implemented over `rados`
//! and `rados-cls`. The configuration bridge and release detection, zone
//! and placement resolution, object naming, manifests and striping,
//! atomic head writes, the index protocol, GC enqueue, the metadata cache
//! with watch and notify, quotas and usage, and the workers. Depends on
//! `rgw-core` and `rgw-meta`; nothing below it names it.
```

`crates/rgwd/src/main.rs`:

```rust
//! The gateway binary: radosgw's argv handling, the beast-spec frontend
//! on hyper with TLS, deadlines and drain, mgr registration and perf
//! reports, metrics, the admin socket, the ops log and build info. The one
//! crate that names both `rgw-core` and `rgw-driver`.

fn main() {}
```

`crates/rgw-tests/src/lib.rs`:

```rust
//! Integration and cluster tests, the golden generator, the populator,
//! the corpus harness and the phase gates. Not published.
```

- [ ] **Step 4: Lock, build, test, lint**

```sh
export CARGO_HOME="$PWD/.cargo-home"
rm -f Cargo.lock
cargo generate-lockfile && cargo fetch            # sandbox off: crates.io and the fork
cargo build --workspace --all-targets --offline   # sandboxed from here on
cargo test --workspace --all-targets --offline --locked
cargo tree --workspace --depth 1 --offline
```

Expected: "Locking 196 packages" (201 entries in `Cargo.lock`, `rados
0.1.4` with source
`git+https://github.com/jhoblitt/rados-rs?rev=0b5d1a2...#0b5d1a2...`);
five test binaries, `0 passed; 0 failed` each; the tree shows exactly the
edges in Review Focus 1. Then the container gate from Global Constraints:
fmt clean, clippy clean.

The lint probe, once, then reverted: append to `crates/rgw-meta/src/lib.rs`

```rust
pub fn probe() -> u8 {
    "1".parse::<u8>().unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn in_tests_unwrap_expect_and_panic_are_allowed() {
        assert_eq!("1".parse::<u8>().unwrap(), 1);
        let _ = "2".parse::<u8>().expect("parses");
        if false {
            panic!("never");
        }
    }
}
```

and run the container clippy. Expected: exactly one error, `used
unwrap()` at the `probe` line, `-D clippy::unwrap-used`, and none from the
test module. Restore the file (`git checkout crates/rgw-meta/src/lib.rs`)
and confirm clippy is clean again.

- [ ] **Step 5: Rewrite `README.md`**

github-conventions' skeleton (`templates/README.md`; rgw-go's
`README.md:1-39` is the rendered model), with the spike line and the
layout table. The two badges are the spike's (`README.md:3-4`).

```markdown
# rgw-rs

[![ci](https://github.com/jhoblitt/rgw-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/jhoblitt/rgw-rs/actions/workflows/ci.yml)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/jhoblitt/rgw-rs/badge)](https://scorecard.dev/viewer/?uri=github.com/jhoblitt/rgw-rs)

rgw-rs is a reimplementation of Ceph's RADOS Gateway in Rust over
[rados-rs](https://github.com/jhoblitt/rados-rs), a pure-Rust RADOS
client, with no librados and no foreign-function boundary in the process.
Its goal is [rgw-go](https://github.com/jhoblitt/rgw-go)'s: a drop-in
replacement for C++ radosgw in a Rook cluster running Squid or Tentacle,
bit-compatible with radosgw's RADOS layout, able to share a zone with
radosgw during a rolling replacement or rollback, and administered by the
unmodified radosgw-admin. The design is in
[docs/superpowers/specs/2026-09-27-rgw-rs-design.md](docs/superpowers/specs/2026-09-27-rgw-rs-design.md);
what is out of scope, and why, is rgw-go's
[docs/exclusions.md](https://github.com/jhoblitt/rgw-go/blob/main/docs/exclusions.md),
which [docs/exclusions.md](docs/exclusions.md) here points at.

The research spike that preceded this tree is tagged
[`spike-2026-09-24`](https://github.com/jhoblitt/rgw-rs/tree/spike-2026-09-24).
It is an insecure research prototype, not for use; its findings are
[docs/FINDINGS.md](docs/FINDINGS.md).

## Layout

One Cargo workspace. Dependencies point downward, and the crate graph
enforces the direction.

| Crate | Holds | Depends on |
|---|---|---|
| `rgw-meta` | RGW's stored types and their Ceph encodings | `rados` |
| `rgw-core` | the operations, the store traits they consume, auth, policy, the protocol layers, dispatch and error documents | `rgw-meta` |
| `rgw-driver` | the RADOS store behind `rgw-core`'s traits, and the workers | `rgw-core`, `rgw-meta`, `rados`, `rados-cls` |
| `rgwd` | the daemon | `rgw-core`, `rgw-driver` |
| `rgw-tests` | integration and cluster tests, the golden generator, the populator and the gates; not published | none yet |

## Install

Nothing to install yet. rgw-rs is in its foundations phase.

## Usage

Not yet usable. The first runnable milestone is the phase 1 gateway
described in the design spec.

## Development

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/);
commitlint enforces this on every pull request. Every GitHub Action is pinned
to a commit SHA: run `pinact run` after editing a workflow and `actionlint`
before committing it.

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it, and a
system cargo of the same minor works too. The gates CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
cargo deny check
```

`rados` and `rados-cls` are git dependencies on the rados-rs fork's `main`,
pinned by revision in `Cargo.toml` and bumped by hand. When the frontend
lands, TLS is rustls on the aws-lc-rs provider; `deny.toml` keeps `ring`
out of the graph from the start. Inside a sandbox whose `~/.cargo` is
read-only, `export CARGO_HOME="$PWD/.cargo-home"` keeps the registry
project-local.

## License

[LGPL-2.1-or-later](LICENSE)
```

- [ ] **Step 6: Commit**

`git add -A` (the lockfile, the new crates, the deletions), then:

```
feat!: replace the spike with the five-crate workspace

The spike answered its question and is tagged spike-2026-09-24; its
eleven crates, CLI and smoke script leave main. In their place, the
workspace the design decided: rgw-meta, rgw-core, rgw-driver, rgwd and
rgw-tests as empty shells with their dependency edges, rados and rados-cls
as git dependencies pinned to the fork's main by revision, workspace lints
denying unwrap, expect and panic outside tests, and the 1.98 toolchain
pinned. The README names the tag an insecure research prototype, not for
use.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 5: CI gates and the `cargo deny` policy

**Files:**
- Rewrite: `.github/workflows/ci.yml` (the spike's, `:1-34`, is not a
  managed template; its rustup rationale, `:25-26`, is kept).
- Create: `deny.toml`.

- [ ] **Step 1: Write `deny.toml`**

```toml
# cargo deny policy: advisories, licenses compatible with LGPL-2.1-or-later,
# and the one git source the workspace may draw from.

[graph]
all-features = true

[advisories]
ignore = [
    # Both come through rados; the fork's dependency bumps remove them.
    { id = "RUSTSEC-2026-0119", reason = "hickory-proto 0.24: rados encodes only its own SRV queries" },
    { id = "RUSTSEC-2024-0436", reason = "paste is unmaintained; a proc-macro helper of rados" },
]

[licenses]
# Licenses compatible with LGPL-2.1-or-later. Not every entry is in the
# graph yet; the list is the policy, so a new dependency under one of
# them needs no decision.
allow = [
    "LGPL-2.1-or-later",
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "MPL-2.0",
    "CC0-1.0",
    "0BSD",
    "BSL-1.0",
    "Unlicense",
]
unused-allowed-license = "allow"

[licenses.private]
ignore = true

[bans]
multiple-versions = "warn"
deny = [
    # rustls runs on aws-lc-rs (spec decision 6); a second provider in the
    # graph would make every test binary install a default by hand.
    { crate = "ring", reason = "rustls's crypto provider is aws-lc-rs" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-git = ["https://github.com/jhoblitt/rados-rs"]
```

What the dry run established about this file: the graph today carries
0BSD, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause,
BSD-3-Clause, MIT, Unicode-3.0, Unlicense, Zlib and our own
LGPL-2.1-or-later; ISC, MPL-2.0, CC0-1.0 and BSL-1.0 are admitted ahead of
need (hyper, rustls and webpki-roots bring them). `wildcards = "deny"`
is deliberately absent: cargo-deny counts the workspace's own `path`
dependencies as wildcards. The `ring` ban is why decision 6 is
mechanical: the spike's lockfile carried both providers (`Cargo.lock` at
8621b5a: `ring 0.17.14` through `rustls-webpki 0.103.15`, `aws-lc-rs
1.18.1` through `rustls 0.23.45`), which is the two-provider trap the
ban closes. `multiple-versions = "warn"` reports `getrandom`, `hashbrown`
and `syn` duplicates from rados-rs's graph without failing.

- [ ] **Step 2: Write `.github/workflows/ci.yml`**

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.sha }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  fmt:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false

      # The runner ships rustup, which installs the toolchain named in
      # rust-toolchain.toml when given no argument; a third-party toolchain
      # action's refs are moving branches that cannot be SHA-pinned.
      - name: Install the pinned toolchain
        run: rustup toolchain install

      - name: cargo fmt
        run: cargo fmt --all --check

  clippy:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false

      - name: Install the pinned toolchain
        run: rustup toolchain install

      - name: Cache the cargo registry and git checkouts
        uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
          key: ${{ runner.os }}-cargo-${{ hashFiles('Cargo.lock', 'rust-toolchain.toml') }}

      - name: cargo clippy
        run: cargo clippy --workspace --all-targets --all-features -- -D warnings

  test:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false

      - name: Install the pinned toolchain
        run: rustup toolchain install

      - name: Cache the cargo registry and git checkouts
        uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
          key: ${{ runner.os }}-cargo-${{ hashFiles('Cargo.lock', 'rust-toolchain.toml') }}

      - name: cargo test
        run: cargo test --workspace --all-targets --locked

  deny:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    env:
      CARGO_DENY_VERSION: "0.20.2"
      CARGO_DENY_SHA256: "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false

      - name: Install the pinned toolchain
        run: rustup toolchain install

      # A release binary, checksummed, rather than a Docker-built action:
      # cargo-deny's own action rebuilds its image on every run.
      - name: Install cargo-deny
        run: |
          archive="cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl"
          curl -fsSLo cargo-deny.tar.gz "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/${archive}.tar.gz"
          echo "${CARGO_DENY_SHA256}  cargo-deny.tar.gz" | sha256sum -c -
          tar -xzf cargo-deny.tar.gz --strip-components=1 -C "$HOME/.cargo/bin" "${archive}/cargo-deny"

      - name: cargo deny
        run: cargo deny check
```

Why this shape: the four jobs mirror rados-rs's split (fmt, clippy, test
as separate jobs, `.github/workflows/ci.yml:16,31,70`) plus `deny`; the
runner's rustup (1.28.0 and later) installs the active toolchain, with the
file's components, on a bare `rustup toolchain install` (verified on
rustup 1.29.1 in the `rust:1.98` image: "the active toolchain
`1.98-x86_64-unknown-linux-gnu` has been installed ... overridden by
'/src/rust-toolchain.toml'"); `target/` is not cached (1.6 GB debug);
`EmbarkStudios/cargo-deny-action` v2.1.1 is a `runs: using: docker,
image: Dockerfile` action, so the pinned tarball with its published
sha256 is both faster and exact; the extraction form was exercised
locally. Every hygiene item of github-conventions `references/workflows.md`
is present: pins with version comments, top-level `contents: read`, the
concurrency group, `timeout-minutes`, `persist-credentials: false`.

- [ ] **Step 3: Lint the workflow, check the policy** (sandbox off for
  pinact, cargo-deny's fetch and nothing else)

```sh
actionlint -color .github/workflows/ci.yml
GITHUB_TOKEN=$(gh auth token) pinact run --check --verify .github/workflows/ci.yml
export CARGO_HOME="$PWD/.cargo-home"
cargo deny fetch all                    # sandbox off, once
cargo deny --offline check              # sandboxed
```

Expected: actionlint silent; pinact reports no rewrite (the two SHAs
above are what pinact resolved on 2026-09-27; if `actions/cache` has
moved on, take pinact's rewrite and its comment); `advisories ok, bans ok,
licenses ok, sources ok` with only the three `duplicate` warnings.

- [ ] **Step 4: Commit**

```
ci: run fmt, clippy, test and cargo deny on every push

The gates are the spec's: cargo fmt --all --check, cargo clippy
--workspace --all-targets --all-features -- -D warnings, cargo test
--workspace --all-targets --locked, and cargo deny check. The policy
admits the licenses compatible with LGPL-2.1-or-later, allows only the
rados-rs fork as a git source, bans ring so rustls has one crypto
provider, and ignores with reasons the two advisories that come through
rados until the fork bumps hickory-resolver and replaces paste. The
toolchain comes from rust-toolchain.toml through the runner's rustup;
cargo-deny is its release binary, checksummed.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 6: The exclusions pointer, the defect registry and `CLAUDE.md`

**Files:**
- Create: `docs/exclusions.md`, `docs/ceph-upstream-bugs.md`, `CLAUDE.md`.

- [ ] **Step 1: `docs/exclusions.md`**

rgw-go's file says it is canonical for rgw-rs and that rgw-rs keeps no
list of its own (`~/github/rgw-go/docs/exclusions.md:11-13`); the spec
repeats it (`:92-93`). The pointer targets `main`, not a commit, because
it names a living canon.

```markdown
# Excluded features

rgw-rs keeps no feature list of its own. What is excluded, the coexistence
obligations and the benchmark parity settings are decided and recorded in
rgw-go's `docs/exclusions.md`, canonical for both projects by owner
decision of 2026-09-25:

https://github.com/jhoblitt/rgw-go/blob/main/docs/exclusions.md

A material change there is announced to the rgw-rs session when it is
made. Section 1 of the design spec records the one update that document
needs at Rook main.
```

- [ ] **Step 2: `docs/ceph-upstream-bugs.md`**

rados-rs's entry format (`docs/superpowers/ceph-upstream-bugs.md:1-27` on
the fork's `design/rgw-mvp` branch; it is not on `main`), with the spec's
rule that client- and class-level defects stay there and the registries
cross-reference each other and rgw-go's (`:94-98`). IDs take their own
prefix so a cross-reference never collides with rados-rs's `CEPH-BUG-NNN`.

```markdown
# Upstream Ceph bug registry

Defects in ceph/ceph that rgw-rs meets as a gateway that must coexist with
radosgw: in radosgw, the RGW object classes, the monitors, the mgr, or the
OSD behaviour they rely on. The entry format is rados-rs's
(`docs/superpowers/ceph-upstream-bugs.md` on the fork's `design/rgw-mvp`
branch): IDs are stable, entries are updated in place, and every claim is
verified against the source or a cluster, never taken from a report.
Client- and class-level defects stay in rados-rs's registry; the two
cross-reference each other and rgw-go's `docs/ceph-upstream-bugs.md`. Add
an entry whenever a task, review or benchmark meets one.

Each entry, `## RGW-BUG-NNN: <title>`, records:

- **Component**, and **Status**: `confirmed` when the evidence below was
  checked, `suspected` with what is still to be checked.
- **Symptom**, as the gateway or its clients see it.
- **Affected** and **Fixed in**, the latter with release and commit.
- **Evidence**: `tag:path:line`, and how it was established (source
  reading, `ceph-dencoder`, or a named cluster test).
- **rgw-rs**: whether it reproduces the behaviour, works around it or is
  unaffected, naming the commit or PR.
- **Found**: who found it, and when.
- **Upstream**: report status; `not filed` unless a tracker or PR exists.
- **See also**: the matching entries in rados-rs's and rgw-go's registries.

No entries yet.
```

- [ ] **Step 3: `CLAUDE.md`**

The repository facts, on rgw-go's model (`~/github/rgw-go/CLAUDE.md:14-39`;
its Task 1 wrote them in Step 6, plan `:210-231`). Rust has no
conventions plugin, so there is no pointer block.

```markdown
# rgw-rs

Design: docs/superpowers/specs/2026-09-27-rgw-rs-design.md. Plans:
docs/superpowers/plans/. Scope is rgw-go's docs/exclusions.md, which
docs/exclusions.md points at; rgw-rs keeps no list of its own.

## rgw-rs facts

- `rados` and `rados-cls` come from the jhoblitt/rados-rs fork's `main` as
  git dependencies pinned by revision in `Cargo.toml`. Dependabot does not
  follow git revisions, so the pin is bumped by hand, and `deny.toml`
  admits no other git source.
- The gates are `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `cargo test --workspace
  --all-targets --locked` and `cargo deny check`. Workspace lints deny
  `unwrap`, `expect` and `panic` outside tests; `clippy.toml` allows them
  in tests.
- On this machine the system cargo is 1.98 without rustfmt, clippy or
  cargo-deny. fmt and clippy run in the `localhost/rust-tools:1.98`
  container with the worktree and its `.cargo-home` mounted (the recipe
  is in the plans); cargo-deny runs from its release binary. Inside the
  sandbox crates.io is unreachable: fetch with
  `CARGO_HOME=$PWD/.cargo-home` and the sandbox off, then build, test and
  check `--offline`.
- TLS is rustls on the aws-lc-rs provider (spec decision 6); `deny.toml`
  bans `ring`.
- Never use the ambient kubectl or Ceph cluster. Cluster tests, from
  package 3 on, use disposable rooket clusters driven from `rgw-tests`.
- `docs/ceph-upstream-bugs.md` is the registry of upstream ceph/ceph
  defects rgw-rs meets, in rados-rs's entry format; add or update an entry
  whenever work meets one, and cross-reference rados-rs's and rgw-go's.
- The tag `spike-2026-09-24` is the research spike, an insecure research
  prototype. Salvage from it with `git show spike-2026-09-24:<path>`;
  never revive its crates.
- Implementation tasks go to Opus 5.5 code-workers in worktrees; judgment
  stays on the session model.
```

- [ ] **Step 4: Commit**

```
docs: add the exclusions pointer, the defect registry and CLAUDE.md

rgw-go's docs/exclusions.md is canonical for both projects, so rgw-rs
carries a pointer rather than a list. The registry of upstream Ceph
defects starts empty in rados-rs's entry format with its own ID prefix,
and CLAUDE.md records the repository facts a session needs: the fork
pin, the gates, how they run on this machine, and the spike tag.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 7: Gate, push, draft PR, watch CI, merge (controller)

- [ ] **Step 1: Per-commit proof** (sandbox off for git)

```sh
git log --oneline 8621b5a..HEAD          # expected: five commits in Task order (two spec, docs move, feat!, ci, docs)
git rebase -q "$(git log --format=%h --grep='move the spike findings' -1)" --exec 'CARGO_HOME=$PWD/.cargo-home cargo check --workspace --all-targets --offline -q'
git diff spike-2026-09-24:FINDINGS.md HEAD:docs/FINDINGS.md   # expected: empty
git ls-files crates scripts | sed 's|/.*||' | sort -u          # expected: only crates
```

Then the container gate (fmt, clippy), `cargo test --workspace
--all-targets --offline --locked`, and `cargo deny --offline check` on
the tip. Any reflow or fix is folded into the commit that owns the lines
(`git commit --fixup` and `rebase --autosquash`), and `git diff <old>
<new>` is shown empty before the push.

- [ ] **Step 2: Push, open the draft PR, start the watcher** (sandbox off)

```sh
git push origin HEAD:scaffold
gh pr create --draft --assignee @me --base main --head scaffold --title 'feat!: replace the spike with the five-crate workspace' --body-file "$TMPDIR/pr-body.md"
```

The body (`wc -w` under 100 across the first two paragraphs; the Claude
Code line last):

```
**Motivation.** The spike answered its question (docs/FINDINGS.md) and its shapes are not the design's; the spec (section 15, decisions 1, 2, 7 and 8) calls for a fresh tree.

**What changed.** The spike is tagged `spike-2026-09-24` and leaves `main`. The workspace is five empty crates with the spec's dependency edges, `rados` and `rados-cls` pinned to the fork by revision, LGPL-2.1-or-later with a matching `cargo deny` policy, workspace lints denying unwrap, expect and panic outside tests, a 1.98 toolchain pin, and CI running fmt, clippy, test and deny. `docs/` gains the spec, the exclusions pointer and an empty defect registry.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

In the same turn, one background watcher polling `gh api
repos/jhoblitt/rgw-rs/actions/runs?head_sha=<sha>` every three minutes
for `ci`, `commitlint`, `workflow-lint`, `codeql` and
`dependency-review`; never `gh run watch`. A failure the PR plausibly
caused is diagnosed and fixed with a commit folded into its owner; a
plausible flake is retried up to three times.

- [ ] **Step 3: Read the cost, merge on green**

From the first green `ci` run, the durations of `fmt`, `clippy`, `test`
and `deny` go into the chat report (spec section 17, "Build time"). Merge
with a merge commit under the owner's standing authorization; then
`ExitWorktree`.

---

## Roadmap for later packages

Phase 0 continues as spec section 14 lists it (from `:762`): 2, `denc`
open-version decode in the fork; 3, `rgw-tests` golden generator and
rooket configuration; 4 to 8, `rgw-meta` primitives, user and account,
bucket types, zone types, object attributes; 9, the fork's gap packages
scheduled to their first user; 10, `rgw-driver` connection and release;
11, the populator and the gate; 12, the integration workflow.

**Salvage from the tag**, by the package that needs each piece (decision
1 names the pieces; the tag is the only source, `git show
spike-2026-09-24:<path>`; nothing is copied in this package):

| Piece | In the spike | Goes to | With the package that |
|---|---|---|---|
| SigV4 core and the AWS vectors, the chunked and trailer readers | `crates/rgw-auth/src/sigv4.rs` (vectors `:353-439`, tests `:322-481`), `chunked.rs` | `rgw-core` auth, as an algorithm and never as a middleware (spec section 17, `:1045` onward) | opens phase 1's auth: SigV4 header, query and presigned |
| Error table's shape: the enum is the S3 code, status and wire name hang off it | `crates/rgw-types/src/error.rs:1-6,10,71,105` | `rgw-core` error documents | first renders an error document (phase 1's dispatch package) |
| XML structs and the whitespace-preserving multi-delete parser | `crates/rgw-rest-s3/src/xml.rs:31-165,167-274` | `rgw-core` S3 protocol layer | serves ListBuckets, ListObjects and DeleteObjects in phase 1 |
| Range parsing | `crates/rgw-rest-s3/src/range.rs:10-52` (tests `:56-80`) | `rgw-core` | serves GetObject with ranges |
| Bucket-name validation (`valid_s3_bucket_name`, strict) | `crates/rgw-rest-s3/src/bucket.rs:22-54` | `rgw-core` | serves CreateBucket |
| `encoding-type=url` character set | `crates/rgw-rest-s3/src/list.rs:42-74` | `rgw-core` | serves ListObjects |
| In-process AWS SDK harness (`TestServer`, signed admin requests) | `crates/rgw-tests/src/lib.rs:34-190` | `rgw-tests`, rewritten over hyper (the spike's `rgwd/src/lib.rs:24-32` is an axum router; only its idea, the app as a library the tests host, transfers) | first serves a request over the hyper frontend |
| Conformance suite idea | `crates/rgw-sal/src/testsuite.rs:1-6,17-23` | `rgw-core` store traits, one suite per trait | declares the first store trait |

**Open items this package leaves**, each for the package named:

- The two `cargo deny` ignores go when the fork bumps `hickory-resolver`
  to a release on `hickory-proto` 0.26.1 or later and replaces `paste`
  (a rados-rs PR; package 9's list is where fork work is scheduled).
- The `OpenSSL` license component of `aws-lc-sys` needs an owner decision
  before `deny.toml` admits it; it arrives with the rustls frontend in
  phase 1. So does the SDK's feature set in `rgw-tests`, which must not
  enable `ring` (the ban).
- `rgw-tests` gains its edges (`rgw-meta` in package 3, `rgw-driver` in
  package 11, `rgwd` in phase 1) and its binaries; its `paths-ignore` in
  the CodeQL config is already in place.
- Dependabot's `cargo` entry will open bumps for crates.io dependencies
  as they arrive; the fork pin is bumped by hand and its bump commit
  states the fork range it takes.

## Open questions for the owner

1. **The spec's landing.** This plan cherry-picks `design/rgw-rs`'s two
   commits into the scaffold PR (Task 2), which is what package 1's "docs/
   with this spec" reads as. If the spec should instead land by its own
   PR first, Task 2 becomes "rebase onto `main` after it merges".
2. **The two advisory ignores.** Ignore now with reasons (as written) and
   bump the fork afterwards, or land the fork's `hickory-resolver` bump
   and `paste` replacement first so the scaffold's `deny.toml` carries
   no ignore.
3. **The `ring` ban as decision 6's record.** The plan makes the
   provider choice mechanical (`deny.toml` bans `ring`) and names
   `aws-lc-rs` because the test-only AWS SDK already enables it and a
   second provider forces every test binary to install a default. If the
   owner prefers `ring` (no cmake in the build), the ban flips to
   `aws-lc-rs` and the SDK's client features are chosen to match.
4. **The registry's ID prefix.** `RGW-BUG-NNN` is proposed so
   cross-references cannot collide with rados-rs's `CEPH-BUG-NNN`.
5. **The toolchain claim in the brief.** The brief said "mirror
   rados-rs's 1.98"; rados-rs's MSRV is 1.88 (`Cargo.toml:14`) and its CI
   runs `stable`. 1.98 is the spike's pin and the host's cargo, and the
   plan keeps it; nothing changes unless the owner wanted rados-rs's
   1.88 as the MSRV, in which case `rust-version` drops to 1.88 while the
   toolchain pin stays 1.98.
