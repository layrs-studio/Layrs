use crate::auth::{AuthError, AuthStore, DevicePoll};

use super::RuntimeConfig;
use super::auth_routes::{auth_error_response, bearer_token};
use super::http::{HttpRequest, HttpResponse};
use super::wire::{
    desktop_bootstrap_json, device_start_json, error_json, escape_json, json_string_field,
    user_json,
};

pub(super) fn device_start(
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    match auth_store.start_device_flow(&config.verification_uri()) {
        Ok(start) => HttpResponse::json(200, "OK", device_start_json(&start)),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn device_poll(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let device_code = json_string_field(&request.body, "device_code")
        .or_else(|| json_string_field(&request.body, "deviceCode"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AuthError::invalid("device_code is required"));

    match device_code {
        Ok(device_code) => match auth_store.poll_device_flow(&device_code) {
            Ok(DevicePoll::AuthorizationPending { interval_seconds }) => HttpResponse::json(
                202,
                "Accepted",
                format!(
                    r#"{{"status":"authorization_pending","interval_seconds":{interval_seconds}}}"#
                ),
            ),
            Ok(DevicePoll::Authorized {
                user,
                desktop_token,
            }) => HttpResponse::json(
                200,
                "OK",
                format!(
                    r#"{{"status":"authorized","user":{},"desktop_token":{{"access_token":"{}","token_type":"{}","expires_in_seconds":{}}}}}"#,
                    user_json(&user),
                    escape_json(&desktop_token.access_token),
                    escape_json(&desktop_token.token_type),
                    desktop_token.expires_in_seconds
                ),
            ),
            Ok(DevicePoll::Expired) => HttpResponse::json(
                400,
                "Bad Request",
                error_json(
                    "expired_device_code",
                    "device code is expired or already consumed",
                ),
            ),
            Err(error) => auth_error_response(error),
        },
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn desktop_bootstrap(
    request: HttpRequest,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    let Some(token) = bearer_token(&request) else {
        return HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", "missing desktop bearer token"),
        );
    };

    match auth_store.desktop_bootstrap(&token, config.database_url_configured()) {
        Ok(Some(bootstrap)) => HttpResponse::json(200, "OK", desktop_bootstrap_json(&bootstrap)),
        Ok(None) => HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", "desktop bearer token is invalid"),
        ),
        Err(error) => auth_error_response(error),
    }
}
