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

use super::speleodb::{local_dto, validate_project_id, MANAGED_PROJECT_TYPE};
use super::speleodb_push::PushOutcome;
use super::AppState;
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
        let _ = (&name, &project_id, &expected_generation);
        crate::core::stub::not_ported("speleodb_project::speleodb_fold_local_name")
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
        let _ = (&params,);
        crate::core::stub::not_ported("speleodb_project::speleodb_publish")
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
