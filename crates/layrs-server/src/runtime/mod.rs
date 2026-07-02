mod auth_routes;
mod device_routes;
mod html;
mod http;
mod routes;
mod wire;

pub use http::{HttpRequest, HttpResponse, handle_connection};
pub use routes::{route_request, routes_json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub addr: String,
    pub studio_url: String,
    pub database_url: Option<String>,
    pub deployment_id: String,
}

impl RuntimeConfig {
    pub fn database_url_configured(&self) -> bool {
        self.database_url
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn verification_uri(&self) -> String {
        format!("http://{}/v1/desktop/device", self.addr)
    }
}

#[cfg(test)]
mod tests;
