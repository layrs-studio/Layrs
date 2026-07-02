use super::wire::json_string_field;
use super::{HttpRequest, RuntimeConfig, route_request, routes_json};
use crate::auth::DevAuthStore;
use std::collections::BTreeMap;

fn config() -> RuntimeConfig {
    RuntimeConfig {
        addr: "127.0.0.1:8787".to_string(),
        studio_url: "http://127.0.0.1:5173".to_string(),
        database_url: None,
        deployment_id: "test".to_string(),
    }
}

fn request(method: &str, path: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        headers: BTreeMap::new(),
        body: body.to_string(),
    }
}

#[test]
fn route_registry_includes_runtime_auth_routes() {
    let json = routes_json();

    assert!(json.contains(r#""path":"/healthz""#));
    assert!(json.contains(r#""path":"/v1/auth/signup""#));
    assert!(json.contains(r#""path":"/v1/desktop/bootstrap""#));
}

#[test]
fn lenses_route_returns_registry_payload() {
    let mut store = DevAuthStore::new();
    let response = route_request(request("GET", "/v1/lenses", ""), &config(), &mut store);

    assert_eq!(response.status, 200);
    assert!(response.body.contains(r#""items""#));
    assert!(response.body.contains(r#""layrs.code""#));
    assert!(response.body.contains(r#""errors""#));
}

#[test]
fn signup_sets_http_only_cookie_and_me_reads_it() {
    let mut store = DevAuthStore::new();
    let signup = route_request(
        request(
            "POST",
            "/v1/auth/signup",
            r#"{"email":"alice@example.com","password":"correct horse","display_name":"Alice"}"#,
        ),
        &config(),
        &mut store,
    );

    assert_eq!(signup.status, 201);
    let cookie = signup
        .headers
        .iter()
        .find(|(name, _)| name == "Set-Cookie")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert!(cookie.contains("HttpOnly"));

    let mut me = request("GET", "/v1/me", "");
    me.headers.insert(
        "cookie".to_string(),
        cookie.split(';').next().unwrap().to_string(),
    );
    let response = route_request(me, &config(), &mut store);

    assert_eq!(response.status, 200);
    assert!(response.body.contains(r#""email":"alice@example.com""#));
}

#[test]
fn auth_session_supports_studio_cookie_cors() {
    let mut store = DevAuthStore::new();
    let signup = route_request(
        request(
            "POST",
            "/v1/auth/signup",
            r#"{"email":"alice@example.com","password":"correct horse","name":"Alice"}"#,
        ),
        &config(),
        &mut store,
    );
    let cookie = signup
        .headers
        .iter()
        .find(|(name, _)| name == "Set-Cookie")
        .map(|(_, value)| value.clone())
        .unwrap();

    let mut session = request("GET", "/v1/auth/session", "");
    session
        .headers
        .insert("origin".to_string(), "http://127.0.0.1:5173".to_string());
    session.headers.insert(
        "cookie".to_string(),
        cookie.split(';').next().unwrap().to_string(),
    );
    let response = route_request(session, &config(), &mut store);

    assert_eq!(response.status, 200);
    assert!(response.body.contains(r#""state":"authenticated""#));
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| { name == "Access-Control-Allow-Credentials" && value == "true" })
    );
    assert!(response.headers.iter().any(|(name, value)| {
        name == "Access-Control-Allow-Origin" && value == "http://127.0.0.1:5173"
    }));
}

#[test]
fn device_flow_bootstraps_with_bearer_token() {
    let mut store = DevAuthStore::new();
    let start = route_request(
        request("POST", "/v1/desktop/device/start", ""),
        &config(),
        &mut store,
    );
    let device_code = json_string_field(&start.body, "device_code").unwrap();

    let pending = route_request(
        request(
            "POST",
            "/v1/desktop/device/poll",
            &format!(r#"{{"device_code":"{}"}}"#, device_code),
        ),
        &config(),
        &mut store,
    );
    assert_eq!(pending.status, 202);

    let authorized = route_request(
        request(
            "POST",
            "/v1/desktop/device/poll",
            &format!(r#"{{"device_code":"{}"}}"#, device_code),
        ),
        &config(),
        &mut store,
    );
    let token = json_string_field(&authorized.body, "access_token").unwrap();
    let mut bootstrap = request("GET", "/v1/desktop/bootstrap", "");
    bootstrap
        .headers
        .insert("authorization".to_string(), format!("Bearer {token}"));

    let response = route_request(bootstrap, &config(), &mut store);

    assert_eq!(response.status, 200);
    assert!(response.body.contains(r#""mode":"dev-memory""#));
}

#[test]
fn device_poll_accepts_desktop_camel_case_payload() {
    let mut store = DevAuthStore::new();
    let start = route_request(
        request("POST", "/v1/desktop/device/start", ""),
        &config(),
        &mut store,
    );
    let device_code = json_string_field(&start.body, "device_code").unwrap();

    let pending = route_request(
        request(
            "POST",
            "/v1/desktop/device/poll",
            &format!(r#"{{"deviceCode":"{}"}}"#, device_code),
        ),
        &config(),
        &mut store,
    );

    assert_eq!(pending.status, 202);
    assert!(pending.body.contains("authorization_pending"));
}
