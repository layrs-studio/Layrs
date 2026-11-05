//! Authenticated Lens catalog, immutable packet download and per-Space pins.
use super::*;
use axum::http::header::CACHE_CONTROL;
use layrs_lens_sdk::LensPin;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRequest {
    #[serde(flatten)]
    packet: LensPacket,
    approve_unsigned: bool,
}

fn no_store<T>(value: T) -> impl IntoResponse
where
    T: serde::Serialize,
{
    ([(CACHE_CONTROL, "no-store")], Json(value))
}

pub async fn runtime_host() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline' data:; worker-src data:; connect-src 'none'; img-src data:; style-src 'unsafe-inline'; frame-ancestors 'self' tauri: http://tauri.localhost; base-uri 'none'; form-action 'none'",
            ),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        axum::response::Html(include_str!("../../../../deploy/lens-runtime.html")),
    )
}

fn pin(id: String, version: String, digest: String) -> Result<LensPin, Error> {
    LensPin::try_new(id, version, digest)
        .map_err(|_| Error(V3StoreError::InvalidDescriptor("invalid_lens_pin".into())))
}

pub async fn catalog(
    State(s): State<CollaborationHttpState>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        serde_json::json!({"items":s.store.lens_catalog(&s.principal(&h).await?, false).await?,"errors":[]}),
    ))
}

pub async fn admin_catalog(
    State(s): State<CollaborationHttpState>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        serde_json::json!({"items":s.store.lens_catalog(&s.principal(&h).await?, true).await?,"errors":[]}),
    ))
}

pub async fn install(
    State(s): State<CollaborationHttpState>,
    h: HeaderMap,
    Json(request): Json<InstallRequest>,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .install_lens(
                &s.principal(&h).await?,
                request.packet,
                request.approve_unsigned,
                &s.trusted_lens_keys,
            )
            .await?,
    ))
}

pub async fn retire(
    State(s): State<CollaborationHttpState>,
    Path((id, version, digest)): Path<(String, String, String)>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .change_lens_state(
                &s.principal(&h).await?,
                &pin(id, version, digest)?,
                "retired",
            )
            .await?,
    ))
}

pub async fn revoke(
    State(s): State<CollaborationHttpState>,
    Path((id, version, digest)): Path<(String, String, String)>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .change_lens_state(
                &s.principal(&h).await?,
                &pin(id, version, digest)?,
                "revoked",
            )
            .await?,
    ))
}

pub async fn space_config(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .space_lenses(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}

pub async fn configure_space(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<LensConfigRequest>,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .configure_space_lenses(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}

pub async fn artifact(
    State(s): State<CollaborationHttpState>,
    Path((space_id, id, version, digest)): Path<(String, String, String, String)>,
    h: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    Ok(no_store(
        s.store
            .fetch_lens_packet(
                &s.principal(&h).await?,
                &space(space_id)?,
                &pin(id, version, digest)?,
            )
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_body_is_flat_packet_with_explicit_unsigned_approval() {
        let packet: LensPacket = serde_json::from_str(include_str!(
            "../../../../deploy/lenses/example-markdown.layrslens.json"
        ))
        .unwrap();
        let mut body = serde_json::to_value(packet).unwrap();
        body["approveUnsigned"] = serde_json::json!(true);
        let parsed: InstallRequest = serde_json::from_value(body).unwrap();
        assert!(parsed.approve_unsigned);
        assert_eq!(parsed.packet.manifest.id, "example.markdown");
    }
}
