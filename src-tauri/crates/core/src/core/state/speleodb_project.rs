//! [#206]/[#207]/[#200] The SpeleoDB project RECORD: fold the server's project
//! name into the local file, and create the record (publish a local project onto
//! the server).
//!
//! Split out of `core/state/speleodb.rs` (which is frozen at the file-size ratchet
//! warn line) into a sibling `impl AppState` block, alongside the #215 admin split.
//! Remote metadata writes require an acquired editing session and fresh server
//! verification of permission and lock ownership. Server enforcement remains the
//! final fence.
//!
//! The create POST is the one non-idempotent call in the family: a retry would mint
//! a second project, so every deterministic failure has to be caught before it.
//! Lock release and file-access checks live in `speleodb_admin`.

use std::path::Path;

use super::open_file::{read_unchanged, write_error, FileOrigin, FileState};
use super::speleodb::{local_dto, validate_project_id, MANAGED_PROJECT_TYPE};
use super::speleodb_push::PushOutcome;
use super::AppState;
use crate::persistence::stalx_v2;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{
    SpeleoDbError, SpeleoErrorDto, SpeleoPublishParams, SpeleoPublishResult, Visibility,
};

/// [sdk#3] What the publish result reports about the first upload's undo history:
/// `Some(true)` only when that upload LANDED with the history left out to fit the
/// server's size limit. An upload that failed or was refused reports nothing - the
/// project stays dirty and the later Save Changes reports its own outcome.
fn publish_undo_history_omitted(upload: &Result<PushOutcome, SpeleoErrorDto>) -> Option<bool> {
    matches!(
        upload,
        Ok(PushOutcome::Confirmed {
            undo_history_omitted: true
        })
    )
    .then_some(true)
}

/// Plan 12.1: what the publish result reports when the server refused the first
/// upload for its file format: `Some(true)`, so the webview says the server
/// project is empty instead of "created". Any other outcome reports nothing.
fn publish_upload_format_refused(upload: &Result<PushOutcome, SpeleoErrorDto>) -> Option<bool> {
    matches!(upload, Err(e) if e.format_refused == Some(true)).then_some(true)
}

/// The canonical SpeleoDB wire label for a visibility ([#206]/[#207]). The server's
/// `CustomChoiceField` accepts the enum NAME case-insensitively on input; we send the
/// UPPERCASE label it returns so a round-trip is byte-identical.
fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "PRIVATE",
        Visibility::Public => "PUBLIC",
    }
}

/// Build the `POST /projects/` body for a create-on-SDB publish ([#207]). `type` is
/// fixed to `stalagmight` ([`MANAGED_PROJECT_TYPE`]) and `exclude_geojson` to `false` (the repo-layout contract,
/// [#208]); `created_by` is injected server-side. lat/long are both-or-neither: sent
/// only when BOTH are present and finite, else omitted. Pure + unit-tested.
fn build_create_body(params: &SpeleoPublishParams) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert(
        "name".to_owned(),
        serde_json::Value::from(params.name.clone()),
    );
    body.insert(
        "description".to_owned(),
        serde_json::Value::from(params.description.clone()),
    );
    body.insert(
        "country".to_owned(),
        serde_json::Value::from(params.country.clone()),
    );
    body.insert(
        "type".to_owned(),
        serde_json::Value::from(MANAGED_PROJECT_TYPE),
    );
    body.insert("exclude_geojson".to_owned(), serde_json::Value::Bool(false));
    body.insert(
        "visibility".to_owned(),
        serde_json::Value::from(visibility_label(params.visibility)),
    );
    if let (Some(lat), Some(lon)) = (params.latitude, params.longitude) {
        if let (Some(lat), Some(lon)) = (
            serde_json::Number::from_f64(lat),
            serde_json::Number::from_f64(lon),
        ) {
            body.insert("latitude".to_owned(), serde_json::Value::Number(lat));
            body.insert("longitude".to_owned(), serde_json::Value::Number(lon));
        }
    }
    serde_json::Value::Object(body)
}

