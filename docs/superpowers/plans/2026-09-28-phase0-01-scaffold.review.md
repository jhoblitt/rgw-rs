## Verdict: adopt with edits

The draft's structure holds up. The series, the tag, the container gate, the checksummed cargo-deny tarball and the crate edges are all sound. Owner decisions 1, 3, 4 and 6 change a large part of the text, and those edits are in section B. Beyond the decisions there is one high-severity problem: `dependency-review` will fail on this PR, so "merge on green" cannot be met as the plan stands. I found no rework-level problem.

PR #5 has already merged: 2026-09-28T17:36Z, main is now `8c203d1c089b09a734da836c018ecb3de00ee5a1`. Fork main is `052c99f6b0510f1c837c6090174315b649a7c058`, and it differs from 0b5d1a2 only in `rados/src/osdclient` plus one test. No manifest changed, so the dependency graph is the same: 196 packages locked, 201 entries.

---

## A. Findings, most severe first

**1. HIGH: `dependency-review` will fail on this PR.**
- The scaffold adds two crates through `rados` that GitHub advisories flag. Neither is in main's lockfile.
  - GHSA-q2qq-hmj6-3wpp, hickory-proto 0.24.4, severity medium. This is the same advisory as RUSTSEC-2026-0119.
  - GHSA-rhfx-m35p-ff5j, lru 0.12.5, severity low. This is RUSTSEC-2026-0002, which RustSec marks "unsound"; cargo-deny does not report that for a transitive crate.
- I queried `/advisories?ecosystem=rust&affects=<every registry crate@version in the lockfile>` and got exactly these two.
- dependency-review-action v5 fails at severity `low` by default. The managed workflow has no allow-list, and `deny.toml` ignores do not reach it. PR #2's run log shows the action does read `Cargo.lock`.
- The ruleset (`deletion`, `non_fast_forward`) has no required checks, so the red check does not block the merge mechanically.
- Replace the Task 7 Step 2 watcher paragraph with:
  > In the same turn, one background watcher polling `gh api repos/jhoblitt/rgw-rs/actions/runs?head_sha=<sha>` every three minutes for `ci`, `commitlint`, `workflow-lint`, `codeql` and `dependency-review`; never `gh run watch`. `dependency-review` is expected red on exactly two advisories that arrive through `rados`: GHSA-q2qq-hmj6-3wpp (hickory-proto 0.24.4, the RUSTSEC-2026-0119 that `deny.toml` ignores) and GHSA-rhfx-m35p-ff5j (lru 0.12.5, RUSTSEC-2026-0002, unsound, which cargo-deny does not report for a transitive crate). Any other failure the PR plausibly caused is diagnosed and fixed with a commit folded into its owner; a plausible flake is retried up to three times.
- Replace Task 7 Step 3's last two sentences with:
  > Merge with a merge commit once every other check is green and the owner has accepted the `dependency-review` red, which the standing merge-on-green authorization does not cover; then `ExitWorktree`.
- Alternative: add `with: allow-ghsas: GHSA-q2qq-hmj6-3wpp, GHSA-rhfx-m35p-ff5j` to the dependency-review step. That edits a workflow github-conventions manages, and `github-converge` would rewrite it. I recommend the owner-OK path.
- Either way, add lru 0.16.3 or later to the queued rados-rs bump (see B, D2).

**2. MEDIUM: Task 4 Step 4's lint probe cannot be restored, and it proves only one of the three lints.**
- `git checkout crates/rgw-meta/src/lib.rs` fails because the file is untracked until Step 6's `git add -A`. The planner's dry run used a backup copy (`scratchpad/lib.rs.bak`), not the command the plan gives.
- The probe puts only `unwrap` outside tests.
- I verified in `localhost/rust-tools:1.98` with clippy 0.1.98:
  - `expect` and `panic!` outside tests are also denied.
  - A non-`#[test]` helper inside `#[cfg(test)] mod tests` is allowed.
  - A helper outside a `#[test]` fn in `tests/*.rs` IS denied.
