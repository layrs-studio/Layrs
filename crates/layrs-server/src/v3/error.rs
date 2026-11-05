use axum::{
    Json,
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_RANGE, WWW_AUTHENTICATE},
    },
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::state::V3ServiceError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct V3HttpError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    total_encoded_len: Option<u64>,
}

impl V3HttpError {
    pub const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            total_encoded_len: None,
        }
    }

    pub const fn invalid_request(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, "request is invalid")
    }

    pub const fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication is required",
        )
    }

    pub const fn invalid_capability() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_capability",
            "transfer capability is invalid",
        )
    }

    pub const fn payload_too_large(_limit: u64) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "request body exceeds its limit",
        )
    }

    pub const fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "request could not be completed",
        )
    }

    pub const fn unsupported_media_type() -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "content type must be application/json",
        )
    }

    pub const fn length_required() -> Self {
        Self::new(
            StatusCode::LENGTH_REQUIRED,
            "content_length_required",
            "content length is required",
        )
    }

    pub const fn range_not_satisfiable(total_encoded_len: u64) -> Self {
        Self {
            status: StatusCode::RANGE_NOT_SATISFIABLE,
            code: "range_not_satisfiable",
            message: "requested byte range is not satisfiable",
            total_encoded_len: Some(total_encoded_len),
        }
    }

    pub const fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "route not found")
    }

    pub const fn method_not_allowed() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "method not allowed",
        )
    }
}

impl From<V3ServiceError> for V3HttpError {
    fn from(error: V3ServiceError) -> Self {
        match error {
            V3ServiceError::InvalidRequest { code } => Self::invalid_request(code),
            V3ServiceError::Unauthorized => Self::unauthorized(),
            V3ServiceError::Forbidden => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "request is not authorized",
            ),
            V3ServiceError::AuthorizationEpochChanged => Self::new(
                StatusCode::CONFLICT,
                "authorization_epoch_changed",
                "authorization epoch changed; bootstrap is required",
            ),
            V3ServiceError::IdempotencyDigestMismatch => Self::new(
                StatusCode::CONFLICT,
                "idempotency_digest_mismatch",
                "idempotency key was already used for another request",
            ),
            V3ServiceError::Conflict { code } => Self::new(
                StatusCode::CONFLICT,
                code,
                "request conflicts with current state",
            ),
            V3ServiceError::RangeNotSatisfiable { total_encoded_len } => {
                Self::range_not_satisfiable(total_encoded_len)
            }
            V3ServiceError::TransferUnavailable => Self::new(
                StatusCode::NOT_FOUND,
                "transfer_unavailable",
                "transfer is unavailable",
            ),
            V3ServiceError::PayloadTooLarge { limit } => Self::payload_too_large(limit),
            V3ServiceError::LengthMismatch => Self::new(
                StatusCode::BAD_REQUEST,
                "length_mismatch",
                "request body length does not match the authorized length",
            ),
            V3ServiceError::BodyReadFailure => Self::new(
                StatusCode::BAD_REQUEST,
                "body_read_error",
                "request body could not be read",
            ),
            V3ServiceError::Unavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "service is unavailable",
            ),
            V3ServiceError::Internal => Self::internal(),
        }
    }
}

impl IntoResponse for V3HttpError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message
                }
            })),
        )
            .into_response();
        if let Some(total) = self.total_encoded_len {
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total}")) {
                response.headers_mut().insert(CONTENT_RANGE, value);
            }
        }
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}
