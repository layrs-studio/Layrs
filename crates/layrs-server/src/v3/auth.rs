use axum::http::{HeaderMap, header::AUTHORIZATION};

use super::error::V3HttpError;

const MAX_AUTHORIZATION_BYTES: usize = 4096;

pub fn bearer_token(headers: &HeaderMap) -> Result<&str, V3HttpError> {
    authorization_bearer(headers, V3HttpError::unauthorized())
}

pub fn capability_token(headers: &HeaderMap) -> Result<&str, V3HttpError> {
    authorization_bearer(headers, V3HttpError::invalid_capability())
}

fn authorization_bearer(headers: &HeaderMap, failure: V3HttpError) -> Result<&str, V3HttpError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or_else(|| failure.clone())?;
    if values.next().is_some() {
        return Err(failure);
    }
    let value = value.to_str().map_err(|_| failure.clone())?;
    if value.len() > MAX_AUTHORIZATION_BYTES {
        return Err(failure);
    }
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| failure.clone())?;
    if token.is_empty()
        || token.trim() != token
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(failure);
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_is_exact_and_fail_closed() {
        let mut headers = HeaderMap::new();
        assert!(bearer_token(&headers).is_err());
        headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer secret"));
        assert!(bearer_token(&headers).is_err());
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        assert_eq!(bearer_token(&headers), Ok("secret"));
    }
}
