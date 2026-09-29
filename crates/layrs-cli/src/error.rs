use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Runtime = 1,
    Usage = 2,
    AuthRequired = 3,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub exit_code: ExitCode,
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::Usage,
            message: message.into(),
        }
    }
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::Runtime,
        }
    }

    pub fn auth_required(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::AuthRequired,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        CliError::runtime(format!("Layrs CLI I/O failed: {error}"))
    }
}
