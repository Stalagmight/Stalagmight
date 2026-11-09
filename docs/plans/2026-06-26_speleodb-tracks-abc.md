# SpeleoDB Tracks A/B/C - Implementation Plan

## Summary & status

This plan turns the merged read-only SpeleoDB spike (#183) into the full single-writer hand-off backend across **three parallel tracks**: **Track A** (transport/contract - #185 production `ureq` client + multipart upload/auto + retry/backoff, #186 status→action contract, #187 `http://` TLS/private-network policy); **Track B** (persistence/edit-state - #189 `speleo_link` table + `SCHEMA_VERSION` 8→9 + stamp-on-pull, #190 carry-forward fence, #191 read-only edit gate + dirty bit + `execute()` chokepoint refactor); and **Track C** (the retired authentication design; see [Configuration](../configuration.md) for its replacement). The data model is: the per-project `.stalx` (sqlite) is canonical; the derived `ariane.tml` rides along only for the server's web-render. **None of the three tracks needs the #184 render gate.** Only **#183 is landed** (it shipped `src-tauri/src/speleodb/{mod,client,auth,dump,models}.rs`, `core/state/speleodb.rs` with `SpeleoDbSession` on `AppState`, `commands/speleodb.rs`, config bits, the telemetry registry with `EVENT_NAMES` pinned at **418**, and a temporary `SpeleoDbSpikeDialog.tsx`). Everything below is unbuilt work.

---

## Decision ledger (locked)

Every `D-*` decision is the authority for resolving any cross-section conflict. One-line resolutions:

| ID | Decision | Resolution (one line) |
|----|----------|-----------------------|
| **D-B1** | Freshness fold | STUB SEAM - #185 methods return the authoritative `Project`; the persistence fold (cache + reset `last_verify_at`) is a documented NO-OP until #189/#191; leave cross-ref comments at the return sites AND on #189/#191. |
| **D-B2** | IPC action enum | EXPORT ts-rs `SpeleoDbAction` NOW on a structured `SpeleoErrorDto`; rework `run_blocking` String flattening; regen ipc types; pass the no-token-leak guard; #192/#194/#203 inherit it. |
| **D-B3** | 5xx retry ownership | TRANSPORT retries idempotent (read/GET) 5xx with existing backoff; WRITES never auto-retry (double-submit) and surface for g7; #186 maps the final surfaced status. |
| **D-B4** | 403 handling | PER-ENDPOINT 403 (list=read-only, upload=fence, acquire=no-write-perm); anchor token validity on the 401-clean `GET /user/` chokepoint; robust to DRF 401→403 downgrade; never drop the token on a bare 403. |
| **D-B5** | Repair/settings gate | GATE BOTH under SDB read-only (reject + would-dirty-on-success); FIRST code-read that the integrity banner does NOT hard-require Repair to dismiss; acquire-lock-first is the intended UX. |
| **D-B6** | Edit-gate enforcement | REFACTOR ALL BYPASS PATHS THROUGH one chokepoint; route the ~8 `execute_*` bypasses + `execute`/`undo`/`redo`/`undo_to`/`redo_to`; own sub-step + regression + entry-point enumeration test; ASSUMPTION (confirm at review): collab toggles + TML imports are gated mutations. |
| **D-B7** | Rejection shape | DISTINCT CODED FAILURE - `EDIT_DISABLED` prefix on `error` (mirroring `PERMISSION_DENIED_CODE`) so #204 FE suppresses the toast; #191 adds NO `edit_enabled` query command (that's #204/#209). |
| **D-B8** | Storage shape | TYPED COLUMNS - `speleo_link(id CHECK=1, endpoint, project_id, project_name, base_commit_sha TEXT NULL=empty-repo BASE never "", has_unpushed_edits INTEGER DEFAULT 0)`; mirror collab_state quartet; `base_commit_sha → Option<String>`. |
| **D-B9** | Stamp on pull | LAND PRIMITIVES AND STAMP at pull path; thread `project_name` + `latest_commit_sha` (captured as `base_commit_sha`, NEVER mutated by #189) through `speleodb_open_project` so `is_remote` is true end-to-end and #190/#191 get a real fixture. |
| **D-B10** | Insecure override | PERSISTED `SpeleoDbConfig.insecure_http: bool` NOW (not env-only); pre-wire #205; relaxes ONLY the http-scheme block, NEVER TLS cert verification (self-signed/internal CAs stay rejected). |
| **D-B11** | `get_project` | ADD NOW in #185 - standalone `get_project` (`GET /projects/{id}/` → parsed `Project`), ~10-line mirror of `list_projects`; #202 piggyback freshness-fold path consumes it. |
| **D-B13** | Carry-forward fence | ROUND-TRIP + STRUCTURAL GUARD + ADDITIVE TABLE - 4-cell round-trip (raw NULL-stays-NULL + typed API), `PRESERVED_SPELEOLINK_COLUMNS` `pragma_table_info` guard, additive table no migration rewrites; created in BOTH `init_schema` + `migrate_schema`; tests in `persistence/sqlite/tests.rs`; cover `open()` + `open_existing()` + fast-path reopen; no backfill. |
| **D-B14** | `is_remote` | DERIVED from `EXISTS(SELECT 1 FROM speleo_link)`; a `ProjectMetadata` field probed like `collab_enabled` (read.rs:145), NOT a stored column; tolerant of missing table on read-only open (`.ok().unwrap_or(false)`). |
| **D-B15** | HTTP test seam | PURE-FN SEAMS + NARROW LOOPBACK `TcpListener` STUB - extract `map_status`, the `UploadParts` descriptor, the IP-classifier as pure fns; tiny loopback stub only for retry-count/backoff + multipart name/MIME; doubles as #187 127.0.0.1 fixture; NO httpmock/wiremock, NO custom ureq Transport. |
| **D-B16** | Test posture | INVARIANT-AS-UNIT / BEHAVIOR-AS-LIVE - every load-bearing invariant is an always-run CI unit via seams; live `#[ignore]` proves wire reality; `edit_enabled` TRANSIENT/DEFAULT-TRUE/never-persisted; dirty bit flips on SUCCESS in the undo-push epilogue, idempotent, set-only; Save/Open/Close/Create/MergeProjects pass through; gate is a no-op for local-only. |
| **D-B18** | Plaintext perms | UNIX `0o600` at create; Windows relies on per-user profile-dir ACL (documented gap); plaintext is primarily a Linux-headless escape hatch; no new platform dep. |
| **D-B19** | Endpoint scheme | REQUIRE EXPLICIT http/https; https unconditional; http runs the #187 IP policy; missing/unknown scheme rejected with a clear error; never silently rewrite. |
| **D-A-retry** | Retry defaults | `MAX_RETRIES=2`, `RETRY_DELAYS=[200ms,500ms]` (existing); reads add idempotent-5xx retry per D-B3; writes stay `idempotent=false` (run once). |
| **D-A-multipart** | Multipart | repeated `Form::part("artifact", …)` (NEVER `artifact[]`) + a message text part; `Part::file(path)?.file_name(...).mime_str(...)`; bare `.file()` FORBIDDEN; stream via `Part::file()`; build the Form INSIDE the retry closure. |
| **D-A-mime** | MIME map | `.tml → application/octet-stream`, `.stalx → application/vnd.sqlite3`; server routes on extension via the AUTO catch-all - explicit MIME is robustness, not a hard requirement. |
| **D-A-methods** | Methods | promote `acquire_lock`/`release_lock`/`upload_auto`/`create_project` to production (each returns the parsed `Project`); keep `delete_project`/`grant_permission` `#[cfg(test)]`; add `get_project`; `#[serde(default)] exclude_geojson: bool` on `Project`; reject blank commit message at the prod boundary; dedicated `UPLOAD_TIMEOUT` (~120s); do NOT build the blob path; per-endpoint telemetry OUT (#210); `EVENT_NAMES` stays 418. |
| **D-186** | Status→action | one pure `map_status(endpoint, status) -> SpeleoDbAction`, per-row unit tests; unmapped FAILS CLOSED; 422 distinct fatal row; upload-200 → adopt-hexsha + CONDITIONAL clear-dirty (hand off to #191); `parse_error_message` stays DISPLAY-only; NO first-class 423/409/412/429 rows; new `SpeleoDbError::InsecureEndpoint{endpoint}`; extend the display-string-stability test. |
| **D-187** | TLS policy | block http unless EVERY resolved addr is loopback/private/link-local; empty/all-filtered fails closed; IPv4 127/8,10/8,172.16/12,192.168/16,169.254/16; IPv6 ::1,fc00::/7,fe80::/10; IPv4-mapped stripped; localhost/*.localhost→loopback; TOCTOU/DNS-rebinding ACCEPTED not mitigated; enforce every-request via `tls.rs::enforce` at the top of `with_retry`; https short-circuits before resolve; reuse ureq `DefaultResolver::resolve`, fall back to `ToSocketAddrs`; `insecure_http` bypasses ONLY the public-http block. |
| **D-T-tiers** | Two-tier convention | always-run pure-fn/unit (inline `#[cfg(test)]`) + env-gated `#[ignore]` live in `speleodb/mod.rs::live_tests`; CI bar = `cargo test --features telemetry` + `TELEMETRY_ENDPOINT`; live never runs in CI. |
| **D-T-live** | Live facts | bad-creds→400; missing-header→403 vs dead-token→401; acquire-contended→400 vs no-perm→403; upload-200 hexsha+project; upload-304 unchanged; WEB_VIEWER download→403; READ_ONLY label parses; lock_holder=other-user email; `.stalx` round-trips through AUTO; permission FIELD ABSENT (not null) when no access; each fact gets a mocked/seam twin. |
| **D-T-unit** | Always-run units | #186 rows; #187 `classify_addr` over pinned literals + localhost branch; #185 multipart name/MIME + bare-`.file()`-impossible; #190 4-cell + HEAD guard; #188 precedence + 0600 + delete-on-401/logout + rehydration; #191 rejection + dirty-bit flip; retry/backoff pure unit + idempotent gating + loopback transient-then-success + write-once. |
| **D-T-stalx** | `.stalx` coupling | rely on the AUTO catch-all (BaseFileProcessor, stored verbatim) + explicit live round-trip + a SKIPPED drift canary; do NOT hard-code `misc/project.stalx` (located by suffix). |
| **D-T-seed** | Seeding | strict self-seed (one UUID-named project per live run); `ProjectCleanup` Drop guard soft-deletes through a panic; box-side recovery `wipe_test_user_projects` / `wipe_test_gitlab`. |
| **D-T-2acct** | 2nd account | CONFIRMED AVAILABLE (`DEV_SPELEODB_EMAIL2/_PASSWORD2`, `user2@dab.com`); the contention/permission/lock-holder live rows RUN (not gated). |
| **D-T-telemetry** | Telemetry | every new `speleodb_*` command (#209 later) bumps `EVENT_NAMES` + the 418 literal in the same commit; the `telemetry-worker/src/registry.ts` reference is misleading (worker src is only `index.ts`); Tracks A-C add NO new IPC commands except where strictly required. |
| **OPEN-1** | `.tml` bare-`.file()` rationale | RE-CORRECTED at #184: the "allow-list includes `text/plain`" claim is **wrong for `.tml`** - `ArianeTMLFileProcessor.ALLOWED_MIMETYPES = ["application/octet-stream","application/zip"]` (no `text/plain`; that list is the `.tmlu`/`.agr` processors). So a `text/plain` `.tml` part IS rejected (400); explicit `octet-stream` is a real server requirement, not just robustness. **RESOLVED live (#184, 2026-06-27): a bare `ureq` `Part::file()` `.tml` → HTTP 400**, so the explicit MIME in `client::mime_for` is load-bearing (comment corrected). See `docs/speleodb-dev-testing.md` § Render-compat. |
| **OPEN-2** | ureq Form parts accessor | CONFIRM ureq 3.1.4 `Form` exposes parts; if not, the `UploadParts` descriptor is MANDATORY. **(Resolved: it does NOT - descriptor is MANDATORY.)** |
| **OPEN-3** | Integrity banner vs Repair | CONFIRM the banner does not hard-require Repair to dismiss before gating Repair (D-B5). **(Resolved by code-read: non-blocking banner is dismissable; blocking banner is not - see Track B.)** |
| **OPEN-4** | `registry.ts` message | CORRECT the misleading `telemetry-worker/src/registry.ts` test message when `registry.rs` is touched. |

> **Confirmed facts (from code):** `SCHEMA_VERSION = 8` (`persistence/sqlite.rs:30`). `with_retry` retries ONLY transport `Err`s; `http_status_as_error(false)` makes HTTP 500 a normal `Response`, so today NOTHING retries a 5xx. Writes run exactly-once (`idempotent=false ⇒ max_retries=0`, `client.rs:61`). `EVENT_NAMES.len() == 418` (`registry.rs:560`). `ureq = "=3.1.4"` + `multipart` (`Cargo.toml:61`). Production client methods: `login`, `list_projects`, `download_dump`; `#[cfg(test)]`-only: `upload_auto_single`, `acquire_lock`, `release_lock`, `create_project` (+ `delete_project`, `grant_permission`). `models.rs` `Permission` is a STRING label, ABSENT (not null) when no access, enum ordered for `>=` checks. The per-project `.stalx` is the ONLY app sqlite store (+ unrelated `tile_cache.db`); NO global app DB. `collab_state` is the DDL/upsert/clear/load quartet to mirror; `read.rs:145` `collab_enabled` `json_extract` is the probe pattern to mirror for `is_remote`.

---

## Track sequencing & cross-track integration points

### What runs in parallel

The three tracks are independent at the issue level and proceed in parallel. None depends on #184. The **only hard cross-track edge** is Track C's 401-drop (#188 → C5) consuming Track A's #186 `SpeleoDbAction`/`map_status`; that single step has a pre-#186 fallback (bare-`Status{401}` guard) so Track C is never blocked on Track A landing first.

```
Track A: #187 tls ─┐                         Track B: #189 ─→ #190 ─→ #191
                   ├→ #185 client ─→ D-B2 IPC rework
         #186 ─────┘                         Track C: C1→C2→C3→C4→[C5 needs #186]→C6
```

### Within-track order

- **Track A:** `#187 tls.rs` + `#186 contract.rs` land first (parallel, both pure additions; #186 adds the `InsecureEndpoint` variant #187 surfaces - pin the merge so #187 can reference it) → `#185 client.rs` (consumes both: `with_retry` calls `tls::enforce`, the 5xx-retry read-side of D-B3 pairs with #186's `map_status` write-side) → **D-B2 IPC rework** last (reshapes `run_blocking` into the typed `SpeleoErrorDto`, regen ts-rs).
- **Track B:** `#189` (table + quartet + `is_remote` + stamp-on-pull + a populated fixture) → `#190` (fence tests over the #189 schema - nothing to fence until the columns exist; its HEAD-guard must include the `speleo_link` columns) → `#191` (needs `is_remote` to scope the gate and `has_unpushed_edits` to flip; needs #190 green so the dirty bit survives the next bump). `#191` splits into **191-B6** (mechanical chokepoint re-routing, no behavior change) then **191-A** (gate + dirty-bit logic).

### Cross-track seams (the load-bearing integration points)

1. **#186 action enum → #188 401-drop (C5).** #186 exports `SpeleoDbAction` + `map_status(endpoint, status)`. C5's centralized session handler drops the token **only** on `SpeleoDbAction::ReLogin`. Pre-#186, C5 keys off the bare `SpeleoDbError::Status { status: 401, .. }` with a `// TODO(#186): replace bare-401 with map_status(...) == ReLogin` marker; the swap is one condition and the unit tests (synthetic `Status{401}`) are stable across it. (D-B2/D-B4/D-188.)
2. **D-B1 freshness-fold stub-seam comments on #189/#191.** #185's `get_project`/`list_projects`/`upload_auto`/`acquire_lock` return the authoritative `Project` at each call site. The fold (cache the verdict + reset `last_verify_at`) is a **documented NO-OP** until #202. Leave `// FRESHNESS FOLD (#189/#191): authoritative Project available here; cache + reset last_verify_at lands later.` at each #185 return site AND matching scope-note comments at `save_speleo_link` (#189) and the #191 fold-point so it isn't forgotten.
3. **#185 `get_project` + stamp-at-pull touching #189.** #185 lands `get_project` (D-B11) that #202 consumes; #189's stamp-on-pull (D-B9) threads `project_name` + `latest_commit_sha` (captured once as `base_commit_sha`, never mutated) through `speleodb_open_project` so the pulled `.stalx` is `is_remote == true` end-to-end. The stamp lives in the per-project `.stalx` (no global app DB), giving #190/#191 a real populated fixture.
4. **`insecure_http` config field pre-wiring #205 and consumed by #187.** Track C's C3 **adds and persists** `SpeleoDbConfig.insecure_http: bool` (D-B10) + ts-rs typegen now; Track A's #187 `tls.rs::enforce` **reads** it to bypass ONLY the public-http scheme block (never TLS cert verification). The flag is *threaded* from config to `enforce` by the orchestration layer: each `speleodb_*` method in `src-tauri/src/core/state/speleodb.rs` (`speleodb_login`, `speleodb_list_projects`, `speleodb_open_project`, and the future upload/lock callers) reads `config.speleodb.insecure_http` under the existing config lock and passes it into every client call that now takes `insecure_http: bool` → `with_retry` → `tls::enforce` (see Track A #185 step 12). #205 Settings later just binds to the field. D-B19's scheme reject lives in `tls.rs`, **not** in config - do not implement it twice.

---

# Track A - Transport / Contract (#185 client, #186 status→action, #187 TLS policy)

## A.1 Overview & build order

Track A turns the spike's read-only `client.rs` into the full production transport surface, adds a typed status→action decision contract that crosses IPC, and gates `http://` to private networks only. None of the three issues needs the #184 render gate. All three live in `src-tauri/src/speleodb/` plus a thin change to `commands/speleodb.rs` and `core/state/speleodb.rs`.

**Internal build order (dependency edges):**

```
#187 tls.rs (pure IP classifier + enforce)            #186 contract.rs (map_status pure fn + SpeleoDbAction enum)
        │  (with_retry calls tls::enforce at its top)        │  (adds SpeleoDbError::InsecureEndpoint variant + ts-rs enum)
        │                                                     │
        └──────────────┬──────────────────────────────────────┘
                       ▼
                #185 client.rs production methods
       (promote agent/with_retry/status_error; add idempotent-5xx retry per D-B3;
        promote acquire_lock/release_lock/upload_auto/create_project; add get_project;
        multipart UploadParts descriptor; UPLOAD_TIMEOUT; blank-message reject)
                       │
                       ▼
        D-B2 IPC rework in commands/speleodb.rs + core/state/speleodb.rs
   (run_blocking returns a structured DTO carrying SpeleoDbAction; regen ipc types)
```

- **#187 first**: `tls::enforce` is called at the top of `with_retry`, so the TLS module must exist before #185 rewires the retry loop. Zero dependency on #185/#186.
- **#186 in parallel with #187**: `map_status` and the `SpeleoDbAction` enum are pure additions to a new `contract.rs` + the `SpeleoDbError` enum in `mod.rs`. #186 adds the `InsecureEndpoint` variant that #187 surfaces, so the variant lands with whichever of the two merges first; pin the merge so #187 can reference it.
- **#185 last of the three client changes**: it consumes both (`with_retry` now calls `tls::enforce`; the new 5xx-retry rule is the read-side half of D-B3 whose write-side maps via #186's `map_status`).
- **D-B2 IPC rework** is the final step: it reshapes `run_blocking`'s `String` flattening into a typed DTO carrying `SpeleoDbAction`, then regenerates ts-rs types. It depends on #186's enum existing.

> NOTE (ledger vs. code, confirmed): the spike already ships `agent()`, `with_retry(idempotent, …)`, `status_error()`, `parse_error_message()`, and `read_body()` (`client.rs:30-103, 215-219`) and the `#[cfg(test)]` methods `upload_auto_single`/`acquire_lock`/`release_lock`/`create_project`/`delete_project`/`grant_permission` (`client.rs:227-398`). "Promote" in D-A-methods means lift the `#[cfg(test)]` gate + generalize, not write from scratch. `delete_project`/`grant_permission` stay `#[cfg(test)]`.

> NOTE (OPEN-2 RESOLVED): ureq 3.1.4's `Form<'a>` exposes only `boundary()` and an `io::Read` impl - there is **no public `parts()` accessor**. A built `Form` is opaque. Therefore the `UploadParts` descriptor struct is **MANDATORY** for CI coverage of part-name/MIME (D-B15), not optional. `Form::part` consumes the `Part`, so introspecting the real form is impossible.

## A.2 Per-issue implementation steps

### #187 - TLS / private-network policy (`src-tauri/src/speleodb/tls.rs`, new file)

The `http://` scheme is blocked unless **every** resolved address is loopback / private / link-local; `https://` short-circuits before any resolution. `insecure_http` (D-B10, owned by Track C config but consumed here) bypasses **only** the public-http block.

1. **Create `src-tauri/src/speleodb/tls.rs`** and register `pub(crate) mod tls;` in `speleodb/mod.rs` (after `mod dump;`, line 19).

2. **`fn enforce(endpoint: &str, insecure_http: bool) -> Result<(), SpeleoDbError>`** (pure dispatch):
   - Parse the endpoint with `http::Uri::from_str` (`http` is already a dep, `Cargo.toml:62`; gives bracket/port/IPv4-mapped-correct authority parsing per D-187). Reuse `ureq::unversioned::resolver::DefaultResolver::host_and_port(scheme, authority)` to assemble the `host:port` the agent dials.
   - Branch on `uri.scheme_str()`: `Some("https")` → `Ok(())` immediately (no resolve, D-187); `Some("http")` → run the IP policy (step 3); if `insecure_http == true`, skip the block and return `Ok(())` (D-B10 relaxes the http-scheme block ONLY); `None`/anything else → `Err(SpeleoDbError::InsecureEndpoint { endpoint })` (D-B19: require explicit http/https, never silently rewrite).

3. **Resolution + classification (the http branch):**
   - Resolve the authority to the **same** address set ureq dials: build a `DefaultResolver` and call `.resolve(uri, config, timeout)`; if wiring the `&Config`/`NextTimeout` is awkward, fall back to `std::net::ToSocketAddrs` on `host_and_port` (D-187 permits the fallback). Produce a `Vec<IpAddr>`.
   - Special-case **before** resolving: `localhost` and `*.localhost` (case-insensitive) → loopback without DNS (D-187), guarding the box-less localhost dev path.
   - Empty resolution **or** all-filtered → fail closed with `InsecureEndpoint` (D-187).
   - `enforce` returns `Ok(())` only if **every** resolved `IpAddr` is private per `classify_addr` (step 4).

4. **Pure classifier `fn classify_addr(ip: IpAddr) -> AddrClass`** (the unit-test seam, D-T-unit):
   - `enum AddrClass { Loopback, Private, LinkLocal, Public }` (keep the enum so the localhost-branch test asserts `Loopback` distinctly).
   - IPv4: `127.0.0.0/8`→Loopback; `10.0.0.0/8`,`172.16.0.0/12`,`192.168.0.0/16`→Private; `169.254.0.0/16`→LinkLocal; else Public.
   - IPv6: `::1`→Loopback; `fc00::/7`→Private; `fe80::/10`→LinkLocal; else Public.
   - IPv4-mapped IPv6: call `ip.to_ipv4_mapped()` first and, if `Some(v4)`, run the v4 set (D-187) - closes the mapped-address bypass.
   - Implement the ranges by hand on octets/segments (no new dep); prefer explicit octet checks so the literals are auditable in tests.

5. **Wire `enforce` into the retry loop** (this is the #185 edit, listed here for ordering): at the **top of `with_retry`** (`client.rs:57`, before the first `attempt()` at `client.rs:71`), call `tls::enforce(base, insecure_http)?` once per request (D-187 "enforce every-request"). Threading `base` + `insecure_http` through `with_retry` is the cleanest single chokepoint so a new method can't forget the check.

### #186 - status→action contract (`src-tauri/src/speleodb/contract.rs`, new file; `mod.rs` enum)

One pure fn `map_status(endpoint, status) -> SpeleoDbAction`, per-row unit tested, failing closed on unmapped status. The action crosses IPC as a typed ts-rs enum (D-B2).

1. **Add `SpeleoDbError::InsecureEndpoint { endpoint: String }`** to the enum in `mod.rs` (after `InvalidProjectId`, line 69). `#[error("refusing to use an insecure SpeleoDB endpoint: {endpoint}")]`. Non-retryable by construction (it errors before any `attempt()`). **Extend** `error_display_strings_are_stable` (`mod.rs:77`) with an `InsecureEndpoint` row (D-186).

2. **Create `src-tauri/src/speleodb/contract.rs`**, register `pub(crate) mod contract;`.

3. **Define the `SpeleoDbAction` enum (ts-rs exported, D-B2):**
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
   #[ts(export)]
   pub enum SpeleoDbAction {
       ReLogin,     // genuine 401 - token revoked (feeds Track C 401-drop, D-B4)
       ReadOnly,    // 403 on a read-capable endpoint (list = read-only access)
       Fence,       // 403 on upload/acquire (no write perm / lock fence)
       StayDirty,   // write surfaced without confirmed server commit - keep dirty (g7)
       UpToDate,    // upload-304 - remote tree already at this state (benign; dirty bit unchanged)
       Retry,       // idempotent 5xx already retried+exhausted; FE may offer manual retry
       Fatal,       // unmapped / 422 / 4xx we don't model - surface, fail closed
   }
   ```
   Order is non-load-bearing (no `>=` use), unlike `Permission`. There is **no `Wait` action** in this issue: `map_status` never produces one (429/Retry-After is dropped to `Fatal` per the ledger). Lock-contention (acquire-400 "lock held") and any wait-for-other-editor action is **deferred to #194** (out of scope for these tracks), so no `Wait` row exists here.

4. **`fn map_status(endpoint: Endpoint, status: u16) -> SpeleoDbAction`** where `Endpoint` classifies the call site (`Login`, `User`, `ListProjects`, `GetProject`, `DownloadDump`, `Acquire`, `Release`, `UploadAuto`, `CreateProject`). Per-endpoint 403 semantics (D-B4):
   - **401** → `ReLogin` on every authenticated endpoint (anchor on the 401-clean `GET /user/`; token has no expiry so 401 = real revocation). Do NOT map 401 on `Login` itself - login returns **400** on bad creds (verified, `client.rs:113`, `live_bad_credentials_returns_400`), so `Login` + `400` → `Fatal`.
   - **403** → `ListProjects`/`GetProject`/`DownloadDump` → `ReadOnly`; `UploadAuto` → `Fence`; `Acquire`/`Release` → `Fence`. A 403 **never** drops the token (robust to DRF 401→403 downgrade, D-B4).
   - **422** → `Fatal` (distinct unprocessable/missing-revision row; no retry, no dirty change).
   - **`UploadAuto` + 304** → `UpToDate` (benign/informational - the remote tree is already at this state; does **NOT** alter the dirty bit, **NOT** an error). Without this row 304 would fall to the unmapped→`Fatal` default, which is wrong.
   - **5xx on an idempotent endpoint** (`User`/`ListProjects`/`GetProject`/`DownloadDump`/`Login`) → `Retry` (transport already retried per D-B3; surfaces the exhausted-retry final state).
   - **5xx on a write endpoint** (`UploadAuto`/`Acquire`/`Release`/`CreateProject`) → `StayDirty` (writes never auto-retry; surface for g7; keep dirty).
   - **No first-class 423/409/412/429 rows** (server doesn't use them - lock contention is acquire-**400** per `mutex.py`); they fall through to the unmapped default.
   - **Unmapped default** → `Fatal` (FAILS CLOSED - surface fatal, keep dirty, never auto-drop token).

5. **Upload-200 handling (NOT inside `map_status`):** the production `upload_auto` (#185) success path returns the parsed `Project` whose `latest_commit.id` is the adopted hexsha. Per D-186, emit "adopt-hexsha + **CONDITIONAL** clear-dirty": the clear is the edit-epoch comparison owned by #191. **Track A hands the hexsha + a "commit confirmed" signal up; it does NOT clear the dirty bit.** Leave a `// CONDITIONAL CLEAR (#191): edit-epoch comparison clears has_unpushed_edits; do not clear unconditionally.` comment at the upload epilogue.

6. **`parse_error_message` stays DISPLAY-only** (`client.rs:94`). Add an **invariant comment** above `map_status` stating no action branch reads the parsed message string - the action is a pure function of `(endpoint, status)`.

7. **`map_status` is consumed where `status_error` is built.** Production methods that surface to IPC compute `map_status(endpoint, status)` and attach the `SpeleoDbAction` to the structured DTO (D-B2). The `SpeleoDbError::Status` variant stays `{status, message}` (no action field - the action is derived at the IPC boundary so the error enum is not coupled to endpoint context).

### #185 - production client methods (`src-tauri/src/speleodb/client.rs`)

1. **Constants:** keep `REQUEST_TIMEOUT = 30s`, `MAX_RETRIES = 2`, `RETRY_DELAYS = [200ms, 500ms]` (D-A-retry "existing"). **Add** `const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);` (D-A-methods).

2. **`agent()` takes a timeout** - split into `agent()` (30s read default) and `agent_with_timeout(timeout)`, so `upload_auto` builds its agent with `UPLOAD_TIMEOUT`. `agent` stays `http_status_as_error(false)` (`client.rs:33`) so the status-branching shape #186 keys off is preserved.

3. **`with_retry` signature change** (the #187 + D-B3 hinge):
   ```rust
   fn with_retry(
       base: &str,
       insecure_http: bool,
       idempotent: bool,
       mut attempt: impl FnMut() -> Result<(u16, Vec<u8>), ureq::Error>,
   ) -> Result<(u16, Vec<u8>), SpeleoDbError>
   ```
   - **First line:** `tls::enforce(base, insecure_http)?` (#187 step 5).
   - **D-B3 idempotent-5xx retry (read-side rule):** today the closure returns the raw `Response` read *after* `with_retry`, so a 5xx is a normal `Ok(Response)` and **nothing retries it** (confirmed: `http_status_as_error(false)` + `with_retry` only catches transport `Err`). Change the closure to read the body and return `(status, body)`; in `with_retry`, after a successful `attempt()`, if `idempotent && (500..600).contains(&status)`, treat it like a transient error (record + continue the backoff loop). Non-idempotent writes (`idempotent == false`, `max_retries == 0`) still run exactly once and return the 5xx straight through (D-B3, double-submit hazard; confirmed `client.rs:61`).
   - Transport `Err` handling stays as-is (`client.rs:71-77`): retried only when `idempotent`.

4. **Promote read methods unchanged in behavior** (`login`, `list_projects`, `fetch_user_email`, `download_dump`) to the new `with_retry` signature - they already pass `idempotent = true`. Thread `base`/`insecure_http` through.

5. **`get_project` (D-B11 - ADD NOW):** new `pub(crate) fn get_project(base, token, insecure_http, project_id) -> Result<Project, SpeleoDbError>`, a ~10-line mirror of `list_projects` hitting `GET /api/v2/projects/{id}/` and parsing a bare single `Project`. `idempotent = true`. The caller in `state/speleodb.rs` already runs `validate_project_id`. #202 consumes it.

6. **Promote write methods to production** (D-A-methods), each returning the **parsed `Project`**:
   - `acquire_lock` / `release_lock`: drop `#[cfg(test)]`; generalize `post_no_body` to parse + return `Project` from the 2xx body; `idempotent = false`.
   - `create_project`: drop `#[cfg(test)]`, return the full parsed `Project` (today parses only `{id}` via `CreatedProject`); `idempotent = false`.
   - `upload_auto`: see step 7.
   - **Keep `delete_project` + `grant_permission` `#[cfg(test)]`.**

7. **`upload_auto` (production multipart, D-A-multipart / D-A-mime):**
   ```rust
   pub(crate) fn upload_auto(
       base: &str, token: &str, insecure_http: bool, project_id: &str,
       stalx_path: &Path, tml_path: &Path, message: &str,
   ) -> Result<Project, SpeleoDbError>
   ```
   - **Reject blank message at the boundary** (D-A-methods): `if message.trim().is_empty() { return Err(…) }` - do **not** substitute `"seed"` (the spike's `upload_auto_single` did, `client.rs:239`).
   - **Two `artifact` parts, never `artifact[]`**: repeated `Form::part("artifact", …)` once for the `.stalx`, once for the `.tml`. Build the Form INSIDE the retry closure (consume-semantics; `idempotent = false` so it runs at most once).
   - **Explicit per-part MIME, bare `.file()` FORBIDDEN**: each part is `Part::file(path)?.file_name(name).mime_str(mime)?`. MIME map: `.stalx → application/vnd.sqlite3`, `.tml → application/octet-stream`. Stream via `Part::file()` (known Content-Length, no RAM buffering).
   - `agent_with_timeout(UPLOAD_TIMEOUT)`, `.put(url)`, Bearer header, `.send(form)`. `idempotent = false`.
   - On 2xx, parse + return the `Project` (its `latest_commit.id` is the adopted hexsha for #186 step 5).

8. **`UploadParts` descriptor struct (MANDATORY per OPEN-2, D-B15):** because a built `Form` is opaque, extract the multipart shape into a pure, testable descriptor:
   ```rust
   struct UploadPart { name: &'static str, file_name: String, mime: &'static str, src: PathBuf }
   struct UploadParts { parts: Vec<UploadPart>, message: String }
   fn build_upload_parts(stalx: &Path, tml: &Path, message: &str) -> Result<UploadParts, SpeleoDbError>
   ```
   `upload_auto` calls `build_upload_parts` then folds each `UploadPart` into the real `Form`. The unit test asserts both names == `"artifact"` (never `"artifact[]"`), each carries the explicit MIME, `.stalx`→`application/vnd.sqlite3`, `.tml`→`application/octet-stream`. The **bare-`.file()`-impossible** invariant is structural: `build_upload_parts` has no path omitting `mime`, and `upload_auto` always goes through it.

9. **`exclude_geojson` on the `Project` DTO** (D-A-methods, `models.rs:55`): add `#[serde(default)] pub(crate) exclude_geojson: bool`. Update `project_parses_full_v2_payload` (`models.rs:180`) to assert it now **parses** and add a default-on-omission assertion in `project_tolerates_omitted_and_null_optionals` (`models.rs:217`).

10. **Do NOT build the blob path** (D-A-methods: "dump-pull won the spike").

11. **Per-endpoint telemetry is OUT** (#210). EVENT_NAMES stays 418. Track A adds **no new IPC commands** (the existing five in `commands/speleodb.rs` are reshaped, not added to) - call this out in the PR.

12. **Thread `insecure_http` from the orchestration layer (`src-tauri/src/core/state/speleodb.rs`).** Every client method now takes `insecure_http: bool`, but the value originates in config - so each `speleodb_*` orchestration method that calls into the client must read `config.speleodb.insecure_http` (under the existing config lock) and thread it into every client call that now takes the flag. Confirmed call sites (`grep -n "client::" src-tauri/src/core/state/speleodb.rs`): `speleodb_login` (`client::login` at `:209`, `client::fetch_user_email` at `:199`), `speleodb_list_projects` (`client::list_projects` at `:243`), `speleodb_open_project` (`client::download_dump` at `:261`), and the future upload/lock callers (`upload_auto`/`acquire_lock`/`release_lock`/`create_project`/`get_project`) when they land. The flag flows: orchestration reads `config.speleodb.insecure_http` → passes to the client method → `with_retry` → `tls::enforce(base, insecure_http)`. Add `src-tauri/src/core/state/speleodb.rs` to Track A's list of edited files (it is also touched by the D-B2 rework below).

### D-B2 - IPC structured-DTO rework (`commands/speleodb.rs` + `core/state/speleodb.rs`)

1. **Define the IPC error DTO** (ts-rs, in `speleodb/models.rs` or `contract.rs`):
   ```rust
   #[derive(Debug, Clone, Serialize, TS)]
   #[ts(export)]
   pub struct SpeleoErrorDto {
       pub message: String,        // DISPLAY-only (from parse_error_message / Display)
       pub action: SpeleoDbAction, // typed branch surface
       pub status: Option<u16>,    // present for Status errors; None for transport/local
   }
   ```
   **No-token-leak guard (D-B2):** extend `ipc_dtos_never_serialize_a_token` (`models.rs:142`) to include `SpeleoErrorDto`.

2. **Map `SpeleoDbError` → `SpeleoErrorDto`** in a single conversion (needs `Endpoint` context, performed at the call site where the endpoint is known, not a blanket `From`): `Status { status, message }` computes `map_status(endpoint, status)`; `Transport` (network/timeout, post-retry) → `Retry`, `status: None`; `NotAuthenticated` → `ReLogin`; `InsecureEndpoint` → `Fatal`; all local errors → `Fatal`.

3. **Rework `run_blocking`** (`commands/speleodb.rs:25`): change `.map_err(|e| e.to_string())` (`:32`) to produce `Result<T, SpeleoErrorDto>`. The orchestration `AppState` methods (`speleodb_login`, `speleodb_list_projects`, `speleodb_open_project`, `speleodb_logout`, `speleodb_session_status`) return `Result<T, SpeleoErrorDto>`. (Note: the `#[tauri::command]` wrapping `speleodb_session_status` is named `speleodb_session` (`commands/speleodb.rs:80`); the other four command names match their method names.)

4. **Update the five `#[tauri::command]` signatures** (`commands/speleodb.rs:40-82`) from `Result<…, String>` to `Result<…, SpeleoErrorDto>`. FE branches on `err.action`. #192/#194/#203 inherit the typed contract.

5. **Regen ts-rs types** (D-B2): add `SpeleoDbAction` + `SpeleoErrorDto` to the `use stalagmight_lib::speleodb::{…}` block (`examples/gen_ipc_types.rs:65`) + `export::<…>(&out_dir)` calls; run `pnpm gen:ipc-types`; commit `src/types/ipc/SpeleoDbAction.ts` + `SpeleoErrorDto.ts`.

> NOTE (D-B1 stub-seam cross-refs): leave a `// FRESHNESS FOLD (#189/#191): authoritative Project available here; cache + reset last_verify_at lands later.` comment at each #185 `Project` return site, and matching scope-note comments on #189 and #191.

## A.3 New / changed types & signatures

```rust
// speleodb/mod.rs - SpeleoDbError (add one variant):
#[error("refusing to use an insecure SpeleoDB endpoint: {endpoint}")]
InsecureEndpoint { endpoint: String },

// speleodb/tls.rs (new):
pub(crate) fn enforce(endpoint: &str, insecure_http: bool) -> Result<(), SpeleoDbError>;
enum AddrClass { Loopback, Private, LinkLocal, Public }
fn classify_addr(ip: std::net::IpAddr) -> AddrClass;          // pure, unit seam
fn is_private(class: AddrClass) -> bool;
fn resolve_addrs(uri: &http::Uri) -> Result<Vec<IpAddr>, SpeleoDbError>;

// speleodb/contract.rs (new):
pub enum SpeleoDbAction { ReLogin, ReadOnly, Fence, StayDirty, UpToDate, Retry, Fatal }  // ts-rs #[ts(export)]
pub(crate) enum Endpoint { Login, User, ListProjects, GetProject, DownloadDump, Acquire, Release, UploadAuto, CreateProject }
pub(crate) fn map_status(endpoint: Endpoint, status: u16) -> SpeleoDbAction; // pure, fail-closed

// speleodb/client.rs (changed/new):
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
fn agent_with_timeout(timeout: Duration) -> ureq::Agent;
fn with_retry(base: &str, insecure_http: bool, idempotent: bool,
              attempt: impl FnMut() -> Result<(u16, Vec<u8>), ureq::Error>)
              -> Result<(u16, Vec<u8>), SpeleoDbError>;     // + tls::enforce + idempotent-5xx retry
pub(crate) fn get_project(base, token, insecure_http, project_id) -> Result<Project, SpeleoDbError>;   // D-B11
pub(crate) fn upload_auto(base, token, insecure_http, project_id, stalx_path, tml_path, message) -> Result<Project, SpeleoDbError>;
pub(crate) fn acquire_lock(base, token, insecure_http, project_id) -> Result<Project, SpeleoDbError>;  // promoted
pub(crate) fn release_lock(base, token, insecure_http, project_id) -> Result<Project, SpeleoDbError>;  // promoted
pub(crate) fn create_project(base, token, insecure_http, name) -> Result<Project, SpeleoDbError>;      // promoted
struct UploadPart { name: &'static str, file_name: String, mime: &'static str, src: PathBuf }
struct UploadParts { parts: Vec<UploadPart>, message: String }
fn build_upload_parts(stalx: &Path, tml: &Path, message: &str) -> Result<UploadParts, SpeleoDbError>;
// still #[cfg(test)]: delete_project, grant_permission

// speleodb/models.rs - Project (add field):
#[serde(default)] pub(crate) exclude_geojson: bool,           // D-A-methods

// IPC DTO (ts-rs):
pub struct SpeleoErrorDto { pub message: String, pub action: SpeleoDbAction, pub status: Option<u16> }

// commands/speleodb.rs - five commands: Result<…, String> → Result<…, SpeleoErrorDto>; run_blocking returns Result<T, SpeleoErrorDto>.
// Generated TS (committed): src/types/ipc/SpeleoDbAction.ts, src/types/ipc/SpeleoErrorDto.ts
```

## A.4 Deliverables checklist

- [ ] `speleodb/tls.rs` with pure `classify_addr` + `enforce`, wired into `with_retry` (every-request, #187).
- [ ] `http://` blocked unless every resolved address is loopback/private/link-local; `https://` short-circuits; missing/unknown scheme rejected; empty/all-filtered fails closed (D-187/D-B19).
- [ ] `insecure_http` config flag consumed to bypass **only** the public-http block (cert verification unchanged) (D-B10).
- [ ] `SpeleoDbError::InsecureEndpoint` variant + display-string-stability coverage (#186).
- [ ] `speleodb/contract.rs` with `SpeleoDbAction` (ts-rs exported) + pure `map_status`, fail-closed, per-endpoint 403, 422-fatal, idempotent-5xx→Retry, write-5xx→StayDirty (#186/D-B3/D-B4).
- [ ] `parse_error_message` documented DISPLAY-only with the no-action-reads-it invariant (D-186).
- [ ] Production `with_retry` that enforces TLS first and retries idempotent 5xx while writes run exactly once (D-A-retry/D-B3).
- [ ] Promoted production methods returning parsed `Project`: `acquire_lock`, `release_lock`, `create_project`, `upload_auto`; `delete_project`/`grant_permission` stay test-only.
- [ ] `get_project` (single-project GET, #202's path) (D-B11).
- [ ] Production multipart `upload_auto`: two `artifact` parts (never `artifact[]`), explicit per-part MIME, streamed via `Part::file`, blank message rejected, `UPLOAD_TIMEOUT` (120s).
- [ ] `UploadParts` descriptor seam (mandatory - `Form` is opaque in ureq 3.1.4).
- [ ] `Project.exclude_geojson` field (`#[serde(default)]`).
- [ ] `SpeleoErrorDto` (ts-rs) carrying `{message, action, status}`; the five commands + `run_blocking` return it; no-token-leak guard extended; ts-rs regenerated + committed (D-B2).
- [ ] Freshness-fold NO-OP cross-ref comments at each `Project` return site + scope notes queued for #189/#191 (D-B1).
- [ ] EVENT_NAMES stays 418; no new IPC commands - called out in PR.
- [ ] Full unit + live test suite (see Testing section); pre-push checklist green.

## A.5 Manual-test steps (human)

1. **TLS public-http block (no box).** Confirm `enforce("http://example.com", false)` → `Err(InsecureEndpoint)`; `enforce("https://example.com", false)` → `Ok`.
2. **Private-box http allowed.** Set `DEV_SPELEODB_URL=http://10.10.0.220:8000`, launch via `./scripts/dev.sh`, open the spike dialog, log in - login + project list must succeed. Flip to a public `http://` host → login fails with a Fatal/insecure-endpoint action (no crash, no token write).
3. **Status→action surfacing.** Log in, replace the token with garbage, trigger a list/open → FE receives `action: ReLogin`, `status: 401` (inspect the IPC error DTO in devtools). With a `WEB_VIEWER` second account, attempt open/download → `action: ReadOnly`, `status: 403`.
4. **Upload round-trip (two-account box).** Run `cargo test … live_upload_auto_returns_hexsha_and_project -- --ignored --nocapture`; confirm the returned `Project.latest_commit.id` is a fresh 40-hex SHA and the upload carried two `artifact` parts.
5. **Retry/backoff.** Run `cargo test … loopback_idempotent_5xx_then_200_retries -- --nocapture`; observe the idempotent call retried after the 200ms/500ms backoff and a write hit the stub exactly once.
6. **Type generation.** Run `pnpm gen:ipc-types`; confirm `SpeleoDbAction.ts` + `SpeleoErrorDto.ts` generated and `pnpm tsc --noEmit` clean against FE branching on `err.action`.

> NOTE (OPEN-1): correct the spike comment at `client.rs:242-243` ("the server 400s") to "the server routes on extension via the AUTO catch-all (D-T-stalx); explicit MIME is robustness, not a hard requirement." Verify on the box in `live_upload_auto_returns_hexsha_and_project`. Comment/rationale correction only - explicit MIME stays.

---

# Track B - Persistence & Edit-State (#189 SpeleoLink table + schema bump, #190 carry-forward fence, #191 read-only edit gate + dirty bit + `execute()` chokepoint refactor)

## B.1 Overview & internal build order

Track B turns the in-cache `.stalx` pulled by the spike into a *stamped, edit-gated* remote project. It adds one additive single-row table (`speleo_link`) recording which SpeleoDB project a `.stalx` belongs to and whether it carries un-pushed edits, derives an `is_remote` flag from it, and refactors the command layer so **every** mutation routes through one chokepoint that enforces a transient read-only gate and flips a persisted dirty bit.

```
#189  SpeleoLink table + SCHEMA_VERSION 8→9 + stamp-on-pull (D-B8, D-B14, D-B9, D-B1 fold-seam)
   │   ── lands the table, the quartet, is_remote derivation, and a populated fixture
   ▼
#190  Carry-forward fence  (D-B13)
   │   ── round-trip + structural-guard + additive-table tests over the #189 schema
   ▼
#191  Read-only edit gate + dirty bit + execute() chokepoint refactor  (D-B6, D-B5, D-B7, D-B16)
       ── needs is_remote (#189) to scope the gate and has_unpushed_edits (#189) to flip;
          needs the fence (#190) green so the dirty bit survives the next schema bump
```

`#190`'s HEAD-schema guard must include the `speleo_link` columns, so it is sequenced after `#189` and before `#191` so the dirty-bit column it pins is the one `#191` writes.

> NOTE (ledger fact correction - affects #191's D-B7): the ledger calls `EDIT_DISABLED` a *"coded `CommandResult::failure`… mirroring `PERMISSION_DENIED_CODE`"*. In the actual code `CommandResult` (`core/command.rs:197`) has **no `code` field** - `failure(error: String)` only populates `error`. The `PERMISSION_DENIED_CODE` idiom is a **string prefix on `error`**: `save_failure` builds `format!("{PERMISSION_DENIED_CODE}: {detail}")` (`project_io.rs:29`) and callers match `err.starts_with(PERMISSION_DENIED_CODE)` (`tests.rs:4368`, `commands/import.rs:149`). So `EDIT_DISABLED` is a `const EDIT_DISABLED_CODE: &str` prefixed onto the `error` string - honoring D-B7's intent via the prefix mechanism the codebase already has.

## B.2 Per-issue implementation steps

### #189 - SpeleoLink table + `SCHEMA_VERSION` 8→9 + stamp-on-pull

**The collab_state quartet to mirror** (D-B8). Confirmed call-sites: DDL in **both** `init_schema` (`schema.rs:630`) and `migrate_schema` (`schema.rs:233`); targeted upsert/clear `save_collab_state` (`settings.rs:12`); in-`save_project` clear+upsert (`write.rs:39`/`:216`); full load `load_collab_state` (`read.rs:792`); fast probe `json_extract` (`read.rs:145`).

1. **`persistence/sqlite.rs:30`** - bump `SCHEMA_VERSION: i32 = 8;` → `9`. Update the doc comment with a v9 stanza: *"v9 adds the additive single-row `speleo_link` table (SpeleoDB hand-off identity + dirty bit). No existing column or row is touched; forward-looking only - no backfill."* Leave `UNDO_FORMAT_SAFE_VERSION` (`:46`) at 7.

2. **`schema.rs` - `init_schema` (`:526`)** - append (after the `collab_state` block at `:630`, same batch):
   ```sql
   -- SpeleoDB single-writer hand-off link (schema v9). Single row id=1.
   -- base_commit_sha NULL = empty-repo BASE (never coerced to ''). is_remote is
   -- DERIVED from EXISTS(this table), never stored. ADDITIVE: no future migration
   -- rewrites this table (see PRESERVED_SPELEOLINK_COLUMNS / carry-forward fence).
   CREATE TABLE IF NOT EXISTS speleo_link (
       id INTEGER PRIMARY KEY CHECK (id = 1),
       endpoint TEXT NOT NULL,
       project_id TEXT NOT NULL,
       project_name TEXT NOT NULL,
       base_commit_sha TEXT,
       has_unpushed_edits INTEGER NOT NULL DEFAULT 0
   );
   ```

3. **`schema.rs` - `migrate_schema` (`:10`)** - add a byte-identical `CREATE TABLE IF NOT EXISTS speleo_link (…)` right after the `collab_state` `CREATE TABLE IF NOT EXISTS` at `:233`. This is what makes the table appear on `open_existing()` of a legacy v8 file. No `ALTER`, no backfill (mirror the `collab_state` pattern, not the `shot_uuid` `ALTER` pattern).

4. **`settings.rs`** - add the upsert/clear helper next to `save_collab_state` (`:12`):
   ```rust
   pub fn save_speleo_link(&self, link: Option<&SpeleoLink>) -> Result<()>
   ```
   `Some(link)` upserts row id=1 (binding `base_commit_sha` as `Option<String>` → SQL NULL when `None`, **never `""`**); `None` runs `DELETE FROM speleo_link`.

5. **`write.rs` - `save_project` (`:11`)** - `speleo_link` rows are **NOT** rewritten by `save_project`. Do **not** add `DELETE FROM speleo_link` to the clear batch at `:39`, and do **not** add an insert at `:216`. Rationale (load-bearing for D-B13c "no future migration rewrites it" and D-B16's "local save NEVER clears the dirty bit"): `save_project` wipes-and-rewrites every table it owns; if `speleo_link` were in that set, a local autosave would clear `has_unpushed_edits` on every write. The link is mutated *only* through `save_speleo_link` (stamp on pull) and the dirty-bit setter (#191). Add `-- speleo_link is intentionally NOT cleared here (see save_speleo_link / D-B13c)` at the `:39` clear batch.

6. **`read.rs`** - add the full loader next to `load_collab_state` (`:792`):
   ```rust
   pub fn load_speleo_link(&self) -> Result<Option<SpeleoLink>>
   ```
   (Note: `load_collab_state` (`read.rs:792`) is a **private** `fn`, not `pub`; `pub fn load_speleo_link` is a **deliberate visibility bump** from that private pattern - it is called cross-module from `core/state/speleodb.rs` and the #190/#191 tests, which `load_collab_state` is not.) Table-absent → `Ok(None)` (mirror the `sqlite_master` probe); row-absent → `Ok(None)`; row present → map the 5 columns with `base_commit_sha: row.get::<_, Option<String>>` so on-disk NULL stays `None`.

7. **`read.rs` - `load_metadata` (`:22`) + `ProjectMetadata`** - add the derived `is_remote` probe (mirror the `collab_enabled` `json_extract` at `:145` but as `EXISTS`, D-B14):
   ```rust
   let is_remote: bool = self.conn
       .query_row("SELECT EXISTS(SELECT 1 FROM speleo_link)", [], |r| r.get::<_, i64>(0))
       .map(|c| c != 0)
       .ok()
       .unwrap_or(false);   // tolerant of a missing table on read-only metadata open
   ```
   Add `pub is_remote: bool` to `ProjectMetadata` (`models/project.rs:344`, after `collab_enabled` at `:361`); the `.ok().unwrap_or(false)` makes this safe on `open_for_metadata` (`SQLITE_OPEN_READ_ONLY`, no migration - `open.rs:131`). `ProjectMetadata` derives `TS` → run `pnpm gen:ipc-types`.

8. **D-B1 fold-seam markers** - add at the top of `save_speleo_link` (settings.rs) AND in `core/state/speleodb.rs` near `speleodb_open_project` (`:253`):
   ```rust
   // FRESHNESS-FOLD SEAM (D-B1): #185's get_project/list_projects return the
   // authoritative Project on every call. The persistence fold (cache the
   // freshness verdict + reset last_verify_at) is a DOCUMENTED NO-OP until the
   // #202 piggyback path consumes it. base_commit_sha is captured ONCE on pull
   // and NEVER mutated by #189 (a later Sync/Release issue re-stamps it).
   ```

9. **D-B9 stamp-on-pull - thread `project_name` + `latest_commit_sha` through the open path** so `is_remote` becomes true end-to-end. Today the flow is `SpeleoDbSpikeDialog.tsx:105 invokeIpc('speleodb_open_project', { projectId })` → `commands/speleodb.rs:62` → `core/state/speleodb.rs:253 -> SpeleoOpenResult{ local_path }`. The `SpeleoProjectSummary` the dialog holds (`models.rs:77`) carries `name` and `latest_commit_sha`. Changes:
   - **`commands/speleodb.rs:62`** - extend the IPC signature to `speleodb_open_project(app, project_id, project_name: String, latest_commit_sha: Option<String>)`. **No new IPC command** (same command name, more args) → **EVENT_NAMES stays 418** (the registry counts command *names*) - call this out in the PR.
   - **`core/state/speleodb.rs:253`** - `speleodb_open_project(&self, project_id, project_name, latest_commit_sha)`. After `dump.extract_entry_to(&entry, &dest)?` (`:269`) and before returning, open the extracted `.stalx` with `Database::open_existing(&dest)` (runs `ensure_schema_version`, creating the v9 table on the pulled file) and call `db.save_speleo_link(Some(&SpeleoLink { endpoint, project_id: project_id.clone(), project_name, base_commit_sha: latest_commit_sha, has_unpushed_edits: false }))`. `endpoint` comes from the `(endpoint, token)` resolved at `:260`. `base_commit_sha` = the pulled SHA (NULL/`None` for empty repo). After this, `load_metadata().is_remote == true`.
   - **`src/components/Dialogs/SpeleoDbSpikeDialog.tsx:105`** - pass `projectName: project.name` and `latestCommitSha: project.latest_commit_sha` alongside `projectId` (the `handleOpen` local is `project`, a `SpeleoProjectSummary` - `:100,106`).

   This gives #190 a real populated `.stalx` fixture and #191 a real `is_remote` project. The stamp lives in the cache `.stalx` itself (`<cache>/speleodb/<id>/project.stalx`, `:284`); the dialog then `loadProject(local_path)` re-opens that same stamped file (no global app DB exists - confirmed).

**New type `SpeleoLink`** - define in `models/` (e.g. `models/speleo_link.rs`, re-exported from `models/mod.rs`):
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SpeleoLink {
    pub endpoint: String,
    pub project_id: String,
    pub project_name: String,
    /// NULL on-disk = empty-repo BASE. NEVER coerced to "". Captured once on pull, never mutated by #189.
    pub base_commit_sha: Option<String>,
    /// Set-only in #191 (false→true on first successful remote-project edit);
    /// cleared only by a later successful server push (Sync/Release), never by local save/autosave.
    pub has_unpushed_edits: bool,
}
```

### #190 - Carry-forward fence (D-B13)

All three sub-tests live in `persistence/sqlite/tests.rs` (no telemetry, avoids the 418 bump). Per-module `#[expect(clippy::unwrap_used, …)]` idiom already on that module (`sqlite.rs:179`).

1. **`schema.rs`** - add the contract const:
   ```rust
   /// Columns of `speleo_link` that MUST survive every future SCHEMA_VERSION bump
   /// unchanged (name, declared type, null-ness). The carry-forward fence asserts
   /// this with pragma_table_info so no later migration silently rewrites the
   /// hand-off identity or drops the dirty bit. (name, decl_type, notnull)
   pub(super) const PRESERVED_SPELEOLINK_COLUMNS: &[(&str, &str, bool)] = &[
       ("id", "INTEGER", true),
       ("endpoint", "TEXT", true),
       ("project_id", "TEXT", true),
       ("project_name", "TEXT", true),
       ("base_commit_sha", "TEXT", false),      // nullable = empty-repo BASE
       ("has_unpushed_edits", "INTEGER", true),
   ];
   ```

2. **D-B13a - 4-cell round-trip** (`set base_commit_sha + dirty → migrate → assert preserved`), at **both** layers: *raw-SQL on-disk NULL-stays-NULL* (raw `INSERT` NULL+dirty, force `migrate_schema`, raw `SELECT base_commit_sha IS NULL, has_unpushed_edits` → still NULL not `''`, dirty still 1) and *typed model API* (`save_speleo_link(Some(&link))` with `None` + `true`, reopen, `load_speleo_link()` → `None` + `true`; a second case with `Some("abc123")`).

3. **D-B13b - structural HEAD-schema guard** - over `PRESERVED_SPELEOLINK_COLUMNS`, via `pragma_table_info('speleo_link')`: for each `(name, decl_type, notnull)` assert the live table has that exact name/type/null-ness. Fails until a future bump preserves the column.

4. **D-B13c - additive-table assertion** - stamp a link, run `save_project(&project)` (wipes/rewrites all owned tables), `load_speleo_link()` → row + dirty bit survived. Pins step #189.5.

5. **Both open shapes** (D-B13): `open()` (`open.rs:9`) calls `init_schema` then stamps `SCHEMA_VERSION` **without** the migrate chain - test a file created via `open()` has the v9 table. `open_existing()` (`open.rs:44`) on a legacy v8 file runs `ensure_schema_version → migrate_schema`; a second `open_existing()` hits the fast path (`schema.rs:498` early-return) - test the table is present and intact after reopen.

### #191 - Read-only edit gate + dirty bit + `execute()` chokepoint refactor

The riskiest issue. Two sub-steps: **(B) the chokepoint refactor (D-B6)** done first (mechanical re-routing, no behavior change, green tests), then **(A) the gate+dirty-bit logic (D-B16, D-B7)** layered on top.

#### 191-B6: the single mutation chokepoint (D-B6)

**Problem:** mutations enter `AppState` through `execute()` (`commands.rs:358`) *and* ~8 sibling methods that each independently acquire `command_lock` and push their own snapshot-pair undo, bypassing `execute()`'s epilogue. A gate placed only in `execute()` leaks every bypass path.

**Precise inventory of the bypass call-sites:**

| # | Method | File:line | Pushes undo via | Notes |
|---|--------|-----------|-----------------|-------|
| 1 | `update_project_settings` | `project_create.rs:169` | `push_snapshot_undo` (`:UpdateProjectSettings`) | own `command_lock`; no-ops if unchanged |
| 2 | `repair_open_project_integrity` | `commands.rs:279` | `push_snapshot_undo` (`:RepairIntegrity`) | own `command_lock`; returns `IntegrityRepairResult`, not `CommandResult` |
| 3 | `execute_move_comment` | `command_comment.rs:57` | `push_snapshot_undo` | called from inside `execute()` via early `return` (`commands.rs:486`) - already under `execute`'s lock |
| 4 | `execute_delete_shot_and_split` | `command_delete_split.rs:9` | `push_snapshot_undo` (`:228`) | own `command_lock` (`:17`) |
| 5 | `execute_delete_shot_range_split` | `command_delete_split.rs:249` | `push_snapshot_undo` (`:465`) | own `command_lock` (`:257`) |
| 6 | `execute_delete_shot_range_shift` | `command_delete_split.rs:484` | `push_snapshot_undo` (`:631`) | own `command_lock` (`:490`) |
| 7 | `execute_delete_last_shot_and_section` | `command_delete_split.rs:645` | `push_snapshot_undo` (`:800`) | own `command_lock` (`:651`) - the "last variant" |
| 8 | `execute_create_and_merge` | `command_import.rs:72` | `push_snapshot_undo` (`:162`) | own `command_lock` (`:85`) |
| 9 | `execute_import_sections` | `command_import.rs:181` | `push_snapshot_undo` (`:886`) | own `command_lock` (`:193`) |
| 10 | `apply_collab_changes` | `core/state/collab.rs:151` | `push_snapshot_undo` (`:213`) | reached via `execute(ApplyCollabChanges)` (`commands.rs:499`) - already under `execute`'s lock |
| 11 | `set_collab_enabled` | `core/state/collab.rs:17` | none (toggles `project.collab`, `touch()`) | **no `command_lock`, no undo** |
| 12 | `set_collab_enabled_for_path` | `core/state/collab.rs:49` | none (writes `.stalx` via `open_existing`+`save_collab_state`) | **operates on a project NOT necessarily open in `AppState`** |

Plus the four history entrypoints: `undo` (`undo.rs:10`), `redo` (`undo.rs:442`), `undo_to` (`undo.rs:791`), `redo_to` (`undo.rs:831`).

**The chokepoint design** - introduce one private gate helper on `AppState` and route every entry through it (do NOT literally funnel 12 heterogeneous signatures through `execute(cmd)`; funnel them through a *guard call* at each entry, immediately after `command_lock` and before mutating):

```rust
/// THE mutation chokepoint. Every state-mutating entry (execute, undo, redo,
/// undo_to, redo_to, every execute_* bypass) calls this FIRST, after taking
/// command_lock and before mutating. Err aborts the mutation when the open
/// project is a remote SpeleoDB project opened without a write lock. No-op (Ok)
/// for local-only (non-is_remote) projects and non-mutating reads.
fn guard_mutation(&self, what: &str) -> Result<(), CommandResult> {
    if self.edit_enabled() { return Ok(()); }
    Err(CommandResult::failure(format!(
        "{EDIT_DISABLED_CODE}: {what} is disabled: this SpeleoDB project is open \
         read-only. Acquire the lock to edit."
    )))
}
```

Routing rules (each is a 2-3 line insertion, no logic moved):
- **`execute()` (`commands.rs:358`)**: insert `if let Err(f) = self.guard_mutation(&cmd.description()) { return f; }` after `command_lock` (`:362`) and before `invalidate_adjusted_cache` (`:364`). But **`Save/Open/Close/Create/MergeProjects` must pass through** (D-B16: autosave must not be blocked). Skip the guard when `matches!(cmd, CreateProject|OpenProject|SaveProject|CloseProject|MergeProjects)`. Implement as `fn is_lifecycle_passthrough(cmd: &Command) -> bool` next to `command_integrity` (`commands.rs:39`) so a new `Command` variant forces a choice (same anti-fail-open discipline `command_integrity` uses).
- **`undo`/`redo`/`undo_to`/`redo_to`**: insert the guard after each `command_lock` (`undo.rs:12`, `:444`, `:793`, `:833`). Gate unconditionally (you cannot mutate a locked project even to revert).
- **`update_project_settings` / `repair_open_project_integrity`** (D-B5: gate BOTH): insert after `command_lock` (`project_create.rs:177`, `commands.rs:288`). `repair_*` returns `IntegrityRepairResult`, not `CommandResult`, so instead return `IntegrityRepairResult { fixed: 0, remaining: 0, changed: false }` and log the rejection - the FE treats `changed: false` as "nothing happened" (it dirties only on `changed`, `command.rs:227`). The coded-failure surface for repair is #204's; #191 just refuses to mutate.
- **`execute_delete_*` (×4), `execute_create_and_merge`, `execute_import_sections`**: insert the guard after each `command_lock`. These return `CommandResult` → return the coded failure directly.
- **`execute_move_comment`, `apply_collab_changes`**: reached *through* `execute()` → already covered by the `execute()` guard (they're not lifecycle, so the guard fires). Add a debug-assert/comment documenting that they inherit the guard; the enumeration test enforces a future direct caller calls `guard_mutation`.
- **`set_collab_enabled` (`collab.rs:17`)**: ASSUMPTION (D-B6, confirm at review) - a collab toggle IS a gated mutation. Insert `if let Err(f) = self.guard_mutation("Toggle collaboration") { return f; }` at the top. Gating without the lock is sufficient (the gate reads transient state, not the project).
- **`set_collab_enabled_for_path` (`collab.rs:49`)**: this is the **exception to the "2-3 line insertion, no logic moved" rule above** - gating it requires a **real reorder**, not a no-logic-move, and it has **no `command_lock`** (it locks `self.project` at `:78`, not a command lock), so the generic "insert after `command_lock`" routing does not apply here. The method writes to disk (`db.save_collab_state`, `:73`) **BEFORE** the open-project identity check (`open.file_path.as_deref() == Some(path.as_path())`, `:83`). It mutates a `.stalx` **by path**, usually NOT the open `AppState` project; the transient `edit_enabled` describes the *open* project, so it cannot speak for an arbitrary path. Gate it only when the path **is** the currently open project AND that project is read-only-remote - but to do so, **reorder the identity check (`:83`) above the disk write (`:73`)** so `guard_mutation` can run before any mutation: compute `open.file_path == Some(path)` first, and call `guard_mutation` only when it matches, *before* `db.save_collab_state`. For a *closed* remote file, defer cross-file remote-gating to the issue that owns closed-file remote state. Document precisely.

**The entry-point enumeration test** (D-B6: "a newly-added mutation path fails until routed") - add `chokepoint_enumeration_test` in `core/state/tests.rs`. Two mechanisms: (1) a **compile-forcing list** `const MUTATION_ENTRYPOINTS: &[&str]` enumerating the 12 + 4 routed entry names with a doc comment instructing new mutation methods be added to both routing and this list; (2) a **behavioral test** that builds an `AppState` with an open project, stamps it remote + sets `edit_enabled=false`, then calls each of `execute(<mutating Command>)`, `undo`, `redo`, `undo_to(0)`, `redo_to(0)`, `update_project_settings`, `repair_open_project_integrity`, all 4 `execute_delete_*`, `execute_create_and_merge`, `execute_import_sections`, `apply_collab_changes` (via `execute`), `set_collab_enabled(true)` and asserts each returns the `EDIT_DISABLED` code (or, for repair, `changed == false`).

#### 191-A: the transient gate + dirty bit (D-B16, D-B7)

1. **`core/command.rs`** - add `pub(crate) const EDIT_DISABLED_CODE: &str = "EDIT_DISABLED";` (next to `CommandResult`, mirroring `loading::PERMISSION_DENIED_CODE` at `loading.rs:87`). Deliberately **no** `edit_enabled` query command (D-B7: that IPC is #204/#209).

2. **`core/state/app_state.rs` - `AppState` struct (`:68`)** - add a transient field:
   ```rust
   /// Whether the open project may be edited. TRANSIENT, never persisted,
   /// DEFAULT-TRUE; false ONLY for a remote project opened without a write lock.
   /// Reset on every open/close. The mutation chokepoint reads it.
   pub(super) edit_enabled: AtomicBool,
   ```
   Initialize `AtomicBool::new(true)` in `AppState::new` (`:153`). Add `fn edit_enabled(&self) -> bool` and `pub(crate) fn set_edit_enabled(&self, v: bool)`. Reset to `true` in the close/open paths so a previously-read-only project can't leave the flag stuck.

3. **Wiring `edit_enabled = false`** - a remote project is read-only when `is_remote` AND no write lock is held. In #191 there is **no lock-acquire path yet**, so the rule is: on open of an `is_remote` project, `set_edit_enabled(false)`. Hook this in the project-load path that already reads `ProjectMetadata`/`load_project`. Local-only projects always stay `edit_enabled = true` (D-B16: no-op for local-only).

4. **Dirty-bit flip (D-B16)** - `has_unpushed_edits` flips `false→true` **on success, in the same epilogue that pushes the undo entry**, **set-only** in #191 (only a later server push clears). Add:
   ```rust
   /// Flip has_unpushed_edits false→true on a successful edit of a remote project.
   /// Idempotent (no-op once true). No-op for local-only (load_speleo_link → None).
   /// Set-only in #191: local save/autosave NEVER clears it; only a successful
   /// server push (later: Sync/Release) does.
   fn mark_remote_dirty(&self) { /* open DB, load_speleo_link, if Some && !dirty: upsert dirty=true */ }
   ```
   Call it at every success-and-undo-pushed point: `execute()` epilogue (`commands.rs:548`) + the Batch success path in `execute_batch` (`commands.rs:681`); `undo`/`redo` success (`undo.rs:417`, `:691`); each `execute_*` bypass after its `push_snapshot_undo`; `update_project_settings`, `repair_open_project_integrity` (`commands.rs:346`), `apply_collab_changes` (`:213`), `set_collab_enabled` after `touch()`. **NOT** on `SaveProject`/autosave (lifecycle pass-throughs must never clear). Because the gate blocks remote edits when read-only, `mark_remote_dirty` only ever runs on a remote project that *does* have edit enabled (after a lock is acquired in a later issue); in #191 the practical effect is the plumbing + the always-run unit test (exercised by tests that set `edit_enabled=true` on a remote fixture).

> NOTE (D-B5 / OPEN-3 - integrity banner verify-step, RESOLVED by code-read): the FE banner (`App.tsx:361`) renders when `integrityIssues.length > 0 && (integritySaveBlocked || !integrityDismissed)`, and **Dismiss** is shown only when `!integritySaveBlocked` (`App.tsx:382`). So Repair is **not** hard-required to dismiss a *non-blocking* banner. It **is** effectively required for a `save_blocked` (critical) corruption (no Dismiss). Consequence for D-B5: a corrupt remote `save_blocked` project shows a non-dismissable banner whose "Review & Repair" now leads to a Repair that `guard_mutation` rejects (`changed:false`). Acceptable dead-end per D-B5 ("acquire-lock-first is the intended UX; corrupt remote project is the owner-with-lock responsibility"). #191 does not change the banner; the FE in #204 should suppress/relabel the Repair affordance on a read-only remote project (it pattern-matches `EDIT_DISABLED`). Flagging so the FE issue owns the UX.

## B.3 New / changed types & signatures

```rust
// models/speleo_link.rs (re-exported from models/mod.rs)
pub struct SpeleoLink {                       // ts-rs #[ts(export)]
    pub endpoint: String, pub project_id: String, pub project_name: String,
    pub base_commit_sha: Option<String>,      // NULL = empty-repo BASE
    pub has_unpushed_edits: bool,
}
// core/command.rs
pub(crate) const EDIT_DISABLED_CODE: &str = "EDIT_DISABLED";
// persistence/sqlite/schema.rs
pub(super) const PRESERVED_SPELEOLINK_COLUMNS: &[(&str, &str, bool)] = …;

// persistence Database
pub fn save_speleo_link(&self, link: Option<&SpeleoLink>) -> Result<()>     // settings.rs
pub fn load_speleo_link(&self) -> Result<Option<SpeleoLink>>                 // read.rs

// AppState (core/state)
fn edit_enabled(&self) -> bool
pub(crate) fn set_edit_enabled(&self, v: bool)
fn guard_mutation(&self, what: &str) -> Result<(), CommandResult>
fn mark_remote_dirty(&self)
fn is_lifecycle_passthrough(cmd: &Command) -> bool                          // commands.rs

// core/state/speleodb.rs - extended args (NOT a new command name)
pub(crate) fn speleodb_open_project(&self, project_id: String, project_name: String,
    latest_commit_sha: Option<String>) -> Result<SpeleoOpenResult, SpeleoDbError>
// commands/speleodb.rs - extended IPC args (same command name → EVENT_NAMES stays 418)
pub async fn speleodb_open_project(app, project_id: String, project_name: String,
    latest_commit_sha: Option<String>) -> Result<SpeleoOpenResult, SpeleoErrorDto>

// Changed struct field: ProjectMetadata { …, pub is_remote: bool }  (models/project.rs)
// Bumped const: SCHEMA_VERSION = 9  (persistence/sqlite.rs)
// ts-rs / IPC: SpeleoLink (new export), ProjectMetadata.is_remote (new field) - pnpm gen:ipc-types
//   FE: SpeleoDbSpikeDialog.tsx passes projectName + latestCommitSha
//   No new IPC command → EVENT_NAMES stays 418 (state in PR)
```

## B.4 Deliverables checklist

- [ ] `SCHEMA_VERSION` 8→9; additive `speleo_link` single-row table in **both** `init_schema` and `migrate_schema` (legacy v8 files gain it on `open_existing`).
- [ ] Full DDL/upsert/clear/load quartet mirroring `collab_state`; **excluded** from `save_project`'s wipe/rewrite so local saves never clear it.
- [ ] `SpeleoLink` model with `base_commit_sha: Option<String>` (NULL = empty-repo BASE, never `""`); ts-rs exported.
- [ ] `ProjectMetadata.is_remote` derived via `EXISTS(speleo_link)`, tolerant of a missing table on read-only metadata open; ts-rs field regenerated.
- [ ] Stamp-on-pull threading `project_name` + `latest_commit_sha` (captured once as `base_commit_sha`, never mutated) so a pulled project is `is_remote == true` end-to-end; FE dialog passes both; **no new IPC command, EVENT_NAMES stays 418**.
- [ ] D-B1 freshness-fold seam comments on `save_speleo_link` and `speleodb_open_project`.
- [ ] Carry-forward fence: 4-cell round-trip (raw + typed), `PRESERVED_SPELEOLINK_COLUMNS` HEAD-schema guard, additive-table assertion, both open-shapes covered.
- [ ] Single mutation chokepoint: all 12 bypass methods + `execute`/`undo`/`redo`/`undo_to`/`redo_to` route through `guard_mutation`; lifecycle (Save/Open/Close/Create/MergeProjects) passes through.
- [ ] Transient, default-true, never-persisted `edit_enabled`; `false` only for remote-open-without-lock; reset on open/close.
- [ ] Distinct coded rejection (`EDIT_DISABLED` prefix on `error`) the FE (#204) can pattern-match; Repair + settings both gated under read-only remote.
- [ ] Persisted dirty bit `has_unpushed_edits` flips `false→true` set-only on every successful edit/undo/redo/`execute_*`, idempotent, no-op for local-only, never cleared by local save/autosave.
- [ ] Entry-point enumeration regression test so a new mutation path fails until routed.

## B.5 Manual-test steps (human, on the dev box)

Precondition: `DEV_SPELEODB_URL` + a verified account; dev build (`./scripts/dev.sh`).

1. **Stamp visible:** open the SpeleoDB dialog, log in, pick a project with commits, click open. After it loads, in the DB-inspector open table **`speleo_link`** - confirm one row with the project's `endpoint`, `project_id`, name, the latest commit SHA in `base_commit_sha`, and `has_unpushed_edits = 0`.
2. **`is_remote` derivation:** return to the recent-projects view; the pulled project's metadata reflects `is_remote` (no opening required).
3. **Empty-repo BASE:** pull a freshly-created (never-committed) project; in `speleo_link`, `base_commit_sha` shows **NULL** (not blank string).
4. **Read-only gate:** with the remote project open (no lock), try to edit a shot value / move a station / undo / Repair / toggle collaboration. Each is **rejected** with a toast the FE recognizes as expected; the survey is unchanged. Save/autosave still work.
5. **Dirty bit (with `edit_enabled` forced true via a debug toggle):** make one edit → `speleo_link.has_unpushed_edits = 1`. Make a second edit + run a local autosave → it stays `1`.
6. **Local projects unaffected:** open any local `.stalx` - no `speleo_link` row, all editing/undo/repair/collab behave as before (gate is a no-op).
7. **Legacy migration:** open a pre-v9 (v8) `.stalx` → it opens cleanly, gains the empty `speleo_link` table (via `migrate_schema`), `is_remote == false`, and remains fully editable.

---

# Track C - Auth (retired design)

The former credential-storage design was retired. Current storage and authentication
are documented in [Configuration](../configuration.md) and
[SpeleoDB integration](../SpeleoDB.md).

---

# Testing, manual-test & delivered-functionality (cross-cutting, all three tracks)

This section owns the test architecture, the live/CI matrix, the human walkthroughs, and the env/setup checklist spanning Tracks A-C. It honors the locked ledger exactly; where code contradicts a ledger phrasing it is flagged in a `>NOTE` rather than silently diverged.

## T.1 Test architecture

### T.1.1 The two-tier convention (do not re-litigate - D-T-tiers)

| Tier | Where | Network? | Runs in CI? | Skip mechanism |
|------|-------|----------|-------------|----------------|
| **Always-run unit** | inline `#[cfg(test)] mod tests`, or `persistence/sqlite/tests.rs` for Track B | No (pure fn or in-memory) | **Yes** (`cargo test --features telemetry` + `TELEMETRY_ENDPOINT`) | n/a - always runs |
| **Env-gated live** | `speleodb/mod.rs::live_tests` only (`mod.rs:116-412`) | Yes (real dev box) | **Never** (CI has no SpeleoDB box) | `#[ignore]` + `let Some(...) = live_env() else { eprintln!("skipping…"); return; }` |

CI bar (unchanged): `TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path src-tauri/Cargo.toml --features telemetry`. `--features telemetry` is mandatory (gates `registry_covers_all_*`; `build.rs` requires `TELEMETRY_ENDPOINT` under that feature). A plain `cargo test` passes locally while CI fails. Live tests are `#[ignore]`d; invoked deliberately with `-- --ignored` + the `DEV_SPELEODB_*` vars; absent those, each prints a skip line and passes (`live_env()`/`live_env2()` at `mod.rs:131-148`).

### T.1.2 The pure-fn seams + loopback stub (D-B15)

No new HTTP-mock dependency (no httpmock/wiremock) and no custom `ureq` `Transport` (it would couple to the unstable unversioned API the `=3.1.4` pin fences off). Instead, **pure functions** extracted as seams + **one tiny loopback `TcpListener` stub**:

| Seam | Lives in | What it lets CI assert without the box |
|------|----------|-----------------------------------------|
| `map_status` | `speleodb/contract.rs` (#186) | every status→action row (D-186), fail-closed default, 422 row |
| `classify_addr` / `enforce` | `speleodb/tls.rs` (#187) | IP allow/block over pinned literals; scheme rejection; `insecure_http` bypass - no DNS in the unit |
| `build_upload_parts` → `UploadParts` | `speleodb/client.rs` (#185) | both parts named `artifact` (never `artifact[]`), each MIME explicit, bare `.file()` impossible |
| `with_retry` loop | `speleodb/client.rs` (promoted) | retry-count, idempotent gating (reads retry 5xx-as-transport, writes run once) - scripted Err/Ok |
| `dump::classify` | `speleodb/dump.rs:37` (exists) | `.stalx`-by-suffix / ambiguous / adopt / nothing |
| carry-forward / schema guard | `persistence/sqlite/*` (#190) | null-SHA NULL-stays-NULL, `PRESERVED_SPELEOLINK_COLUMNS` via `pragma_table_info` |
| `edit_enabled` gate | `core/state/*` (#191) | rejection code + dirty-bit flip, entry-point enumeration |

**The loopback stub (the only socket I/O in CI):** `TcpListener::bind("127.0.0.1:0")`, accepts one connection, returns a scripted status sequence (e.g. `503,503,200`), records the raw request bytes. Three purposes a pure fn cannot serve: (1) **retry timing/count** over a real socket (read retries `503→503→200`; write attempted *exactly once* by counting connections); (2) **multipart wire assertions** (capture the raw `PUT …/upload/auto/` body, assert both `Content-Disposition: …name="artifact"` parts + their `Content-Type`); (3) **the #187 `127.0.0.1` fixture** (a real loopback `enforce` must allow, plus a public host blocked via resolver injection).

> NOTE (OPEN-2, RESOLVED): the spike already uses `ureq::unversioned::multipart::{Form, Part}` (`client.rs:235`) - the unstable surface the pin fences. The `Form`/`Part` builder there is *write-only* (`Form::new().part(…).text(…)` then `send(form)` consumes it); no public accessor enumerates a built `Form`'s parts. **The `UploadParts` descriptor struct is MANDATORY** - CI asserts against the pre-build descriptor; the loopback stub asserts the post-build wire bytes. Do not attempt to introspect a `ureq::Form`.

## T.2 Dev-instance live test matrix (EXHAUSTIVE - D-T-live + retry/backoff/exactly-once/.stalx canary)

Every row runs against the real box (server-truth). The **CI twin** names the always-run seam test guarding the same client reaction. Two-account rows RUN per D-T-2acct.

| # | Live test (`speleodb/mod.rs::live_tests`) | Endpoint(s) | Precondition | Expected server truth | CI seam twin |
|---|---|---|---|---|---|
| L1 | `live_bad_credentials_returns_400` *(exists)* | `POST /user/auth-token/` | `DEV_SPELEODB_URL` only | bad creds → **400** (not 401) | `map_status(Login,400)==Fatal`; `parse_error_message` shape |
| L2 | `live_round_trip_seed_pull_open` *(exists)* | auth → create → acquire → upload/auto → release → download/dump | full single-acct env + `.stalx` fixture | create→seed→pull round-trips; `.stalx` opens self-contained | `dump::classify` arms; `UploadParts` descriptor |
| L3 | `live_fetch_user_email_matches_token_owner` *(exists)* | `GET /user/` | full single-acct env | whoami returns owner email; the 401-clean chokepoint (D-B4) | `UserInfo` parse; `map_status(User,401)==ReLogin` |
| L4 | `live_missing_auth_header_returns_403` **(new)** | `GET /projects/` no `Authorization` | `DEV_SPELEODB_URL` only | missing header → **403** (vs dead token → 401) | `map_status(ListProjects,403)==ReadOnly` + `(*,401)==ReLogin` |
| L5 | `live_dead_token_returns_401` **(new)** | `GET /user/` with revoked token | `DEV_SPELEODB_URL` (junk token) | dead token → **401** | synthetic `Status{401}` → 401-drop handler full-clear unit |
| L6 | `live_collaborator_sees_read_only_label` *(exists, 2acct)* | auth×2, permission detail, `GET /projects/` | full env + `_EMAIL2/_PASSWORD2` | `READ_ONLY` parses to `Permission::ReadOnly` (string label) | `permission_labels_round_trip`, `project_tolerates_omitted_and_null_optionals` |
| L7 | `live_web_viewer_cannot_download` *(exists, 2acct)* | seed + grant WEB_VIEWER + download as viewer | full env + 2acct | `WEB_VIEWER` download → **403** | `map_status(DownloadDump,403)==ReadOnly`; `WebViewer < ReadOnly` |
| L8 | `live_lock_holder_reflects_other_user` *(exists, 2acct)* | grant RW, collaborator acquire, owner `GET /projects/` | full env + 2acct | owner sees `active_mutex.user == user2@dab.com` | `project_parses_full_v2_payload` lock-holder projection |
| L9 | `live_no_permission_field_absent` **(new, 2acct)** | `GET /projects/` as user with no grant | full env + 2acct | `permission` field **ABSENT** (not null) | `project_tolerates_omitted_and_null_optionals` (absent≡null≡None) |
| L10 | `live_acquire_contended_returns_400` **(new, 2acct)** | collaborator holds mutex, owner acquire | full env + 2acct | contention → **400** (mutex.py - NOT 423/409/412) | `map_status(Acquire,400)`; no first-class 423/409/412 rows |
| L11 | `live_acquire_no_perm_returns_403` **(new, 2acct)** | READ_ONLY/WEB_VIEWER acquire | full env + 2acct | no write-perm acquire → **403** (vs contention 400) | `map_status(Acquire,403)==Fence` |
| L12 | `live_upload_200_returns_hexsha_and_project` **(new)** | seed change + upload/auto | full single-acct env | 200 body carries new `latest_commit.id` + project block | upload-200 → adopt-hexsha + conditional clear-dirty handoff; `with_retry(false)` once |
| L13 | `live_upload_304_when_unchanged` **(new)** | re-upload identical tree | full single-acct env | unchanged tree → **304** | `map_status(UploadAuto,304)==UpToDate` |
| L14 | `live_upload_422_missing_revision` **(new)** | upload/auto malformed payload | full single-acct env | unprocessable → **422** distinct fatal-surface | `map_status(UploadAuto,422)==Fatal` |
| L15 | `live_stalx_round_trips_through_auto` **(new)** | seed `.stalx` via upload/auto then download/dump | full single-acct env | `.stalx` stored verbatim by AUTO catch-all, located by suffix | `dump::classify` suffix tests |
| L16 | `live_auto_rejects_unknown_extension_CANARY` **(new, SKIPPED drift canary)** | upload/auto with unknown extension | un-skipped only when investigating drift | TODAY accepts/stores it; FAILS LOUDLY if AUTO ever starts rejecting unknown extensions | none - by design a server-drift tripwire (D-T-stalx) |
| L17 | `live_retry_succeeds_after_transient` **(loopback-primary)** | `GET` against a flaky endpoint | loopback stub | a read retries transient failure with `[200ms,500ms]` backoff and succeeds | loopback `503→503→200`; pure `with_retry(true,…)` |
| L18 | `live_write_attempted_exactly_once` **(loopback)** | `PUT upload/auto/` dropped mid-body | loopback stub | non-idempotent write attempted **once** (no double-submit) | loopback counts 1 connection; `with_retry(false)` `max_retries==0` |
| L19 | `live_http_to_public_blocked` **(#187)** | any `GET` to a public `http://` host | URL pointed at a public hostname over http | **blocked before dial** (`InsecureEndpoint`) unless `insecure_http` set | `enforce` + `classify_addr` public→block |
| L20 | `live_http_to_private_box_allowed` **(#187)** | `GET /projects/` to `http://10.10.0.220:8000/` | full single-acct env (box IS RFC-1918) | private/loopback http **allowed** | `classify_addr(10.10.0.220)==Private`; `[::1]==Loopback` |

> NOTE (OPEN-1, verify at #185): the `.tml` bare-`.file()` "fails every push" rationale is overstated - Ariane's allow-list includes `text/plain`, so a missing-Content-Type `.tml` may PASS. L15/L16 keep explicit MIME (`.tml→application/octet-stream`, `.stalx→application/vnd.sqlite3`) as a **robustness rule, not a hard server requirement** - the server routes on *extension* via the AUTO catch-all. Correct the comment at `client.rs:243-244` when promoting `upload_auto`.

## T.3 Always-run CI unit inventory (D-T-unit), by track

`✓` = already exists in the spike and is retained.

**Track A - #185 transport (`client.rs` inline):** `✓ api_url_joins_without_double_slash`, `✓ bearer_header_format`, `✓ parse_error_message_handles_v2_shapes`, `✓ user_info_parses_email_and_ignores_extra_fields`; `upload_parts_both_named_artifact_never_bracketed`; `upload_parts_carry_explicit_mime`; `upload_auto_rejects_blank_message`; `project_dto_defaults_exclude_geojson`; `get_project_parses_single_project`; `with_retry_reads_retry_transient_then_succeed`; `with_retry_writes_run_exactly_once`; `with_retry_idempotent_5xx_is_retried`; `with_retry_calls_tls_enforce_first` (public `http://8.8.8.8` → `Err(InsecureEndpoint)` and the closure is never invoked); loopback: `loopback_read_retries_503_then_200`, `loopback_write_attempted_once`, `loopback_multipart_parts_on_the_wire`.

**Track A - #186 contract (`contract.rs` inline):** `map_status_per_row` (table-driven: `{Login,400}`,`{User,401}`,`{ListProjects,403}`,`{GetProject,403}`,`{DownloadDump,403}`,`{Acquire,400}`,`{Acquire,403}`,`{UploadAuto,403}`,`{UploadAuto,304}`,`{UploadAuto,422}`,`{UploadAuto,5xx}`); `map_status_unmapped_fails_closed` (418/451/409/423 → Fatal); `map_status_422_is_distinct_fatal`; `map_status_upload_304_is_uptodate`; `map_status_idempotent_5xx_retry`; `map_status_write_5xx_stay_dirty`; `parse_error_message_is_display_only`; `error_display_strings_are_stable` (extend with `InsecureEndpoint`); `speleo_action_ts_export`. D-B2: `ipc_dtos_never_serialize_a_token` (extend with `SpeleoErrorDto`), `speleo_error_dto_maps_status_to_action`.

**Track A - #187 TLS (`tls.rs` inline):** `classify_addr_private_v4_ranges` (`10.10.0.220`→Private, `127.0.0.1`→Loopback, `10/172.16/192.168`→Private, `169.254`→LinkLocal); `classify_addr_public_v4_blocks` (`8.8.8.8`,`172.32.0.1`,`1.1.1.1`); `classify_addr_v6` (`::1`,`fc00::1`,`fe80::1`,`2001:db8::1`); `classify_addr_v4_mapped_stripped`; `enforce_https_short_circuits`; `enforce_http_public_blocked`; `enforce_http_private_allowed`; `enforce_localhost_resolution_branch`; `enforce_missing_scheme_rejected`; `enforce_empty_resolution_fails_closed`; `enforce_insecure_http_bypasses_public_block`.

**Track B - #189 (`persistence/sqlite/tests.rs` - no telemetry):** `speleo_link_roundtrips_through_save_and_load`; `speleo_link_null_base_sha_stays_none`; `speleo_link_clear_removes_row`; `init_schema_creates_speleo_link_v9`; `migrate_creates_speleo_link_on_legacy_v8`; `is_remote_true_when_link_present_false_when_absent`; `is_remote_false_on_metadata_open_of_pre_v9_file`; `save_project_does_not_clear_speleo_link`.

**Track B - #190 (pragma_table_info over `PRESERVED_SPELEOLINK_COLUMNS`):** `carry_forward_four_cell_raw_sql_null_stays_null`; `carry_forward_four_cell_typed_api`; `preserved_speleolink_columns_match_head_schema`; `speleo_link_survives_open_then_fast_path_reopen`.

**Track B - #191 (`core/state/tests.rs`, stamped-remote in-memory fixture):** `edit_enabled_defaults_true_and_local_project_passes`; `remote_open_without_lock_disables_edit`; `gate_rejects_every_mutation_entrypoint_with_edit_disabled_code` (the enumeration tripwire - all 12 + 4 entries); `lifecycle_commands_pass_through_gate`; `dirty_bit_flips_false_to_true_on_successful_remote_edit`; `dirty_bit_is_idempotent_and_set_only`; `dirty_bit_noop_for_local_only_project`; `edit_disabled_code_prefix_is_distinct`; `repair_and_settings_gated_on_locked_remote`.


> NOTE (D-B6 assumption, confirm at review): collab toggles (`set_collab_enabled`/`_for_path`) and TML imports (`execute_import_sections`) are treated as **mutations** and ARE gated on an unlocked remote project - flag for explicit sign-off.

## T.4 Consolidated manual test guide (human walkthroughs)

Prereqs: built dev app (`./scripts/dev.sh`), the dev box reachable, the `.env` values to hand. The temporary `SpeleoDbSpikeDialog.tsx` is the entry surface until #195/#205.


**M2 - Open a remote project → `is_remote` badge + read-only gate:** log in, open a non-locked project → `.stalx` downloads/extracts/opens; project shows an `is_remote` badge (derived from `speleo_link`, #189/#190; recent-projects panel shows it too); project is read-only (`edit_enabled == false`).

**M3 - Attempt edits/undo/redo/repair/settings → `EDIT_DISABLED`:** on the read-only remote project, each of edit/delete/move/merge, Undo/Redo (and undo-to/redo-to), Repair (rejected AND the banner still dismissable without repairing - D-B5 verify), settings change → a distinct `EDIT_DISABLED` rejection (suppressed-toast in #204). Sanity: Save/autosave still works and the dirty bit does NOT get set by a local save. Open a local-only project → all the above work normally (gate is a no-op off-SDB).

**M4 - `http`-to-public blocked vs `insecure_http` allowed (#187):** public `http://example.com` with `insecure_http` unset → insecure-endpoint rejection before any dial. Enable `insecure_http` → http scheme block lifted (self-signed *https* still rejects). `http://10.10.0.220:8000/` works WITHOUT the flag. No-scheme `10.10.0.220:8000` → clear "explicit http/https required" error.

**M5 - 401 auto-drop (#188):** log in, revoke the token box-side, trigger any authed call → 401 detected, session fully cleared, dialog returns to logged-out. Contrast: cause a 403 (list as a no-grant user) → token NOT dropped.

## T.5 Env / setup checklist

**One-time clean-clone setup (CLAUDE.md "Pinned Toolchain"):**
```bash
corepack enable      # pin pnpm to 9.15.9 via packageManager
pnpm install         # build scripts pre-approved (pnpm.onlyBuiltDependencies)
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

**Always-run CI bar (Rust half - must pass before every push):**
```bash
TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml --features telemetry
cargo fmt --manifest-path src-tauri/Cargo.toml --check
TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo clippy --manifest-path src-tauri/Cargo.toml --features telemetry --all-targets
```
`--features telemetry` + `TELEMETRY_ENDPOINT` are mandatory. Windows runs the Rust tests via `./scripts/test.ps1` (`STALAGMIGHT_TEST_MANIFEST=1`; without it the `comctl32`/`TaskDialogIndirect` loader abort hits every test binary).

**Live tests (single-account - never in CI):**
```bash
DEV_SPELEODB_URL=http://10.10.0.220:8000/ \
DEV_SPELEODB_EMAIL=dab@dab.com \
DEV_SPELEODB_PASSWORD=dab \
TELEMETRY_ENDPOINT=http://localhost/dummy \
cargo test --manifest-path src-tauri/Cargo.toml --features telemetry \
  speleodb::live_tests -- --ignored
```

**Live tests (two-account rows - RUN per D-T-2acct, add these two vars):**
```bash
DEV_SPELEODB_EMAIL2=user2@dab.com \
DEV_SPELEODB_PASSWORD2=ChangeMe123! \
# …plus the four vars above; run with -- --ignored
```
`user2@dab.com` must be **pre-provisioned with a verified email box-side** (a verified user can't be created over HTTP - mandatory email verification). The superuser (`dab@dab.com`) provisions extra accounts; the two-account tests self-seed projects/grants/locks and tear down via the `ProjectCleanup` `Drop` guard.

| Env var | Meaning | Used by |
|---|---|---|
| `DEV_SPELEODB_URL` | dev box base URL; client appends `/api/v2/…` | all live rows (L1 needs only this) |
| `DEV_SPELEODB_EMAIL` / `_PASSWORD` | primary superuser test account | single-account suite |
| `DEV_SPELEODB_EMAIL2` / `_PASSWORD2` | pre-verified collaborator `user2@dab.com` | two-account rows (L6-L11) |
| `TELEMETRY_ENDPOINT` | required under `--features telemetry` (`build.rs`) | every Rust test command |

`.env` at repo root is the gitignored local record only - `cargo test` does NOT auto-load it; vars are passed on the command line. Box-side recovery if a run leaks: `manage.py wipe_test_user_projects` and `manage.py wipe_test_gitlab` (run on the SpeleoDB box).

## T.6 Telemetry / registry gate (no new events in Tracks A-C)

`EVENT_NAMES` is pinned at **418** (`registry.rs:560`). Tracks A-C add **no new IPC commands** beyond what is strictly required (`speleodb_open_project` gains *args*, not a new command name), and per-endpoint SpeleoDB telemetry is OUT (#210). So **no `EVENT_NAMES` / 418 bump is expected**. The #186 ts-rs `SpeleoDbAction` export (D-B2) is an IPC *type* (`pnpm gen:ipc-types`), not a telemetry event, and does not touch the 418 literal. If any track is forced to add an IPC command, bump `EVENT_NAMES` + the 418 literal in the **same commit** and call it out (D-T-telemetry).

> NOTE (OPEN-4, fix if `registry.rs` is touched): the registry test messages reference `telemetry-worker/src/registry.ts` (`registry.rs:574,591`), which **does not exist** - the worker source is only `index.ts`. New events need only `EVENT_NAMES` + the 418 bump. If you edit `registry.rs` for any reason, correct the misleading message in the same change.

---

## Open verify-steps (pre-merge checks, NOT blockers)

The four OPEN UNKNOWNS from the ledger, as explicit pre-merge checks:

1. **OPEN-1 - `.tml` bare-`.file()` rationale (RE-CORRECTED at #184).** The earlier "correction" was itself wrong: it claimed *Ariane's allow-list includes `text/plain` so a missing-type `.tml` may PASS*, but that allow-list belongs to the `.tmlu`/`.agr` processors. The actual `.tml` processor is `ArianeTMLFileProcessor.ALLOWED_MIMETYPES = ["application/octet-stream","application/zip"]` (server source `speleodb/processors/_impl/ariane.py`), enforced in `processors/artifact.py::assert_valid`; selection is by **extension** (`auto_selector.py`: `.tml` → `ArianeTMLFileProcessor`). So a `.tml` part sent as `text/plain` IS **rejected → 400** - explicit `application/octet-stream` is a genuine server requirement for `.tml`, not merely robustness. **RESOLVED live (#184, 2026-06-27, `live_open1_bare_tml_mime_outcome`):** a bare `ureq` `Part::file()` `.tml` part (no explicit `Content-Type`) is rejected with **HTTP 400** - ureq's default is not in the `{octet-stream, zip}` allow-list. So the explicit MIME in `client::mime_for` is **load-bearing for `.tml`, not robustness**; the comment there has been corrected accordingly. Render-compat in the same run is GREEN (georef `.tml` → `ProjectGeoJSON`, 1 point + 2 lines). See `docs/speleodb-dev-testing.md` § Render-compat.
2. **OPEN-2 - ureq 3.1.4 `Form` parts accessor (RESOLVED → MANDATORY).** Confirmed: `Form` exposes only `boundary()` + `io::Read`, no `parts()`; `Form::part` consumes the `Part`. **Action:** the `UploadParts` descriptor struct is **mandatory** for CI part-name/MIME coverage; the loopback stub asserts the on-the-wire bytes. Do not attempt to introspect a built `Form`. (Re-verify if the pinned ureq version is ever bumped.)
3. **OPEN-3 - integrity banner does not hard-require Repair to dismiss (RESOLVED by code-read; re-confirm before committing the #191 gate).** `App.tsx:361/382`: a *non-blocking* banner is dismissable without repairing; a `save_blocked` (critical) banner is not. **Action:** before gating Repair under read-only remote (D-B5), confirm the FE still allows dismissing a non-blocking banner; accept the `save_blocked` dead-end (owner-with-lock responsibility) and flag the Repair-affordance suppression for #204.
4. **OPEN-4 - correct the misleading `telemetry-worker/src/registry.ts` message (only if `registry.rs` is touched).** Tracks A-C do not bump `EVENT_NAMES` and should not touch `registry.rs`. **Action:** if any change does touch it, correct the test message (`registry.rs:574,591`) - the worker source is only `index.ts`, not `registry.ts` - in the same commit.

---

## Risks

1. **The `execute()` chokepoint refactor blast radius (#191/D-B6).** Routing ~8 `execute_*` bypass paths + `execute`/`undo`/`redo`/`undo_to`/`redo_to` through one `guard_mutation` chokepoint touches the hottest mutation paths in the app. Mitigations: split #191 into **191-B6** (mechanical re-routing, no behavior change, tests stay green) before **191-A** (gate logic); the `is_lifecycle_passthrough` exhaustive `match` forces a deliberate choice for every new `Command` variant (anti-fail-open, like `command_integrity`); the `chokepoint_enumeration_test` (a behavioral tripwire over a read-only remote fixture) fails the moment a new mutation path forgets the guard. The two awkward seams - `set_collab_enabled_for_path` (operates by path, not the open project) and `repair_open_project_integrity` (returns `IntegrityRepairResult`, not `CommandResult`, so it can't carry the coded failure and instead returns `changed:false`) - are documented in-place to avoid a future direct caller bypassing the gate.
2. **DNS-rebinding / TOCTOU accepted, NOT mitigated (#187/D-187).** `tls.rs::enforce` resolves and classifies addresses per-request at the top of `with_retry`, but a hostname that resolves to a private address at check-time and a public one at connect-time (DNS rebinding) is **explicitly accepted** as out of scope. The policy is a guard against accidental plaintext-to-public misconfiguration, not an adversarial SSRF defense. `insecure_http` further widens this by design (public-http allowed when set). Documented so no reviewer mistakes the IP policy for a hardened SSRF boundary.
4. **`.stalx` catch-all coupling (D-T-stalx).** The hand-off relies on the SpeleoDB server's AUTO catch-all (`BaseFileProcessor`) storing the `.stalx` verbatim and the dump locating it **by suffix** (`dump.rs:58`), not by a hard-coded path. If the server ever starts *rejecting* unknown extensions, every push/pull breaks silently. Mitigations: an explicit live round-trip assertion (L15) + a SKIPPED drift canary (L16, `live_auto_rejects_unknown_extension_CANARY`) that flags the moment AUTO changes behavior; the dump locator never hard-codes `misc/project.stalx`. Note the explicit-MIME robustness rule (D-A-mime) does **not** by itself protect against extension-rejection - the catch-all routing on *extension*, not MIME, is the real coupling.