- Replace from "The lint probe, once, then reverted" through "confirm clippy is clean again." with:

  > The lint probe, once, in place: `cp crates/rgw-meta/src/lib.rs "$TMPDIR/rgw-meta-lib.rs"`, then append
  > ```rust
  > pub fn probe_unwrap() -> u8 {
  >     "1".parse::<u8>().unwrap()
  > }
  >
  > pub fn probe_expect() -> u8 {
  >     "1".parse::<u8>().expect("parses")
  > }
  >
  > pub fn probe_panic(x: u8) -> u8 {
  >     if x == 0 {
  >         panic!("zero");
  >     }
  >     x
  > }
  >
  > #[cfg(test)]
  > mod tests {
  >     fn helper() -> u8 {
  >         "3".parse::<u8>().unwrap()
  >     }
  >
  >     #[test]
  >     fn in_tests_unwrap_expect_and_panic_are_allowed() {
  >         assert_eq!("1".parse::<u8>().unwrap(), 1);
  >         let _ = "2".parse::<u8>().expect("parses");
  >         assert_eq!(helper(), 3);
  >         if false {
  >             panic!("never");
  >         }
  >     }
  > }
  > ```
  > and run the container clippy. Expected: exactly three errors, "used `unwrap()` on a `Result` value", "used `expect()` on a `Result` value" and "`panic` should not be present in production code", at the three probe functions, and none from the test module, its non-test helper included. Restore with `cp "$TMPDIR/rgw-meta-lib.rs" crates/rgw-meta/src/lib.rs` (the file is untracked until Step 6, so `git checkout` cannot restore it) and confirm clippy is clean again.

- In Review Focus 2, replace "The probe in Task 4 Step 4 is the proof." with:
  > The probe in Task 4 Step 4 is the proof. The allowance covers `#[test]` functions and `#[cfg(test)]` items only: a helper in `tests/*.rs` outside a `#[test]` function, and `rgw-tests`'s library and binaries, are not tests to clippy (CLAUDE.md records this).

**3. MEDIUM: the README claims "no foreign-function boundary in the process", which is false against this graph.**
- `rados` pulls `zstd-sys` and `lz4-sys`, which are C code reached through FFI. The plan's own `.cargo/config.toml` comment says so, and ring adds C and assembly later.
- Replace README's first sentence with:
  > rgw-rs is a reimplementation of Ceph's RADOS Gateway in Rust over [rados-rs](https://github.com/jhoblitt/rados-rs), a pure-Rust RADOS client, with no librados or libceph-common in the process.
- The spec's section 1 (`:21-22`) makes the same overstatement. Flag it for a spec fix; it can join the D4 spec commit.

**4. MEDIUM: the license allow-list is broader than needed, and its rationale is false.**
- MPL-2.0, CC0-1.0 and BSL-1.0 appear in no graph. The draft says "hyper, rustls and webpki-roots bring them"; that is wrong:
  - hyper is MIT.
  - rustls is `Apache-2.0 OR ISC OR MIT`.
  - webpki-roots is not in the graph, because the SDK uses rustls-native-certs.
- 0BSD, BSD-2-Clause, Unlicense and Apache-2.0 WITH LLVM-exception appear only as one arm of an OR expression that the other licenses already satisfy.
- I verified a seven-license list (LGPL-2.1-or-later, Apache-2.0, BSD-3-Clause, ISC, MIT, Unicode-3.0, Zlib) in two probes:
  - the scaffold graph at 052c99f;
  - a probe of section 4's complete dependency list on ring, dev-dependencies included. That covers rustls and tokio-rustls on ring, hyper, the test-only aws-sdk-s3, aws-sigv4, reqwest and tempfile.
