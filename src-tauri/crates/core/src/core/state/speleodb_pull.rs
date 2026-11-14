//! Native Stalagmight downloads and cache installation.
use super::speleodb::{local_dto, validate_project_id};
use super::speleodb_cache::{
    preserved_undo_mode, restore_display_settings, InheritedUndo, LocalEditsPolicy,
};
use super::speleodb_cached::copy_recorded_for_other_account;
use super::speleodb_revisions::validate_hexsha;
use super::AppState;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::models::SpeleoProjectSummary;
use crate::speleodb::{
    client, sync, SpeleoDbError, SpeleoErrorDto, SpeleoOpenResult, SpeleoReadOnlyOpen,
    TransportCause,
};
pub(super) fn usable_tip(tip: Option<String>, project_id: &str) -> Option<String> {
    let sha = tip?;
    if validate_hexsha(&sha).is_ok() {
        return Some(sha);
    }
    tracing::warn!(
        project_id = %project_id,
        len = sha.len(),
        "server history reported a tip that is not a usable revision id; falling back \
         to an undated HEAD download"
    );
    None
}

pub(super) const DOWNLOADED_VALUE_LIMIT: i32 = 64 * 1024 * 1024;
pub(super) const DOWNLOADED_PROCESSING_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(30);

const DOWNLOADED_LIMITS: (i32, std::time::Duration) =
    (DOWNLOADED_VALUE_LIMIT, DOWNLOADED_PROCESSING_DEADLINE);

struct DownloadOptions {
    limits: (i32, std::time::Duration),
    /// The session generation the download was reserved under. The install refuses
    /// once a newer open or login has moved it (`replace_cache_for_session`).
    generation: u64,
}
impl DownloadOptions {
    fn for_session(generation: u64) -> Self {
        Self {
            limits: DOWNLOADED_LIMITS,
            generation,
        }
    }
}

/// A connection failure is the only reason to open a cache instead of downloading.
/// Never use cache availability to bypass access denial or invalid server content.
fn offline_open_result(
    dest: &std::path::Path,
    endpoint: &str,
    project_id: &str,
    error: &SpeleoDbError,
) -> Option<SpeleoOpenResult> {
    if !matches!(
        error,
        SpeleoDbError::Transport {
            cause: TransportCause::Refused | TransportCause::Timeout | TransportCause::Dns,
            ..
        }
    ) {
        return None;
    }
    let db = crate::persistence::Database::open_for_metadata(dest).ok()?;
    let link = db.load_speleo_link().ok()??;
    if link.project_id != project_id || link.endpoint != endpoint {
        return None;
    }
    Some(SpeleoOpenResult {
        local_path: dest.to_string_lossy().into_owned(),
        offline: Some(true),
        has_unpushed_edits: Some(link.has_unpushed_edits),
        read_only: None,
        other_owner: None,
    })
}

/// An offline open of a copy that holds ANOTHER account's un-uploaded edits (its
/// owner record names someone else) opens read-only and says whose work it is: the
/// signed-in account must never take that work over (`speleodb_cached`). A clean
/// copy, or one with no owner record, opens as before.
fn other_accounts_work_read_only(
    mut opened: SpeleoOpenResult,
    dest: &std::path::Path,
    endpoint: &str,
    email: &str,
) -> SpeleoOpenResult {
    if opened.has_unpushed_edits == Some(true) {
        opened.other_owner = copy_recorded_for_other_account((endpoint, email), dest);
        if opened.other_owner.is_some() {
            opened.read_only = Some(SpeleoReadOnlyOpen::OtherAccount);
        }
    }
    opened
}

/// The guarded download's refusal names whose work the copy holds when its owner
/// record names another account, so the prompt does not call it "your changes" and
/// opens it read-only (`UnpushedLocalEdits::other_owner`).
fn name_other_owner(
    mut dto: SpeleoErrorDto,
    dest: &std::path::Path,
    endpoint: &str,
    email: &str,
) -> SpeleoErrorDto {
    if let Some(local) = dto.unpushed_local.as_mut() {
        local.other_owner = copy_recorded_for_other_account((endpoint, email), dest);
    }
    dto
}

