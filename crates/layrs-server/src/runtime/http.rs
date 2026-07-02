use crate::auth::AuthStore;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::RuntimeConfig;
use super::routes::route_request;
use super::wire::error_json;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn json(status: u16, reason: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

pub fn handle_connection(
    stream: &mut TcpStream,
    config: &RuntimeConfig,
    auth_store: &mut impl AuthStore,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let request = read_request(stream)?;
    let response = match request {
        Some(request) => route_request(request, config, auth_store),
        None => HttpResponse::json(
            400,
            "Bad Request",
            error_json("bad_request", "empty request"),
        ),
    };

    write_response(stream, response)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<HttpRequest>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut headers_end = None;

    while headers_end.is_none() && buffer.len() < 64 * 1024 {
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
        headers_end = find_header_end(&buffer);
    }

    let Some(headers_end) = headers_end else {
        return Ok(None);
    };
    let head = String::from_utf8_lossy(&buffer[..headers_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let Some(method) = request_parts.next() else {
        return Ok(None);
    };
    let Some(target) = request_parts.next() else {
        return Ok(None);
    };
    let path = target.split('?').next().unwrap_or(target).to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = headers_end + 4;

    while buffer.len().saturating_sub(body_start) < content_length {
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
    }

    let available_body = buffer.len().saturating_sub(body_start).min(content_length);
    let body =
        String::from_utf8_lossy(&buffer[body_start..body_start + available_body]).to_string();

    Ok(Some(HttpRequest {
        method: method.to_string(),
        path,
        headers,
        body,
    }))
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> std::io::Result<()> {
    let mut headers = response.headers;
    headers.push((
        "Content-Type".to_string(),
        response.content_type.to_string(),
    ));
    headers.push((
        "Content-Length".to_string(),
        response.body.len().to_string(),
    ));
    headers.push(("Connection".to_string(), "close".to_string()));

    write!(
        stream,
        "HTTP/1.1 {} {}\r\n",
        response.status, response.reason
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n{}", response.body)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(super) fn apply_cors(response: &mut HttpResponse, origin: &str) {
    response.headers.push((
        "Access-Control-Allow-Origin".to_string(),
        origin.to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Headers".to_string(),
        "content-type, authorization".to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Methods".to_string(),
        "GET, POST, PUT, DELETE, OPTIONS".to_string(),
    ));
    response.headers.push((
        "Access-Control-Allow-Credentials".to_string(),
        "true".to_string(),
    ));
}