- ISC is needed only by ring (`Apache-2.0 AND ISC`), rustls-webpki (`ISC`) and untrusted (`ISC`).
- cargo-deny leaves dev-dependencies out of the license check by default (`[licenses] include-dev`), but not out of bans.
- The replacement list is in B, D3. Replace the paragraph after Task 5 Step 1 with:
  > What the dry run established about this file, at fork pin 052c99f: the graph needs Apache-2.0, BSD-3-Clause, MIT, Unicode-3.0, Zlib and our own LGPL-2.1-or-later; every other license in it is one arm of an OR these satisfy. ISC is admitted ahead of need for ring, rustls-webpki and untrusted, which arrive with TLS: a probe of section 4's complete dependency list on ring, dev-dependencies included, passes this list and locks no aws-lc crate. `wildcards = "deny"` is deliberately absent: cargo-deny counts the workspace's own `path` dependencies as wildcards. `required-git-spec = "rev"` makes decision 7's revision pin mechanical: a `branch = "main"` dependency fails `git-source-underspecified`. The aws-lc bans make the provider choice mechanical: the spike's lockfile (8621b5a) carried aws-lc-rs 1.18.1 through rustls 0.23.45 by way of the SDK's `default-https-client`, and that SDK configuration fails `bans` on aws-lc-rs and aws-lc-sys. `multiple-versions = "warn"` reports `getrandom`, `hashbrown` and `syn` duplicates from rados-rs's graph without failing.

**5. MEDIUM-LOW: `actions/cache` is pinned at v4.3.0.**
- That release runs on node20, while the current release is v6.1.0 on node24 (`55cc8345863c7cc4c66a329aec7e433d2d1c52a9`). Every other action in the repository is on its current major.
- The draft also caches the extracted `registry/src` and `git/checkouts` directories. The Cargo book's guidance is to cache only the index, the `.crate` cache and `git/db`.
- In both the clippy and test jobs, replace the cache step with:
  ```yaml
      - name: Cache the cargo registry and git checkouts
        uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            ~/.cargo/registry/index
            ~/.cargo/registry/cache
            ~/.cargo/git/db
          key: ${{ runner.os }}-cargo-${{ hashFiles('Cargo.lock', 'rust-toolchain.toml') }}
  ```
- In the deny job, use `run: cargo deny --locked check`. cargo-deny 0.20.2 has `--locked`, and it fails a stale lockfile instead of checking a silently updated one.
- In Task 5 Step 3, replace "the two SHAs above are what pinact resolved on 2026-09-27; if `actions/cache` has moved on, take pinact's rewrite" with "checkout v7.0.1 and cache v6.1.0, verified with pinact on 2026-09-28".
- The revised `ci.yml` passes actionlint 1.7.7 with shellcheck, and `pinact run --check --verify`.

**6. LOW: Task 3 re-points only one of two README references.**
- The spike README mentions `FINDINGS.md` at `:11` and again at `:53` ("See `FINDINGS.md`."), so "the one reference" is false.
- Change the Files line to "Modify: `README.md:11,53` (both references)".
- Change the command to: ``sed -i 's|`FINDINGS.md`|`docs/FINDINGS.md`|g' README.md``.
- The final tree is unaffected, because Task 4 rewrites the README, but the docs-move commit carries the stale line.

**7. LOW: `cargo tree --workspace --depth 1` hides the edge it is meant to show.**
- It deduplicates: `rgw-meta` prints as `(*)` and its `rados` edge never appears.
- In Review Focus 1 and Task 4 Step 4, use `cargo tree --workspace --depth 1 --no-dedupe -e normal --offline`. I verified that it prints all nine edges exactly.

