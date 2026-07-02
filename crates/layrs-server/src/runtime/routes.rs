use crate::auth::AuthStore;
use crate::{HttpMethod, ROUTES, RouteDescriptor};

use super::RuntimeConfig;
use super::auth_routes::{
    audit_events, auth_session, create_workspace, list_devices, list_workspaces, login, logout, me,
    signup, studio_snapshot,
};
use super::device_routes::{desktop_bootstrap, device_poll, device_start};
use super::html::server_page;
use super::http::{HttpRequest, HttpResponse, apply_cors};
use super::wire::{auth_label, error_json, escape_json, health_json, method_label};

pub fn route_request(
    request: HttpRequest,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> HttpResponse {
    let method = request.method.as_str();
    let path = request.path.as_str();
    let cors_origin = request
        .headers
        .get("origin")
        .cloned()
        .unwrap_or_else(|| config.studio_url.clone());

    let mut response = match (method, path) {
        ("OPTIONS", _) => HttpResponse {
            status: 204,
            reason: "No Content",
            content_type: "text/plain; charset=utf-8",
            headers: vec![("Access-Control-Max-Age".to_string(), "600".to_string())],
            body: String::new(),
        },
        ("GET", "/") | ("GET", "/server") => HttpResponse::html(server_page(config)),
        ("GET", "/healthz") => HttpResponse::json(200, "OK", health_json(config)),
        ("GET", "/v1/routes") => HttpResponse::json(200, "OK", routes_json()),
        ("POST", "/v1/auth/signup") => signup(request, auth_store),
        ("POST", "/v1/auth/login") => login(request, auth_store),
        ("POST", "/v1/auth/logout") => logout(request, auth_store),
        ("GET", "/v1/auth/session") => auth_session(request, auth_store),
        ("GET", "/v1/me") => me(request, auth_store),
        ("GET", "/v1/lenses") => {
            HttpResponse::json(200, "OK", crate::lenses::registry_response_json_from_env())
        }
        ("GET", "/v1/studio/snapshot") => studio_snapshot(request, auth_store),
        ("GET", "/v1/workspaces") => list_workspaces(request, auth_store),
        ("POST", "/v1/workspaces") => create_workspace(request, auth_store),
        ("GET", "/v1/devices") => list_devices(request, auth_store),
        ("POST", "/v1/desktop/device/start") => device_start(config, auth_store),
        ("POST", "/v1/desktop/device/poll") => device_poll(request, auth_store),
        ("GET", "/v1/desktop/bootstrap") => desktop_bootstrap(request, config, auth_store),
        _ if method == "GET" && is_workspace_audit_events_path(path) => {
            audit_events(request, auth_store)
        }
        _ if route_exists(method, path) => HttpResponse::json(
            501,
            "Not Implemented",
            error_json(
                "registered_route_not_implemented",
                "route is registered but no handler is wired yet",
            ),
        ),
        _ => HttpResponse::json(404, "Not Found", error_json("not_found", "route not found")),
    };

    apply_cors(&mut response, &cors_origin);
    response
}

fn is_workspace_audit_events_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 5 && parts[1] == "v1" && parts[2] == "workspaces" && parts[4] == "audit-events"
}

fn route_exists(method: &str, path: &str) -> bool {
    let method = match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        _ => return false,
    };

    ROUTES
        .iter()
        .any(|route| route.method == method && route.path == path)
}

pub fn routes_json() -> String {
    let mut json = String::from("[");

    for (index, route) in ROUTES.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&route_json(route));
    }

    json.push(']');
    json
}

fn route_json(route: &RouteDescriptor) -> String {
    format!(
        r#"{{"method":"{}","path":"{}","name":"{}","auth":"{}"}}"#,
        method_label(route.method),
        escape_json(route.path),
        escape_json(route.name),
        auth_label(route.auth)
    )
}
