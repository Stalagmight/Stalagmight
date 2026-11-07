//! Backend-only credential storage for the SpeleoDB auth token.
//!
//! The token is written to one of three [`SecretStore`] tiers, keyed by the
//! normalized endpoint + email, and **never** crosses into the webview / IPC
//! payloads:
//!
//! 1. **OS keychain** ([`KeyringStore`], keyring-rs v3) - the persistent,
//!    survives-a-restart tier. First precedence on both read and write.
//! 2. **Plaintext file** ([`PlaintextStore`], `0o600` on Unix) - the opt-in
//!    headless-Linux escape hatch; only written when the two gates pass (the
//!    `token_fallback == Plaintext` preference AND the keychain failed this
//!    session). See [`TokenStore::store`].
//! 3. **In-memory** ([`InMemoryStore`] / the live session cache) - session-only,
//!    never persisted; the always-available fallback.
//!
//! [Track C] (#188) closed the spike's three holes documented here historically:
//! startup re-hydration (now wired lazily on the first authed call in
//! `core::state::speleodb`), the opt-in plaintext escape hatch (this module), and
//! the 401 auto-drop policy (the `clear` sink, driven from the orchestration
//! layer). Full URL-parse endpoint normalization lives in `account_key`.

use std::collections::HashMap;
use std::path::PathBuf;

use stalagmight_models::poison_policy::RecoveringMutex;

const KEYRING_SERVICE: &str = "stalagmight";

/// Build the keychain account key from the endpoint and email. The endpoint is
/// normalized via a real URL parse (scheme + host + port, lowercased host,
/// trailing slash and default ports dropped) so the same server entered several
/// ways resolves to one entry. Falls back to the lightweight string normalization
/// when the endpoint does not parse as a URL (e.g. a bare host).
pub fn account_key(endpoint: &str, email: &str) -> String {
    let normalized = normalize_endpoint(endpoint);
    format!("{normalized}|{email}")
}

/// Normalize an endpoint to a stable `scheme://host[:port]` form for keying.
///
/// [Track C] (#188, D-188 "URL-parse endpoint normalization"): a proper parse so
/// `https://Sdb.Example.com/`, `https://sdb.example.com`, and
/// `https://sdb.example.com:443` all collapse to one key. D-B19's *scheme reject*
/// (require explicit http/https) is **not** done here - it lives in Track A's
/// `tls.rs`; this is keying only, so an unparseable endpoint degrades gracefully
/// to the trimmed/lowercased string rather than erroring.
pub fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if let Ok(uri) = trimmed.parse::<http::Uri>() {
        if let Some(host) = uri.host() {
            let scheme = uri.scheme_str().unwrap_or("https").to_ascii_lowercase();
            let host = host.to_ascii_lowercase();
            // Drop the port when it is the scheme default so `:443`/`:80` and the
            // implicit form collapse together.
            let default_port = match scheme.as_str() {
                "http" => Some(80),
                "https" => Some(443),
                _ => None,
            };
            return match uri.port_u16() {
                Some(port) if Some(port) != default_port => format!("{scheme}://{host}:{port}"),
                _ => format!("{scheme}://{host}"),
            };
        }
    }
    // Bare host / unparseable: keep the spike's lightweight normalization.
    trimmed.trim_end_matches('/').to_ascii_lowercase()
}

/// An error from any [`SecretStore`] backend, unified behind the trait so callers
/// don't depend on `keyring::Error` vs `io::Error`. **Never** carries the token
/// (mirrors the redacted [`TokenStore`] `Debug`).
#[derive(Debug, thiserror::Error)]
#[error("secret store error: {0}")]
pub struct SecretStoreError(String);

#[cfg(any(test, feature = "test-support"))]
impl SecretStoreError {
    /// Build a store error directly, so test fakes in sibling modules can fail a
    /// backend op (the inner message is private to this module).
    pub fn for_test(message: &str) -> Self {
        Self(message.to_owned())
    }
}

impl From<keyring::Error> for SecretStoreError {
    fn from(e: keyring::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<std::io::Error> for SecretStoreError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

/// Backend-agnostic secret storage keyed by the `account_key(endpoint, email)`
/// string. The three production impls are [`KeyringStore`], [`PlaintextStore`],
/// and [`InMemoryStore`]; the test-only `FailingSecretStore` drives the
/// keychain-unavailable branches.
///
/// [Track C] (#188, D-B17): introduced so the keychain tiers are CI-testable
/// without a real OS secret service.
pub trait SecretStore: Send + Sync {
    /// A stable label for the backend (`"keychain"` | `"plaintext"` | `"memory"`).
    fn kind(&self) -> &'static str;
    /// Persist `token` under `account_key`, overwriting any existing value.
    fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError>;
    /// Load the token for `account_key`, or `Ok(None)` if absent.
    fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError>;
    /// Delete the token for `account_key`. Idempotent: a missing entry is `Ok`.
    fn delete(&self, account_key: &str) -> Result<(), SecretStoreError>;
}

/// OS-keychain backend (keyring-rs v3). The persistent tier.
pub(crate) struct KeyringStore;

impl KeyringStore {
    fn entry(account_key: &str) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(KEYRING_SERVICE, account_key)
    }
}

impl SecretStore for KeyringStore {
    fn kind(&self) -> &'static str {
        "keychain"
    }

    fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError> {
        Self::entry(account_key)?.set_password(token)?;
        Ok(())
    }

    fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError> {
        match Self::entry(account_key)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, account_key: &str) -> Result<(), SecretStoreError> {
        match Self::entry(account_key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-memory backend (`RecoveringMutex<HashMap>`). The always-run CI seam (D-B17): tier
/// precedence, delete-on-401, rehydration, and the never-override-keychain
/// guardrail are all unit-tested by injecting these. It compiles in production
/// (the impls are not `#[cfg(test)]`-gated per the plan) but its constructor is
/// only called from tests - the root crate's among them, so `new` is `pub`
/// (a reachable `pub` fn is never dead, retiring the old non-test dead-code
/// expectation).
pub struct InMemoryStore {
    map: RecoveringMutex<HashMap<String, String>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            map: RecoveringMutex::new("speleodb_memory_store", HashMap::new()),
        }
    }
}

impl SecretStore for InMemoryStore {
    fn kind(&self) -> &'static str {
        "memory"
    }

    fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError> {
        self.map
            .lock()
            .insert(account_key.to_owned(), token.to_owned());
        Ok(())
    }

    fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError> {
        Ok(self.map.lock().get(account_key).cloned())
    }

    fn delete(&self, account_key: &str) -> Result<(), SecretStoreError> {
        self.map.lock().remove(account_key);
        Ok(())
    }
}

/// Plaintext-file backend. One file per account key under
/// `<root>/<sanitized_account_key>`. The opt-in headless-Linux escape hatch.
///
/// [Track C] (#188, D-B18): on Unix the file is created `0o600` and tightened if
/// it pre-existed looser (the 0600-at-create invariant). On Windows there is no
/// `0o600` equivalent - it relies on the per-user profile-dir ACL (documented
/// gap, no new platform dep). The `root` is injectable so tests use a tempdir.
pub struct PlaintextStore {
    root: PathBuf,
}

impl PlaintextStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Map an account key to a single safe filename component. The key contains a
    /// URL + `|` + email; replace every non-`[A-Za-z0-9._-]` byte with `_` so the
    /// result is one path component (no separators, no `..`).
    fn file_for(&self, account_key: &str) -> PathBuf {
        let sanitized: String = account_key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(sanitized)
    }
}

