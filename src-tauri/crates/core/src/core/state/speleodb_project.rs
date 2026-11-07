//! [#206]/[#207]/[#200] The SpeleoDB project RECORD: read its authoritative
//! metadata, PATCH it, and create it (publish a local project onto the server).
//!
//! Split out of `core/state/speleodb.rs` (which is frozen at the file-size ratchet
//! warn line) into a sibling `impl AppState` block, alongside the #215 admin split.
//! Everything here is permission-gated only in the UI - the server's 401/403 is the
//! real enforcement - and none of it takes the edit mutex, which governs only the
//! survey `.stalx`.
//!
//! The create POST is the one non-idempotent call in the family: a retry would mint
//! a second project, so every deterministic failure has to be caught before it.
//! Collaborators, teams and project delete live in `speleodb_admin`.

use super::speleodb::{local_dto, validate_project_id, MANAGED_PROJECT_TYPE};
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::{
    SpeleoDbError, SpeleoErrorDto, SpeleoProjectMetadata, SpeleoProjectMetadataPatch,
    SpeleoPublishParams, SpeleoPublishResult, Visibility,
};

/// The canonical SpeleoDB wire label for a visibility ([#206]/[#207]). The server's
/// `CustomChoiceField` accepts the enum NAME case-insensitively on input; we send the
/// UPPERCASE label it returns so a round-trip is byte-identical.
///
/// `None` for [`Visibility::Unknown`] (#576 B16): a value the server sent that this
/// build cannot name has no faithful label to send back, so the caller leaves the
/// field out and the server keeps (or defaults) its own.
fn visibility_label(visibility: Visibility) -> Option<&'static str> {
    match visibility {
        Visibility::Private => Some("PRIVATE"),
        Visibility::Public => Some("PUBLIC"),
        Visibility::Unknown => None,
    }
}