**8. LOW: salvage-table line ranges are off.**
- sigv4 tests: `:322-481` should be `:321-518`. Line 481 starts `authorization_rejects_malformed`, and the test module closes at 518.
- range.rs: `range.rs:10-52` with tests at `:56-80` should be `range.rs:5-49` with tests at `:51-88`.
- rgw-tests: `lib.rs:34-190` should be `lib.rs:32-188`; the file has 188 lines.
- The chunked and trailer readers and the `encoding-type=url` set are not among decision 1's named pieces. Change "decision 1 names the pieces" to "decision 1 names the pieces; the chunked and trailer readers and the `encoding-type=url` set are two more that section 4 assigns to `rgw-core`".
- Everything else checks out: the vectors at `:353-439`, and the error.rs, xml.rs, bucket.rs, list.rs, rgwd, testsuite and the section-17 `:1057` references.

**9. LOW: the hickory-proto ignore reason makes an unverified exploitability claim.**
- "rados encodes only its own SRV queries" is unverified, and incomplete: `dns_srv.rs` also runs `lookup_ip` through the same resolver.
- Use the neutral reason in B, D2.
- The fork has issues disabled, and no rados-rs PR exists yet, so the pointer to the queued bump can only be text for now.

**10. LOW: the `feat!` commit's README describes things that do not exist until the next commit.**
- At that commit the README lists `cargo deny check` as a CI gate and names `deny.toml`, but both arrive in Task 5, and CI there is still the spike's single test job.
- Task 4's README should omit the `cargo deny check` line and the `deny.toml` clause. Add to Task 5: "Modify `README.md`: add `cargo deny check` to the gates block and append '; `deny.toml` keeps aws-lc out of the graph from the start' to the TLS sentence."

**11. LOW: the draft is inconsistent about when `rgw-tests` gains edges.**
- Architecture says its first edges arrive with "packages 3, 4 and 11"; the Roadmap says "`rgw-meta` in package 3, `rgw-driver` in package 11, `rgwd` in phase 1".
- Use "its first edges arrive with the packages that need them" in Architecture and keep the Roadmap's list.

**12. LOW: the aws-lc-sys "OpenSSL" claim is false.**
- aws-lc-sys 0.45.0, the version in the spike's lock, declares `ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)`, with no OpenSSL term.
- The point is moot under decision 3. Delete the OpenSSL text in Review Focus 3 and in the Roadmap.

**13. LOW: the CLAUDE.md spike line drops "not for use".**
- Replace it with: "The tag `spike-2026-09-24` is the research spike, an insecure research prototype, not for use. Salvage from it with `git show spike-2026-09-24:<path>`; never revive its crates."

**14. NIT, optional under decision 5: the container re-downloads the toolchain on every run.**
- `channel = "1.98"` does not match the image's installed `1.98.1` toolchain, so each `podman run --rm` re-downloads 1.98 (about 9 s, needs network; I verified the auto-install).
- `channel = "1.98.1"` is still 1.98, uses the image's toolchain, and pins CI to the exact patch.

**15. WATCH: first CodeQL run on the new tree.**
- `codeql` is the first run over a tree with `rust-toolchain.toml`. Its Rust extractor installs its own fixed toolchain (`FIXED_RUST_TOOLCHAIN = "1.97.0"`, `rust/extractor/src/toolchain.rs`), so it is likely fine; it is already in the watcher list.
- The `codeql-config.yml` comment describes `rgw-tests` behaviour in the present tense. "will drive" is accurate.

---

## B. Consolidated edits for decisions 1–6