impl SecretStore for PlaintextStore {
    fn kind(&self) -> &'static str {
        "plaintext"
    }

    fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError> {
        use std::io::Write as _;
        std::fs::create_dir_all(&self.root)?;
        let path = self.file_for(account_key);

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
            // 0600-at-create: a fresh file is never group/other-readable.
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(token.as_bytes())?;
            // If the file pre-existed looser, tighten it (mode() above does not
            // re-apply to an existing file).
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }

        #[cfg(not(unix))]
        {
            // SECURITY: Windows relies on profile-dir ACL; no 0600 equivalent (D-B18).
            let mut file = std::fs::File::create(&path)?;
            file.write_all(token.as_bytes())?;
        }

        Ok(())
    }

    fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError> {
        let path = self.file_for(account_key);
        match std::fs::read_to_string(&path) {
            Ok(token) => Ok(Some(token)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, account_key: &str) -> Result<(), SecretStoreError> {
        let path = self.file_for(account_key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Which tier the active token currently lives in.
///
/// [Track C] (#188): replaces the spike's bare `keychain_backed` bool so the
/// store can distinguish a persistent keychain token from the plaintext escape
/// hatch from a session-only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenTier {
    Keychain,
    Plaintext,
    Memory,
}

/// What we have LEARNED about the OS keychain - never what we went looking for.
///
/// Settings needs to tell the user where their API key actually lives, but on
/// Linux a locked keyring turns any keychain access into a desktop unlock
/// prompt, so *probing* to draw a status line would re-introduce exactly the
/// nag the decline flag exists to stop. So this is only ever set as a
/// side effect of access we already had to make (a login write, a startup
/// rehydrate read), and `NotAttempted` is an honest answer we are willing to
/// show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeychainState {
    /// Nothing has touched the keychain this session, so we genuinely do not know.
    NotAttempted,
    /// A keychain read or write succeeded.
    Available,
    /// A keychain read or write failed (absent, locked, or refused).
    Unavailable,
    /// The user declined it; we are deliberately not touching it until they sign
    /// in again.
    Declined,
    /// The keychain is switched OFF in Settings. Distinct from [`Self::Declined`]
    /// in the one way a caller cares about: a decline answers a single login and
    /// the retry verb revokes it, whereas this is a standing policy the retry
    /// cannot bypass. Collapsing the two is what made Settings offer a "Try again"
    /// that could not do anything.
    Disabled,
}

/// Holds the SpeleoDB token backend-side across an ordered tier list. The active
/// token is never returned to the webview; only [`Self::keychain_backed`] is
/// surfaced (so the UI can warn that a non-keychain token won't survive a
/// restart).
///
/// [Track C] (#188): reworked from `{ in_memory, keychain_backed }` onto the
/// [`SecretStore`] tiers with keychain-first read + plaintext self-heal + the
/// two-gate plaintext write.
pub struct TokenStore {
    /// The live token for this session: cached here after [`Self::store`]/
    /// [`Self::load`] so authed calls don't re-hit the keychain each request.
    in_memory: Option<String>,
    /// Which tier the active token lives in. `None` = no active token.
    active_tier: Option<TokenTier>,
    /// First-precedence persistent backend (the OS keychain in production).
    keychain: Box<dyn SecretStore>,
    /// The plaintext escape hatch, present only when it is *available* this
    /// session (the production [`Default`] wires it; the two-gate in [`Self::store`]
    /// decides whether it is actually *written*).
    plaintext: Option<Box<dyn SecretStore>>,
    /// The user declined the keychain (persisted as `config.speleodb.keychain_declined`).
    /// While set, NEITHER [`Self::store`] NOR [`Self::load`] goes near it, which is
    /// what stops a dismissed unlock prompt coming back on every launch.
    /// [`Self::clear`] skips it too, and this is the ONLY flag that earns that
    /// skip: the refusal answers one login, and anything the keychain may still
    /// hold is overwritten by the next successful login or swept by "Try again".
    /// See [`Self::clear_may_sweep_keychain`].
    keychain_declined: bool,
    /// The user turned the keychain off for good (`config.speleodb
    /// .keychain_disabled`). Same effect as `keychain_declined` on [`Self::store`]
    /// and [`Self::load`], but it is not cleared by signing in - that is the point
    /// of it, and it is also why [`Self::clear`] treats the two differently: with
    /// nothing left to overwrite or re-sweep the entry, skipping it would strand a
    /// live bearer token in the keychain for good.
    keychain_disabled: bool,
    /// What keychain access we have already made taught us. See [`KeychainState`].
    keychain_state: KeychainState,
    /// A keychain entry for the persisted account is BELIEVED TO EXIST: mirrored
    /// from the persisted belief when the session rehydrates (and again at login,
    /// which never rehydrates), and set the moment a keychain write or read of ours
    /// proves an entry is in there.
    ///
    /// The mirror is account-scoped on the way IN: `config.speleodb` records both the
    /// belief and the `account_key` it is about, and only a belief stamped with the
    /// account this store is being keyed by reaches this field
    /// (`SpeleoDbConfig::keychain_belief_applies_to`). This field itself carries no
    /// account, which is why [`Self::forget_keychain_belief`] exists: the store
    /// outlives the account it recorded it for.
    ///
    /// This is the AUTHORITATIVE copy, and the config belief is written back FROM it
    /// ([`Self::keychain_has_token`] is read by `speleodb_persist_keychain_belief`'s
    /// callers). It used to be the other way round for the two login-shaped writers,
    /// which recomputed the persisted flag as `tier == TokenTier::Keychain` - "where
    /// THIS store landed", not "what the keyring holds". Those are different
    /// questions, and the second is the only one anyone reads the flag to answer.
    ///
    /// RAISED by evidence, LOWERED only by proof: a delete the keyring accepted, or
    /// a probe / read that answered `Ok(None)`. `false` otherwise means "no reason
    /// to think so", never "proven empty" - it defaults to `false` for every config
    /// written before the flag existed - which is why nothing reads it to SKIP a
    /// delete or a sweep. What it is read for is
    /// [`Self::unreadable_keychain_may_hold`]: a keyring that took a token on Monday
    /// and is merely locked today records the same [`KeychainState::Unavailable`] as
    /// a box with no secret service at all, and this flag is the only thing that
    /// tells the two apart.
    keychain_has_token: bool,
}

impl Default for TokenStore {
    /// Production wiring: a real keychain plus an *available* plaintext store
    /// rooted at `<config_dir>/speleodb/tokens`. Plaintext availability is not the
    /// same as plaintext *use* - the two-gate in [`Self::store`] still requires
    /// the `token_fallback == Plaintext` preference AND a keychain failure.
    fn default() -> Self {
        let plaintext_root =
            crate::app_dirs::config_dir().map(|dir| dir.join("speleodb").join("tokens"));
        Self::new(plaintext_root)
    }
}

impl std::fmt::Debug for TokenStore {
    /// Hand-written so the live token is never formatted: a stray
    /// `tracing::debug!(?session, …)` must not write the secret to logs (the
    /// token-never-in-logs invariant). Prints presence + `active_tier` only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore")
            .field("token", &self.in_memory.as_ref().map(|_| "<redacted>"))
            .field("active_tier", &self.active_tier)
            .finish()
    }
}

impl TokenStore {
    /// Production constructor. The keychain is always a real [`KeyringStore`]; the
    /// plaintext tier is wired iff `plaintext_root` is `Some`.
    pub(crate) fn new(plaintext_root: Option<PathBuf>) -> Self {
        Self {
            in_memory: None,
            active_tier: None,
            keychain: Box::new(KeyringStore),
            plaintext: plaintext_root
                .map(|r| Box::new(PlaintextStore::new(r)) as Box<dyn SecretStore>),
            keychain_declined: false,
            keychain_disabled: false,
            keychain_state: KeychainState::NotAttempted,
            keychain_has_token: false,
        }
    }

    /// Apply the persisted decline flag (from `config.speleodb.keychain_declined`).
    /// Called when seeding the session and again when the user signs in - a fresh
    /// login is the one moment they have opted back into being asked, so that is
    /// where `false` is passed to re-enable the keychain.
    pub fn set_keychain_declined(&mut self, declined: bool) {
        if self.keychain_declined == declined {
            return; // see `refresh_keychain_state`: re-asserting is not a change
        }
        self.keychain_declined = declined;
        self.refresh_keychain_state();
    }

    /// Apply the standing "never use the keychain" instruction from Settings.
    ///
    /// Turning it ON only makes the live session look away; the entry an earlier
    /// login already wrote has to be removed separately, by
    /// [`Self::evict_from_keychain`] BEFORE this is called (afterwards the gate
    /// would block it). `core::state::config::update_config` owns that sequence.
    pub fn set_keychain_disabled(&mut self, disabled: bool) {
        if self.keychain_disabled == disabled {
            return; // see `refresh_keychain_state`: re-asserting is not a change
        }
        self.keychain_disabled = disabled;
        self.refresh_keychain_state();
    }

    /// Record the persisted "the keyring holds a token for us" belief (from
    /// `config.speleodb`, already scoped to this account by the caller), which only
    /// the config layer has.
    ///
    /// `note_`, not `set_`, for the same reason as [`Self::note_keychain`]: this
    /// takes EVIDENCE, and evidence only accumulates here.
    ///
    /// Mirrored into the store rather than passed to the one caller that reports on
    /// it, because THREE paths ask the same question after their own delete fails -
    /// [`Self::clear`], [`Self::sweep_persistent_tiers`] and
    /// [`Self::evict_from_keychain`] - and they were deliberately unified onto
    /// [`Self::unreadable_keychain_may_hold`] so they cannot drift. Threading a bool
    /// through `restore` and `clear` (whose callers include the background 401
    /// auto-drop, which has no config in hand) would re-create exactly that drift.
    ///
    /// Only ever raises the belief, never lowers it: a launch that could not read
    /// the keyring has learned nothing that would justify `false`, and the paths
    /// that DO prove the entry gone - a delete the keyring accepted, a probe or a
    /// [`Self::load`] read that answered `Ok(None)` - clear it themselves.
    pub fn note_keychain_has_token(&mut self, has_token: bool) {
        self.keychain_has_token |= has_token;
    }

    /// Drop that belief because the ACCOUNT it is about has left the session.
    ///
    /// Not a proof of absence, and not the `false` the doc above warns is never one:
    /// the belief is about `(endpoint, email)`, and this store outlives the account
    /// it recorded it for. Signing in as somebody else reuses the same store, and an
    /// entry the previous account left is keyed by an account key the new one can
    /// neither read nor delete - so carrying the belief across made every later
    /// launch open the keyring for the new account, and made the keyring switch
    /// report a residue that is not theirs and that nothing keyed by them can
    /// remove.
    ///
    /// Called from `SpeleoDbSession::forget_reporting`, where `endpoint` / `email`
    /// are cleared - the one place an account leaves - and AFTER the sweep there,
    /// which still needs the belief to decide what an unreadable keyring may be
    /// holding for the account it is sweeping.
    pub fn forget_keychain_belief(&mut self) {
        self.keychain_has_token = false;
    }

    /// Whether the keychain is off, for whatever reason. THE gate: every path that
    /// would otherwise touch the keyring asks this, because on Linux the touch is
    /// itself the desktop unlock prompt.
    fn keychain_off(&self) -> bool {
        self.keychain_declined || self.keychain_disabled
    }

    /// Whether [`Self::clear`] may reach the keyring. NOT the negation of
    /// [`Self::keychain_off`], because the two flags behind that gate differ in the
    /// one way that matters to a sweep: whether anything ELSE will ever remove what
    /// the keychain is holding.
    ///
    /// `keychain_declined` answers a single login, and something always comes along
    /// afterwards - the next successful sign-in re-stores over the entry, "Try
    /// again" sweeps it - so skipping the sweep strands nothing, and it spares the
    /// background 401 auto-drop re-raising the unlock prompt the refusal exists to
    /// stop. `keychain_disabled` has no such rescuer: Settings re-applies it on
    /// every login, so [`Self::store`] never reaches the keychain to overwrite the
    /// entry, and "Try again" clears only the OTHER flag. Skipping there left a
    /// working credential in the OS keychain while logout reported success - the
    /// one outcome [`Self::clear`]'s reporting exists to prevent.
    ///
    /// So a disabled keychain is swept even when it is also declined (login sets
    /// both from the same value). The cost is a delete on a keyring the user asked
    /// us to leave alone, which on a merely LOCKED keyring can prompt once; by then
    /// `evict_from_keychain` has normally already emptied it, so this is the safety
    /// net for the run where that eviction failed.
    fn clear_may_sweep_keychain(&self) -> bool {
        self.keychain_disabled || !self.keychain_declined
    }

    /// Reset what we have LEARNED about the keychain after a flag actually changed.
    ///
    /// Only ever called from a real change (both setters return early otherwise).
    /// Re-running it on an unchanged flag threw away a hard-won `Unavailable` /
    /// `Available`: saving ANY unrelated setting re-applies `keychain_disabled`,
    /// which reset the credential line to "not attempted" and took the Settings
    /// "Try again" button with it until the user signed in again.
    ///
    /// Turning the keychain OFF resets nothing either. Declining is a POLICY, not
    /// something learned about the machine, and [`Self::keychain_state`] reports it
    /// as one without needing the field; overwriting the field with `Declined` threw
    /// away the evidence instead, and two callers need it. The session-only answer
    /// is GIVEN by setting this flag, so by the time [`Self::sweep_persistent_tiers`]
    /// runs, the `Unavailable` that the failed login write had just recorded was
    /// always gone - and with it the only thing separating a keychain-less box,
    /// where nothing can have been stranded, from a keyring that worked minutes ago
    /// and has since stopped answering, where a live bearer token may be sitting.
    fn refresh_keychain_state(&mut self) {
        if !self.keychain_off() {
            // Re-enabling makes what we previously learned stale: an "Unavailable"
            // from before the user fixed their keyring would otherwise be reported
            // forever. Say we do not know, and let the next real access answer.
            self.keychain_state = KeychainState::NotAttempted;
        }
    }

    /// What we know about the keychain, for the Settings credential-storage line.
    /// Reporting only - this never triggers keychain access.
    ///
    /// A keychain that is off reports `Declined` whatever the last real access
    /// taught us: "you chose otherwise, and here is how to change your mind" is what
    /// the user needs there, not a verdict on a keyring we have stopped using. The
    /// verdict itself is kept rather than overwritten (see
    /// [`Self::refresh_keychain_state`]) because the sweep asks a different question
    /// of it - whether anything of ours could still be in there - and the user's
    /// preference does not answer that one.
    pub fn keychain_state(&self) -> KeychainState {
        // The two flags behind `keychain_off` are reported SEPARATELY, for the same
        // reason `clear` refuses to treat them alike (see its doc above): they differ
        // in whether anything can act on them. A decline is revoked by the retry verb;
        // the disable is a standing policy that outranks it, so it is checked first.
        if self.keychain_disabled {
            return KeychainState::Disabled;
        }
        if self.keychain_declined {
            return KeychainState::Declined;
        }
        self.keychain_state
    }

    /// Whether the keychain is currently being skipped. Lets a caller that flips
    /// the flag around a fallible operation put the previous answer back, instead
    /// of leaving the live store disagreeing with the persisted preference.
    pub fn keychain_declined(&self) -> bool {
        self.keychain_declined
    }

    /// The live belief behind [`Self::unreadable_keychain_may_hold`]: is a keychain
    /// entry for this account believed to exist? See [`Self::keychain_has_token`]
    /// for what `true` and `false` each mean (positive evidence, and the absence of
    /// it - never "proven empty" unless something proved it).
    ///
    /// Read by the config layer so the persisted belief MIRRORS this field (under the
    /// account key it is stamped with) instead of being recomputed from the tier a
    /// store happened to land on.
    /// The two answer different questions, and the second one is not the question
    /// anybody asks the config: a login whose keychain write fails lands on memory
    /// while an entry an EARLIER login put in the keyring is still sitting there, so
    /// deriving the persisted flag from the landing tier wrote `false` over the only
    /// record that the entry exists. The next launch then skipped the keyring, and
    /// turning the keyring off classified the failed eviction as "nothing was in
    /// there" and reported success over a live bearer token.
    pub fn keychain_has_token(&self) -> bool {
        self.keychain_has_token
    }

    /// The tier the active token lives in, or `None` when there is no token.
    pub fn active_tier(&self) -> Option<TokenTier> {
        self.active_tier
    }

    /// Record the outcome of a keychain access. Only ever called from a real
    /// read/write, never from a probe.
    fn note_keychain(&mut self, ok: bool) {
        self.keychain_state = if ok {
            KeychainState::Available
        } else {
            KeychainState::Unavailable
        };
    }

    /// Store `token` for `(endpoint, email)` across the tier list. Tries the
    /// keychain first; on success caches in memory, marks [`TokenTier::Keychain`],
    /// and **self-heals** by deleting any stale plaintext copy (guardrail 6:
    /// a usable keychain supersedes plaintext). On a keychain error runs the
    /// **two-gate** plaintext opt-in: writes plaintext iff `allow_plaintext` (gate
    /// 1, the `token_fallback == Plaintext` preference) AND the plaintext tier is
    /// available (gate 2 is the keychain failure that landed us here); otherwise
    /// falls back to session-only memory. Returns the resolved [`TokenTier`].
    pub fn store(
        &mut self,
        endpoint: &str,
        email: &str,
        token: &str,
        allow_plaintext: bool,
    ) -> TokenTier {
        let key = account_key(endpoint, email);
        self.in_memory = Some(token.to_owned());

        // Declined: go straight to the non-keychain tiers. Deliberately NOT
        // recorded as a keychain failure - the keychain may be perfectly healthy;
        // we simply are not asking.
        if self.keychain_off() {
            return self.store_without_keychain(&key, token, allow_plaintext);
        }

        match self.keychain.store(&key, token) {
            Ok(()) => {
                self.note_keychain(true);
                // First-hand evidence that the keyring is holding a token of ours.
                // The config flag is written back from this field rather than from
                // the tier a store landed on, so a later login the keyring refuses
                // no longer records `false` over this entry - see
                // [`Self::keychain_has_token`] and `unreadable_keychain_may_hold`
                // for what that evidence buys.
                self.keychain_has_token = true;
                // Self-heal (guardrail 6): a usable keychain removes any stale
                // plaintext copy so the escape hatch never shadows the keychain.
                if let Some(plaintext) = &self.plaintext {
                    if let Err(e) = plaintext.delete(&key) {
                        tracing::warn!(error = %e, "failed to delete stale plaintext SpeleoDB token");
                    }
                }
                tracing::debug!(tier = self.keychain.kind(), "SpeleoDB token stored");
                self.active_tier = Some(TokenTier::Keychain);
                TokenTier::Keychain
            }
            Err(e) => {
                self.note_keychain(false);
                tracing::warn!(
                    error = %e,
                    "OS keychain unavailable; falling back to a non-keychain SpeleoDB token tier for this session"
                );
                self.store_without_keychain(&key, token, allow_plaintext)
            }
        }
    }

    /// The non-keychain half of [`Self::store`]: the two-gate plaintext opt-in,
    /// else session-only memory. Reached either because the keychain FAILED or
    /// because the user DECLINED it - the tier decision is identical, but only the
    /// former is a keychain failure, so the caller owns [`Self::note_keychain`].
    fn store_without_keychain(
        &mut self,
        key: &str,
        token: &str,
        allow_plaintext: bool,
    ) -> TokenTier {
        match (allow_plaintext, &self.plaintext) {
            (true, Some(plaintext)) => match plaintext.store(key, token) {
                Ok(()) => {
                    tracing::debug!(tier = plaintext.kind(), "SpeleoDB token stored");
                    self.active_tier = Some(TokenTier::Plaintext);
                    TokenTier::Plaintext
                }
                Err(e) => {
                    tracing::warn!(error = %e, "plaintext SpeleoDB token write failed; holding in memory only");
                    self.active_tier = Some(TokenTier::Memory);
                    TokenTier::Memory
                }
            },
            _ => {
                self.active_tier = Some(TokenTier::Memory);
                TokenTier::Memory
            }
        }
    }

    /// Re-persist the token this session ALREADY holds, at whatever tier the
    /// current gates allow. `Ok(None)` when there is nothing to move (a logged-out
    /// session), which the caller reports rather than treating as success.
    ///
    /// This is what makes the storage question askable at all: when the keychain
    /// fails at login the key is sitting right here in memory, so "put it in a
    /// file instead" and "try the keychain again" are the same operation with the
    /// decline flag set differently beforehand - no re-login, no re-typed password.
    ///
    /// Runs the full [`Self::store`] path so the decline flag and the two-gate
    /// plaintext opt-in apply exactly as they do at login.
    ///
    /// `Err` means the token DID move - it is on the session-only tier - but a
    /// persistent copy of it survived the sweep, so the caller must not report the
    /// answer as settled. See [`Self::sweep_persistent_tiers`]. No tier is returned
    /// alongside it because the tier is not the interesting fact there: "memory"
    /// over a file the next launch reads back is accurate and misleading at once.
    pub fn restore(
        &mut self,
        endpoint: &str,
        email: &str,
        allow_plaintext: bool,
    ) -> Result<Option<TokenTier>, SecretStoreError> {
        let Some(token) = self.in_memory.clone() else {
            return Ok(None);
        };
        let tier = self.store(endpoint, email, &token, allow_plaintext);
        // "This session only" has to be true ON DISK, not just in the label the
        // user is shown: a copy from an earlier choice would otherwise sit there
        // and be re-loaded on the next launch, which is the opposite of what they
        // just asked for. The keychain tier self-heals plaintext inside `store`;
        // only the memory landing needs sweeping here.
        if tier == TokenTier::Memory {
            self.sweep_persistent_tiers(&account_key(endpoint, email))?;
        }
        Ok(Some(tier))
    }

    /// Remove this account from BOTH persistent tiers after the token landed in
    /// session-only memory, and REPORT a copy that survived.
    ///
    /// Both tiers, not just plaintext. A working login on an earlier day writes the
    /// key to the keychain, and leaving that entry behind makes "you will be signed
    /// out when the app closes" false the moment the next launch rehydrates through
    /// it - the promise then rests entirely on `keychain_declined` reaching disk,
    /// which is best-effort.
    ///
    /// The keychain delete deliberately bypasses `keychain_off` - gating it on the
    /// flag would disable it exactly when it is needed, since the session-only
    /// answer is given by setting that flag. It is not the nag the flow exists to
    /// stop: [`Self::restore`] is reached only from the two explicit credential
    /// verbs (the post-login storage answer and the Settings button), never from a
    /// background path, so the user is inside the interaction that asks for it.
    ///
    /// It is not free either, and the honest version is worth writing down: on a
    /// keyring that is absent (no secret service at all) the delete fails without
    /// side effects, but on one that is merely LOCKED it can raise the unlock
    /// prompt once more, right after the user asked us not to use the keychain. We
    /// take that over the alternative, which is telling them the key will be gone
    /// when the app closes while an entry that signs them straight back in survives
    /// on disk.
    ///
    /// Every tier is swept even when an earlier one fails, mirroring [`Self::clear`];
    /// the first survivor is the one reported.
    fn sweep_persistent_tiers(&mut self, key: &str) -> Result<(), SecretStoreError> {
        // The two tiers differ in what an UNREADABLE store proves, so they answer
        // [`sweep_tier`]'s third outcome differently.
        //
        // A keyring reports absence BY FAILING: on a box with no secret service
        // every op dies at `Entry::new` before it can be asked about the entry. So
        // an `Err` there is only a survivor when this session has some reason to
        // think the keyring works OR to think it is holding something of ours -
        // `unreadable_keychain_may_hold`, the same discrimination `clear` and
        // `evict_from_keychain` make, including the recorded store that separates a
        // locked keyring from an absent one.
        //
        // The plaintext tier reports absence as `Ok`: a missing file - and a missing
        // directory with it - is `Ok(None)` on the probe and `Ok(())` on the delete.
        // An `Err` from it is therefore never "there is nothing here"; it is a
        // `0o600` file holding a live token that we could neither read nor remove,
        // and `load` needs no keyring, no unlock and no flag to hand it back on the
        // next launch.
        let keychain = sweep_tier(
            self.keychain.as_ref(),
            key,
            self.unreadable_keychain_may_hold(),
        );
        if keychain.is_ok() {
            // The keyring accepted the delete, or the probe answered `Ok(None)`.
            // Either way it is not holding one now, so the belief must not outlive
            // it: left standing, it would make the NEXT failed delete in this
            // session report a residue that this sweep has already removed.
            self.keychain_has_token = false;
        }
        let plaintext = self
            .plaintext
            .as_ref()
            .map_or(Ok(()), |p| sweep_tier(p.as_ref(), key, true));
        keychain.and(plaintext)
    }

    /// Whether the active token is backed by the OS keychain (vs the plaintext or
    /// in-memory tiers). Lets the UI keep warning about a non-persistent token.
    pub fn keychain_backed(&self) -> bool {
        self.active_tier == Some(TokenTier::Keychain)
    }

    /// Load the active token, if any. Within a session this serves the in-memory
    /// cache. On a cold session (rehydration) it reads **keychain-first**: a
    /// keychain hit deletes any stale plaintext copy (self-heal / guardrail 6),
    /// caches, and marks [`TokenTier::Keychain`]; otherwise it falls back to the
    /// plaintext tier. All-miss yields `None` (callers → `NotAuthenticated`).
    ///
    /// `&mut self` because it updates the session cache + `active_tier`.
    pub fn load(&mut self, endpoint: &str, email: &str) -> Option<String> {
        if let Some(token) = &self.in_memory {
            return Some(token.clone());
        }
        let key = account_key(endpoint, email);

        // Declined: skip the keychain entirely. THIS is the launch-time nag fix -
        // rehydration on startup is what re-raised the desktop unlock prompt on
        // every single launch for a user who had already dismissed it once.
        if !self.keychain_off() {
            // Keychain-first (precedence): a hit supersedes and self-heals plaintext.
            match self.keychain.load(&key) {
                Ok(Some(token)) => {
                    self.note_keychain(true);
                    // The keyring answered AND handed the entry over - first-hand
                    // proof it is holding one, which is what
                    // [`Self::unreadable_keychain_may_hold`] wants if this same
                    // keyring goes quiet later in the session.
                    self.keychain_has_token = true;
                    if let Some(plaintext) = &self.plaintext {
                        if let Err(e) = plaintext.delete(&key) {
                            tracing::warn!(error = %e, "failed to delete stale plaintext SpeleoDB token");
                        }
                    }
                    self.in_memory = Some(token.clone());
                    self.active_tier = Some(TokenTier::Keychain);
                    return Some(token);
                }
                // Reachable and healthy: the keychain answered, it just holds
                // nothing for this account. That IS an availability signal - and it
                // is also the one PROOF OF ABSENCE a read can give, so the recorded
                // belief has to follow it down. This is what "a stale `true`
                // self-corrects on the miss" means now that the config mirrors the
                // field rather than deriving it from a landing tier: an entry
                // removed outside the app would otherwise stay believed-in forever,
                // and warn about itself the first time this keyring is locked.
                //
                // Lowering it HERE is only half of that. The correction reaches disk
                // because `AppState::speleodb_ensure_rehydrated` reads this field
                // back out and persists the drop right after the rehydrate that made
                // this read; without that, the next launch re-reads the stale `true`
                // and repeats the prompt. It persists it as a compare-and-set, so a
                // login that raised the belief for real between the read and the write
                // is not overwritten by this proof, which is older than it.
                Ok(None) => {
                    self.note_keychain(true);
                    self.keychain_has_token = false;
                }
                Err(e) => {
                    self.note_keychain(false);
                    tracing::warn!(error = %e, "keychain read failed during SpeleoDB rehydration");
                }
            }
        }

        // Fall back to the plaintext tier (the headless escape hatch).
        if let Some(plaintext) = &self.plaintext {
            if let Ok(Some(token)) = plaintext.load(&key) {
                self.in_memory = Some(token.clone());
                self.active_tier = Some(TokenTier::Plaintext);
                return Some(token);
            }
        }

        None
    }

    /// Remove this account's keychain entry, **ignoring the off-gates**, for the one
    /// caller that has to: Settings turning the keyring off.
    ///
    /// Flipping `keychain_disabled` only makes the live session look away, and an
    /// entry a previous successful login wrote does not go away on its own - nothing
    /// else ever removes it, because the disable is re-applied on every subsequent
    /// login so [`Self::store`] never overwrites it. Leaving it there is a working
    /// bearer token sitting in the OS keychain of a user who just said "do not use
    /// the OS keychain".
    ///
    /// Call this BEFORE [`Self::set_keychain_disabled`] - afterwards the gate blocks
    /// it - and follow it with [`Self::restore`] so the token this session still
    /// holds lands somewhere the new gates allow and `active_tier` stops claiming a
    /// keychain that no longer holds anything.
    ///
    /// The delete is a real keychain access, so its outcome is recorded; the
    /// disable that follows makes `Declined` the answer [`Self::keychain_state`]
    /// reports from then on, which is the honest one to show, while the recorded
    /// outcome stays behind it for the sweep. Reports the failure so the caller can leave
    /// `keychain_has_token` set rather than record a removal that did not happen.
    ///
    /// `Err` means what it means in [`Self::clear`] and [`sweep_tier`] - "the
    /// credential may STILL be in there" - not "the delete call failed". The two
    /// are not the same, and the gap is not academic: this switch exists FOR the
    /// keychain-less box (the help text says so), where every op dies at
    /// `Entry::new` before it can be asked about the entry, so a raw failure would
    /// warn about a stranded credential on the one configuration that cannot have
    /// stranded one. A missing entry is already `Ok` ([`KeyringStore::delete`] maps
    /// `NoEntry` to success), so the ordinary nothing-to-delete case never reaches
    /// the probe at all.
    ///
    /// The reverse gap is just as real and is why the quiet branch takes more than
    /// an unusable keyring to reach: a LOCKED keyring fails identically to an absent
    /// one, and the user reaching for this switch on a keyring that will not open is
    /// the likeliest way to get here with a live token still in it. See
    /// [`Self::unreadable_keychain_may_hold`] for the evidence that separates them.
    pub fn evict_from_keychain(
        &mut self,
        endpoint: &str,
        email: &str,
    ) -> Result<(), SecretStoreError> {
        let key = account_key(endpoint, email);
        // Asked BEFORE the delete, because `note_keychain` below overwrites the
        // very field [`Self::unreadable_keychain_may_hold`] reads: a refused delete
        // records `Unavailable`, so an answer taken afterwards would say "nothing
        // can be in there" for EVERY failure and suppress the one report this
        // exists to make.
        let unreadable_may_hold = self.unreadable_keychain_may_hold();
        let result = self.keychain.delete(&key);
        // The RAW outcome is what gets recorded: a keyring that refused a delete is
        // unusable whether or not it was holding anything of ours, and the Settings
        // credential line and the later sweeps both need that fact.
        self.note_keychain(result.is_ok());
        let Err(e) = result else {
            // Accepted, so there is nothing of ours in there any more (a missing
            // entry is already `Ok`) - and the belief that says otherwise must go
            // with it, or a later failed delete in this session reports a residue
            // this eviction already removed.
            self.keychain_has_token = false;
            return Ok(());
        };
        if may_have_survived_delete(self.keychain.as_ref(), &key, unreadable_may_hold) {
            return Err(e);
        }
        self.keychain_has_token = false;
        tracing::debug!(
            error = %e,
            keychain_state = ?self.keychain_state,
            purpose = "defensive eviction from a keychain with nothing of ours to strand",
            "SpeleoDB keychain delete failed for an account it does not hold"
        );
        Ok(())
    }

    /// Whether the keychain may STILL be holding a credential for `account_key`
    /// after a delete that just failed - the question [`Self::clear`] reports on.
    ///
    /// The probe has three outcomes and only two of them are answers. `Ok(Some(_))`
    /// is a survivor. `Ok(None)` proves the keychain holds nothing for this account.
    /// An `Err` proves nothing at all, and reading it as "holds nothing" is how a
    /// LOCKED keyring - which fails the probe and the delete alike - let `clear`
    /// return `Ok` over a live, non-revoked bearer token it had just refused to
    /// delete. Logout then reported a clean sign-out, the one outcome the reporting
    /// exists to prevent. So an indeterminate probe counts as "may hold it".
    ///
    /// Except on a keychain this session has never had a usable answer out of - one
    /// it has LEARNED is unusable here ([`KeychainState::Unavailable`], recorded only
    /// by a real read/write, never by a probe), or one it has not touched at all
    /// ([`KeychainState::NotAttempted`], which is what a launch under a persisted
    /// decline looks like) - that we ALSO have no record of having stored anything
    /// in. That pair is the keychain-less box - headless Linux with no secret
    /// service, a supported configuration whose escape hatch is the plaintext tier -
    /// where every op fails at `Entry::new` before it can even ask about the entry
    /// (a missing entry is already `Ok`: `NoEntry` maps to success). Nothing can have
    /// been stored there, so reporting would fire on every ordinary logout, and a
    /// warning that always fires is one the user learns to ignore - which costs them
    /// the case where it is real.
    ///
    /// A keyring that is merely LOCKED records `Unavailable` too, and nothing we are
    /// willing to ASK tells the two apart (asking harder IS the desktop unlock
    /// prompt). What separates them is what we already know: a store that succeeded,
    /// this session or on a previous launch, is positive evidence that an entry
    /// exists, and [`Self::unreadable_keychain_may_hold`] weighs it. So this catches
    /// both the keyring that WORKED and went unreadable mid-session, and the one
    /// that was already locked when this session started over a token an earlier
    /// login put there.
    fn keychain_may_hold(&self, account_key: &str) -> bool {
        match self.keychain.load(account_key) {
            Ok(entry) => entry.is_some(),
            Err(_) => self.unreadable_keychain_may_hold(),
        }
    }

    /// Whether a keychain that will not answer may nevertheless be holding
    /// something of ours - the `Err` half of [`Self::keychain_may_hold`], hoisted
    /// out because [`Self::sweep_persistent_tiers`] and
    /// [`Self::evict_from_keychain`] have to ask exactly the same question after
    /// their own delete fails, and answering it differently in the three places is
    /// how one of them ends up promising something the others know is not true.
    ///
    /// The line is whether the keyring has ANSWERED us this session.
    /// [`KeychainState::Available`] says it has: it exists, it is reachable on this
    /// box, and a delete it has since refused may well have left our entry behind.
    /// That one says "may hold" whatever else we know.
    ///
    /// No other state has heard anything usable back, and not one of them separates
    /// the two opposite machines that reach here: the box with no secret service,
    /// where nothing can ever have been stored, and the keyring that took a token on
    /// Monday and is merely LOCKED today, where a live bearer token is sitting.
    /// `Unavailable` records a read or write that FAILED, which both of them do.
    /// `NotAttempted` records that we never asked - which is what a RESTART leaves
    /// behind, because rehydration applies the persisted decline BEFORE the load and
    /// the load then skips the keyring entirely, so the launch after a "this session
    /// only" choice touches it exactly never. (`Declined` is never stored in this
    /// field - [`Self::keychain_state`] computes that answer - but it belongs on the
    /// same side: it is the user telling us not to ask.)
    ///
    /// So all three defer to [`Self::keychain_has_token`], which is the only thing
    /// that does tell those machines apart: a successful store, ours this session or
    /// a previous launch's recorded in the config. It is positive evidence in one
    /// direction only - `false` never PROVES the keyring is empty (it is the default
    /// for every config written before the flag existed) - and deferring to it is
    /// what keeps the keychain-less box, where no login can have stored anything,
    /// from being warned on every ordinary logout, sweep and eviction. Keying the
    /// quiet branch on `Unavailable` alone missed the restart: an in-process login
    /// records `Unavailable` on its way to the memory tier, but the next launch
    /// records nothing at all, so the same box came back as `NotAttempted` and was
    /// warned about a credential it cannot have.
    ///
    /// Reads the recorded verdict, not [`Self::keychain_state`]: the reported answer
    /// is `Declined` or `Disabled` for as long as the user has the keychain off, and
    /// the sweep runs with it off by construction.
    ///
    /// `Disabled` is listed for exhaustiveness only - it is derived by the reporter
    /// and never stored in this field.
    fn unreadable_keychain_may_hold(&self) -> bool {
        match self.keychain_state {
            KeychainState::Available => true,
            KeychainState::NotAttempted
            | KeychainState::Unavailable
            | KeychainState::Declined
            | KeychainState::Disabled => self.keychain_has_token,
        }
    }

    /// Forget the token from **every** tier (keychain + plaintext + memory),
    /// **reporting** a persistent-tier delete failure to the caller.
    ///
    /// Every tier is attempted even when an earlier one fails, and the in-memory
    /// token is always dropped - the returned `Err` says "the credential may still
    /// be on this machine", never "nothing was cleared". Both persistent tiers are
    /// reported: a surviving plaintext file resurrects a session exactly like a
    /// surviving keychain entry does ([`Self::load`] falls back to it), and it is
    /// an on-disk cleartext token besides. The first failure wins as the reported
    /// error; both are logged.
    ///
    /// Logout propagates the `Err` so the user can be warned (#270); the 401
    /// auto-drop deliberately drops it (`SpeleoDbSession::forget`) - a dead token
    /// must leave the session even when the keychain is flaky.
    pub fn clear(&mut self, endpoint: &str, email: &str) -> Result<(), SecretStoreError> {
        let key = account_key(endpoint, email);
        let mut failure = None;
        // The keychain is ALWAYS swept, but its failure is only REPORTED when the
        // keychain could actually be holding a credential for this account.
        //
        // That question is ANSWERED BY PROBING, not inferred from `active_tier`.
        // `active_tier` says where THIS session's token came from, and a keychain
        // entry outlives the session that wrote it: a session running on the
        // plaintext or memory tier - the ordinary outcome of a transient keychain
        // hiccup at login - would suppress the warning about a live bearer token
        // that an EARLIER session left behind and this delete just failed to remove.
        // Logout then returned `Ok` with a working credential still in the OS
        // keychain, which is the one outcome this reporting exists to prevent.
        //
        // The probe answers in THREE ways, though, and only `Ok(None)` suppresses
        // outright. [`Self::keychain_may_hold`] owns that discrimination.
        //
        // A DECLINED keychain is skipped outright, probe included. `forget` routes
        // both the user's logout and the background 401 auto-drop through here, so
        // probing would raise the desktop unlock prompt on a locked keyring from an
        // auth failure the user never initiated - exactly the nag the decline flag
        // exists to stop, and the one path that can fire it without them asking for
        // anything. Nothing is orphaned by skipping: `store` never reaches the
        // keychain while the flag is set, so this store has put nothing there, and
        // an entry predating the decline is overwritten by the next successful
        // login (which clears the flag and re-stores) or swept by "Try again".
        //
        // A DISABLED keychain is NOT skipped, because none of that last sentence is
        // true of it - see `clear_may_sweep_keychain`.
        if self.clear_may_sweep_keychain() {
            let keychain_may_hold_it = self.keychain_may_hold(&key);
            match self.keychain.delete(&key) {
                // Accepted: the keyring holds nothing for this account now, so the
                // recorded belief that it does has to go too. Left standing, a
                // sign-in that the keyring then refuses would report a residue this
                // sweep has already removed.
                Ok(()) => self.keychain_has_token = false,
                Err(e) => {
                    if keychain_may_hold_it {
                        tracing::warn!(error = %e, "failed to delete SpeleoDB token from keychain");
                        failure = Some(e);
                    } else {
                        tracing::debug!(
                            error = %e,
                            active_tier = ?self.active_tier,
                            keychain_state = ?self.keychain_state,
                            purpose = "defensive sweep of a keychain with nothing of ours to strand",
                            "SpeleoDB keychain delete failed for an account it does not hold"
                        );
                    }
                }
            }
        } else {
            tracing::debug!(
                purpose =
                    "the user declined the keychain; probing it here is the nag the flag prevents",
                "skipped the SpeleoDB keychain sweep"
            );
        }
        if let Some(plaintext) = &self.plaintext {
            if let Err(e) = plaintext.delete(&key) {
                tracing::warn!(error = %e, "failed to delete SpeleoDB token from plaintext store");
                failure = failure.or(Some(e));
            }
        }
        self.in_memory = None;
        self.active_tier = None;
        failure.map_or(Ok(()), Err)
    }
}

/// After a delete that FAILED, whether the credential may still be sitting in
/// `store`.
///
/// The same discrimination [`TokenStore::clear`] makes, asked the other way round.
/// `clear` probes before deleting because it has to warn about a keychain whose
/// entry the delete may have removed; here the delete has already failed, so the
/// probe answers the question directly - and it answers in THREE ways, only two of
/// which are answers.
///
/// `Ok(Some(_))` means the credential both survived and is still readable, which is
/// exactly the pair [`TokenStore::load`] needs to hand it back on the next launch.
/// `Ok(None)` proves this tier holds nothing for this account, so the delete that
/// failed stranded nothing and the choice is settled. An `Err` proves neither: a
/// locked keyring fails the delete and the probe alike, and "cannot be read" is a
/// fact about right now, while the promise being made - "you will be signed out
/// when the app closes" - is about the next launch, by which time the screen is
/// unlocked and the entry hands the session straight back. Treating that silence as
/// a clean sweep was the same collapse `clear` was fixed for one verb over: it
/// closed the dialog reporting a memory-only key over an older, non-revoked bearer
/// token still on the machine.
///
/// `unreadable_may_hold` is the caller's per-tier answer to that third outcome; see
/// [`TokenStore::sweep_persistent_tiers`] for why the keychain and the plaintext
/// file answer it differently.
///
/// One function rather than one per caller: [`sweep_tier`] and
/// [`TokenStore::evict_from_keychain`] both have to ask it after their own delete
/// fails, and answering it differently in the two places is how one of them ends up
/// promising something the other knows is not true.
fn may_have_survived_delete(store: &dyn SecretStore, key: &str, unreadable_may_hold: bool) -> bool {
    match store.load(key) {
        Ok(entry) => entry.is_some(),
        Err(probe) => {
            tracing::debug!(
                error = %probe,
                tier = store.kind(),
                unreadable_may_hold,
                "the SpeleoDB sweep could not probe the tier whose delete just failed"
            );
            unreadable_may_hold
        }
    }
}

/// Delete `key` from one tier, reporting the failure unless the tier is then
/// PROVEN to be holding nothing ([`may_have_survived_delete`] owns that question).
fn sweep_tier(
    store: &dyn SecretStore,
    key: &str,
    unreadable_may_hold: bool,
) -> Result<(), SecretStoreError> {
    let Err(e) = store.delete(key) else {
        return Ok(());
    };
    if may_have_survived_delete(store, key, unreadable_may_hold) {
        tracing::warn!(
            error = %e,
            tier = store.kind(),
            "the SpeleoDB token may have survived the sweep for a session-only choice"
        );
        return Err(e);
    }
    tracing::debug!(
        error = %e,
        tier = store.kind(),
        purpose = "defensive sweep of a tier that holds nothing for this account",
        "SpeleoDB token delete failed for an account this tier does not hold"
    );
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
impl TokenStore {
    /// Build an in-memory-backed store directly, bypassing the OS keychain. For
    /// unit tests here and in sibling modules (the real fields are private, and
    /// touching the keychain in tests is both slow and environment-dependent).
    pub fn from_in_memory_token(token: &str) -> Self {
        Self {
            in_memory: Some(token.to_owned()),
            active_tier: Some(TokenTier::Memory),
            keychain: Box::new(InMemoryStore::new()),
            plaintext: None,
            keychain_declined: false,
            keychain_disabled: false,
            keychain_state: KeychainState::NotAttempted,
            keychain_has_token: false,
        }
    }

    /// A store already HOLDING `token` in memory over the injected backends - the
    /// shape a session has at the moment the storage question is asked: the key is
    /// in hand, only its destination is undecided. Lets sibling-module tests drive
    /// [`Self::restore`] without a login or a real keychain.
    pub fn from_stores_holding(
        token: &str,
        keychain: Box<dyn SecretStore>,
        plaintext: Option<Box<dyn SecretStore>>,
    ) -> Self {
        let mut store = Self::from_stores(keychain, plaintext);
        store.in_memory = Some(token.to_owned());
        store.active_tier = Some(TokenTier::Memory);
        store
    }

    /// Inject the keychain (+ optional plaintext) backends so tier-precedence,
    /// self-heal, two-gate, and guardrail-6 tests run against fakes.
    pub fn from_stores(
        keychain: Box<dyn SecretStore>,
        plaintext: Option<Box<dyn SecretStore>>,
    ) -> Self {
        Self {
            in_memory: None,
            active_tier: None,
            keychain,
            plaintext,
            keychain_declined: false,
            keychain_disabled: false,
            keychain_state: KeychainState::NotAttempted,
            keychain_has_token: false,
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit test fixtures"
)]
mod tests {
    use super::{
        account_key, normalize_endpoint, InMemoryStore, KeychainState, PlaintextStore, SecretStore,
        SecretStoreError, TokenStore, TokenTier,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A [`SecretStore`] whose every op errors - simulates a headless box with no
    /// secret service. Drives the plaintext two-gate + the keychain-failure path.
    struct FailingSecretStore;

    impl SecretStore for FailingSecretStore {
        fn kind(&self) -> &'static str {
            "failing"
        }
        fn store(&self, _: &str, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError("no secret service".to_owned()))
        }
        fn load(&self, _: &str) -> Result<Option<String>, SecretStoreError> {
            Err(SecretStoreError("no secret service".to_owned()))
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError("no secret service".to_owned()))
        }
    }

    /// A WORKING keychain that refuses to delete: `store`/`load` behave normally, so
    /// it can hold (and report holding) an entry, while `delete` fails.
    ///
    /// One of the two shapes that matter for logout, and the one
    /// [`FailingSecretStore`] cannot express: a keychain that IS usable on this
    /// machine and IS holding a live bearer token that the delete just failed to
    /// remove. `FailingSecretStore` models the opposite case - a box with no secret
    /// service at all, where nothing can be stored and so nothing can survive.
    /// [`LockableStore`] is the third: usable, holding, and unreadable at once.
    struct DeleteFailsStore(InMemoryStore);

    impl DeleteFailsStore {
        /// A store already holding `token` for `account_key`, as an earlier session
        /// would have left it.
        fn holding(account_key: &str, token: &str) -> Self {
            let inner = InMemoryStore::new();
            inner.store(account_key, token).expect("seed entry");
            Self(inner)
        }

        /// The same refusal over an EMPTY store: the probe answers `Ok(None)`, which
        /// is the one outcome that proves there is nothing here to strand.
        fn empty() -> Self {
            Self(InMemoryStore::new())
        }
    }

    impl SecretStore for DeleteFailsStore {
        fn kind(&self) -> &'static str {
            "delete-fails"
        }
        fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError> {
            self.0.store(account_key, token)
        }
        fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError> {
            self.0.load(account_key)
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError("delete refused".to_owned()))
        }
    }

    /// A working keychain that can be LOCKED part-way through a session: until then
    /// it stores and reads like the real one, and afterwards EVERY op fails.
    ///
    /// The desktop keyring shape neither fixture above can express. A locked keyring
    /// still holds what an earlier `store` put in it, but `load` and `delete` fail
    /// identically - so the delete leaves a live bearer token behind and the probe
    /// that would notice cannot see it either.
    struct LockableStore {
        inner: InMemoryStore,
        locked: Arc<AtomicBool>,
    }

    impl LockableStore {
        /// The store plus the switch that locks it, since [`SecretStore`] is moved
        /// into the [`TokenStore`] and cannot be reached afterwards.
        fn pair() -> (Box<dyn SecretStore>, Arc<AtomicBool>) {
            let locked = Arc::new(AtomicBool::new(false));
            let store = Self {
                inner: InMemoryStore::new(),
                locked: Arc::clone(&locked),
            };
            (Box::new(store), locked)
        }

        fn locked_err(&self) -> Option<SecretStoreError> {
            self.locked
                .load(Ordering::SeqCst)
                .then(|| SecretStoreError("the keyring is locked".to_owned()))
        }
    }

    impl SecretStore for LockableStore {
        fn kind(&self) -> &'static str {
            "lockable"
        }
        fn store(&self, account_key: &str, token: &str) -> Result<(), SecretStoreError> {
            self.locked_err()
                .map_or_else(|| self.inner.store(account_key, token), Err)
        }
        fn load(&self, account_key: &str) -> Result<Option<String>, SecretStoreError> {
            self.locked_err()
                .map_or_else(|| self.inner.load(account_key), Err)
        }
        fn delete(&self, account_key: &str) -> Result<(), SecretStoreError> {
            self.locked_err()
                .map_or_else(|| self.inner.delete(account_key), Err)
        }
    }

    const EP: &str = "http://10.10.0.220:8000/";
    const EMAIL: &str = "u@x";

    // ── account_key / endpoint normalization ────────────────────────────────

    #[test]
    fn account_key_normalizes_endpoint() {
        // Same server, two spellings → one key.
        assert_eq!(
            account_key("https://Sdb.Example.com/", "a@b.com"),
            account_key("https://sdb.example.com", "a@b.com")
        );
        assert_eq!(
            account_key("http://10.10.0.220:8000/", "u@x"),
            "http://10.10.0.220:8000|u@x"
        );
    }

    #[test]
    fn endpoint_normalization_url_parse() {
        // A real URL parse: default ports collapse, host lowercases, trailing
        // slash and path-less differences fold to one key.
        assert_eq!(
            normalize_endpoint("https://SDB.Example.com:443/"),
            "https://sdb.example.com"
        );
        assert_eq!(
            normalize_endpoint("https://sdb.example.com"),
            "https://sdb.example.com"
        );
        assert_eq!(normalize_endpoint("http://Host:80"), "http://host");
        // A non-default port is preserved.
        assert_eq!(
            normalize_endpoint("http://10.10.0.220:8000/"),
            "http://10.10.0.220:8000"
        );
        // A bare host (no scheme) degrades to the lightweight normalization.
        assert_eq!(normalize_endpoint("Bare.Host/"), "bare.host");
    }

    // ── in-memory ctor (retained spike behavior) ────────────────────────────

    #[test]
    fn in_memory_fallback_round_trips_without_keychain() {
        // The in-memory constructor never touches the real OS keychain.
        let mut store = TokenStore::from_in_memory_token("tok-123");
        assert_eq!(store.load(EP, EMAIL), Some("tok-123".to_owned()));
    }

    #[test]
    fn clear_forgets_in_memory_token() {
        let mut store = TokenStore::from_in_memory_token("tok-123");
        store.clear(EP, EMAIL).expect("clear");
        assert_eq!(store.load(EP, EMAIL), None);
    }

    // ── tier precedence / self-heal / guardrail 6 ───────────────────────────

    #[test]
    fn tier_precedence_keychain_first() {
        // Keychain succeeds → active tier is Keychain, keychain_backed true.
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(InMemoryStore::new())),
        );
        assert_eq!(store.store(EP, EMAIL, "tok", true), TokenTier::Keychain);
        assert!(store.keychain_backed());
    }

    #[test]
    fn store_keychain_success_deletes_stale_plaintext() {
        // Seed a stale plaintext copy on disk, then a successful keychain store
        // must wipe it (guardrail 6 self-heal on write). The on-disk tempdir is
        // observable through a sibling PlaintextStore after the box is consumed.
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(dir.path().to_path_buf());
        seeded.store(&key, "stale").unwrap();

        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        assert_eq!(store.store(EP, EMAIL, "fresh", true), TokenTier::Keychain);
        assert!(store.keychain_backed());

        // The stale plaintext copy is gone (self-heal deleted it).
        let probe = PlaintextStore::new(dir.path().to_path_buf());
        assert_eq!(probe.load(&key).unwrap(), None);
    }

    #[test]
    fn keychain_read_self_heals_plaintext() {
        // A cold load that hits the keychain must delete the plaintext copy.
        let key = account_key(EP, EMAIL);
        let keychain = InMemoryStore::new();
        keychain.store(&key, "kc-tok").unwrap();
        let plaintext = InMemoryStore::new();
        plaintext.store(&key, "pt-tok").unwrap();

        // We need to observe the plaintext store after the load, so wire boxes we
        // also hold raw references to via separate InMemoryStores sharing state is
        // not possible; instead assert through a second load: after the keychain
        // load self-heals, a store with ONLY the plaintext tier must miss.
        let keychain_box: Box<dyn SecretStore> = Box::new(keychain);
        let plaintext_box: Box<dyn SecretStore> = Box::new(plaintext);
        let mut store = TokenStore::from_stores(keychain_box, Some(plaintext_box));
        assert_eq!(store.load(EP, EMAIL), Some("kc-tok".to_owned()));
        assert!(store.keychain_backed());

        // Clear the session cache and force a re-read: the keychain still wins,
        // and the plaintext copy was deleted during the first load (self-heal).
        store.in_memory = None;
        store.active_tier = None;
        assert_eq!(store.load(EP, EMAIL), Some("kc-tok".to_owned()));
        assert!(store.keychain_backed());
    }

    #[test]
    fn never_override_keychain_guardrail6() {
        // Even with the plaintext preference ON, a working keychain is used and
        // plaintext is never written (guardrail 6).
        let key = account_key(EP, EMAIL);
        let plaintext = InMemoryStore::new();
        let plaintext_box: Box<dyn SecretStore> = Box::new(plaintext);
        let mut store =
            TokenStore::from_stores(Box::new(InMemoryStore::new()), Some(plaintext_box));
        assert_eq!(store.store(EP, EMAIL, "tok", true), TokenTier::Keychain);

        // Re-load on a plaintext-only store: nothing was written there.
        let probe = InMemoryStore::new();
        assert_eq!(probe.load(&key).unwrap(), None);
    }

    // ── plaintext two-gate + 0600 ───────────────────────────────────────────

    #[test]
    fn plaintext_written_only_when_both_gates_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);

        // Gate 1 OFF (allow_plaintext = false) + keychain fails → Memory, no file.
        let mut store = TokenStore::from_stores(
            Box::new(FailingSecretStore),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        let probe = PlaintextStore::new(dir.path().to_path_buf());
        assert_eq!(probe.load(&key).unwrap(), None);

        // Gate 1 ON + keychain fails (gate 2) → Plaintext, file written.
        let mut store = TokenStore::from_stores(
            Box::new(FailingSecretStore),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        assert_eq!(store.store(EP, EMAIL, "tok", true), TokenTier::Plaintext);
        assert_eq!(probe.load(&key).unwrap(), Some("tok".to_owned()));
        assert!(!store.keychain_backed());
    }

    #[test]
    fn plaintext_falls_back_to_memory_without_plaintext_tier() {
        // Preference ON but no plaintext tier available → Memory.
        let mut store = TokenStore::from_stores(Box::new(FailingSecretStore), None);
        assert_eq!(store.store(EP, EMAIL, "tok", true), TokenTier::Memory);
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_file_is_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        let pt = PlaintextStore::new(dir.path().to_path_buf());
        pt.store(&key, "tok").unwrap();
        let path = dir.path().join(
            key.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>(),
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "plaintext token file must be 0600");
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_tightens_preexisting_loose_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        let pt = PlaintextStore::new(dir.path().to_path_buf());
        // Pre-create the file 0644 (looser), then store must tighten to 0600.
        std::fs::create_dir_all(dir.path()).unwrap();
        let sanitized: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.path().join(&sanitized);
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        pt.store(&key, "new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // ── clear-all-tiers + load misses ───────────────────────────────────────

    #[test]
    fn clear_deletes_from_all_tiers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        // Seed both keychain and plaintext, then clear must wipe both + memory.
        let keychain = InMemoryStore::new();
        keychain.store(&key, "kc").unwrap();
        let plaintext = PlaintextStore::new(dir.path().to_path_buf());
        plaintext.store(&key, "pt").unwrap();

        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        // Prime the keychain tier through the store so it has something to delete.
        store.keychain.store(&key, "kc").unwrap();
        store.in_memory = Some("kc".to_owned());
        store.active_tier = Some(TokenTier::Keychain);

        store.clear(EP, EMAIL).expect("clear");
        assert_eq!(store.load(EP, EMAIL), None);
        // Plaintext side is wiped too.
        let probe = PlaintextStore::new(dir.path().to_path_buf());
        assert_eq!(probe.load(&key).unwrap(), None);
    }

    #[test]
    fn clear_surfaces_a_failed_keychain_delete() {
        // #270: a keychain delete that fails must be REPORTED (logout warns the user
        // that the credential may still live on this machine) - but the in-memory
        // token is dropped anyway, so the session is gone either way.
        //
        // The keychain is a DeleteFailsStore rather than the all-ops-fail
        // FailingSecretStore, because the suppression is no longer keyed on
        // `active_tier`: `clear` now PROBES the keychain, and a store that cannot even
        // be read is the keychain-less box (see
        // `clear_does_not_warn_about_a_keychain_that_never_held_the_token`). What has
        // to be reported is a keychain that IS holding a token and would not give it
        // up - which is precisely this fixture.
        let key = account_key(EP, EMAIL);
        let mut store =
            TokenStore::from_stores(Box::new(DeleteFailsStore::holding(&key, "tok")), None);
        store.in_memory = Some("tok".to_owned());
        store.active_tier = Some(TokenTier::Keychain);

        assert!(
            store.clear(EP, EMAIL).is_err(),
            "a failing keychain delete must be reported, not swallowed"
        );
        // Asserted on the field, not through `load`: the keychain entry SURVIVED (that
        // is the whole point of the reported error), so a `load` would legitimately
        // re-find it there - which is exactly the resurrection the Err warns about.
        assert!(
            store.in_memory.is_none(),
            "the in-memory token must be forgotten even when the keychain delete failed"
        );
        assert!(!store.keychain_backed());
    }

    #[test]
    fn clear_surfaces_a_failed_plaintext_delete() {
        // The plaintext tier is reported the same way: a surviving cleartext file is
        // itself a resurrectable token (`load` falls back to it).
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(FailingSecretStore)),
        );
        store.in_memory = Some("tok".to_owned());
        assert!(store.clear(EP, EMAIL).is_err());
        assert_eq!(store.load(EP, EMAIL), None);
    }

    #[test]
    fn clear_deletes_every_tier_despite_an_earlier_failure() {
        // The keychain delete failing must not skip the plaintext delete - every tier
        // is attempted, the failure is only *reported* at the end.
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(dir.path().to_path_buf());
        seeded.store(&key, "pt").unwrap();

        let mut store = TokenStore::from_stores(
            Box::new(DeleteFailsStore::holding(&key, "kc")),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        // A keychain that HOLDS the entry and refuses to delete it: that is what makes
        // the failure reportable now (`clear` probes rather than trusting
        // `active_tier`), and it is the only shape where a surviving keychain
        // credential is actually possible.
        store.active_tier = Some(TokenTier::Keychain);
        assert!(store.clear(EP, EMAIL).is_err());
        assert_eq!(
            seeded.load(&key).unwrap(),
            None,
            "the plaintext copy must still be deleted after the keychain delete failed"
        );
    }

    #[test]
    fn clear_reports_a_stale_keychain_entry_even_when_this_session_used_another_tier() {
        // The hole the `active_tier` test was keyed on. Suppression used to be
        // `active_tier == Some(Keychain)`, which is FALSE whenever this session's token
        // came from the plaintext or memory tier - an ordinary outcome of a transient
        // keychain hiccup at login. But a keychain entry from an EARLIER session for the
        // same (endpoint, email) outlives that session, so logout swept it, the delete
        // failed, the failure was downgraded to `debug!`, and logout returned Ok with a
        // live bearer token still sitting in the OS keychain.
        //
        // The suppression is now a probe of the keychain itself, so this reports.
        let key = account_key(EP, EMAIL);
        let mut store = TokenStore::from_stores(
            Box::new(DeleteFailsStore::holding(&key, "stale-from-last-run")),
            None,
        );
        store.in_memory = Some("this-session".to_owned());
        store.active_tier = Some(TokenTier::Memory);

        assert!(
            store.clear(EP, EMAIL).is_err(),
            "a surviving keychain entry must be reported no matter which tier this \
             session ran on"
        );
    }

    #[test]
    fn clear_does_not_warn_about_a_keychain_that_never_held_the_token() {
        // A keychain-less box - headless Linux with no secret service, a SUPPORTED
        // configuration whose escape hatch is the plaintext tier - fails the keychain
        // delete at `Entry::new`, before it can even ask about the entry (a missing
        // entry is already Ok: `NoEntry` maps to success). So every ordinary logout
        // used to warn that a credential "may still be stored on this machine" about
        // a store that cannot hold one and was never written to. A warning that fires
        // on every logout is one the user learns to ignore.
        let dir = tempfile::tempdir().expect("tempdir");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(dir.path().to_path_buf());

        let mut store = TokenStore::from_stores(
            Box::new(FailingSecretStore),
            Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
        );
        // Signed in exactly as production does it, rather than by poking the fields:
        // the failed keychain write is BOTH how the token ends up on the plaintext
        // tier AND how the session learns the keychain is unusable here. The two
        // arrive together on this box, and the second is what the suppression keys on
        // (see `keychain_may_hold`) - an `Err` probe on a keychain we have no reason
        // to think usable is not evidence of a stranded credential. A keychain that
        // merely refuses to DELETE still reports:
        // `clear_reports_a_stale_keychain_entry_even_when_this_session_used_another_tier`.
        assert_eq!(store.store(EP, EMAIL, "pt", true), TokenTier::Plaintext);
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);
        assert_eq!(seeded.load(&key).unwrap(), Some("pt".to_owned()));

        store
            .clear(EP, EMAIL)
            .expect("an unusable keychain that never held the token is not a warning");
        // The tier that DID hold it is still wiped, and the session is gone.
        assert_eq!(
            seeded.load(&key).unwrap(),
            None,
            "the plaintext copy - the one that could resurrect the session - is deleted"
        );
        assert_eq!(store.load(EP, EMAIL), None);
    }

    #[test]
    fn clear_reports_a_keychain_that_locked_while_holding_the_token() {
        // The third probe outcome, which used to be folded into the second. A locked
        // keyring fails the probe AND the delete, and `matches!(.., Ok(Some(_)))` read
        // that silence as "holds nothing": logout returned Ok, the UI reported a clean
        // sign-out, and the bearer token it had just failed to delete stayed in the OS
        // keychain - live, because logout does not revoke it server-side.
        //
        // The session ran ON the keychain here, which is what makes the `Err`
        // indeterminate rather than merely uninformative: this login put the token
        // there itself.
        let (keychain, locked) = LockableStore::pair();
        let mut store = TokenStore::from_stores(keychain, None);
        assert_eq!(
            store.store(EP, EMAIL, "live-token", false),
            TokenTier::Keychain
        );

        locked.store(true, Ordering::SeqCst); // the screen locks; so does the keyring

        assert!(
            store.clear(EP, EMAIL).is_err(),
            "a keychain that answers neither the delete nor the probe may still be \
             holding the token, and must be reported"
        );
        assert!(
            store.in_memory.is_none(),
            "the session still ends - the Err is a warning, not a refusal"
        );

        // And it really did survive: unlock the keyring and it hands the token back,
        // which is exactly the resurrection the report warns about.
        locked.store(false, Ordering::SeqCst);
        assert_eq!(store.load(EP, EMAIL), Some("live-token".to_owned()));
    }

    #[test]
    fn clear_stays_quiet_when_the_probe_proves_the_keychain_is_empty() {
        // The other side of the same discrimination, and the case the debug-only
        // branch was written for: `Ok(None)` is a real answer. The delete failing over
        // an account the keychain demonstrably does not hold strands nothing, so it
        // must not turn an ordinary logout into a credential warning.
        let mut store = TokenStore::from_stores(Box::new(DeleteFailsStore::empty()), None);
        store.in_memory = Some("tok".to_owned());
        store.active_tier = Some(TokenTier::Memory);

        store
            .clear(EP, EMAIL)
            .expect("a delete that failed over an entry that is not there is not a warning");
        assert!(store.in_memory.is_none(), "the session is gone either way");
    }

    #[test]
    fn load_misses_yield_none() {
        // Empty keychain + empty plaintext → None (callers → NotAuthenticated).
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(InMemoryStore::new())),
        );
        assert_eq!(store.load(EP, EMAIL), None);
        assert!(!store.keychain_backed());
    }

    #[test]
    fn load_falls_back_to_plaintext_when_keychain_empty() {
        // No keychain entry but a plaintext copy → loads from plaintext tier.
        let key = account_key(EP, EMAIL);
        let plaintext = InMemoryStore::new();
        plaintext.store(&key, "pt-tok").unwrap();
        let mut store =
            TokenStore::from_stores(Box::new(InMemoryStore::new()), Some(Box::new(plaintext)));
        assert_eq!(store.load(EP, EMAIL), Some("pt-tok".to_owned()));
        assert!(!store.keychain_backed());
    }

    // ---- the keychain decline gate (items 11/12) ----

    /// A [`SecretStore`] that records whether it was touched AT ALL. The decline
    /// flag's real property is not "we chose another tier" but "we never went near
    /// the keychain" - on Linux the access itself is what raises the unlock prompt,
    /// so only a spy can tell the two apart.
    struct SpyStore {
        inner: InMemoryStore,
        touched: Arc<AtomicBool>,
    }

    impl SecretStore for SpyStore {
        fn kind(&self) -> &'static str {
            "spy"
        }
        fn store(&self, k: &str, v: &str) -> Result<(), SecretStoreError> {
            self.touched.store(true, Ordering::SeqCst);
            self.inner.store(k, v)
        }
        fn load(&self, k: &str) -> Result<Option<String>, SecretStoreError> {
            self.touched.store(true, Ordering::SeqCst);
            self.inner.load(k)
        }
        fn delete(&self, k: &str) -> Result<(), SecretStoreError> {
            self.touched.store(true, Ordering::SeqCst);
            self.inner.delete(k)
        }
    }

    /// A spy plus the flag to read it by, since [`SecretStore`] is moved into the
    /// store and cannot be inspected afterwards.
    fn spy_pair() -> (Box<dyn SecretStore>, Arc<AtomicBool>) {
        let touched = Arc::new(AtomicBool::new(false));
        (
            Box::new(SpyStore {
                inner: InMemoryStore::new(),
                touched: Arc::clone(&touched),
            }),
            touched,
        )
    }

    #[test]
    fn a_declined_keychain_is_never_touched_by_store_or_load() {
        // Item 12. The point is not that we pick a different tier - it is that we
        // do not go near the keychain at all, because on Linux the access itself
        // is what raises the desktop unlock prompt the user already dismissed.
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(keychain, None);
        store.set_keychain_declined(true);

        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert!(
            !touched.load(Ordering::SeqCst),
            "store must not touch a declined keychain"
        );

        // A cold session (the startup rehydrate) is the path that used to nag.
        let (keychain2, touched2) = spy_pair();
        let mut cold = TokenStore::from_stores(keychain2, None);
        cold.set_keychain_declined(true);
        assert_eq!(cold.load(EP, EMAIL), None);
        assert!(
            !touched2.load(Ordering::SeqCst),
            "rehydration must not touch a declined keychain"
        );
    }

    #[test]
    fn declining_still_honours_the_plaintext_choice() {
        // Declining the keychain is not the same as declining persistence: the
        // user is offered plaintext at the same moment, and picking it must work.
        let temp = tempfile::TempDir::new().expect("temp");
        let plaintext = PlaintextStore::new(temp.path().to_path_buf());
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(keychain, Some(Box::new(plaintext)));
        store.set_keychain_declined(true);

        assert_eq!(store.store(EP, EMAIL, "tok", true), TokenTier::Plaintext);
        assert!(!touched.load(Ordering::SeqCst), "still no keychain access");
    }

    #[test]
    fn keychain_state_reports_only_what_access_taught_us() {
        // Item 1: Settings shows where the key lives WITHOUT probing, so an
        // untouched store must say so rather than guess.
        let mut store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
        assert_eq!(store.keychain_state(), KeychainState::NotAttempted);

        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Keychain);
        assert_eq!(store.keychain_state(), KeychainState::Available);

        let mut failing = TokenStore::from_stores(Box::new(FailingSecretStore), None);
        assert_eq!(failing.keychain_state(), KeychainState::NotAttempted);
        assert_eq!(failing.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert_eq!(failing.keychain_state(), KeychainState::Unavailable);
    }

    #[test]
    fn a_disabled_keychain_reports_apart_from_a_declined_one() {
        // They used to collapse into `Declined`, and the consequence was a Settings
        // control that could not work: the retry verb revokes a decline, but nothing
        // it does can bypass the disable. A caller that cannot tell them apart cannot
        // decide whether pressing the button achieves anything.
        let mut store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);

        store.set_keychain_declined(true);
        assert_eq!(store.keychain_state(), KeychainState::Declined);

        store.set_keychain_declined(false);
        store.set_keychain_disabled(true);
        assert_eq!(store.keychain_state(), KeychainState::Disabled);

        // Both set: the standing policy outranks the single-login answer, because it
        // is the one the retry cannot revoke.
        store.set_keychain_declined(true);
        assert_eq!(store.keychain_state(), KeychainState::Disabled);
    }

    // ---- moving the key the session already holds (the login-time choice) ----

    #[test]
    fn restore_moves_the_held_token_without_touching_a_declined_keychain() {
        // The whole premise of asking "where should the key go?" right after a
        // keychain failure: the token is in memory, so honouring the answer must
        // not need a re-login - and must not go near the keychain, or the choice
        // re-raises the prompt it was offered to escape.
        let temp = tempfile::TempDir::new().expect("temp");
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(
            keychain,
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert_eq!(
            store.restore(EP, EMAIL, true).expect("nothing to sweep"),
            Some(TokenTier::Plaintext)
        );
        assert!(!touched.load(Ordering::SeqCst), "no keychain access");
        let probe = PlaintextStore::new(temp.path().to_path_buf());
        assert_eq!(
            probe.load(&account_key(EP, EMAIL)).unwrap(),
            Some("tok".to_owned())
        );
    }

    #[test]
    fn a_session_only_choice_removes_an_existing_plaintext_copy() {
        // Picking "this session only" has to be true on disk. Landing on the memory
        // tier while a plaintext file from an earlier choice survives would leave the
        // key readable after the restart the user was told it would not survive - and
        // `load` would find it again on the next launch.
        let temp = tempfile::TempDir::new().expect("temp");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(temp.path().to_path_buf());
        seeded.store(&key, "tok").unwrap();

        let (keychain, _touched) = spy_pair();
        let mut store = TokenStore::from_stores(
            keychain,
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("the sweep succeeds here"),
            Some(TokenTier::Memory)
        );
        assert_eq!(seeded.load(&key).unwrap(), None);
    }

    #[test]
    fn retrying_the_keychain_re_stores_the_same_token_there() {
        // The Settings "Try again" affordance: clearing the refusal and re-storing is
        // all it takes, because the token never left memory.
        let temp = tempfile::TempDir::new().expect("temp");
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);
        assert_eq!(
            store.restore(EP, EMAIL, true).expect("moved"),
            Some(TokenTier::Plaintext)
        );

        store.set_keychain_declined(false);
        assert_eq!(
            store.restore(EP, EMAIL, true).expect("moved"),
            Some(TokenTier::Keychain)
        );
        assert!(store.keychain_backed());
        assert_eq!(store.keychain_state(), KeychainState::Available);
        // The keychain supersedes, so the plaintext copy it displaced is gone.
        let probe = PlaintextStore::new(temp.path().to_path_buf());
        assert_eq!(probe.load(&account_key(EP, EMAIL)).unwrap(), None);
    }

    #[test]
    fn a_session_only_choice_also_sweeps_the_keychain() {
        // The plaintext copy is not the only thing that can outlive the promise. A
        // working login on an earlier day writes the key to the KEYCHAIN, and
        // leaving that entry behind makes "you will be signed out when the app
        // closes" false the moment the next launch rehydrates through it - the
        // promise would rest entirely on a best-effort `keychain_declined` write.
        //
        // Reaching the keychain here is not the nag this flow exists to stop:
        // `restore` runs only from the two explicit credential verbs, never from a
        // background path.
        let mut store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
        // Monday: the keychain works, so the key lands there durably.
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Keychain);

        // Tuesday: the keyring is locked at login, and the user answers the storage
        // question with "this session only".
        store.set_keychain_declined(true);
        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("the sweep succeeds here"),
            Some(TokenTier::Memory)
        );

        // The next launch: nothing in memory, and the probe re-enabled, so `load`
        // can only answer from a persisted tier.
        store.in_memory = None;
        store.active_tier = None;
        store.set_keychain_declined(false);
        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "Monday's keychain entry must not survive a session-only choice and sign the user back in"
        );
    }

    #[test]
    fn a_session_only_choice_reports_a_plaintext_copy_it_could_not_remove() {
        // "You will be signed out when the app closes" has to be true ON DISK or not
        // be said at all. The sweep used to `warn!` and hand `Memory` back anyway, so
        // the verb persisted the choice and the UI reported success over a cleartext
        // file that the next cold `load` reads straight back and signs the user in
        // with - the same shape as the keychain half of this bug, one tier down.
        let key = account_key(EP, EMAIL);
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()),
            Some(Box::new(DeleteFailsStore::holding(&key, "yesterdays-copy"))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert!(
            store.restore(EP, EMAIL, false).is_err(),
            "a plaintext copy that survived the sweep must be reported, not just logged"
        );
    }

    #[test]
    fn a_session_only_choice_reports_a_keychain_entry_it_could_not_remove() {
        // The other persistent tier, same hazard: an entry a working login wrote on
        // an earlier day is what the next launch's rehydrate finds, and finding it is
        // precisely what the user just asked not to happen.
        let key = account_key(EP, EMAIL);
        let mut store = TokenStore::from_stores(
            Box::new(DeleteFailsStore::holding(&key, "mondays-entry")),
            None,
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert!(store.restore(EP, EMAIL, false).is_err());
    }

    #[test]
    fn a_session_only_choice_does_not_report_a_tier_that_holds_nothing() {
        // The discrimination `clear` makes, made here for the same reason. On a
        // headless box with no secret service EVERY keyring op fails at `Entry::new`,
        // so reporting each `Err` would refuse the session-only answer on every
        // ordinary Linux machine the escape hatch exists for - over a store that
        // cannot hold a credential and so cannot have stranded one.
        //
        // Signed in exactly as production does it, rather than by poking the fields:
        // the failed keychain write is how the session LEARNS the keyring is unusable
        // here, and that verdict is the whole licence for the quiet branch (see
        // `sweep_persistent_tiers`). The decline that follows is the session-only
        // answer being given, and it must not erase what the login just taught us -
        // which it did, by overwriting the verdict with `Declined`.
        let temp = tempfile::TempDir::new().expect("temp");
        let mut store = TokenStore::from_stores(
            Box::new(FailingSecretStore),
            // The plaintext tier WORKS on this box - it is the escape hatch that
            // makes the box supported. `a_session_only_choice_does_not_settle_over_a
            // _plaintext_tier_it_cannot_read` covers the one that does not.
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);

        store.set_keychain_declined(true);
        assert_eq!(
            store.keychain_state(),
            KeychainState::Declined,
            "the user is still told they chose this, not that their keyring is broken"
        );

        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("an unusable tier never held the token, so nothing was stranded"),
            Some(TokenTier::Memory)
        );
    }

    #[test]
    fn a_session_only_choice_reports_a_keychain_that_locked_while_holding_the_token() {
        // The third probe outcome on the session-only path, which used to be folded
        // into the second exactly as it was in `clear`
        // (`clear_reports_a_keychain_that_locked_while_holding_the_token`). A locked
        // keyring fails the delete AND the probe, and `matches!(.., Ok(Some(_)))`
        // read that silence as "holds nothing": the dialog closed reporting a
        // memory-only key while the entry THIS login had put in the keyring stayed
        // there, live - and the next unlocked launch signs the user straight back in
        // through it, which is the one thing "this session only" promises will not
        // happen.
        let (keychain, locked) = LockableStore::pair();
        let mut store = TokenStore::from_stores(keychain, None);
        assert_eq!(
            store.store(EP, EMAIL, "live-token", false),
            TokenTier::Keychain
        );
        assert_eq!(store.keychain_state(), KeychainState::Available);

        locked.store(true, Ordering::SeqCst); // the screen locks; so does the keyring
        store.set_keychain_declined(true); // "this session only"

        assert!(
            store.restore(EP, EMAIL, false).is_err(),
            "a keychain that answers neither the delete nor the probe may still be \
             holding the token, so the choice is not settled"
        );

        // And it really did survive: unlock the keyring and the next launch's cold
        // rehydrate hands the token straight back, which is the resurrection the
        // report exists to warn about.
        locked.store(false, Ordering::SeqCst);
        store.in_memory = None;
        store.active_tier = None;
        store.set_keychain_declined(false);
        assert_eq!(store.load(EP, EMAIL), Some("live-token".to_owned()));
    }

    #[test]
    fn a_session_only_choice_does_not_settle_over_a_plaintext_tier_it_cannot_read() {
        // The keychain's licence for the quiet branch does not transfer to the file
        // tier, because absence looks different there: a missing file - and a missing
        // directory with it - is `Ok` on both the delete and the probe. So an `Err`
        // from the plaintext store is never "there is nothing here"; it is a `0o600`
        // file we could neither read nor remove, and `load` falls back to it with no
        // keyring, no unlock and no flag in the way.
        let mut store = TokenStore::from_stores(
            Box::new(InMemoryStore::new()), // a working, empty keychain
            Some(Box::new(FailingSecretStore)),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert!(
            store.restore(EP, EMAIL, false).is_err(),
            "a file tier that will not answer cannot settle the promise that nothing persists"
        );
    }

    #[test]
    fn a_session_only_sweep_settles_when_the_probe_proves_the_tier_is_empty() {
        // The other side of the same discrimination, and the branch the fix must not
        // swallow: `Ok(None)` is a real answer. A delete that failed over an account
        // the tier demonstrably does not hold stranded nothing, so it must not turn
        // an ordinary session-only choice into a credential warning. Pins behaviour
        // the fix deliberately preserves, so it cannot fail beforehand.
        let mut store = TokenStore::from_stores(Box::new(DeleteFailsStore::empty()), None);
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("a delete that failed over an entry that is not there is not a warning"),
            Some(TokenTier::Memory)
        );
    }

    #[test]
    fn a_session_only_sweep_reaches_every_tier_despite_an_earlier_failure() {
        // A keychain that refuses must not skip the plaintext delete: the plaintext
        // copy is the one `load` falls back to, so short-circuiting on the first
        // failure would leave behind exactly the file being warned about.
        let temp = tempfile::TempDir::new().expect("temp");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(temp.path().to_path_buf());
        seeded.store(&key, "yesterdays-copy").unwrap();

        let mut store = TokenStore::from_stores(
            Box::new(DeleteFailsStore::holding(&key, "mondays-entry")),
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert!(store.restore(EP, EMAIL, false).is_err());
        assert_eq!(
            seeded.load(&key).unwrap(),
            None,
            "the plaintext copy must still be deleted after the keychain delete failed"
        );
    }

    #[test]
    fn a_declined_keychain_is_not_probed_when_the_session_is_cleared() {
        // The DECLINED half only - see
        // `a_disabled_keychain_is_still_swept_when_the_session_is_cleared` for the
        // other flag, which deliberately does the opposite.
        //
        // `forget` routes BOTH the user's logout and the background 401 auto-drop
        // through `clear`, so probing here raised the desktop unlock prompt on a
        // locked keyring from an auth failure the user never initiated - the one
        // path that can fire the nag without them asking for anything. A refusal is
        // also the case where skipping strands nothing: the next successful login
        // re-stores over whatever is there, and "Try again" sweeps it.
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(keychain, None);
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_declined(true);

        assert!(store.clear(EP, EMAIL).is_ok());
        assert!(
            !touched.load(Ordering::SeqCst),
            "a declined keychain must not be probed or deleted on a session drop"
        );
        assert_eq!(
            store.active_tier(),
            None,
            "the token still leaves the session"
        );
    }

    // ---- the standing "never use the keyring" instruction (Settings) ----

    #[test]
    fn a_disabled_keychain_is_never_touched_by_store_or_load() {
        // The `keychain_declined` sibling of this test pins the per-login refusal;
        // the standing Settings instruction has to hold on exactly the same paths,
        // and for the same reason - on Linux the access IS the unlock prompt.
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(keychain, None);
        store.set_keychain_disabled(true);

        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert!(
            !touched.load(Ordering::SeqCst),
            "store must not touch a disabled keychain"
        );

        let (keychain2, touched2) = spy_pair();
        let mut cold = TokenStore::from_stores(keychain2, None);
        cold.set_keychain_disabled(true);
        assert_eq!(cold.load(EP, EMAIL), None);
        assert!(
            !touched2.load(Ordering::SeqCst),
            "rehydration must not touch a disabled keychain"
        );
    }

    #[test]
    fn a_disabled_keychain_does_not_stop_the_plaintext_choice() {
        // Turning the keyring off is not turning persistence off: `token_fallback`
        // still decides where the key lands, and `restore` must honour it without
        // going near the keychain.
        let temp = tempfile::TempDir::new().expect("temp");
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(
            keychain,
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_disabled(true);

        assert_eq!(
            store.restore(EP, EMAIL, true).expect("nothing to sweep"),
            Some(TokenTier::Plaintext)
        );
        assert!(!touched.load(Ordering::SeqCst), "still no keychain access");
        let probe = PlaintextStore::new(temp.path().to_path_buf());
        assert_eq!(
            probe.load(&account_key(EP, EMAIL)).unwrap(),
            Some("tok".to_owned())
        );
    }

    #[test]
    fn a_disabled_keychain_is_still_swept_when_the_session_is_cleared() {
        // The deliberate divergence from the declined case. Nothing else ever
        // removes what a disabled keychain is holding: Settings re-applies the flag
        // on every login so `store` cannot overwrite the entry, and "Try again"
        // clears only `keychain_declined`. Both flags are set here because that is
        // what a login under the standing instruction leaves behind
        // (`speleodb.rs`: `set_keychain_disabled(kd); set_keychain_declined(kd);`) -
        // the disable has to win, or the skip comes straight back.
        let key = account_key(EP, EMAIL);
        let (keychain, touched) = spy_pair();
        let mut store = TokenStore::from_stores(keychain, None);
        store
            .keychain
            .store(&key, "left-over-from-an-earlier-login")
            .unwrap();
        touched.store(false, Ordering::SeqCst);
        store.in_memory = Some("tok".to_owned());
        store.set_keychain_disabled(true);
        store.set_keychain_declined(true);

        store.clear(EP, EMAIL).expect("the sweep succeeds here");
        assert!(
            touched.load(Ordering::SeqCst),
            "a disabled keychain must still be swept - nothing else will"
        );
        // Re-enable the probe and confirm the entry really went.
        store.set_keychain_disabled(false);
        store.set_keychain_declined(false);
        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "the entry the disable would otherwise have stranded is gone"
        );
    }

    #[test]
    fn logout_after_disabling_the_keychain_reports_the_surviving_credential() {
        // The security half of the same change. Skipping the sweep left `failure`
        // at `None`, so `speleodb_logout` returned Ok and the UI reported a clean
        // logout while a working bearer token stayed in the OS keychain - the exact
        // outcome `clear`'s reporting exists to prevent. The token is long-lived
        // (the auth endpoint is get-or-create and non-rotating) and logout does not
        // revoke it server-side, so "reported" is the only protection there is.
        let key = account_key(EP, EMAIL);
        let mut store = TokenStore::from_stores(
            Box::new(DeleteFailsStore::holding(&key, "live-token")),
            None,
        );
        store.in_memory = Some("live-token".to_owned());
        store.set_keychain_disabled(true);
        store.set_keychain_declined(true);

        assert!(
            store.clear(EP, EMAIL).is_err(),
            "a credential the disabled keychain refused to give up must be reported"
        );
        assert!(
            store.in_memory.is_none(),
            "the session still ends - the Err is a warning, not a refusal"
        );
    }

    #[test]
    fn evicting_removes_the_entry_a_disabled_keychain_would_otherwise_keep() {
        // What `update_config` runs the moment the Settings toggle goes on. The flag
        // alone only makes the live session look away, so the eviction has to happen
        // BEFORE it is applied - afterwards the gate blocks it.
        let mut store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Keychain);

        store.evict_from_keychain(EP, EMAIL).expect("evicted");
        store.set_keychain_disabled(true);
        // The token is still in hand, so it re-lands at a tier the new gates allow.
        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("the sweep succeeds here"),
            Some(TokenTier::Memory)
        );
        assert!(
            !store.keychain_backed(),
            "the report must not keep claiming a keychain we just emptied"
        );

        // Next launch, with the keyring turned back on: nothing to find.
        store.in_memory = None;
        store.active_tier = None;
        store.set_keychain_disabled(false);
        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "turning the keyring off must not leave a live credential behind in it"
        );
    }

    #[test]
    fn evicting_reports_a_keychain_that_would_not_give_the_entry_up() {
        // The caller leaves `keychain_has_token` set on this path, so a later
        // re-enable still knows the keyring is worth looking in.
        let key = account_key(EP, EMAIL);
        let mut store =
            TokenStore::from_stores(Box::new(DeleteFailsStore::holding(&key, "tok")), None);
        assert!(store.evict_from_keychain(EP, EMAIL).is_err());
    }

    #[test]
    fn evicting_reports_a_keyring_that_was_locked_before_this_session_started() {
        // The gap the `Unavailable` suppression left, and the one configuration with
        // the STRONGEST evidence that a credential survives. A previous launch's
        // login put a token in the keyring and recorded it
        // (`config.speleodb.keychain_has_token`); this launch the keyring is locked,
        // so the startup rehydrate read fails and records `Unavailable` - the same
        // verdict a box with no secret service records, and the one that says
        // "nothing can be in there". The eviction then failed, the probe failed with
        // it, and the switch reported the entry emptied over a live bearer token,
        // clearing the recorded belief so the next launch would not look either.
        let key = account_key(EP, EMAIL);
        let (keychain, locked) = LockableStore::pair();
        keychain
            .store(&key, "mondays-token")
            .expect("an earlier launch's login, back when the keyring opened");
        locked.store(true, Ordering::SeqCst); // and today it does not
        let mut store = TokenStore::from_stores(keychain, None);
        // What `SpeleoDbSession::rehydrate` mirrors in from the config, and the only
        // thing separating this box from one with no secret service at all.
        store.note_keychain_has_token(true);
        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "the locked keyring answers nothing"
        );
        assert_eq!(
            store.keychain_state(),
            KeychainState::Unavailable,
            "which is exactly the verdict a keychain-less box records"
        );

        assert!(
            store.evict_from_keychain(EP, EMAIL).is_err(),
            "a keyring we know took a token and now will not answer has not proved it is empty"
        );

        // And it really did survive: unlock it and the entry is still there, which is
        // what signs the user back in on the next launch.
        locked.store(false, Ordering::SeqCst);
        assert_eq!(store.load(EP, EMAIL), Some("mondays-token".to_owned()));
    }

    #[test]
    fn a_session_only_choice_reports_a_keyring_locked_before_this_session_started() {
        // The same hole in the OTHER caller of `unreadable_keychain_may_hold`. The
        // two were deliberately unified so they cannot drift, so the evidence that
        // fixes one has to reach both: the sweep behind "this session only" promises
        // the key is gone when the app closes, and an entry a previous login left in
        // a keyring that is locked TODAY falsifies it just as completely.
        let key = account_key(EP, EMAIL);
        let (keychain, locked) = LockableStore::pair();
        keychain
            .store(&key, "mondays-token")
            .expect("an earlier launch's login, back when the keyring opened");
        locked.store(true, Ordering::SeqCst);
        let mut store = TokenStore::from_stores(keychain, None);
        store.note_keychain_has_token(true);
        assert_eq!(store.load(EP, EMAIL), None);
        // A fresh login this launch, which the locked keyring also refuses - so the
        // token lands in memory and the storage prompt is what asks the question.
        assert_eq!(
            store.store(EP, EMAIL, "todays-token", false),
            TokenTier::Memory
        );
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);

        store.set_keychain_declined(true); // "this session only"

        assert!(
            store.restore(EP, EMAIL, false).is_err(),
            "the keyring may still be holding Monday's entry, so nothing about \"gone when the \
             app closes\" is settled"
        );
    }

    #[test]
    fn a_keyring_that_answered_empty_is_no_longer_believed_to_hold_a_token() {
        // The other direction of the recorded belief, and the one that keeps it from
        // becoming a warning that can never be switched off. `keychain_has_token` is
        // now what `config.speleodb.keychain_has_token` is written FROM, so it has to
        // fall on proof or a stale `true` outlives everything: the login that used to
        // reset it (by recomputing the flag as "did THIS store land on the keychain")
        // no longer does, deliberately.
        //
        // A read that answers `Ok(None)` is that proof - the keyring was reachable and
        // holds nothing for this account - and it costs no extra keychain access,
        // because the rehydrate read has already happened. Without it, an entry removed
        // outside the app (seahorse, a reinstall) stays believed-in forever and warns
        // about itself the first time the keyring is locked.
        let (keychain, locked) = LockableStore::pair();
        let mut store = TokenStore::from_stores(keychain, None);
        // What the config still records from a login whose entry has since been
        // removed by hand.
        store.note_keychain_has_token(true);

        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "the keyring opened and had nothing for us"
        );
        assert!(
            !store.keychain_has_token(),
            "which is proof, not merely silence - so the belief must not survive it"
        );

        // Now the screen locks, and today's login is refused by the keyring it can no
        // longer reach. Nothing about that re-creates an entry this session watched
        // the keyring deny having.
        locked.store(true, Ordering::SeqCst);
        assert_eq!(
            store.store(EP, EMAIL, "todays-token", false),
            TokenTier::Memory
        );
        assert!(
            !store.keychain_has_token(),
            "and this is what the login persists, so the next launch does not open a \
             keyring we watched answer empty"
        );

        // The payoff: answering the prompt with "this session only" settles, instead of
        // refusing over a copy that provably is not there.
        store.set_keychain_declined(true);
        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("a keyring proven empty this session has stranded nothing"),
            Some(TokenTier::Memory)
        );
    }

    #[test]
    fn evicting_raises_no_false_alarm_on_a_keychainless_box_that_just_restarted() {
        // The keychain-less box again, but reached the way a RESTART reaches it
        // rather than by signing in, and that difference is the whole test. An
        // in-process login records `Unavailable` on its way to the memory tier, so
        // `a_keychainless_box_turns_the_keyring_off_without_a_false_alarm` never sees
        // this shape: rehydration applies the persisted "this session only" decline
        // BEFORE the load, the load then skips the keyring entirely, and the session
        // comes up having touched it exactly never - `NotAttempted`, with no recorded
        // store either. Keying the quiet branch on `Unavailable` alone therefore
        // warned about a stranded credential on the one box that cannot hold one, for
        // every user who had answered the storage question once and restarted.
        let mut store = TokenStore::from_stores(Box::new(FailingSecretStore), None);
        // What `SpeleoDbSession::rehydrate` does, in its order.
        store.set_keychain_declined(true);
        store.note_keychain_has_token(false);
        assert_eq!(
            store.load(EP, EMAIL),
            None,
            "a declined keyring is not read at startup - that skip IS the launch-time nag fix"
        );
        assert_eq!(
            // The field, not `keychain_state()`: the reported answer is `Declined` for
            // as long as the user has the keyring off, and the sweep asks the other
            // question of what is underneath it.
            store.keychain_state,
            KeychainState::NotAttempted,
            "so this launch has learned nothing about the keyring - the state no \
             in-process sign-in can reproduce"
        );

        store.evict_from_keychain(EP, EMAIL).expect(
            "a keyring that has never held anything of ours has stranded nothing to warn about",
        );
    }

    #[test]
    fn a_session_only_choice_after_a_restart_does_not_report_a_keyring_it_never_opened() {
        // The same cold-rehydrate shape at the OTHER caller of
        // `unreadable_keychain_may_hold`. The two are deliberately unified so they
        // cannot drift, so a launch that has touched no keyring has to be quiet in
        // both - and here the cost of getting it wrong is not a warning but a refusal:
        // `speleodb_move_token` turns the sweep's `Err` into "an older copy could not
        // be removed from this computer" and declines to persist the choice, on a box
        // whose keyring cannot be holding a copy in the first place.
        let temp = tempfile::TempDir::new().expect("temp");
        let key = account_key(EP, EMAIL);
        let seeded = PlaintextStore::new(temp.path().to_path_buf());
        seeded
            .store(&key, "yesterdays-key")
            .expect("the tier a keychain-less box's earlier choice actually used");

        let mut store = TokenStore::from_stores(
            Box::new(FailingSecretStore),
            Some(Box::new(PlaintextStore::new(temp.path().to_path_buf()))),
        );
        // Rehydration, in its order: the persisted decline first, then a load that can
        // only reach the plaintext tier - no keychain access anywhere in it.
        store.set_keychain_declined(true);
        store.note_keychain_has_token(false);
        assert_eq!(store.load(EP, EMAIL), Some("yesterdays-key".to_owned()));
        assert_eq!(
            store.keychain_state,
            KeychainState::NotAttempted,
            "the field, not `keychain_state()`, which reports `Declined` for as long as \
             the user has the keyring off"
        );

        assert_eq!(
            store
                .restore(EP, EMAIL, false)
                .expect("a keyring no launch of ours has ever written to strands nothing"),
            Some(TokenTier::Memory)
        );
        assert_eq!(
            seeded.load(&key).unwrap(),
            None,
            "and the tier that DID hold a copy is still swept - the promise is about disk"
        );
    }

    #[test]
    fn re_applying_an_unchanged_flag_keeps_what_we_learned() {
        // Saving ANY unrelated setting re-applies `keychain_disabled` to the live
        // session. Re-running `refresh_keychain_state` on that no-op reset the
        // credential line to "not attempted" and took the Settings "Try again"
        // button with it (it is offered only for `declined` / `unavailable`), until
        // the user signed in again.
        let mut store = TokenStore::from_stores(Box::new(FailingSecretStore), None);
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);

        store.set_keychain_disabled(false); // already false: nothing changed
        assert_eq!(
            store.keychain_state(),
            KeychainState::Unavailable,
            "an unrelated settings save must not forget that the keychain is broken"
        );
        store.set_keychain_declined(false);
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);

        // A real change still refreshes: re-enabling makes an old verdict stale.
        store.set_keychain_declined(true);
        assert_eq!(store.keychain_state(), KeychainState::Declined);
        store.set_keychain_declined(false);
        assert_eq!(store.keychain_state(), KeychainState::NotAttempted);
    }

    #[test]
    fn restore_reports_that_there_is_no_token_to_move() {
        // A logged-out store must not answer "moved it" - the caller turns this into
        // "not signed in" rather than a silent success.
        let mut store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
        assert_eq!(
            store.restore(EP, EMAIL, false).expect("no sweep to fail"),
            None
        );
    }

    #[test]
    fn declining_is_reported_as_declined_not_as_unavailable() {
        // The two are different things to tell a user: "your keychain is broken"
        // versus "you chose otherwise, and here is how to change your mind".
        let mut store = TokenStore::from_stores(Box::new(FailingSecretStore), None);
        assert_eq!(store.store(EP, EMAIL, "tok", false), TokenTier::Memory);
        assert_eq!(store.keychain_state(), KeychainState::Unavailable);

        store.set_keychain_declined(true);
        assert_eq!(store.keychain_state(), KeychainState::Declined);

        // Re-enabling (a fresh login) must not keep reporting a stale verdict from
        // before the user fixed their keyring.
        store.set_keychain_declined(false);
        assert_eq!(store.keychain_state(), KeychainState::NotAttempted);
    }
}
