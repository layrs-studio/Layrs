use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

const SESSION_COOKIE_NAME: &str = "layrs_session";
const DEFAULT_DEVICE_POLL_INTERVAL_SECONDS: u64 = 2;
const DEFAULT_DEVICE_EXPIRES_IN_SECONDS: u64 = 600;

pub fn session_cookie_name() -> &'static str {
    SESSION_COOKIE_NAME
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserPrincipal {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthSession {
    pub user: UserPrincipal,
    pub session_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: u64,
    pub expires_in_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DevicePoll {
    AuthorizationPending {
        interval_seconds: u64,
    },
    Authorized {
        user: UserPrincipal,
        desktop_token: DeviceToken,
    },
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopBootstrap {
    pub user: UserPrincipal,
    pub server_mode: String,
    pub database_url_configured: bool,
    pub routes_path: String,
}

pub type AuthResult<T> = Result<T, AuthError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthError {
    pub code: AuthErrorCode,
    pub message: String,
}

impl AuthError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: AuthErrorCode::InvalidRequest,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: AuthErrorCode::Conflict,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            code: AuthErrorCode::Unauthorized,
            message: message.into(),
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AuthError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AuthErrorCode {
    InvalidRequest,
    Unauthorized,
    Conflict,
    StoreUnavailable,
}

pub trait AuthStore {
    fn signup(
        &mut self,
        email: &str,
        password: &str,
        display_name: Option<&str>,
    ) -> AuthResult<AuthSession>;
    fn login(&mut self, email: &str, password: &str) -> AuthResult<AuthSession>;
    fn logout(&mut self, session_token: &str) -> AuthResult<()>;
    fn session_user(&self, session_token: &str) -> AuthResult<Option<UserPrincipal>>;
    fn start_device_flow(&mut self, verification_uri: &str) -> AuthResult<DeviceStart>;
    fn poll_device_flow(&mut self, device_code: &str) -> AuthResult<DevicePoll>;
    fn desktop_bootstrap(
        &self,
        desktop_token: &str,
        database_url_configured: bool,
    ) -> AuthResult<Option<DesktopBootstrap>>;
}

#[derive(Clone, Debug)]
pub struct DevAuthStore {
    users_by_email: BTreeMap<String, String>,
    users_by_id: BTreeMap<String, UserRecord>,
    sessions_by_hash: BTreeMap<String, String>,
    device_flows: BTreeMap<String, DeviceFlowRecord>,
    desktop_tokens_by_hash: BTreeMap<String, String>,
    secret_pepper: String,
    next_id: u64,
}

impl DevAuthStore {
    pub fn new() -> Self {
        let secret_pepper = format!("layrs-dev-{}", current_epoch_nanos());

        Self {
            users_by_email: BTreeMap::new(),
            users_by_id: BTreeMap::new(),
            sessions_by_hash: BTreeMap::new(),
            device_flows: BTreeMap::new(),
            desktop_tokens_by_hash: BTreeMap::new(),
            secret_pepper,
            next_id: 1,
        }
    }

    fn create_user(
        &mut self,
        email: &str,
        password: &str,
        display_name: Option<&str>,
    ) -> AuthResult<UserPrincipal> {
        let email = normalize_email(email)?;
        validate_password(password)?;

        if self.users_by_email.contains_key(&email) {
            return Err(AuthError::conflict("email is already registered"));
        }

        let user = UserPrincipal {
            id: self.next_identifier("usr"),
            display_name: display_name
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(email.as_str())
                .to_string(),
            email: email.clone(),
        };
        let password_salt = self.next_secret("pwd-salt");
        let password_hash = hash_secret(&self.secret_pepper, &password_salt, password);
        let record = UserRecord {
            principal: user.clone(),
            password_salt,
            password_hash,
        };

        self.users_by_email.insert(email, user.id.clone());
        self.users_by_id.insert(user.id.clone(), record);

        Ok(user)
    }

    fn issue_session(&mut self, user: UserPrincipal) -> AuthSession {
        let session_token = self.next_secret("session");
        let session_hash = hash_secret(&self.secret_pepper, "session", &session_token);
        self.sessions_by_hash.insert(session_hash, user.id.clone());

        AuthSession {
            user,
            session_token,
        }
    }

    fn ensure_dev_desktop_user(&mut self) -> AuthResult<UserPrincipal> {
        let email = "desktop.dev@local.layrs";
        if let Some(user_id) = self.users_by_email.get(email) {
            return self
                .users_by_id
                .get(user_id)
                .map(|record| record.principal.clone())
                .ok_or_else(|| AuthError::unauthorized("dev desktop user is unavailable"));
        }

        self.create_user(email, "desktop-device-dev", Some("Layrs Desktop Dev"))
    }

    fn next_identifier(&mut self, prefix: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("{prefix}_{id:016x}")
    }

    fn next_secret(&mut self, prefix: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        let seed = format!(
            "{prefix}:{id}:{}:{}",
            current_epoch_nanos(),
            self.secret_pepper
        );
        format!(
            "{prefix}_{}",
            hash_secret(&self.secret_pepper, prefix, &seed)
        )
    }
}

impl Default for DevAuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStore for DevAuthStore {
    fn signup(
        &mut self,
        email: &str,
        password: &str,
        display_name: Option<&str>,
    ) -> AuthResult<AuthSession> {
        let user = self.create_user(email, password, display_name)?;
        Ok(self.issue_session(user))
    }

    fn login(&mut self, email: &str, password: &str) -> AuthResult<AuthSession> {
        let email = normalize_email(email)?;
        let user_id = self
            .users_by_email
            .get(&email)
            .ok_or_else(|| AuthError::unauthorized("email or password is incorrect"))?;
        let record = self
            .users_by_id
            .get(user_id)
            .ok_or_else(|| AuthError::unauthorized("email or password is incorrect"))?;
        let password_hash = hash_secret(&self.secret_pepper, &record.password_salt, password);

        if password_hash != record.password_hash {
            return Err(AuthError::unauthorized("email or password is incorrect"));
        }

        Ok(self.issue_session(record.principal.clone()))
    }

    fn logout(&mut self, session_token: &str) -> AuthResult<()> {
        let session_hash = hash_secret(&self.secret_pepper, "session", session_token);
        self.sessions_by_hash.remove(&session_hash);
        Ok(())
    }

    fn session_user(&self, session_token: &str) -> AuthResult<Option<UserPrincipal>> {
        let session_hash = hash_secret(&self.secret_pepper, "session", session_token);
        let Some(user_id) = self.sessions_by_hash.get(&session_hash) else {
            return Ok(None);
        };

        Ok(self
            .users_by_id
            .get(user_id)
            .map(|record| record.principal.clone()))
    }

    fn start_device_flow(&mut self, verification_uri: &str) -> AuthResult<DeviceStart> {
        let user = self.ensure_dev_desktop_user()?;
        let device_code = self.next_secret("device");
        let user_code = format!("LAYRS-{:06}", self.next_id % 1_000_000);
        let expires_at_epoch_seconds = current_epoch_seconds() + DEFAULT_DEVICE_EXPIRES_IN_SECONDS;

        self.device_flows.insert(
            device_code.clone(),
            DeviceFlowRecord {
                user_id: user.id,
                poll_count: 0,
                expires_at_epoch_seconds,
                token_retrieved: false,
            },
        );

        Ok(DeviceStart {
            device_code,
            user_code,
            verification_uri: verification_uri.to_string(),
            interval_seconds: DEFAULT_DEVICE_POLL_INTERVAL_SECONDS,
            expires_in_seconds: DEFAULT_DEVICE_EXPIRES_IN_SECONDS,
        })
    }

    fn poll_device_flow(&mut self, device_code: &str) -> AuthResult<DevicePoll> {
        let now = current_epoch_seconds();
        let (user_id, should_authorize) = {
            let flow = self
                .device_flows
                .get_mut(device_code)
                .ok_or_else(|| AuthError::unauthorized("device code is unknown"))?;

            if now > flow.expires_at_epoch_seconds || flow.token_retrieved {
                return Ok(DevicePoll::Expired);
            }

            flow.poll_count += 1;
            (flow.user_id.clone(), flow.poll_count >= 2)
        };

        if !should_authorize {
            return Ok(DevicePoll::AuthorizationPending {
                interval_seconds: DEFAULT_DEVICE_POLL_INTERVAL_SECONDS,
            });
        }

        let user = self
            .users_by_id
            .get(&user_id)
            .map(|record| record.principal.clone())
            .ok_or_else(|| AuthError::unauthorized("device user is unavailable"))?;
        let raw_token = self.next_secret("desktop");
        let token_hash = hash_secret(&self.secret_pepper, "desktop", &raw_token);

        self.desktop_tokens_by_hash
            .insert(token_hash, user.id.clone());
        if let Some(flow) = self.device_flows.get_mut(device_code) {
            flow.token_retrieved = true;
        }

        Ok(DevicePoll::Authorized {
            user,
            desktop_token: DeviceToken {
                access_token: raw_token,
                token_type: "Bearer".to_string(),
                expires_in_seconds: 30 * 24 * 60 * 60,
            },
        })
    }

    fn desktop_bootstrap(
        &self,
        desktop_token: &str,
        database_url_configured: bool,
    ) -> AuthResult<Option<DesktopBootstrap>> {
        let token_hash = hash_secret(&self.secret_pepper, "desktop", desktop_token);
        let Some(user_id) = self.desktop_tokens_by_hash.get(&token_hash) else {
            return Ok(None);
        };
        let Some(record) = self.users_by_id.get(user_id) else {
            return Ok(None);
        };

        Ok(Some(DesktopBootstrap {
            user: record.principal.clone(),
            server_mode: "dev-memory".to_string(),
            database_url_configured,
            routes_path: "/v1/routes".to_string(),
        }))
    }
}

#[derive(Clone, Debug)]
struct UserRecord {
    principal: UserPrincipal,
    password_salt: String,
    password_hash: String,
}

#[derive(Clone, Debug)]
struct DeviceFlowRecord {
    user_id: String,
    poll_count: u32,
    expires_at_epoch_seconds: u64,
    token_retrieved: bool,
}

fn normalize_email(email: &str) -> AuthResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(AuthError::invalid("email must be a valid address"));
    }
    Ok(email)
}

fn validate_password(password: &str) -> AuthResult<()> {
    if password.len() < 8 {
        return Err(AuthError::invalid("password must be at least 8 characters"));
    }
    Ok(())
}

fn hash_secret(pepper: &str, salt: &str, secret: &str) -> String {
    let mut hasher = DefaultHasher::new();
    pepper.hash(&mut hasher);
    salt.hash(&mut hasher);
    secret.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_epoch_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_store_hashes_passwords_and_sessions() {
        let mut store = DevAuthStore::new();
        let session = store
            .signup("ALICE@example.com", "correct horse", Some("Alice"))
            .unwrap();

        assert_eq!(session.user.email, "alice@example.com");
        assert_eq!(store.users_by_id.len(), 1);
        assert_ne!(
            store
                .users_by_id
                .get(&session.user.id)
                .unwrap()
                .password_hash,
            "correct horse"
        );
        assert!(!store.sessions_by_hash.contains_key(&session.session_token));
        assert_eq!(
            store.session_user(&session.session_token).unwrap(),
            Some(session.user)
        );
    }

    #[test]
    fn device_flow_returns_token_once_after_pending_poll() {
        let mut store = DevAuthStore::new();
        let start = store.start_device_flow("http://127.0.0.1:8787").unwrap();

        assert_eq!(
            store.poll_device_flow(&start.device_code).unwrap(),
            DevicePoll::AuthorizationPending {
                interval_seconds: DEFAULT_DEVICE_POLL_INTERVAL_SECONDS
            }
        );

        let poll = store.poll_device_flow(&start.device_code).unwrap();
        let DevicePoll::Authorized { desktop_token, .. } = poll else {
            panic!("second poll should be authorized");
        };
        assert!(
            !store
                .desktop_tokens_by_hash
                .contains_key(&desktop_token.access_token)
        );
        assert!(
            store
                .desktop_bootstrap(&desktop_token.access_token, false)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store.poll_device_flow(&start.device_code).unwrap(),
            DevicePoll::Expired
        );
    }
}