**D1: the spec landed by #5.**
- Global Constraints, branch bullet: "from `origin/main` (8c203d1, the merge of #5, which landed the spec)".
- Spec header: "on `main` since #5 (8c203d1)" in place of "at 231cdd3 (the `design/rgw-rs` branch)".
- Goal: replace "and `docs/` holding the spec, the exclusions pointer and an empty defect registry" with "and `docs/` gaining the exclusions pointer and a pointer to the shared Ceph defect registry, beside the spec #5 landed".
- File structure: drop the spec row.
- Delete Task 2. In the task index, Task 3 depends on 0.
- Task 0 Step 1: "branch `worktree-scaffold` at 8c203d1".
- Task 1: keep `8621b5a` and add: "8621b5a is the spike's last commit; `main` has since moved to 8c203d1 with the spec, which the tag deliberately excludes." The tag push is safe: no workflow triggers on tags, and the ruleset covers branches only.
- Task 7 Step 1: `git log --oneline origin/main..HEAD   # expected: four commits in Task order (docs move, feat!, ci, docs)`.
- Delete open question 1.
- PR body, 97 words by `wc -w`, verified:
  ```
  **Motivation.** The spike answered its question (docs/FINDINGS.md), but its shapes are not the design's; spec decisions 1, 2, 7 and 8 call for a fresh tree.

  **What changed.** The spike is tagged `spike-2026-09-24` and leaves `main`. In its place: five empty crates with the spec's dependency edges, `rados` and `rados-cls` pinned by revision, a `cargo deny` policy keeping rustls on ring, lints denying unwrap, expect and panic outside tests, a 1.98 toolchain pin, and CI running fmt, clippy, test and deny.

  **Notable decisions.** Advisories reached through rados wait for a queued rados-rs bump, so dependency-review stays red.

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  ```
  Drop the "Notable decisions" line if the owner chooses the `allow-ghsas` path from finding 1.

**D2: the inherited advisories are ignored for now.**
- The advisory block goes into the full `deny.toml` under D3.
- Delete open question 2.
- Global Constraints advisories bullet: "…are ignored with reasons rather than fixed here. The fix is a queued rados-rs dependency bump, and the Roadmap removes both ignores with it."
- Roadmap open item:
  > The two `cargo deny` ignores, and `dependency-review`'s GHSA-q2qq-hmj6-3wpp and GHSA-rhfx-m35p-ff5j, go when the queued rados-rs dependency bump lands and the fork pin takes it: hickory-resolver onto hickory-proto 0.26.1 or later, paste replaced, lru 0.16.3 or later (package 9's list is where fork work is scheduled).

**D3: ring is the crypto provider.** Full `deny.toml`, verified with cargo-deny 0.20.2 (`advisories ok, bans ok, licenses ok, sources ok` on the scaffold, plus the three duplicate warnings):
```toml
# cargo deny policy: advisories, the licenses this tree admits (each
# compatible with LGPL-2.1-or-later), one rustls crypto provider, and the
# one git source the workspace may draw from.

[graph]
all-features = true

[advisories]
ignore = [
    # Both arrive through rados and leave with the queued rados-rs dependency
    # bump (hickory-resolver onto hickory-proto 0.26.1 or later, paste
    # replaced); the fork pin that takes it empties this list.
    { id = "RUSTSEC-2026-0119", reason = "hickory-proto 0.24 via rados's mon DNS SRV lookup; fixed in 0.26.1, awaiting the rados-rs dependency bump" },
    { id = "RUSTSEC-2024-0436", reason = "paste is unmaintained; a proc-macro dependency of rados, awaiting the rados-rs dependency bump" },
]

[licenses]
# The licenses the dependency graph needs, with ISC admitted now for ring,
# rustls-webpki and untrusted, which arrive with TLS. Every other license
# in the graph is one choice of an OR expression these already satisfy.
allow = [
    "LGPL-2.1-or-later",
    "Apache-2.0",
    "BSD-3-Clause",
    "ISC",
    "MIT",
    "Unicode-3.0",
    "Zlib",
]
unused-allowed-license = "allow"

[licenses.private]
ignore = true

[bans]
multiple-versions = "warn"
deny = [
    # rustls's crypto provider is ring. aws-lc beside it would be a second
    # provider, and every rustls user would then have to install a default
    # by hand.
    { crate = "aws-lc-rs", reason = "rustls's crypto provider is ring" },
    { crate = "aws-lc-sys", reason = "rustls's crypto provider is ring" },
    { crate = "aws-lc-fips-sys", reason = "rustls's crypto provider is ring" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-git = ["https://github.com/jhoblitt/rados-rs"]
required-git-spec = "rev"
```