/// Without a usable session the server cannot be asked anything, and nothing it
/// could say would make the copy editable, so the cached copy of `project_id` opens
/// read-only as it is: no network call, nothing written, un-uploaded edits kept.
///
/// Only the cached copy of THIS project qualifies (any instance: with no session
/// there is no instance to compare against, and nothing is sent anywhere). No copy,
/// or a copy that cannot be read, is no fallback - the caller reports the original
/// error.
fn signed_out_open_result(
    dest: &std::path::Path,
    project_id: &str,
    reason: SpeleoReadOnlyOpen,
) -> Option<SpeleoOpenResult> {
    let db = crate::persistence::Database::open_for_metadata(dest).ok()?;
    let link = db.load_speleo_link().ok()??;
    if link.project_id != project_id {
        return None;
    }
    Some(SpeleoOpenResult {
        local_path: dest.to_string_lossy().into_owned(),
        offline: None,
        has_unpushed_edits: Some(link.has_unpushed_edits),
        read_only: Some(reason),
        other_owner: None,
    })
}

impl AppState {
    pub fn speleodb_list_projects(&self) -> Result<Vec<SpeleoProjectSummary>, SpeleoErrorDto> {
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let projects = self
            .speleodb_handle_auth(
                Endpoint::ListProjects,
                client::list_projects(&endpoint, &token, insecure_http),
            )
            .map_err(|e| dto_for_error(Endpoint::ListProjects, &e))?;
        Ok(projects
            .into_iter()
            .map(SpeleoProjectSummary::from_project)
            .collect())
    }
    pub fn speleodb_open_project(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        self.speleodb_pull_project(
            project_id,
            project_name,
            latest_commit_sha,
            LocalEditsPolicy::Protect,
        )
    }
    pub fn speleodb_open_project_discarding_local(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        self.speleodb_pull_project(
            project_id,
            project_name,
            latest_commit_sha,
            LocalEditsPolicy::Discard,
        )
    }
    fn speleodb_pull_project(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        policy: LocalEditsPolicy,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        self.speleodb_pull_project_to(&dest, project_id, project_name, latest_commit_sha, policy)
    }

    /// [`Self::speleodb_pull_project`] into an explicit cache path (`project_id`
    /// already validated), so tests can point it at a temporary cache.
    pub(super) fn speleodb_pull_project_to(
        &self,
        dest: &std::path::Path,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        policy: LocalEditsPolicy,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        let generation = self.reserve_download_generation();
        let dest = dest.to_path_buf();
        // Endpoint, token and email from ONE session read, so the owner recorded
        // below is the account whose token did the download.
        let (endpoint, token, email) = match self.speleodb_active_account() {
            Ok(account) => account,
            Err(SpeleoDbError::NotAuthenticated) if policy == LocalEditsPolicy::Protect => {
                return signed_out_open_result(&dest, &project_id, SpeleoReadOnlyOpen::SignedOut)
                    .ok_or_else(|| local_dto(SpeleoDbError::NotAuthenticated));
            }
            Err(e) => return Err(local_dto(e)),
        };
        let insecure_http = self.speleodb_insecure_http();

        // Download first so offline opens can recover unsynced cached work too.
        // install_linked_download_with_limits still refuses to replace unpushed edits.

        let tip = match self.speleodb_handle_auth(
            Endpoint::GetProject,
            client::get_revisions(&endpoint, &token, insecure_http, &project_id),
        ) {
            Ok(history) => usable_tip(
                sync::tip_of(&history.commits).map(str::to_owned),
                &project_id,
            ),
            Err(
                e @ SpeleoDbError::Status {
                    status: 401 | 403 | 404,
                    ..
                },
            ) => {
                if let Some(expired) = self.session_expired_open(&dest, &project_id, &e, policy) {
                    return Ok(expired);
                }
                return Err(dto_for_error(Endpoint::GetProject, &e));
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "resolve server HEAD on pull",
                    "speleodb_open_project: could not resolve server HEAD; \
                     falling back to an undated HEAD download"
                );
                None
            }
        };
        // Un-uploaded local work of THIS account sitting on the server tip is a
        // fast-forward, not a conflict: open it as it is (see
        // `speleodb_local_fast_forward_open`). Another account's work, or work
        // nobody recorded an owner for, goes through the guarded download below
        // instead, whose refusal prompt asks, and its owner record is not touched.
        if policy == LocalEditsPolicy::Protect {
            if let Some(local) = self.speleodb_local_fast_forward_open(
                &project_id,
                &dest,
                &endpoint,
                &email,
                tip.as_deref(),
            ) {
                // Already recorded as this account's (the arm requires it): refresh
                // the record, which carries the project's current name.
                self.speleodb_record_cache_owner(&dest, &endpoint, &email, &project_name);
                return Ok(local);
            }
        }
        let download = self.speleodb_handle_auth(
            Endpoint::DownloadStalx,
            match tip.as_deref() {
                Some(sha) => client::download_stalx_at_hash(
                    &endpoint,
                    &token,
                    insecure_http,
                    &project_id,
                    sha,
                ),
                None => client::download_stalx(&endpoint, &token, insecure_http, &project_id),
            },
        );
        let native = match download {
            Ok(native) => native,
            Err(error) => {
                if policy == LocalEditsPolicy::Protect {
                    if let Some(cached) = offline_open_result(&dest, &endpoint, &project_id, &error)
                    {
                        return Ok(other_accounts_work_read_only(
                            cached, &dest, &endpoint, &email,
                        ));
                    }
                }
                if let Some(expired) = self.session_expired_open(&dest, &project_id, &error, policy)
                {
                    return Ok(expired);
                }
                return Err(dto_for_error(Endpoint::DownloadStalx, &error));
            }
        };