/// Map an optional coordinate to a JSON value for a PATCH body: a finite number when
/// present, else `null` (the server clears the pin on null - both-or-neither, [#206]).
/// A non-finite `f64` (should never reach here) also becomes `null`.
fn coord_json(value: Option<f64>) -> serde_json::Value {
    value
        .and_then(serde_json::Number::from_f64)
        .map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// Build the `PATCH /projects/{id}/` body from ONLY the changed fields ([#206]). A
/// `None` scalar means "unchanged, omit"; the location pin is gated on
/// `coordinates_edited` (both-or-neither: two numbers set it, two nulls clear it, and
/// it is omitted entirely when the user did not touch it). An EMPTY map means nothing
/// changed - the caller then skips the no-op PATCH. Pure + unit-tested.
fn build_patch_body(
    patch: &SpeleoProjectMetadataPatch,
) -> serde_json::Map<String, serde_json::Value> {
    let mut body = serde_json::Map::new();
    if let Some(name) = &patch.name {
        body.insert("name".to_owned(), serde_json::Value::from(name.clone()));
    }
    if let Some(description) = &patch.description {
        body.insert(
            "description".to_owned(),
            serde_json::Value::from(description.clone()),
        );
    }
    if let Some(country) = &patch.country {
        body.insert(
            "country".to_owned(),
            serde_json::Value::from(country.clone()),
        );
    }
    if let Some(label) = patch.visibility.and_then(visibility_label) {
        body.insert("visibility".to_owned(), serde_json::Value::from(label));
    }
    if patch.coordinates_edited {
        body.insert("latitude".to_owned(), coord_json(patch.latitude));
        body.insert("longitude".to_owned(), coord_json(patch.longitude));
    }
    body
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
    // An unknown visibility (#576 B16) is left out: the server applies its own
    // default, PRIVATE - the fail-safe one.
    if let Some(label) = visibility_label(params.visibility) {
        body.insert("visibility".to_owned(), serde_json::Value::from(label));
    }
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

/// [#576 NF-A8R-4] The metadata PATCH's licence: a user-initiated edit of the
/// project's metadata, which has no precondition - no lock (the edit lock governs
/// only the survey file) and no landing; the server enforces the permission.
struct UserMetadataEdit;
impl client::PatchLicence for UserMetadataEdit {}

impl AppState {
    /// Set the open project's server-side `exclude_geojson` flag (#200
    /// Enable-rendering): `PATCH /projects/{id}/`. The server enforces permission
    /// (READ_AND_WRITE) and NO lock is required - the edit mutex governs only the
    /// survey `.stalx`, not project metadata. Setting it `false` re-enables
    /// web-viewer GeoJSON generation, which takes effect only on the NEXT push
    /// (the server has no retroactive re-render for the current tip; the FE
    /// confirmation states this). A 403 surfaces as an error the button reports.
    pub fn speleodb_set_exclude_geojson(&self, exclude: bool) -> Result<(), SpeleoErrorDto> {
        let link = self.speleodb_link_made_for("Changing web-viewer rendering")?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let payload = serde_json::json!({ "exclude_geojson": exclude }).to_string();
        // [#576 B5] Mapped through the PATCH's own row. It used to borrow GetProject
        // ("purely cosmetic reuse"), which made a refused PATCH read-only access and a
        // 5xx "already retried" over a write that runs once.
        let row = client::spec::PATCH_PROJECT.category;
        self.speleodb_handle_auth(
            row,
            client::patch_project(
                &UserMetadataEdit,
                &endpoint,
                &token,
                insecure_http,
                &link.project_id,
                &payload,
            ),
        )
        .map_err(|e| dto_for_error(row, &e))?;
        Ok(())
    }

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

    /// [#206] PATCH the open remote project's metadata with ONLY the changed fields,
    /// then fold the SDB-authoritative name into the local `.stalx` (the name is the
    /// one field stored on both sides; SDB is authoritative - see [#216]). No lock is
    /// required (PATCH `/projects/{id}/` is `READ_AND_WRITE`, mutex-free); the edit
    /// lock governs only the survey file. Returns the post-edit server truth.
    pub fn speleodb_patch_project(
        &self,
        patch: SpeleoProjectMetadataPatch,
    ) -> Result<SpeleoProjectMetadata, SpeleoErrorDto> {
        let link = self.speleodb_link_made_for("Editing the project details")?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Reject a blank rename at the boundary: the server requires a non-empty name,
        // and a folded "" would wipe the embedded `.stalx` name. `None` (unchanged) is
        // fine; only an explicit empty/whitespace `Some` is refused. (The FE also gates
        // this, but the backend must not depend on it.)
        if patch.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
            return Err(local_dto(SpeleoDbError::refused(
                "a project name cannot be empty".to_owned(),
            )));
        }

        // Build the PATCH body from only the changed fields (both-or-neither location
        // pin gated on `coordinates_edited`; see `build_patch_body`).
        let body = build_patch_body(&patch);

        // Nothing actually changed: return the current server truth without a no-op
        // PATCH (avoids an empty-body 400 and a needless write).
        if body.is_empty() {
            return self.speleodb_get_project_metadata();
        }

        let payload = serde_json::Value::Object(body).to_string();
        let row = client::spec::PATCH_PROJECT.category;
        let updated = self
            .speleodb_handle_auth(
                row,
                client::patch_project(
                    &UserMetadataEdit,
                    &endpoint,
                    &token,
                    insecure_http,
                    &link.project_id,
                    &payload,
                ),
            )
            .map_err(|e| dto_for_error(row, &e))?;

        // Fold the (possibly server-normalized) name into the open `.stalx` when it
        // was edited, so the embedded project name tracks SDB without dirtying.
        // [#576 NF-W4C-2] Under `command_lock`, after the PATCH (never across it):
        // the fold rewrites the project rows and the link row of the live file, and an
        // autosave copies the link at its start and renames a fresh file over the
        // live one, so a write landing inside it was lost. Done's fold is already
        // under the lock (`speleodb_upload`).
        if patch.name.is_some() {
            let _commands = self.hold_commands();
            // [#576 RVF-c-C1] The fold is best-effort like every failure of it: a
            // project a switch opened since the PATCH does not take this one's name.
            if self
                .refuse_other_project("Folding the SpeleoDB name")
                .is_ok()
            {
                self.speleodb_fold_local_name(&updated.name);
            }
        }

        Ok(SpeleoProjectMetadata::from_project(&updated))
    }

    /// Fold an SDB-authoritative project name into the OPEN local `.stalx` WITHOUT
    /// setting the dirty bit ([#206]/[#216]): update the in-memory project, persist it
    /// directly (`db.save_project` bypasses the command/undo/dirty path and never
    /// touches the `speleo_link` table), and refresh the link's cached `project_name`.
    /// Best-effort - a failure is logged, never surfaced (the server write already
    /// landed). Locks project → database in that order (mirrors `speleodb_render_tml`).
    ///
    /// [#576 RV2-R-6] Refused - nothing renamed, persisted or re-cached - whenever a
    /// save would refuse to write this project ([`Self::model_write_refusal`]): the
    /// in-place save below writes the WHOLE model, so in the B15 state it put the
    /// in-memory defaults over the stored settings, and it ran before the flush that
    /// would have refused. The server keeps the name; the next fold after a clean
    /// reopen picks it up.
    pub(super) fn speleodb_fold_local_name(&self, name: &str) {
        // Snapshot + update the in-memory name under the project lock, then release it
        // before taking the database lock (project → database order).
        let to_persist = {
            let mut proj_guard = self.project.lock();
            match proj_guard.as_mut() {
                Some(project) => {
                    if let Some(refusal) = self.model_write_refusal(project) {
                        tracing::warn!(
                            reason = ?refusal.error,
                            purpose = "never write a project the save would refuse (#576 RV2-R-6)",
                            "speleodb: not folding the SpeleoDB name into this project"
                        );
                        return;
                    }
                    (project.name != name).then(|| {
                        project.name = name.to_owned();
                        // [#576 NF-W14BE-1] The in-place write below is a
                        // never-saved section's first save too: stamp it on the
                        // model, or the next save would write a second time.
                        super::project_io::stamp_section_creation_times(
                            project,
                            chrono::Utc::now(),
                        );
                        project.clone()
                    })
                }
                None => None,
            }
        };
        if let Some(project) = to_persist {
            {
                let mut db_guard = self.database.lock();
                if let Some(db) = db_guard.as_mut() {
                    if let Err(e) = db.save_project(&project) {
                        tracing::warn!(
                            error = %e,
                            purpose = "fold SDB-authoritative name into local .stalx",
                            "speleodb: failed to persist folded project name"
                        );
                    }
                }
            }
        }
        // Keep the cached link name in step (dirty bit deliberately untouched).
        let _ = self.speleodb_mutate_link(|l| {
            if l.project_name != name {
                l.project_name = name.to_owned();
            }
        });
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
    ) -> Result<crate::core::command::SpeleoPublishReport, SpeleoErrorDto> {
        // [#576 RVF-c-C1] Made for another project than the open one: refused before
        // the create POST, which cannot be taken back (the relocation checks again).
        {
            let _commands = self.hold_commands();
            self.speleodb_refuse_other_project("Publish")?;
        }
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
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
        //  - [#576 B6-D1] a project a newer build saved: publishing saves it into
        //    the cache, and that save is refused - after the create POST.
        if let Some(reason) = self.newer_file_refusal() {
            return Err(local_dto(super::speleodb::newer_file_refused(reason)));
        }
        if params.name.trim().is_empty() {
            return Err(local_dto(SpeleoDbError::refused(
                "a project name is required to publish".to_owned(),
            )));
        }
        // The server's Project model requires a non-blank description (`blank=False`),
        // so a blank one 400s the create. Reject it up front (the FE gates on it too)
        // for a clear message instead of a raw server validation error.
        if params.description.trim().is_empty() {
            return Err(local_dto(SpeleoDbError::refused(
                "a project description is required to publish".to_owned(),
            )));
        }
        if self.speleodb_open_link().is_ok() {
            return Err(local_dto(SpeleoDbError::refused(
                "this project is already linked to SpeleoDB".to_owned(),
            )));
        }
        {
            let proj_guard = self.project.lock();
            let project = proj_guard.as_ref().ok_or_else(|| {
                local_dto(SpeleoDbError::refused(
                    "no project is open to publish".to_owned(),
                ))
            })?;
            if project.has_unresolvable_cross_references() {
                return Err(local_dto(SpeleoDbError::refused(
                    "can't publish: the project has unresolved cross-references (use Repair first)"
                        .to_owned(),
                )));
            }
        }

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

        // 2. + 3. Relocate the open project's working copy into the SpeleoDB cache
        //    and stamp a fresh hand-off link onto it, in one `command_lock` span (see
        //    `speleodb_publish_relocate`). BASE = the create response's latest_commit
        //    (None for a brand-new empty repo); dirty = true (the local content is not
        //    on the server yet). New sanctioned stamp site.
        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        }
        let (link, relocation_lost) = self
            .speleodb_publish_relocate(&dest, |undo_mode| crate::models::SpeleoLink {
                endpoint: endpoint.clone(),
                project_id: project_id.clone(),
                project_name: params.name.clone(),
                base_commit_sha: created.latest_commit.as_ref().map(|c| c.id.clone()),
                has_unpushed_edits: true,
                last_edit_at: Some(chrono::Utc::now().to_rfc3339()),
                last_verify_at: None,
                undo_mode: Some(undo_mode),
            })
            .map_err(local_dto)?;

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
        let mut pre_push_lost = Vec::new();
        let acquired = match self.speleodb_handle_auth(
            Endpoint::Acquire,
            client::acquire_lock(&endpoint, &token, insecure_http, &project_id),
        ) {
            Ok(_) => Ok(()),
            // [#576 FZ-SDB-6-D1] A lost reply: re-read the holder at once. Ours right
            // after our own acquire means the acquire landed, so the claim is
            // recorded and the initial push goes ahead as if the reply had arrived
            // (its release hands the lock back). Anything else stays the failed
            // acquire below.
            Err(e) => match self.speleodb_reread_after_lock_verb(
                &endpoint,
                &token,
                insecure_http,
                &project_id,
                &e,
            ) {
                super::speleodb_lock_release::AfterLockVerb::Ours => {
                    tracing::info!(
                        project_id = %project_id,
                        error = %e,
                        purpose = "follow an acquire whose reply was lost (#576 FZ-SDB-6)",
                        "speleodb_publish: the acquire reply was lost, but the lock is ours"
                    );
                    self.speleodb_note_lock_acquired(Some(&project_id));
                    Ok(())
                }
                // [#576 SDB-POLL-D1] The re-read failed too: the acquire on this
                // just-created project may have landed. The next successful poll
                // settles it (ours: the claim is recorded).
                super::speleodb_lock_release::AfterLockVerb::Unknown => {
                    self.speleodb_mark_acquire_unconfirmed(&project_id);
                    Err(e)
                }
                _ => Err(e),
            },
        };
        match acquired {
            Ok(()) => {
                let upload = self.speleodb_upload(
                    &endpoint,
                    &token,
                    insecure_http,
                    &link,
                    Some("Initial publish from Stalagmight".to_owned()),
                    // Skip the #216 name fold: the project was just created with this
                    // exact name, so the fold GET would be a guaranteed no-op.
                    false,
                    // [#576 NF-W4M-2] The pre-push save's warnings ride the
                    // publish answer, like Done's and Push's (NF-W2SAVE-1).
                    &mut pre_push_lost,
                );
                // Release the lock regardless of the push outcome (best-effort).
                self.speleodb_release_best_effort(&endpoint, &token, insecure_http, &project_id);
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
        // [#576 FZ-SDB-2] The project is now linked and its lock is not held: it comes
        // back read-only like any opened linked project, and Start editing takes the
        // lock. Under the `project` mutex, re-reading the open link, as
        // `speleodb_shut_gate_over_lost_lock` does, so a project opened meanwhile keeps
        // its own gate.
        {
            let _proj_guard = self.project.lock();
            if self
                .speleodb_open_link()
                .is_ok_and(|open| open.project_id == project_id)
            {
                self.set_edit_enabled(false);
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
                    // [#576 NF-W4C-2] Under `command_lock`, like every link write.
                    let _commands = self.hold_commands();
                    // [#576 RVF-c-C1] Best-effort, and never into another project's link.
                    if self
                        .refuse_other_project("Recording the published tip")
                        .is_ok()
                    {
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

        Ok(crate::core::command::SpeleoPublishReport {
            result: SpeleoPublishResult {
                project_id,
                endpoint,
                local_path: dest.to_string_lossy().into_owned(),
            },
            save_warnings: publish_save_losses(relocation_lost, pre_push_lost),
        })
    }

    /// Publish's LOCAL half (steps 2 and 3): relocate the open project into `dest`
    /// and stamp the hand-off link `make_link` builds onto it, both inside ONE
    /// `command_lock` span [#576 NF-W4C-1]. `make_link` gets the undo mode the new
    /// link materializes.
    ///
    /// One span, because an autosave (`execute` -> `SaveProject`) copies the link at
    /// its start, writes a fresh file and renames it over the live one: landing
    /// between the relocation and the stamp, it wrote the cache file without the
    /// link (the stamp went into the file it replaced), or the stamp found the
    /// handle out of the slot. No network call happens inside the span.
    ///
    /// `save_project(Some(dest))` relocates only because the project is still
    /// UNLINKED when it runs - a linked explicit-destination save is a Save a Copy -
    /// so the relocation must come before the stamp.
    ///
    /// Returns the stamped link and the relocation save's warnings (#576 NF-W4M-2).
    pub(super) fn speleodb_publish_relocate(
        &self,
        dest: &std::path::Path,
        make_link: impl FnOnce(crate::models::UndoMode) -> crate::models::SpeleoLink,
    ) -> Result<
        (
            crate::models::SpeleoLink,
            Vec<crate::core::ValidationWarning>,
        ),
        SpeleoDbError,
    > {
        // [#576 A8] The order is the types': the relocation needs the span's proof,
        // and the stamp needs the relocation's - which borrows that same span, so the
        // two cannot be split across two acquisitions (#576 RV8a-1).
        let commands = self.hold_commands();
        // [#576 RVF-c-C1] Never relocate (and link) a project a switch opened since
        // Publish was asked for.
        self.refuse_other_project("Publish")
            .map_err(super::speleodb::refused_by)?;
        let (relocated, relocation_losses) =
            self.speleodb_publish_relocate_into(&commands, dest)?;
        let link = self.speleodb_publish_stamp(relocated, make_link)?;
        Ok((link, relocation_losses))
    }
}

/// [#576 A8] Publish's local half as types: the relocation takes the caller's
/// `command_lock` proof, and the stamp takes the relocation's [`Relocated`] proof,
/// made only here. Stamping first - which would turn the relocation into a Save a
/// Copy, leaving the session on the user's original file - does not compile.
mod publish_order {
    use std::path::Path;

    use super::super::session_commit::CommandsHeld;
    use super::AppState;
    use crate::core::ValidationWarning;
    use crate::speleodb::SpeleoDbError;

    /// Proof that the open project was relocated into the cache while still UNLINKED,
    /// borrowing the `command_lock` span it was relocated under: the stamp takes only
    /// this, so it runs in that span (no autosave between the two).
    pub(super) struct Relocated<'c> {
        _span: &'c CommandsHeld<'c>,
    }

    impl AppState {
        /// 2. Relocate the open project's working copy into the SpeleoDB cache. A
        ///    remote project's canonical file lives at `speleodb_cache_path`
        ///    (open/fast-forward both extract there), and the push machinery reads
        ///    exactly that path. `save_project(Some(dest))` writes a fresh `.stalx`
        ///    there AND switches the active database + `project.file_path` to it.
        pub(super) fn speleodb_publish_relocate_into<'c>(
            &self,
            commands: &'c CommandsHeld<'c>,
            dest: &Path,
        ) -> Result<(Relocated<'c>, Vec<ValidationWarning>), SpeleoDbError> {
            let save = self.save_project(Some(dest.to_path_buf()));
            if !save.success {
                // The save's own refusal, shown as written (as
                // `speleodb_flush_before_push` does): wrapped in `Io` it read
                // "filesystem error: failed to write project into the SpeleoDB cache:
                // <marker>", hiding a coded save refusal (such as the unread-settings
                // marker) behind a disk-error prefix (#576 RV4-R-NITS S-9).
                return Err(super::super::speleodb::refused_by_save(
                    save,
                    "the project could not be written into the SpeleoDB cache",
                ));
            }
            Ok((Relocated { _span: commands }, save.warnings))
        }

        /// 3. Stamp the hand-off link onto the relocated copy, through the live handle
        ///    the relocation switched to.
        pub(super) fn speleodb_publish_stamp(
            &self,
            _relocated: Relocated<'_>,
            make_link: impl FnOnce(crate::models::UndoMode) -> crate::models::SpeleoLink,
        ) -> Result<crate::models::SpeleoLink, SpeleoDbError> {
            let db_guard = self.database.lock();
            let db = db_guard.as_ref().ok_or_else(|| {
                SpeleoDbError::Io("no open database after writing the cache file".to_owned())
            })?;
            // [#303] A new link materializes the application default immediately.
            // Re-publishing a detached managed copy preserves its explicit project value.
            let link = make_link(
                self.effective_undo_mode(super::super::speleodb_cache::preserved_undo_mode(db)),
            );
            db.save_speleo_link(Some(&link))
                .map_err(|e| SpeleoDbError::Io(e.to_string()))?;
            Ok(link)
        }
    }
}

/// [#576 NF-W4M-2] What publish's two saves could not keep, each code once: the
/// relocation's first, then any the pre-push save adds.
pub(super) fn publish_save_losses(
    relocation: Vec<crate::core::ValidationWarning>,
    pre_push: Vec<crate::core::ValidationWarning>,
) -> Vec<crate::core::ValidationWarning> {
    let mut lost = relocation;
    for warning in pre_push {
        if !lost.iter().any(|w| w.code == warning.code) {
            lost.push(warning);
        }
    }
    lost
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{build_create_body, build_patch_body, coord_json, visibility_label};
    use crate::speleodb::{SpeleoProjectMetadataPatch, SpeleoPublishParams, Visibility};

    #[test]
    fn visibility_label_matches_the_wire_contract() {
        // The server's CustomChoiceField returns the UPPERCASE label; we send exactly
        // that so a metadata round-trip is byte-identical (#206/#207).
        assert_eq!(visibility_label(Visibility::Private), Some("PRIVATE"));
        assert_eq!(visibility_label(Visibility::Public), Some("PUBLIC"));
        // [#576 B16] A visibility the server sent that this build cannot name is
        // never sent back: there is no faithful label for it.
        assert_eq!(visibility_label(Visibility::Unknown), None);
    }

    #[test]
    fn coord_json_maps_present_finite_absent_and_non_finite() {
        assert_eq!(coord_json(Some(12.5)), serde_json::json!(12.5));
        assert_eq!(coord_json(None), serde_json::Value::Null);
        // A non-finite value (should never reach here) degrades to null, never a NaN
        // that would serialize invalidly.
        assert_eq!(coord_json(Some(f64::NAN)), serde_json::Value::Null);
    }

    /// A patch with the given fields, all others defaulted to "unchanged".
    fn patch(
        name: Option<&str>,
        visibility: Option<Visibility>,
        coordinates_edited: bool,
        latitude: Option<f64>,
        longitude: Option<f64>,
    ) -> SpeleoProjectMetadataPatch {
        SpeleoProjectMetadataPatch {
            name: name.map(str::to_owned),
            description: None,
            country: None,
            visibility,
            coordinates_edited,
            latitude,
            longitude,
        }
    }

    #[test]
    fn patch_body_sends_only_changed_fields() {
        // Only name + visibility changed; description/country omitted, pin untouched.
        let body = build_patch_body(&patch(
            Some("New Name"),
            Some(Visibility::Public),
            false,
            None,
            None,
        ));
        assert_eq!(body.get("name").and_then(|v| v.as_str()), Some("New Name"));
        assert_eq!(
            body.get("visibility").and_then(|v| v.as_str()),
            Some("PUBLIC")
        );
        assert!(!body.contains_key("description"));
        assert!(!body.contains_key("country"));
        // Pin not edited -> latitude/longitude omitted entirely (server keeps them).
        assert!(!body.contains_key("latitude"));
        assert!(!body.contains_key("longitude"));
    }

    #[test]
    fn patch_body_is_empty_when_nothing_changed() {
        assert!(
            build_patch_body(&patch(None, None, false, None, None)).is_empty(),
            "no-op patch -> empty body -> caller skips the PATCH (no empty-body 400)"
        );
    }

    #[test]
    fn patch_body_sets_and_clears_the_location_pin() {
        // coordinates_edited + both present -> set the pin (two numbers).
        let body = build_patch_body(&patch(None, None, true, Some(1.0), Some(2.0)));
        assert_eq!(body.get("latitude"), Some(&serde_json::json!(1.0)));
        assert_eq!(body.get("longitude"), Some(&serde_json::json!(2.0)));
        // coordinates_edited + both None -> clear the pin (two explicit nulls).
        let body = build_patch_body(&patch(None, None, true, None, None));
        assert_eq!(body.get("latitude"), Some(&serde_json::Value::Null));
        assert_eq!(body.get("longitude"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn create_body_pins_the_repo_layout_contract() {
        // #208: every create fixes type=stalagmight + exclude_geojson=false; visibility label;
        // country as given; lat/long omitted when not both present (#207 both-or-neither).
        let params = SpeleoPublishParams {
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
            Some("stalagmight"),
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
}