- **The SDK can run on ring, but not through its own feature flags.**
  - In aws-sdk-s3 1.150.0, `default-https-client` selects `aws-smithy-http-client/rustls-aws-lc`.
  - Its default `rustls` feature selects the deprecated hyper 0.14 client on rustls 0.21 (`legacy-rustls-ring`).
  - No aws-sdk-s3 or aws-config feature selects the hyper 1 client on ring.
  - The way to do it is to depend directly on `aws-smithy-http-client` with `rustls-ring`:
    ```toml
    aws-sdk-s3 = { version = "1", default-features = false, features = ["behavior-version-latest", "rt-tokio"] }
    aws-smithy-http-client = { version = "1", default-features = false, features = ["rustls-ring"] }
    ```
    ```rust
    use aws_smithy_http_client::tls::{Provider, rustls_provider::CryptoMode};
    let http = aws_smithy_http_client::Builder::new()
        .tls_provider(Provider::Rustls(CryptoMode::Ring))
        .build_https();
    let conf = aws_sdk_s3::Config::builder().http_client(http) /* credentials, region, endpoint */ .build();
    ```
  - I verified this on host cargo 1.98.1 with aws-smithy-http-client 1.4.2:
    - it compiles;
    - `ListBuckets` dispatches over both `http://` and `https://` endpoints, and the connection-refused error shows the connector ran;
    - `Cargo.lock` holds no aws-lc crate, and `bans` passes;
    - the spike's SDK features fail `bans` on aws-lc-rs and aws-lc-sys, including as dev-dependencies.
  - `aws-config` is not needed; it is not in the spec's test-only list. Use `aws_sdk_s3::config::Credentials::new`.
  - On the production side, rustls, tokio-rustls and hyper-rustls all default to aws-lc, so each takes `default-features = false` plus `ring` and the other features it needs. For example, rustls takes `["ring", "std", "tls12", "logging"]` and tokio-rustls takes `["ring", "tls12", "logging"]`.
- Text edits for D3:
  - Architecture, last sentence: "The rustls crypto provider, which spec decision 6 leaves to this package, is ring by owner decision: recorded in `README.md` and `CLAUDE.md`, and mechanically in `deny.toml`, which bans `aws-lc-rs`, `aws-lc-sys` and `aws-lc-fips-sys`."
  - Review Focus 3: "The license list is what the graph needs plus ISC for ring; the two advisory ignores name their crate, reason and the queued rados-rs bump; `aws-lc-rs`, `aws-lc-sys` and `aws-lc-fips-sys` are banned; the only git source is the fork, and only by `rev`."
  - README TLS sentence: "When the frontend lands, TLS is rustls on the ring provider; `deny.toml` keeps aws-lc out of the graph from the start."
  - CLAUDE.md TLS bullet:
    > TLS is rustls on the ring provider (owner decision; spec decision 6 left the provider to this package). `deny.toml` bans `aws-lc-rs`, `aws-lc-sys` and `aws-lc-fips-sys`. rustls, tokio-rustls and hyper-rustls default to aws-lc, so each is declared with `default-features = false` and its `ring` feature. The test-only AWS SDK takes `aws-sdk-s3` with `default-features = false` and its HTTP client from `aws-smithy-http-client` with `rustls-ring`, passed through `Config::builder().http_client(..)`; the SDK's own `default-https-client` and `rustls` features select aws-lc or the legacy hyper 0.14 client.
  - CLAUDE.md, new lint bullet:
    > `clippy.toml` allows `unwrap`, `expect` and `panic` in `#[test]` functions and `#[cfg(test)]` items only; a helper in `tests/*.rs` outside a `#[test]` function, and `rgw-tests`'s library and binaries, return `Result` or carry `#[expect(clippy::…, reason = "…")]`.
  - Task 5 commit:
    ```
    ci: run fmt, clippy, test and cargo deny

    The gates are the spec's: cargo fmt --all --check, cargo clippy
    --workspace --all-targets --all-features -- -D warnings, cargo test
    --workspace --all-targets --locked, and cargo deny check. The policy
    admits the licenses the graph needs plus ISC for ring, allows only the
    rados-rs fork as a git source and only by revision, bans aws-lc so
    rustls keeps ring as its one crypto provider, and ignores with reasons
    the two advisories that come through rados until the fork's dependency
    bump. The toolchain comes from rust-toolchain.toml through the runner's
    rustup; cargo-deny is its release binary, checksummed.

    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
    ```
  - Roadmap: replace the OpenSSL open item with "The test-only AWS SDK arrives without aws-lc: [the two toml lines and the builder above]. The frontend's rustls and tokio-rustls take `default-features = false` with `ring`. The `bans` check fails any other shape."
  - Delete open question 3.