        let base_sha = tip;
        let _ = latest_commit_sha; // A list hint cannot date an unpinned download.
        let link = crate::models::SpeleoLink {
            endpoint: endpoint.clone(),
            project_id: project_id.clone(),
            project_name: project_name.clone(),
            base_commit_sha: base_sha,
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        };
        self.install_linked_download_with_limits(
            native.path(),
            &dest,
            link,
            policy,
            InheritedUndo::Drop,
            DownloadOptions::for_session(generation),
        )
        .map_err(|e| name_other_owner(local_dto(e), &dest, &endpoint, &email))?;
        self.speleodb_record_cache_owner(&dest, &endpoint, &email, &project_name);

        Ok(SpeleoOpenResult {
            local_path: dest.to_string_lossy().into_owned(),
            offline: None,
            has_unpushed_edits: None,
            read_only: None,
            other_owner: None,
        })
    }

    /// An open the server refused for the SESSION rather than for the project: a
    /// 401, or a 403 the whoami probe confirmed is a dead token. `speleodb_handle_auth`
    /// has already dropped the token, so the app is signed out now and the open
    /// continues as a signed-out one (read-only cached copy, no further calls).
    ///
    /// A 403 or 404 while the token is still good is the server saying this account
    /// may not have the project, and keeps the no-fallback rule: `None`.
    fn session_expired_open(
        &self,
        dest: &std::path::Path,
        project_id: &str,
        error: &SpeleoDbError,
        policy: LocalEditsPolicy,
    ) -> Option<SpeleoOpenResult> {
        if policy != LocalEditsPolicy::Protect
            || !matches!(
                error,
                SpeleoDbError::Status {
                    status: 401 | 403,
                    ..
                }
            )
            || self.speleodb_active_token().is_ok()
        {
            return None;
        }
        signed_out_open_result(dest, project_id, SpeleoReadOnlyOpen::SessionExpired)
    }
    pub(super) fn speleodb_fast_forward(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        tip: Option<&str>,
        transition: (LocalEditsPolicy, u64, Option<Vec<u8>>),
    ) -> Result<String, SpeleoDbError> {
        let (policy, generation, expected_snapshot) = transition;
        let bytes = {
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if generation != self.speleodb_session_generation() {
                return Err(SpeleoDbError::SessionMoved);
            }
            let bytes =
                serde_json::to_vec(&*project).map_err(|e| SpeleoDbError::Io(e.to_string()))?;
            if expected_snapshot
                .as_ref()
                .is_some_and(|expected| expected != &bytes)
            {
                return Err(SpeleoDbError::SessionMoved);
            }
            bytes
        };
        let was_editing = self.edit_enabled();
        let project_id = &link.project_id;
        // A verified revision must be downloaded by its exact identifier.
        let pinned = tip.map(str::to_owned);
        if let Some(sha) = pinned.as_deref() {
            validate_hexsha(sha)?;
        }
        let native = self.speleodb_handle_auth(
            Endpoint::DownloadStalx,
            match pinned.as_deref() {
                Some(sha) => {
                    client::download_stalx_at_hash(endpoint, token, insecure_http, project_id, sha)
                }
                None => client::download_stalx(endpoint, token, insecure_http, project_id),
            },
        )?;
        let dest = self.speleodb_cache_path(project_id)?;
        if policy == LocalEditsPolicy::Protect {
            self.guard_local_edits(project_id, &dest)?;
        }
        let mut new_link = crate::models::SpeleoLink {
            endpoint: endpoint.to_owned(),
            project_id: project_id.clone(),
            project_name: link.project_name.clone(),
            base_commit_sha: pinned,
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
            undo_mode: None,
        };
        {
            let db = crate::persistence::Database::open_existing_untrusted(
                native.path(),
                DOWNLOADED_VALUE_LIMIT,
                DOWNLOADED_PROCESSING_DEADLINE,
            )
            .map_err(super::speleodb_cache_install::downloaded_file_error)?;
            db.load_project()
                .map_err(|e| SpeleoDbError::Parse(format!("invalid downloaded project: {e}")))?;
            new_link.undo_mode = Some(self.effective_undo_mode(preserved_undo_mode(&db)));
            db.save_speleo_link(Some(&new_link))
                .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        }
        let staged = super::speleodb_cache_install::stage_download(
            native.path(),
            &dest,
            InheritedUndo::Drop,
            super::speleodb_cache_install::sync_staged_file,
        )?;
        {
            let db = crate::persistence::Database::open_existing(&staged)
                .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
            restore_display_settings(
                &db,
                &super::speleodb_cache::preserved_display_settings(&dest),
            );
        }
        super::speleodb_cache_install::sync_staged_file(&staged)?;
        let result = self
            .open_project_download(
                staged.to_path_buf(),
                dest.clone(),
                (generation, bytes),
                was_editing,
            )
            .map_err(|e| SpeleoDbError::Io(e.message))?;
        if !result.success {
            return Err(SpeleoDbError::Io(
                result
                    .error
                    .unwrap_or_else(|| "Could not reload project".into()),
            ));
        }
        self.speleodb_set_unwound_from(None);
        self.speleodb_set_inherited_through(None);
        Ok(dest.to_string_lossy().into_owned())
    }

    /// Validate the downloaded file at `source` and stamp `link` into it, before it
    /// can replace a working cache. A failed link write must never turn a remote
    /// project into an editable local file.
    pub(super) fn stamp_downloaded_link(
        &self,
        source: &std::path::Path,
        link: crate::models::SpeleoLink,
    ) -> Result<crate::models::SpeleoLink, SpeleoDbError> {
        self.stamp_downloaded_link_within(source, link, DOWNLOADED_LIMITS)
    }

    fn stamp_downloaded_link_within(
        &self,
        source: &std::path::Path,
        mut link: crate::models::SpeleoLink,
        limits: (i32, std::time::Duration),
    ) -> Result<crate::models::SpeleoLink, SpeleoDbError> {
        let db = crate::persistence::Database::open_existing_untrusted(source, limits.0, limits.1)
            .map_err(super::speleodb_cache_install::downloaded_file_error)?;
        // Decode the complete project before any working bytes are replaced.
        db.load_project()
            .map_err(|e| SpeleoDbError::Parse(format!("invalid downloaded project: {e}")))?;
        link.undo_mode = Some(self.effective_undo_mode(preserved_undo_mode(&db)));
        db.save_speleo_link(Some(&link))
            .map_err(|e| SpeleoDbError::Io(format!("could not link downloaded project: {e}")))?;
        Ok(link)
    }

    fn install_linked_download_with_limits(
        &self,
        source: &std::path::Path,
        dest: &std::path::Path,
        link: crate::models::SpeleoLink,
        policy: LocalEditsPolicy,
        undo: InheritedUndo,
        options: DownloadOptions,
    ) -> Result<(), SpeleoDbError> {
        let link = self.stamp_downloaded_link_within(source, link, options.limits)?;
        let preserved_view = self.replace_cache_for_session(
            source,
            dest,
            &link.project_id,
            policy,
            undo,
            options.generation,
        )?;
        let db = crate::persistence::Database::open_existing(dest)
            .map_err(|e| SpeleoDbError::Io(format!("could not reopen downloaded project: {e}")))?;
        restore_display_settings(&db, &preserved_view);
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "synthetic native download regression fixtures"
)]
mod tests {
    use super::*;
    use crate::models::{Project, SpeleoLink, UndoMode};
    use crate::persistence::Database;