impl AppState {
    /// Fold an SDB-authoritative project name into the OPEN local `.stalx` WITHOUT
    /// setting the dirty bit ([#206]/[#216]): update the in-memory project and the
    /// link's cached `project_name`, and make both durable at once - the name by a
    /// rename of the file's `project` member (`stalx_v2::rename_project`: every
    /// other member copied verbatim, so unsaved survey edits stay unsaved), the
    /// link by the local-only save. Neither marks the project changed: the name is
    /// already the server's. Best-effort - a failure is logged, never surfaced (the
    /// server write already landed).
    ///
    /// Everything happens under ONE `project` acquisition, with the identity check
    /// first: a late PATCH/GET response must never rename the replacement project.
    /// A file opened read-only ("made with a newer Stalagmight") keeps its name in
    /// memory and on disk; only the link's cached name follows. A legacy file not
    /// upgraded yet, or a project never saved, keeps the fold in memory for its
    /// first save.
    pub(super) fn speleodb_fold_local_name(
        &self,
        name: &str,
        project_id: &str,
        expected_generation: u64,
    ) {
        let Ok(mut project_guard) = self.project.lock() else {
            return;
        };
        if self.speleodb_session_generation() != expected_generation {
            return;
        }
        let Some(open) = project_guard.as_mut() else {
            return;
        };
        let path = open.file_path.clone();
        let renamed = {
            let mut file = self
                .file
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(mut link) = file.speleo_link() else {
                return;
            };
            if link.project_id != project_id {
                return;
            }
            name.clone_into(&mut link.project_name);
            file.set_speleo_link(Some(&link));
            let rename = open.name != name && file.extras.read_only.is_none();
            if rename {
                name.clone_into(&mut open.name);
                if let Err(error) = rename_on_disk(path.as_deref(), &mut file, name) {
                    tracing::warn!(%error, "could not persist the SpeleoDB project name");
                }
            }
            rename
        };
        if renamed {
            // A name change is a change of the project slot.
            self.bump_revision();
        }
        if let Err(error) = self.persist_local_state_at(path) {
            tracing::warn!(%error, "could not persist the SpeleoDB link name");
        }
    }

    /// [#207] Create the OPEN local project on SpeleoDB and push its first revision.
    /// Requires an authenticated session but NOT an existing remote link (this is
    /// what mints one). Steps: `POST /projects/` → relocate the working copy into the
    /// SpeleoDB cache (remote projects live there, mirroring `speleodb_open_project`)
    /// → stamp a fresh `SpeleoLink` (dirty) → acquire → upload → release. No fence is
    /// taken: a project created a moment ago and locked by us cannot have diverged.
    /// Returns the new id + endpoint + the cache `local_path` the webview reloads.
    pub fn speleodb_publish(
        &self,
        params: SpeleoPublishParams,
    ) -> Result<SpeleoPublishResult, SpeleoErrorDto> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        let (endpoint, token, email) = self.speleodb_active_account().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Guard rails BEFORE the irreversible create POST. The create is
        // non-idempotent, so any deterministic failure AFTER it would leak an empty
        // server project that a user retry then DUPLICATES - catch those up front:
        //  - blank name: the server requires one, and a later name fold of "" would
        //    also wipe the embedded `.stalx` name;
        //  - already linked: the FE only offers Publish for a LOCAL project, but a stale
        //    menu / race must not POST a duplicate and overwrite the existing hand-off
        //    link (abandoning its lock);
        //  - the two DETERMINISTIC failures of step 2's save, checked here so its
        //    relocate cannot strand a freshly-created project: a survey that breaks an
        //    invariant (the save refuses to write it) and a file open read-only.
        if params.name.trim().is_empty() {
            return Err(local_dto(SpeleoDbError::Io(
                "a project name is required to publish".to_owned(),
            )));
        }
        if self.speleodb_open_link().is_ok() {
            return Err(local_dto(SpeleoDbError::Io(
                "this project is already linked to SpeleoDB".to_owned(),
            )));
        }
        let generation = {
            let proj_guard = self
                .project
                .lock()
                .map_err(|_| local_dto(SpeleoDbError::Io("project lock poisoned".to_owned())))?;
            let project = proj_guard.as_ref().ok_or_else(|| {
                local_dto(SpeleoDbError::Io(
                    "no project is open to publish".to_owned(),
                ))
            })?;
            if !project.validate().is_empty() {
                return Err(local_dto(SpeleoDbError::Io(
                    "can't publish: the project has unresolved cross-references (use Repair first)"
                        .to_owned(),
                )));
            }
            if self.with_file(|file| file.extras.read_only.is_some()) {
                return Err(local_dto(SpeleoDbError::Io(
                    "can't publish: this project was made with a newer Stalagmight and is \
                     open read-only"
                        .to_owned(),
                )));
            }
            self.speleodb_session_generation()
        };

        // 1. Create the project. `type: stalagmight` + `exclude_geojson: false` are fixed
        //    (the repo-layout contract, [#208]); lat/long are both-or-neither;
        //    `created_by` is injected server-side. Runs once (create is
        //    non-idempotent) so a mid-response failure can't double-submit.
        let payload = build_create_body(&params).to_string();