**D4: no separate `RGW-BUG` registry.**
- Replace Task 6 Step 2 with a pointer file, `docs/ceph-upstream-bugs.md`:
  ```markdown
  # Upstream Ceph bug registry

  rgw-rs keeps no registry of its own. Defects in ceph/ceph that rgw-rs
  meets, in radosgw, the RGW object classes, the monitors, the mgr or the
  OSD, are recorded in the registry shared with rados-rs, in the rados-rs
  fork, as `CEPH-BUG-NNN` entries in its format:

  https://github.com/jhoblitt/rados-rs/blob/design/rgw-mvp/docs/superpowers/ceph-upstream-bugs.md

  Add or update an entry there whenever a task, review or benchmark meets a
  defect, and cross-reference rgw-go's `docs/ceph-upstream-bugs.md` where it
  has the matching entry.
  ```
  The link targets the branch because the registry is on `design/rgw-mvp` (a18c0f1, CEPH-BUG-001 to 020), not on fork main.
- CLAUDE.md registry bullet:
  > Upstream Ceph defects rgw-rs meets are recorded in the shared registry in the rados-rs fork (`docs/superpowers/ceph-upstream-bugs.md` on `design/rgw-mvp`) as `CEPH-BUG-NNN` entries in its format; `docs/ceph-upstream-bugs.md` here only points at it. Add or update an entry there whenever work meets one.
- File structure row: "`docs/ceph-upstream-bugs.md` pointer to the shared registry in the rados-rs fork".
- Task 6 commit:
  ```
  docs: point at the shared exclusions and defect registry, add CLAUDE.md

  rgw-go's docs/exclusions.md is canonical for both projects, and upstream
  Ceph defects are recorded in the registry the rados-rs fork keeps, so
  rgw-rs carries a pointer to each rather than a list of its own. CLAUDE.md
  records the repository facts a session needs: the fork pin, the gates,
  how they run on this machine, the crypto provider and the spike tag.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
- Delete open question 4.
- The merged spec now contradicts D4. It still describes an rgw-rs registry in section 3 (`:94-98`) and "the empty defect registry" in section 14 package 1 (`:760-761`). Recommend a commit in Task 6, `docs: the spec points at the shared defect registry`, that:
  - replaces the section 3 bullet with:
    > - **Upstream Ceph defects** that a gateway meets are recorded in the registry shared with rados-rs, the fork's `docs/superpowers/ceph-upstream-bugs.md` on `design/rgw-mvp`, as `CEPH-BUG-NNN` entries in its format: stable IDs, evidence at a tag, the handling, the upstream status. rgw-rs keeps no registry of its own; the entries cross-reference rgw-go's.
  - in package 1, replaces "`docs/` with this spec, the exclusions pointer and the empty defect registry" with "`docs/` with this spec and pointers to the exclusions and to the shared defect registry".
  - optionally also fixes section 1's FFI overstatement from finding 3.
- Out of this PR: the shared registry's header still says "that rados-rs meets as a client". Widening it is a docs change on the fork's `design/rgw-mvp`.

**D5: 1.98 stays.** Delete open question 5. No other change is needed.

**D6: pin rados-rs at 052c99f.**
- `Cargo.toml`: both `rev = "052c99f6b0510f1c837c6090174315b649a7c058"`.
- The rados-rs facts bullet opens: "at `origin/main` 052c99f6b0510f1c837c6090174315b649a7c058 (PR #28, the same-tid resend that section 14 lists as gap 3, over 0b5d1a2; it touches `rados/src/osdclient` and a test, no manifest)". Every cited line in that bullet is unchanged at 052c99f.
- Task 4 Step 4 expected source: `git+https://github.com/jhoblitt/rados-rs?rev=052c99f6b0510f1c837c6090174315b649a7c058#052c99f6b0510f1c837c6090174315b649a7c058`, still "Locking 196 packages" with 201 entries.
- Roadmap: "gap 3 is in the pin; package 10 pins it by test".