    fn link() -> SpeleoLink {
        SpeleoLink {
            endpoint: "https://example.com".into(),
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            project_name: "Remote project".into(),
            base_commit_sha: Some("abc123".into()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }
    }

    #[test]
    fn authentication_rejection_invalidates_downloads_without_closing_the_project() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("working.stalx");
            for (path, name) in [(&source, "Remote"), (&dest, "Local")] {
                let mut db = Database::open(path).expect("database");
                db.save_project(&Project::new(name.into(), None))
                    .expect("project");
                db.save_speleo_link(Some(&link())).expect("link");
            }
            let state = AppState::new().expect("state");
            assert!(state.open_project(dest.clone()).success);
            let generation = state.reserve_download_generation();
            state.speleodb_forget();
            let before = std::fs::read(&dest).expect("cache");
            assert!(matches!(
                state.install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(generation)
                ),
                Err(SpeleoDbError::SessionMoved)
            ));
            assert_eq!(std::fs::read(&dest).expect("cache"), before);
            assert_eq!(state.get_project_state().expect("open").name, "Local");
        });
    }

    #[test]
    fn a_newer_download_reservation_excludes_older_cache_installations() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("working.stalx");
            Database::open(&source)
                .expect("db")
                .save_project(&Project::new("New remote".into(), None))
                .expect("project");
            let state = AppState::new().expect("state");
            let older = state.reserve_download_generation();
            let newer = state.reserve_download_generation();
            state
                .install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(newer),
                )
                .expect("newer installs");
            let before = std::fs::read(&dest).expect("cache");
            for generation in [older, newer] {
                assert!(matches!(
                    state.install_linked_download_with_limits(
                        &source,
                        &dest,
                        link(),
                        LocalEditsPolicy::Protect,
                        InheritedUndo::Drop,
                        DownloadOptions::for_session(generation)
                    ),
                    Err(SpeleoDbError::SessionMoved)
                ));
                assert_eq!(std::fs::read(&dest).expect("cache"), before);
            }
        });
    }

    #[test]
    fn hostile_downloads_and_resource_limits_preserve_the_working_cache() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let dest = dir.path().join("working.stalx");
            Database::open(&dest)
                .expect("database")
                .save_project(&Project::new("Unpushed local work".into(), None))
                .expect("project");
            let before = std::fs::read(&dest).expect("bytes");
            let state = AppState::new().expect("state");
            for case in ["invalid", "schema", "value", "deadline", "future"] {
                let source = dir.path().join(format!("{case}.stalx"));
                if case == "invalid" {
                    std::fs::write(&source, b"not SQLite").expect("invalid");
                } else {
                    Database::open(&source)
                        .expect("database")
                        .save_project(&Project::new("X".repeat(4096), None))
                        .expect("project");
                    if case == "future" {
                        rusqlite::Connection::open(&source)
                            .expect("connection")
                            .pragma_update(
                                None,
                                "user_version",
                                crate::persistence::SCHEMA_VERSION + 1,
                            )
                            .expect("newer schema");
                    }
                    if case == "schema" {
                        rusqlite::Connection::open(&source)
                            .expect("connection")
                            .execute_batch("CREATE VIEW hostile AS SELECT 1;")
                            .expect("schema");
                    }
                }
                let limits = match case {
                    "value" => (1024, DOWNLOADED_PROCESSING_DEADLINE),
                    "deadline" => (DOWNLOADED_VALUE_LIMIT, std::time::Duration::ZERO),
                    _ => (DOWNLOADED_VALUE_LIMIT, DOWNLOADED_PROCESSING_DEADLINE),
                };
                let error = state
                    .install_linked_download_with_limits(
                        &source,
                        &dest,
                        link(),
                        LocalEditsPolicy::Protect,
                        InheritedUndo::Drop,
                        DownloadOptions {
                            limits,
                            generation: state.speleodb_session_generation(),
                        },
                    )
                    .expect_err("download must be refused");
                if case == "future" {
                    let SpeleoDbError::NewerSchema(newer) = error else {
                        panic!("newer-schema payload lost: {error:?}");
                    };
                    assert_eq!(newer.file, "project.stalx");
                    assert_eq!(newer.found, crate::persistence::SCHEMA_VERSION + 1);
                }
                assert_eq!(std::fs::read(&dest).expect("cache bytes"), before, "{case}");
            }
        });
    }

    #[test]
    fn delayed_download_cannot_close_a_newer_open_of_the_same_cache() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("working.stalx");
            for (path, name) in [(&source, "Remote replacement"), (&dest, "Newer local open")] {
                let mut db = Database::open(path).expect("database");
                db.save_project(&Project::new(name.into(), None))
                    .expect("project");
                db.save_speleo_link(Some(&link())).expect("link");
            }
            let state = AppState::new().expect("state");
            let generation = state.speleodb_session_generation();
            let (download_started, started) = std::sync::mpsc::channel();
            let (finish_download, finished) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let state = &state;
                let source = &source;
                let dest = &dest;
                let pull = scope.spawn(move || {
                    download_started.send(()).expect("start");
                    finished.recv().expect("download completion");
                    state.install_linked_download_with_limits(
                        source,
                        dest,
                        link(),
                        LocalEditsPolicy::Protect,
                        InheritedUndo::Drop,
                        DownloadOptions::for_session(generation),
                    )
                });
                started.recv().expect("download pending");
                assert!(state.open_project(dest.clone()).success);
                let before = std::fs::read(dest).expect("newer cache");
                finish_download.send(()).expect("finish");
                assert!(matches!(
                    pull.join().expect("thread"),
                    Err(SpeleoDbError::SessionMoved)
                ));
                assert_eq!(std::fs::read(dest).expect("unchanged"), before);
                assert_eq!(
                    state.get_project_state().expect("still open").name,
                    "Newer local open"
                );
            });
        });
    }

    #[test]
    fn a_confirmed_native_transition_invalidates_older_downloads_even_without_a_commit() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("working.stalx");
            for path in [&source, &dest] {
                let mut db = Database::open(path).expect("database");
                db.save_project(&Project::new("Synthetic".into(), None))
                    .expect("project");
                db.save_speleo_link(Some(&link())).expect("link");
            }
            let state = AppState::new().expect("state");
            let generation = state.speleodb_session_generation();
            state.cancel_loading(); // Every confirmed native open crosses this boundary.
            let before = std::fs::read(&dest).expect("cache");
            assert!(matches!(
                state.install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(generation)
                ),
                Err(SpeleoDbError::SessionMoved)
            ));
            assert_eq!(std::fs::read(dest).expect("unchanged"), before);
        });
    }

    #[test]
    fn a_pending_native_reader_excludes_cache_replacement() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("working.stalx");
            for path in [&source, &dest] {
                let mut db = Database::open(path).expect("database");
                db.save_project(&Project::new("Synthetic".into(), None))
                    .expect("project");
                db.save_speleo_link(Some(&link())).expect("link");
            }
            let state = AppState::new().expect("state");
            let generation = state.speleodb_session_generation();
            let token = state.start_new_load();
            let before = std::fs::read(&dest).expect("cache");
            assert!(matches!(
                state.install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(generation)
                ),
                Err(SpeleoDbError::SessionMoved)
            ));
            assert_eq!(std::fs::read(dest).expect("unchanged"), before);
            state.clear_active_load(&token);
        });
    }

    #[test]
    fn offline_fallback_requires_connection_failure_and_matching_cache_identity() {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("project.stalx");
        let mut cached_link = link();
        cached_link.has_unpushed_edits = true;
        let mut db = Database::open(&dest).expect("cache");
        db.save_project(&Project::new("Cached".into(), None))
            .expect("project");
        db.save_speleo_link(Some(&cached_link)).expect("link");
        for cause in [
            TransportCause::Refused,
            TransportCause::Timeout,
            TransportCause::Dns,
        ] {
            let error = SpeleoDbError::Transport {
                cause,
                detail: "offline".into(),
            };
            let result = offline_open_result(
                &dest,
                &cached_link.endpoint,
                &cached_link.project_id,
                &error,
            )
            .expect("matching cache opens offline");
            assert_eq!(result.offline, Some(true));
            assert_eq!(result.has_unpushed_edits, Some(true));
            assert!(offline_open_result(
                &dest,
                "https://other.example",
                &cached_link.project_id,
                &error
            )
            .is_none());
            assert!(
                offline_open_result(&dest, &cached_link.endpoint, "another-project", &error)
                    .is_none()
            );
            assert!(offline_open_result(
                &dir.path().join("missing.stalx"),
                &cached_link.endpoint,
                &cached_link.project_id,
                &error
            )
            .is_none());
        }
        let errors = [
            SpeleoDbError::Status {
                status: 401,
                message: "unauthorized".into(),
            },
            SpeleoDbError::Status {
                status: 403,
                message: "forbidden".into(),
            },
            SpeleoDbError::Status {
                status: 404,
                message: "gone".into(),
            },
            SpeleoDbError::Status {
                status: 500,
                message: "server error".into(),
            },
            SpeleoDbError::Parse("invalid download".into()),
            SpeleoDbError::Io("unreadable".into()),
            SpeleoDbError::Transport {
                cause: TransportCause::Tls,
                detail: "certificate".into(),
            },
            SpeleoDbError::Transport {
                cause: TransportCause::Unknown,
                detail: "unknown".into(),
            },
        ];
        for error in errors {
            assert!(offline_open_result(
                &dest,
                &cached_link.endpoint,
                &cached_link.project_id,
                &error
            )
            .is_none());
        }
        assert!(
            db.load_speleo_link()
                .expect("preserved link")
                .expect("linked")
                .has_unpushed_edits
        );
        db.save_speleo_link(None).expect("unlink");
        assert!(offline_open_result(
            &dest,
            &cached_link.endpoint,
            &cached_link.project_id,
            &SpeleoDbError::Transport {
                cause: TransportCause::Refused,
                detail: "offline".into()
            }
        )
        .is_none());
    }

    #[test]
    fn an_offline_http_hostname_endpoint_opens_its_cached_copy() {
        // sdk#6: an `http://` endpoint the transport policy allows behaves like
        // `https://` when it is offline. A hostname is outside the automatic HTTP
        // exceptions, so it is allowed only with the insecure-HTTP override; then a
        // host that does not resolve is the DNS transport failure, which is
        // offline-eligible exactly as it is for https. Without the override it is
        // refused before any request, and that refusal never opens the cache.
        // `.invalid` never resolves (RFC 6761), so this is offline by construction.
        let endpoint = "http://speleodb-offline.invalid:8000";
        let mut cached_link = link();
        cached_link.endpoint = endpoint.into();
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("project.stalx");
        let mut db = Database::open(&dest).expect("cache");
        db.save_project(&Project::new("Cached".into(), None))
            .expect("project");
        db.save_speleo_link(Some(&cached_link)).expect("link");
        drop(db);

        let refused = client::download_stalx(endpoint, "token", false, &cached_link.project_id)
            .expect_err("a plain-http hostname is refused without the override");
        assert!(
            matches!(refused, SpeleoDbError::InsecureEndpoint { .. }),
            "{refused:?}"
        );
        assert!(
            offline_open_result(&dest, endpoint, &cached_link.project_id, &refused).is_none(),
            "a refused endpoint is not offline"
        );

        let error = client::download_stalx(endpoint, "token", true, &cached_link.project_id)
            .expect_err("an unresolvable host cannot be downloaded from");
        assert!(
            matches!(
                error,
                SpeleoDbError::Transport {
                    cause: TransportCause::Dns,
                    ..
                }
            ),
            "an unresolvable http host must be an offline DNS failure, got {error:?}"
        );
        let opened = offline_open_result(&dest, endpoint, &cached_link.project_id, &error)
            .expect("the cached copy opens offline");
        assert_eq!(opened.offline, Some(true));
        assert_eq!(opened.local_path, dest.to_string_lossy());
    }

    #[test]
    fn first_download_creates_its_nested_cache_directory() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("cache/speleodb/new-project/project.stalx");
            Database::open(&source)
                .expect("download")
                .save_project(&Project::new("First download".into(), None))
                .expect("project");
            assert!(!dest.parent().expect("cache directory").exists());

            let state = AppState::new().expect("state");
            state
                .install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(state.speleodb_session_generation()),
                )
                .expect("first download installs without a pre-existing cache");

            let db = Database::open_existing(&dest).expect("installed SQLite");
            assert_eq!(db.load_project().expect("project").name, "First download");
            let installed = db.load_speleo_link().expect("link").expect("linked");
            assert_eq!(installed.project_id, link().project_id);
            assert_eq!(installed.endpoint, link().endpoint);
        });
    }

    #[test]
    fn failed_download_link_write_keeps_the_previous_project_open_and_untouched() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("project.stalx");
            for (path, name) in [(&source, "Downloaded"), (&dest, "Previous")] {
                Database::open(path)
                    .expect("database")
                    .save_project(&Project::new(name.into(), None))
                    .expect("project");
            }
            // A readable database can still reject the link upsert, just as a full
            // disk or write failure can. No trigger or external fixture is needed.
            rusqlite::Connection::open(&source)
                .expect("download")
                .execute_batch("ALTER TABLE speleo_link ADD COLUMN required_value TEXT NOT NULL")
                .expect("make link inserts fail");
            let state = AppState::new().expect("state");
            assert!(state.open_project(dest.clone()).success);
            let before = std::fs::read(&dest).expect("previous bytes");
            let err = state
                .install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(state.speleodb_session_generation()),
                )
                .expect_err("a remote project must not open without its link");
            assert!(
                err.to_string()
                    .contains("could not link downloaded project"),
                "{err}"
            );
            assert_eq!(std::fs::read(&dest).expect("unchanged cache"), before);
            assert_eq!(
                state.get_project_state().expect("still open").name,
                "Previous"
            );
        });
    }

    #[test]
    fn installed_download_keeps_remote_undo_policy_and_local_display_settings() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            let dest = dir.path().join("project.stalx");
            {
                let mut db = Database::open(&source).expect("download");
                db.save_project(&Project::new("Downloaded".into(), None))
                    .expect("project");
                let mut old_link = link();
                old_link.endpoint = "https://old.example.com".into();
                old_link.undo_mode = Some(UndoMode::Disabled);
                db.save_speleo_link(Some(&old_link))
                    .expect("old identity and undo policy");
            }
            {
                let mut db = Database::open(&dest).expect("cache");
                db.save_project(&Project::new("Previous".into(), None))
                    .expect("project");
                db.save_display_setting("test_view", "local").expect("view");
            }
            let state = AppState::new().expect("state");
            state
                .install_linked_download_with_limits(
                    &source,
                    &dest,
                    link(),
                    LocalEditsPolicy::Protect,
                    InheritedUndo::Drop,
                    DownloadOptions::for_session(state.speleodb_session_generation()),
                )
                .expect("install");
            let db = Database::open_existing(&dest).expect("installed");
            let actual = db.load_speleo_link().expect("link").expect("linked");
            assert_eq!(actual.endpoint, "https://example.com");
            assert_eq!(actual.base_commit_sha.as_deref(), Some("abc123"));
            assert_eq!(actual.undo_mode, Some(UndoMode::Disabled));
            assert_eq!(
                db.load_display_settings()
                    .expect("view")
                    .get("test_view")
                    .map(String::as_str),
                Some("local")
            );
        });
    }
}