        let created = self
            .speleodb_handle_auth(
                Endpoint::CreateProject,
                client::create_project_with(&endpoint, &token, insecure_http, &payload),
            )
            .map_err(|e| dto_for_error(Endpoint::CreateProject, &e))?;

        let project_id = created.id.clone();
        // Validate the server-issued id before it reaches a cache-path join.
        validate_project_id(&project_id).map_err(local_dto)?;

        // 2. Relocate the open project's working copy into the SpeleoDB cache. A remote
        //    project's canonical file lives at `speleodb_cache_path` (open/fast-forward
        //    both extract there), and the push machinery reads exactly that path.
        //    `save_project(Some(dest))` writes a fresh `.stalx` there AND switches the
        //    open project's `file_path` (and the save guard's fingerprint) to it. A
        //    legacy original is left as it was: only the open file's own save upgrades.
        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        }
        let save = self.save_project_at_generation(Some(dest.clone()), Some(generation));
        if !save.success {
            return Err(local_dto(SpeleoDbError::Io(format!(
                "failed to write project into the SpeleoDB cache: {}",
                save.error.unwrap_or_else(|| "unknown error".to_owned())
            ))));
        }

        // 3. Stamp a fresh hand-off link onto the cached file. BASE = the create
        //    response's latest_commit (None for a brand-new empty repo); dirty = true
        //    (the local content is not on the server yet). New sanctioned stamp site.
        //    [#303] A new link materializes the application default immediately: the
        //    project was unlinked, so it carries no undo mode of its own.
        let link = crate::models::SpeleoLink {
            endpoint: endpoint.clone(),
            project_id: project_id.clone(),
            project_name: params.name.clone(),
            base_commit_sha: created.latest_commit.as_ref().map(|c| c.id.clone()),
            has_unpushed_edits: true,
            last_edit_at: Some(chrono::Utc::now().to_rfc3339()),
            last_verify_at: None,
            undo_mode: Some(self.effective_undo_mode(None)),
        };
        {
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.speleodb_session_generation() != generation {
                return Err(local_dto(SpeleoDbError::SessionMoved));
            }
            let path = project.as_ref().and_then(|p| p.file_path.clone());
            self.with_file(|file| file.set_speleo_link(Some(&link)));
            self.persist_local_state_at(path)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
            // Conversion has crossed into a managed project. Initial upload is
            // internal; subsequent editing must acquire and reconcile the lock, so the
            // gate is now the managed one, with no claim bound to it (read-only).
            self.speleodb_set_open_gate(Some(&link));
        }
        // The new cached copy belongs to the account that created the project.
        if let Some(email) = self.speleodb_my_email() {
            self.speleodb_record_cache_owner(&dest, &endpoint, &email, &params.name);
        }

        // 4. Initial push (BEST-EFFORT): acquire the lock (we are the creator/ADMIN, so
        //    it is free), upload the snapshot + `.tml` render, then release. No fence -
        //    nothing can have raced a just-created project we hold the lock on.
        //
        //    CRUCIAL: the project now EXISTS on the server and is linked + dirty locally
        //    (steps 1-3 already committed, irreversibly). Publish has therefore SUCCEEDED
        //    as a create; the first push is a follow-on. A failure here must NOT return a
        //    bare Err, because the webview only reloads the now-remote project on Ok - a
        //    bare Err would strand it on a stale "local" view, where the Publish action
        //    stays enabled and a retry re-POSTs a DUPLICATE server project. So we log any
        //    acquire/upload failure and STILL return the cache path: the webview reloads a
        //    dirty remote project and the user completes the upload via the normal Start
        //    editing → Done (which surfaces a genuine push error, in context, if any).
        //    A server that refused the FILE FORMAT (plan 12.1) is the one failure
        //    the result names (`upload_format_refused`): the server project is empty
        //    and Save Changes would only be refused again, so the webview says so
        //    instead of "use Save Changes to finish uploading".
        let mut undo_history_omitted = None;
        let mut upload_format_refused = None;
        match self.speleodb_handle_auth(
            Endpoint::Acquire,
            client::acquire_lock(&endpoint, &token, insecure_http, &project_id),
        ) {
            Ok(_) => {
                let upload = self.speleodb_upload(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link,
                    params.message,
                    // Skip the #216 name fold: the project was just created with this
                    // exact name, so the fold GET would be a guaranteed no-op.
                    false,
                    generation,
                );
                // Release the lock regardless of the push outcome (best-effort,
                // holder-checked like every automatic release).
                self.speleodb_release_best_effort(
                    &super::speleodb_lock_release::LockReleaseParams {
                        endpoint: endpoint.clone(),
                        token: token.clone(),
                        email,
                        insecure_http,
                        project_id: project_id.clone(),
                    },
                );
                undo_history_omitted = publish_undo_history_omitted(&upload);
                upload_format_refused = publish_upload_format_refused(&upload);
                if let Err(e) = upload {
                    tracing::warn!(
                        error = ?e,
                        project_id = %project_id,
                        purpose = "initial publish push",
                        "speleodb_publish: project created + linked, but the initial push did not confirm; it stays dirty for a later Done"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "initial publish push",
                    "speleodb_publish: project created + linked, but could not acquire the lock for the initial push; it stays dirty for a later Done"
                );
            }
        }

        // 5. If the initial push did not confirm, BASE is still the empty-repo
        //    sentinel from step 3 while the copy is dirty - and that pair fences
        //    straight to Diverged, so the user's first Start editing on a project
        //    they JUST published would open the divergence prompt. Resolve the
        //    server tip and adopt it instead: we created this project moments ago
        //    and nobody else can have pushed to it, so the tip is exactly the state
        //    our local content sits on top of. The dirty bit stays set (our content
        //    genuinely is not on the server), which makes the next push a clean
        //    Upload rather than a reconcile. Best-effort and non-fatal: on failure
        //    we are no worse off than before. A confirmed push already advanced BASE
        //    in `speleodb_upload`, so this is a no-op on the happy path.
        if self
            .speleodb_open_link()
            .is_ok_and(|l| l.base_commit_sha.is_none())
        {
            match self.speleodb_server_head(&endpoint, &token, insecure_http, &project_id) {
                Ok((tip, _)) if tip.is_some() => {
                    let project = self
                        .project
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if self.speleodb_session_generation() == generation {
                        let path = project.as_ref().and_then(|p| p.file_path.clone());
                        let stamped = self.with_file(|file| {
                            let mut link = file.speleo_link()?;
                            link.base_commit_sha.clone_from(&tip);
                            file.set_speleo_link(Some(&link));
                            Some(())
                        });
                        if stamped.is_some() {
                            if let Err(e) = self.persist_local_state_at(path) {
                                tracing::warn!(error = %e, "speleodb_publish: the adopted BASE is set in memory only");
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "publish BASE recovery",
                    "speleodb_publish: could not resolve the server tip to stamp BASE; the first Start editing may report a divergence"
                ),
            }
        }

        Ok(SpeleoPublishResult {
            project_id,
            endpoint,
            local_path: dest.to_string_lossy().into_owned(),
            undo_history_omitted,
            upload_format_refused,
        })
    }
}

/// Rename the `project` member of the open version 2 file at `path` (the
/// caller holds `project` and `file`), guarded by the fingerprint last read or
/// written. A legacy file not upgraded yet, or a project never saved, keeps the
/// name in memory for its first save.
fn rename_on_disk(path: Option<&Path>, file: &mut FileState, name: &str) -> Result<(), String> {
    let (Some(path), Some(expected), FileOrigin::V2) =
        (path, file.fingerprint.clone(), file.origin)
    else {
        file.local_pending = true;
        file.local_pending_asks = true;
        return Ok(());
    };
    let original = read_unchanged(path, &expected).map_err(String::from)?;
    let bytes = stalx_v2::rename_project(&original, name).map_err(|e| e.to_string())?;
    let fingerprint = stalx_v2::write_atomic(path, &bytes, stalx_v2::Guard::Expect(&expected))
        .map_err(|e| String::from(write_error(&e)))?;
    file.fingerprint = Some(fingerprint);
    Ok(())
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        build_create_body, publish_undo_history_omitted, publish_upload_format_refused,
        visibility_label,
    };
    use crate::core::state::speleodb_push::PushOutcome;
    use crate::speleodb::{SpeleoPublishParams, Visibility};

    #[test]
    fn visibility_label_matches_the_wire_contract() {
        // The server's CustomChoiceField returns the UPPERCASE label; we send exactly
        // that so a metadata round-trip is byte-identical (#206/#207).
        assert_eq!(visibility_label(Visibility::Private), "PRIVATE");
        assert_eq!(visibility_label(Visibility::Public), "PUBLIC");
    }

    #[test]
    fn publish_reports_an_omitted_undo_history_only_for_a_landed_upload() {
        // sdk#3: the publish result tells the webview the first revision went up
        // without its undo history, exactly when the upload landed that way.
        let landed = |undo_history_omitted| {
            publish_undo_history_omitted(&Ok(PushOutcome::Confirmed {
                undo_history_omitted,
            }))
        };
        assert_eq!(landed(true), Some(true));
        assert_eq!(landed(false), None);
        assert_eq!(
            publish_undo_history_omitted(&Ok(PushOutcome::Unconfirmed)),
            None
        );
        let refused = crate::speleodb::contract::dto_for_error(
            crate::speleodb::contract::Endpoint::UploadStalx,
            &crate::speleodb::SpeleoDbError::UploadTooLarge { size: 2, limit: 1 },
        );
        assert_eq!(publish_undo_history_omitted(&Err(refused)), None);
    }

    #[test]
    fn publish_reports_a_format_refusal_and_nothing_else() {
        use crate::speleodb::contract::{dto_for_error, Endpoint};
        use crate::speleodb::SpeleoDbError;
        let refused = dto_for_error(
            Endpoint::UploadStalx,
            &SpeleoDbError::UploadFormatRefused {
                status: 400,
                detail: "synthetic".to_owned(),
            },
        );
        assert_eq!(publish_upload_format_refused(&Err(refused)), Some(true));
        let other = dto_for_error(
            Endpoint::UploadStalx,
            &SpeleoDbError::Status {
                status: 403,
                message: "fence".to_owned(),
            },
        );
        assert_eq!(publish_upload_format_refused(&Err(other)), None);
        assert_eq!(
            publish_upload_format_refused(&Ok(PushOutcome::Unconfirmed)),
            None
        );
        assert_eq!(
            publish_upload_format_refused(&Ok(PushOutcome::Confirmed {
                undo_history_omitted: false
            })),
            None
        );
    }

    #[test]
    fn create_body_pins_the_repo_layout_contract() {
        // #208: every create fixes type=stalagmight + exclude_geojson=false; visibility label;
        // country as given; lat/long omitted when not both present (#207 both-or-neither).
        let params = SpeleoPublishParams {
            message: Some("Synthetic test publish".to_owned()),
            name: "Cave X".to_owned(),
            description: "desc".to_owned(),
            country: "US".to_owned(),
            visibility: Visibility::Private,
            latitude: None,
            longitude: None,
        };
        let body = build_create_body(&params);
        assert_eq!(
            body.get("type").and_then(|v| v.as_str()),
            Some("STALAGMIGHT"),
            "#208: the repo type is always `stalagmight`"
        );
        assert_eq!(
            body.get("exclude_geojson"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            body.get("visibility").and_then(|v| v.as_str()),
            Some("PRIVATE")
        );
        assert_eq!(body.get("country").and_then(|v| v.as_str()), Some("US"));
        assert_eq!(body.get("name").and_then(|v| v.as_str()), Some("Cave X"));
        let obj = body.as_object().expect("create body is a JSON object");
        assert!(
            !obj.contains_key("latitude") && !obj.contains_key("longitude"),
            "no pin -> lat/long omitted (both-or-neither)"
        );
    }

    #[test]
    fn create_body_includes_both_coordinates_when_pinned() {
        let params = SpeleoPublishParams {
            message: Some("Synthetic test publish".to_owned()),
            name: "Cave Y".to_owned(),
            description: String::new(),
            country: "FR".to_owned(),
            visibility: Visibility::Public,
            latitude: Some(45.1),
            longitude: Some(-1.2),
        };
        let body = build_create_body(&params);
        assert_eq!(body.get("latitude"), Some(&serde_json::json!(45.1)));
        assert_eq!(body.get("longitude"), Some(&serde_json::json!(-1.2)));
        assert_eq!(
            body.get("visibility").and_then(|v| v.as_str()),
            Some("PUBLIC")
        );
    }

    #[test]
    fn publish_relocates_into_the_cache_before_it_stamps_the_link() {
        // save_project treats "linked + explicit destination" as Save a Copy (the
        // copy is written WITHOUT the link and the session stays put). Publish's
        // relocate-into-cache is an explicit-destination save, so it is only a
        // real relocate because the project is STILL UNLINKED when it runs - the
        // link is stamped after. Reorder those two steps and publish silently
        // stops relocating (the cache gets an unlinked copy while the session
        // stays on the user's original file). Guarded by ORDER in the source, the
        // same idiom the pull-guard test uses: a behavioural version needs the
        // process-global app dirs.
        let src = include_str!("speleodb_project.rs");
        let start = src
            .find("fn speleodb_publish")
            .expect("the publish body is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];
        let relocate = body
            .find("self.save_project_at_generation(Some(")
            .expect("publish relocates via an explicit-destination save");
        let stamp = body
            .find("file.set_speleo_link(Some(&link))")
            .expect("publish stamps the fresh link");
        assert!(
            relocate < stamp,
            "publish must relocate into the cache BEFORE the link exists - a linked \
             explicit-destination save is a copy save and would not relocate"
        );
    }
}