---

## C. What I verified
- **PR #5 and main.** PR #5 has merged; main is 8c203d1. No tags existed on the remote. The ruleset targets the branch only, with no required checks.
- **Scaffold at 052c99f** (my probe copy, host cargo 1.98.1):
  - Cargo locked 196 packages and wrote 201 entries.
  - `cargo build` finished in 17.7 s; five test binaries ran with 0 tests each.
  - The deduplicated-off tree shows exactly the section 4 edges.
  - `cargo deny --locked --offline check` with the D3 policy passes.
- **Ring and the SDK.**
  - Probe with aws-sdk-s3 and aws-smithy-http-client on `rustls-ring`: compiles, runs, locks no aws-lc crate.
  - A probe of the spec's complete dependency list on ring passes the seven-license list, including dev-dependencies (`include-dev`).
  - The ban catches the spike's SDK features.
  - `required-git-spec = "rev"` rejects a `branch = "main"` dependency.
- **dependency-review.** The GitHub advisories API over every added registry crate returns only GHSA-q2qq-hmj6-3wpp and GHSA-rhfx-m35p-ff5j. PR #2's dependency-review run shows the action reads `Cargo.lock`.
- **Lints** (clippy 1.98 in the tools container):
  - `unwrap`, `expect` and `panic` outside tests are denied.
  - Inside `#[cfg(test)]`, a helper included, they are allowed.
  - A `tests/*.rs` helper is denied.
  - The three `allow-*-in-tests` keys are accepted.
- **Container gate.** The fmt command with `--user` works; with channel "1.98" it auto-installs the toolchain on every run.
- **cargo-deny.** 0.20.2 is the latest release. The pinned sha256 matches GitHub's own release-asset `digest`, which is independent of the `.sha256` file shipped beside the tarball. The tarball layout matches the extraction command. The official action runs a Docker image built from a Dockerfile.
- **Workflows.** `actions/cache` v6.1.0's SHA and its node24 runtime checked. The revised `ci.yml` is clean under actionlint and `pinact --check --verify`. The kept managed workflows are unchanged, and none triggers on a tag push.
- **Commits and tag.**
  - The subjects satisfy commitlint's config-conventional rules (`feat!:` included).
  - No body starts a line with the breaking-change footer keyword that breaking-footer checks for.
  - The tag, README, commit and PR wording describes no weakness, and `FINDINGS.md` has no weakness wording or relative links.
  - `git diff <blob> <path>` works as Task 3 uses it.
  - LICENSE is identical to rgw-go's.
- **Draft citations.** The rados-rs, rgw-rs and spec line citations are correct except those corrected above.

Side effects: I ran `git fetch` in `~/github/rgw-rs` and in the scratch rados-rs clone, which updated remote-tracking refs only. Scratch probes live under `/tmp/claude-1000/{probe,scaf,lintprobe,wfrev,gdt}`. Nothing was posted, and no cluster was touched.
