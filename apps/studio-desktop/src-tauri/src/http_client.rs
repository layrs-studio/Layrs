use serde::{de::DeserializeOwned, Serialize};
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub fn get_json<T: DeserializeOwned>(
    endpoint: &str,
    path: &str,
    bearer: Option<&str>,
) -> Result<T, String> {
    let response = request(endpoint, "GET", path, bearer, None, &[])?;
    decode_response(response, path)
}

pub fn get_bytes(endpoint: &str, path: &str, bearer: Option<&str>) -> Result<Vec<u8>, String> {
    let target = HttpTarget::parse(endpoint)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port)).map_err(|error| {
        format!(
            "Layrs Desktop could not reach Layrs server at {}:{}: {error}",
            target.host, target.port
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| format!("Layrs Desktop could not set server read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| format!("Layrs Desktop could not set server write timeout: {error}"))?;

    let path_with_base = format!("{}{}", target.base_path, path);
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request_head = format!(
        "GET {path_with_base} HTTP/1.1\r\nHost: {}:{}\r\nAccept: application/octet-stream\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n",
        target.host, target.port, auth
    );
    stream.write_all(request_head.as_bytes()).map_err(|error| {
        format!("Layrs Desktop could not send request to Layrs server: {error}")
    })?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("Layrs Desktop could not read Layrs server response: {error}"))?;
    let (status, body) = parse_http_response_bytes(&response)?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "Layrs server returned HTTP {} for {path}: {}",
            status,
            String::from_utf8_lossy(&body).trim()
        ));
    }
    Ok(body)
}

pub fn post_json<TBody: Serialize, TResponse: DeserializeOwned>(
    endpoint: &str,
    path: &str,
    bearer: Option<&str>,
    body: &TBody,
) -> Result<TResponse, String> {
    let body = serde_json::to_string(body)
        .map_err(|error| format!("Layrs Desktop could not encode request for {path}: {error}"))?;
    let response = request(
        endpoint,
        "POST",
        path,
        bearer,
        Some("application/json"),
        body.as_bytes(),
    )?;
    decode_response(response, path)
}

pub fn put_bytes_json<TResponse: DeserializeOwned>(
    endpoint: &str,
    path: &str,
    bearer: Option<&str>,
    bytes: &[u8],
) -> Result<TResponse, String> {
    let response = request(
        endpoint,
        "PUT",
        path,
        bearer,
        Some("application/octet-stream"),
        bytes,
    )?;
    decode_response(response, path)
}

pub fn delete_json<T: DeserializeOwned>(
    endpoint: &str,
    path: &str,
    bearer: Option<&str>,
) -> Result<T, String> {
    let response = request(endpoint, "DELETE", path, bearer, None, &[])?;
    decode_response(response, path)
}

fn decode_response<T: DeserializeOwned>(response: HttpResponse, path: &str) -> Result<T, String> {
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Layrs server returned HTTP {} for {path}: {}",
            response.status,
            response.body.trim()
        ));
    }

    serde_json::from_str(&response.body).map_err(|error| {
        format!(
            "Layrs Desktop could not decode JSON response from {path}: {error}. Body: {}",
            response.body.trim()
        )
    })
}

fn request(
    endpoint: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<HttpResponse, String> {
    let target = HttpTarget::parse(endpoint)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port)).map_err(|error| {
        format!(
            "Layrs Desktop could not reach Layrs server at {}:{}: {error}",
            target.host, target.port
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| format!("Layrs Desktop could not set server read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| format!("Layrs Desktop could not set server write timeout: {error}"))?;

    let path = format!("{}{}", target.base_path, path);
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let content_headers = if let Some(content_type) = content_type {
        format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        )
    } else {
        "Content-Length: 0\r\n".to_string()
    };

    let request_head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nAccept: application/json\r\n{}{}Connection: close\r\n\r\n",
        target.host, target.port, auth, content_headers
    );
    stream
        .write_all(request_head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| {
            format!("Layrs Desktop could not send request to Layrs server: {error}")
        })?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("Layrs Desktop could not read Layrs server response: {error}"))?;

    parse_http_response(&response)
}

fn parse_http_response(raw: &str) -> Result<HttpResponse, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "Layrs server returned a malformed HTTP response.".to_string())?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "Layrs server response had no HTTP status.".to_string())?
        .parse::<u16>()
        .map_err(|error| format!("Layrs server response had an invalid HTTP status: {error}"))?;

    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}

fn parse_http_response_bytes(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let Some(split_at) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err("Layrs server returned a malformed HTTP response.".to_string());
    };
    let head = std::str::from_utf8(&raw[..split_at])
        .map_err(|_| "Layrs server returned a malformed HTTP response header.".to_string())?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "Layrs server response had no HTTP status.".to_string())?
        .parse::<u16>()
        .map_err(|error| format!("Layrs server response had an invalid HTTP status: {error}"))?;
    Ok((status, raw[split_at + 4..].to_vec()))
}

#[derive(Clone, Debug)]
struct HttpTarget {
    host: String,
    port: u16,
    base_path: String,
}

impl HttpTarget {
    fn parse(endpoint: &str) -> Result<Self, String> {
        let endpoint = endpoint.trim().trim_end_matches('/');
        let rest = endpoint
            .strip_prefix("http://")
            .ok_or_else(|| "Layrs Desktop currently accepts only http:// server endpoints for local development.".to_string())?;
        let (host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) => {
                let port = port.parse::<u16>().map_err(|error| {
                    format!("Layrs Desktop server endpoint has an invalid port: {error}")
                })?;
                (host.to_string(), port)
            }
            None => (host_port.to_string(), 80),
        };

        if host.trim().is_empty() {
            return Err("Layrs Desktop server endpoint is missing a host.".to_string());
        }

        Ok(Self {
            host,
            port,
            base_path: if base_path.is_empty() {
                String::new()
            } else {
                format!("/{base_path}")
            },
        })
    }
}
