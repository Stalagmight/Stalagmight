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
fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "PRIVATE",
        Visibility::Public => "PUBLIC",
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
    if let Some(visibility) = patch.visibility {
        body.insert(
            "visibility".to_owned(),
            serde_json::Value::from(visibility_label(visibility)),
        );
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
    /// Set the open project's server-side `exclude_geojson` flag (#200
    /// Enable-rendering): `PATCH /projects/{id}/`. The server enforces permission
    /// (READ_AND_WRITE) and NO lock is required - the edit mutex governs only the
    /// survey `.stalx`, not project metadata. Setting it `false` re-enables
    /// web-viewer GeoJSON generation, which takes effect only on the NEXT push
    /// (the server has no retroactive re-render for the current tip; the FE
    /// confirmation states this). A 403 surfaces as an error the button reports.
    pub fn speleodb_set_exclude_geojson(&self, exclude: bool) -> Result<(), SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let payload = serde_json::json!({ "exclude_geojson": exclude }).to_string();
        // The PATCH reuses the GetProject endpoint mapping (there is no dedicated
        // PatchProject variant): both are project-scoped calls whose only actionable
        // failure here is a permission/transport error the button surfaces verbatim,
        // so the humanized-error attribution is equivalent. Purely cosmetic reuse.
        self.speleodb_handle_auth(
            Endpoint::GetProject,
            client::patch_project(&endpoint, &token, insecure_http, &link.project_id, &payload),
        )
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
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
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Reject a blank rename at the boundary: the server requires a non-empty name,
        // and a folded "" would wipe the embedded `.stalx` name. `None` (unchanged) is
        // fine; only an explicit empty/whitespace `Some` is refused. (The FE also gates
        // this, but the backend must not depend on it.)
        if patch.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
            return Err(local_dto(SpeleoDbError::Io(
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
        let updated = self
            .speleodb_handle_auth(
                Endpoint::GetProject,
                client::patch_project(&endpoint, &token, insecure_http, &link.project_id, &payload),
            )
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        // Fold the (possibly server-normalized) name into the open `.stalx` when it
        // was edited, so the embedded project name tracks SDB without dirtying.
        if patch.name.is_some() {
            self.speleodb_fold_local_name(&updated.name);
        }

        Ok(SpeleoProjectMetadata::from_project(&updated))
    }

    /// Fold an SDB-authoritative project name into the OPEN local `.stalx` WITHOUT
    /// setting the dirty bit ([#206]/[#216]): update the in-memory project, persist it
    /// directly (`db.save_project` bypasses the command/undo/dirty path and never
    /// touches the `speleo_link` table), and refresh the link's cached `project_name`.
    /// Best-effort - a failure is logged, never surfaced (the server write already
    /// landed). Locks project → database in that order (mirrors `speleodb_render_tml`).
    pub(super) fn speleodb_fold_local_name(&self, name: &str) {
        // Snapshot + update the in-memory name under the project lock, then release it
        // before taking the database lock (project → database order).
        let to_persist = match self.project.lock() {
            Ok(mut proj_guard) => match proj_guard.as_mut() {
                Some(project) if project.name != name => {
                    project.name = name.to_owned();
                    Some(project.clone())
                }
                _ => None,
            },
            Err(_) => None,
        };
        if let Some(project) = to_persist {
            if let Ok(mut db_guard) = self.database.lock() {
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
    ) -> Result<SpeleoPublishResult, SpeleoErrorDto> {
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
        {
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
        let save = self.save_project(Some(dest.clone()));
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
                    Some("Initial publish from Stalagmight".to_owned()),
                    // Skip the #216 name fold: the project was just created with this
                    // exact name, so the fold GET would be a guaranteed no-op.
                    false,
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
                    let _ = self.speleodb_mutate_link(|l| l.base_commit_sha = tip.clone());
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
        })
    }
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
        assert_eq!(visibility_label(Visibility::Private), "PRIVATE");
        assert_eq!(visibility_label(Visibility::Public), "PUBLIC");
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
            .find("self.save_project(Some(")
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
