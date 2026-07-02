use crate::auth::{
    AuthError, AuthErrorCode, AuthSession, AuthStore, UserPrincipal, session_cookie_name,
};

use super::http::{HttpRequest, HttpResponse};
use super::wire::{
    audit_event_json, auth_session_json, device_json, error_json, expired_session_cookie,
    json_string_field, me_json, required_json_field, session_cookie_header, session_json, slugify,
    studio_snapshot_json, workspace_json, workspace_json_with,
};

pub(super) fn signup(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let email = required_json_field(&request.body, "email");
    let password = required_json_field(&request.body, "password");
    let display_name = json_string_field(&request.body, "display_name")
        .or_else(|| json_string_field(&request.body, "name"));

    match (email, password) {
        (Ok(email), Ok(password)) => {
            match auth_store.signup(&email, &password, display_name.as_deref()) {
                Ok(session) => session_response(201, "Created", &session),
                Err(error) => auth_error_response(error),
            }
        }
        (Err(error), _) | (_, Err(error)) => auth_error_response(error),
    }
}

pub(super) fn login(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let email = required_json_field(&request.body, "email");
    let password = required_json_field(&request.body, "password");

    match (email, password) {
        (Ok(email), Ok(password)) => match auth_store.login(&email, &password) {
            Ok(session) => session_response(200, "OK", &session),
            Err(error) => auth_error_response(error),
        },
        (Err(error), _) | (_, Err(error)) => auth_error_response(error),
    }
}

pub(super) fn logout(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    let Some(token) = session_cookie(&request) else {
        return unauthorized();
    };

    match auth_store.logout(&token) {
        Ok(()) => HttpResponse::json(200, "OK", r#"{"ok":true}"#)
            .with_header("Set-Cookie", expired_session_cookie()),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn me(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", me_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn auth_session(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", auth_session_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn studio_snapshot(
    request: HttpRequest,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(200, "OK", studio_snapshot_json(&user)),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn list_workspaces(
    request: HttpRequest,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    match authenticated_user_from_session_or_bearer(&request, auth_store) {
        Ok(Some(_)) => {
            HttpResponse::json(200, "OK", format!(r#"{{"items":[{}]}}"#, workspace_json()))
        }
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn create_workspace(
    request: HttpRequest,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(_)) => {
            let name = json_string_field(&request.body, "name")
                .unwrap_or_else(|| "Layrs Workspace".to_string());
            let slug = json_string_field(&request.body, "slug").unwrap_or_else(|| slugify(&name));
            let description = json_string_field(&request.body, "description")
                .unwrap_or_else(|| "Server-backed Layrs workspace.".to_string());
            HttpResponse::json(
                201,
                "Created",
                workspace_json_with(&name, &slug, &description),
            )
        }
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn list_devices(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"items":[{}]}}"#, device_json(&user)),
        ),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn audit_events(request: HttpRequest, auth_store: &mut impl AuthStore) -> HttpResponse {
    match authenticated_session_user(&request, auth_store) {
        Ok(Some(user)) => HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"items":[{}]}}"#, audit_event_json(&user)),
        ),
        Ok(None) => unauthorized(),
        Err(error) => auth_error_response(error),
    }
}

pub(super) fn session_response(
    status: u16,
    reason: &'static str,
    session: &AuthSession,
) -> HttpResponse {
    HttpResponse::json(status, reason, session_json(session))
        .with_header("Set-Cookie", session_cookie_header(&session.session_token))
}

pub(super) fn unauthorized() -> HttpResponse {
    HttpResponse::json(
        401,
        "Unauthorized",
        error_json("unauthorized", "authentication is required"),
    )
}

pub(super) fn auth_error_response(error: AuthError) -> HttpResponse {
    match error.code {
        AuthErrorCode::InvalidRequest => HttpResponse::json(
            400,
            "Bad Request",
            error_json("invalid_request", &error.message),
        ),
        AuthErrorCode::Unauthorized => HttpResponse::json(
            401,
            "Unauthorized",
            error_json("unauthorized", &error.message),
        ),
        AuthErrorCode::Conflict => {
            HttpResponse::json(409, "Conflict", error_json("conflict", &error.message))
        }
        AuthErrorCode::StoreUnavailable => HttpResponse::json(
            503,
            "Service Unavailable",
            error_json("store_unavailable", &error.message),
        ),
    }
}

pub(super) fn session_cookie(request: &HttpRequest) -> Option<String> {
    let cookies = request.headers.get("cookie")?;
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == session_cookie_name()).then(|| value.to_string())
    })
}

pub(super) fn bearer_token(request: &HttpRequest) -> Option<String> {
    let value = request.headers.get("authorization")?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

pub(super) fn authenticated_session_user(
    request: &HttpRequest,
    auth_store: &mut impl AuthStore,
) -> Result<Option<UserPrincipal>, AuthError> {
    let Some(token) = session_cookie(request) else {
        return Ok(None);
    };

    auth_store.session_user(&token)
}

pub(super) fn authenticated_user_from_session_or_bearer(
    request: &HttpRequest,
    auth_store: &mut impl AuthStore,
) -> Result<Option<UserPrincipal>, AuthError> {
    if let Some(user) = authenticated_session_user(request, auth_store)? {
        return Ok(Some(user));
    }

    let Some(token) = bearer_token(request) else {
        return Ok(None);
    };

    auth_store
        .desktop_bootstrap(&token, false)
        .map(|bootstrap| bootstrap.map(|value| value.user))
}
