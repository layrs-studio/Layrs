use std::fmt;

pub type ApiResult<T> = Result<T, ValidationError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl ValidationError {
    pub const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

pub trait Validate {
    fn validate(&self) -> ApiResult<()>;
}

pub fn required(field: &'static str, value: &str) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(ValidationError::new(field, "must not be empty"));
    }
    Ok(())
}

pub fn optional_required(field: &'static str, value: &Option<String>) -> ApiResult<()> {
    if let Some(value) = value {
        required(field, value)?;
    }
    Ok(())
}

pub fn bounded_len(field: &'static str, value: &str, min: usize, max: usize) -> ApiResult<()> {
    required(field, value)?;
    if value.len() < min {
        return Err(ValidationError::new(field, "is shorter than expected"));
    }
    if value.len() > max {
        return Err(ValidationError::new(field, "is longer than expected"));
    }
    Ok(())
}

pub fn slug(field: &'static str, value: &str) -> ApiResult<()> {
    bounded_len(field, value, 2, 96)?;
    let valid = value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid || value.starts_with('-') || value.ends_with('-') {
        return Err(ValidationError::new(field, "must be lowercase kebab-case"));
    }
    Ok(())
}

pub fn max_items<T>(field: &'static str, values: &[T], max: usize) -> ApiResult<()> {
    if values.len() > max {
        return Err(ValidationError::new(field, "contains too many items"));
    }
    Ok(())
}

pub fn optional_positive(field: &'static str, value: Option<u32>) -> ApiResult<()> {
    if matches!(value, Some(0)) {
        return Err(ValidationError::new(field, "must be greater than zero"));
    }
    Ok(())
}
