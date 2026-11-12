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
            message: with_hint(message.into()),
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

/// A registered pack of the Store cannot be read (council s17, s18 r1):
/// what the folder still has can be stored again.
fn with_hint(message: String) -> String {
    let unreadable_pack = message.contains("corrupt CAS pack")
        && (message.contains("registered pack is missing")
            || message.contains("unreadable")
            || message.contains("checksum"));
    if unreadable_pack && !message.contains("layrs step --full") {
        format!(
            "{message}\nA pack of this Store cannot be read: `layrs step --full` stores again what this folder still holds."
        )
    } else {
        message
    }
}
