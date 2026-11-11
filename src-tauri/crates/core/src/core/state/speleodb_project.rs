//! [#206]/[#207]/[#200] The SpeleoDB project RECORD: read its authoritative
//! metadata, PATCH it, and create it (publish a local project onto the server).
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

use super::speleodb::{local_dto, validate_project_id, MANAGED_PROJECT_TYPE};
use super::speleodb_push::PushOutcome;
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{
    SpeleoDbError, SpeleoErrorDto, SpeleoProjectMetadata, SpeleoPublishParams, SpeleoPublishResult,
    Visibility,
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
    /// [#206] GET the open remote project's authoritative metadata so the Project
    /// Settings tab can prefill the current server truth. No lock required (reading
    /// metadata is permission-free); routed through the auth handler so a dead token
    /// here forces re-login (mirrors the other project-scoped reads).
    pub fn speleodb_get_project_metadata(&self) -> Result<SpeleoProjectMetadata, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let project = self
            .speleodb_handle_auth(
                Endpoint::GetProject,
                client::get_project(&endpoint, &token, insecure_http, &link.project_id),
            )
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
        Ok(SpeleoProjectMetadata::from_project(&project))
    }

    /// Fold an SDB-authoritative project name into the OPEN local `.stalx` WITHOUT
    /// setting the dirty bit ([#206]/[#216]): update the in-memory project, persist it
    /// directly (`db.save_project` bypasses the command/undo/dirty path and never
    /// touches the `speleo_link` table), and refresh the link's cached `project_name`.
    /// Best-effort - a failure is logged, never surfaced (the server write already
    /// landed). Locks project → database in that order (mirrors `speleodb_render_tml`).
    pub(super) fn speleodb_fold_local_name(
        &self,
        name: &str,
        project_id: &str,
        expected_generation: u64,
    ) {
        // Keep project -> database locked across the identity check and both writes.
        // A late PATCH/GET response must never rename the replacement project.
        let Ok(mut project_guard) = self.project.lock() else {
            return;
        };
        if self.speleodb_session_generation() != expected_generation {
            return;
        }
        let Some(open) = project_guard.as_mut() else {
            return;
        };
        let Ok(mut database_guard) = self.database.lock() else {
            return;
        };
        let Some(db) = database_guard.as_mut() else {
            return;
        };
        let Ok(Some(mut link)) = db.load_speleo_link() else {
            return;
        };
        if link.project_id != project_id {
            return;
        }
        if open.name != name {
            open.name = name.to_owned();
            if let Err(error) = db.save_project(open) {
                tracing::warn!(%error, "could not persist the SpeleoDB project name");
            }
        }
        link.project_name = name.to_owned();
        if let Err(error) = db.save_speleo_link(Some(&link)) {
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
        //  - unresolvable cross-references: the one DETERMINISTIC `save_project` failure,
        //    checked here so step 2's relocate cannot strand a freshly-created project.
        if params.name.trim().is_empty() {
            return Err(local_dto(SpeleoDbError::Io(
                "a project name is required to publish".to_owned(),
            )));
        }
        // The server's Project model requires a non-blank description (`blank=False`),
        // so a blank one 400s the create. Reject it up front (the FE gates on it too)
        // for a clear message instead of a raw server validation error.
        if params.description.trim().is_empty() {
            return Err(local_dto(SpeleoDbError::Io(
                "a project description is required to publish".to_owned(),
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
            if project.has_unresolvable_cross_references() {
                return Err(local_dto(SpeleoDbError::Io(
                    "can't publish: the project has unresolved cross-references (use Repair first)"
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
        //    active database + `project.file_path` to it.
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
        let link = crate::models::SpeleoLink {
            endpoint: endpoint.clone(),
            project_id: project_id.clone(),
            project_name: params.name.clone(),
            base_commit_sha: created.latest_commit.as_ref().map(|c| c.id.clone()),
            has_unpushed_edits: true,
            last_edit_at: Some(chrono::Utc::now().to_rfc3339()),
            last_verify_at: None,
            // [#303] A new link materializes the application default immediately.
            // Re-publishing a detached managed copy preserves its explicit project value.
            undo_mode: Some({
                let db_guard = self.database.lock().map_err(|_| {
                    local_dto(SpeleoDbError::Io("database lock poisoned".to_owned()))
                })?;
                let db = db_guard.as_ref().ok_or_else(|| {
                    local_dto(SpeleoDbError::Io(
                        "no open database after writing the cache file".to_owned(),
                    ))
                })?;
                self.effective_undo_mode(super::speleodb_cache::preserved_undo_mode(db))
            }),
        };
        {
            let _project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.speleodb_session_generation() != generation {
                return Err(local_dto(SpeleoDbError::SessionMoved));
            }
            let db_guard = self
                .database
                .lock()
                .map_err(|_| local_dto(SpeleoDbError::Io("database lock poisoned".to_owned())))?;
            let db = db_guard.as_ref().ok_or_else(|| {
                local_dto(SpeleoDbError::Io(
                    "no open database after writing the cache file".to_owned(),
                ))
            })?;
            db.save_speleo_link(Some(&link))
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
        let mut undo_history_omitted = None;
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
                    params.message.clone(),
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
                    let _project = self
                        .project
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if self.speleodb_session_generation() == generation {
                        let _ = self.speleodb_mutate_link(|l| l.base_commit_sha = tip.clone());
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
        })
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{build_create_body, publish_undo_history_omitted, visibility_label};
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
            .find(".save_speleo_link(Some(")
            .expect("publish stamps the fresh link");
        assert!(
            relocate < stamp,
            "publish must relocate into the cache BEFORE the link exists - a linked \
             explicit-destination save is a copy save and would not relocate"
        );
    }
}
