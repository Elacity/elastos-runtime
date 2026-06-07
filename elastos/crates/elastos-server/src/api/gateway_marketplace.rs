use super::*;

pub(super) async fn marketplace_catalog(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_capsule_catalog_token(&state.data_dir, &headers) {
        Ok(_) => Json(capsule_catalog_summary(&state.data_dir)).into_response(),
        Err(err) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}
