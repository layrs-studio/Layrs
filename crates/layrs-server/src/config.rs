//! Validated native-server configuration. Secrets deliberately have no Debug implementation.
use layrs_core::scm::StoreId;
use layrs_store_server::v3::{TransferBaseUrl, TransferUrlPolicy};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

pub struct ServerConfig {
    pub addr: SocketAddr,
    pub public_url: TransferBaseUrl,
    pub database_url: String,
    pub cas_root: PathBuf,
    pub deployment_id: String,
    pub store_id: StoreId,
    pub signing_key: Vec<u8>,
    pub bootstrap_secret: Option<String>,
    pub cas_quotas: layrs_store_server::v3::CasQuotas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub &'static str);
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ConfigError {}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::parse(&std::env::vars().collect())
    }
    pub fn parse(env: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let required = |name: &str| {
            env.get(name)
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .ok_or(ConfigError("missing required server configuration"))
        };
        let addr: SocketAddr = env
            .get("LAYRS_SERVER_ADDR")
            .map(String::as_str)
            .unwrap_or("127.0.0.1:8787")
            .parse()
            .map_err(|_| ConfigError("invalid LAYRS_SERVER_ADDR"))?;
        let public = required("LAYRS_PUBLIC_URL")?;
        let development = env
            .get("LAYRS_ALLOW_LOOPBACK_HTTP")
            .is_some_and(|v| v == "true");
        let policy = if development && public.starts_with("http://") {
            if !addr.ip().is_loopback() {
                return Err(ConfigError("development HTTP requires a loopback listener"));
            }
            let uri: axum::http::Uri = public
                .parse()
                .map_err(|_| ConfigError("invalid public URL"))?;
            TransferUrlPolicy::new(Vec::<String>::new(), [uri.port_u16().unwrap_or(80)])
        } else {
            TransferUrlPolicy::new([&public], [])
        }
        .map_err(|_| ConfigError("public URL must be an HTTPS origin"))?;
        let public_url = policy
            .validate_base(&public)
            .map_err(|_| ConfigError("invalid public origin"))?;
        let database_url = secret(env, "LAYRS_DATABASE_URL")?
            .ok_or(ConfigError("database URL secret is required"))?;
        if !(database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")) {
            return Err(ConfigError("database URL must use PostgreSQL"));
        }
        let cas_root = PathBuf::from(required("LAYRS_CAS_ROOT")?);
        if !cas_root.is_absolute() {
            return Err(ConfigError("CAS root must be absolute"));
        }
        let deployment_id = required("LAYRS_DEPLOYMENT_ID")?;
        if !valid_id(&deployment_id, "deployment:") {
            return Err(ConfigError("invalid deployment ID"));
        }
        let store_id = StoreId::parse(required("LAYRS_STORE_ID")?)
            .map_err(|_| ConfigError("invalid store ID"))?;
        let key = secret(env, "LAYRS_SIGNING_KEY")?
            .ok_or(ConfigError("signing key secret is required"))?;
        let signing_key = hex::decode(key)
            .map_err(|_| ConfigError("signing key must be 32 random bytes encoded as hex"))?;
        if signing_key.len() != 32 {
            return Err(ConfigError(
                "signing key must be 32 random bytes encoded as hex",
            ));
        }
        let bootstrap_secret = secret(env, "LAYRS_BOOTSTRAP_SECRET")?;
        if bootstrap_secret
            .as_ref()
            .is_some_and(|s| hex::decode(s).map_or(true, |v| v.len() != 32))
        {
            return Err(ConfigError(
                "bootstrap secret must be 32 random bytes encoded as hex",
            ));
        }
        Ok(Self {
            addr,
            public_url,
            database_url,
            cas_root,
            deployment_id,
            store_id,
            signing_key,
            bootstrap_secret,
            cas_quotas: parse_quotas(env)?,
        })
    }
}

fn parse_quotas(
    env: &BTreeMap<String, String>,
) -> Result<layrs_store_server::v3::CasQuotas, ConfigError> {
    let limit = |name| {
        env.get(name)
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| ConfigError("invalid CAS quota integer"))
            })
            .transpose()
    };
    layrs_store_server::v3::CasQuotas {
        max_total_bytes: limit("LAYRS_CAS_MAX_TOTAL_BYTES")?,
        max_total_objects: limit("LAYRS_CAS_MAX_TOTAL_OBJECTS")?,
        max_space_bytes: limit("LAYRS_CAS_MAX_SPACE_BYTES")?,
        max_space_objects: limit("LAYRS_CAS_MAX_SPACE_OBJECTS")?,
    }
    .validate()
    .map_err(|_| ConfigError("CAS quotas must be in 1..=i64::MAX"))
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    })
}

fn secret(env: &BTreeMap<String, String>, name: &str) -> Result<Option<String>, ConfigError> {
    match (env.get(name), env.get(&format!("{name}_FILE"))) {
        (Some(_), Some(_)) => Err(ConfigError(
            "configure a secret value or secret file, never both",
        )),
        (Some(value), None) => Ok(Some(value.trim().to_owned())),
        (None, Some(path)) => {
            use std::io::Read;
            let file = std::fs::File::open(path)
                .map_err(|_| ConfigError("cannot read configured secret file"))?;
            let mut value = String::new();
            file.take(8193)
                .read_to_string(&mut value)
                .map_err(|_| ConfigError("cannot read configured secret file"))?;
            if value.len() > 8192 {
                return Err(ConfigError("secret file exceeds 8192 bytes"));
            }
            Ok(Some(value.trim().to_owned()))
        }
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env() -> BTreeMap<String, String> {
        [
            ("LAYRS_PUBLIC_URL", "https://layrs.example"),
            ("LAYRS_DATABASE_URL", "postgres://db/layrs"),
            (
                "LAYRS_CAS_ROOT",
                if cfg!(windows) {
                    "D:\\layrs-cas"
                } else {
                    "/var/lib/layrs"
                },
            ),
            ("LAYRS_DEPLOYMENT_ID", "deployment:test"),
            ("LAYRS_STORE_ID", "store:test"),
            (
                "LAYRS_SIGNING_KEY",
                "11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff",
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    #[test]
    fn valid_https_config() {
        assert!(ServerConfig::parse(&env()).is_ok());
    }
    #[test]
    fn quota_configuration_is_explicit_and_checked() {
        assert_eq!(
            ServerConfig::parse(&env()).unwrap().cas_quotas,
            Default::default()
        );
        for value in ["0", "-1", "not-a-number", "9223372036854775808"] {
            let mut values = env();
            values.insert("LAYRS_CAS_MAX_TOTAL_BYTES".into(), value.into());
            assert!(ServerConfig::parse(&values).is_err());
        }
        let mut values = env();
        values.insert("LAYRS_CAS_MAX_TOTAL_BYTES".into(), "1048576".into());
        values.insert("LAYRS_CAS_MAX_SPACE_OBJECTS".into(), "1000".into());
        let limits = ServerConfig::parse(&values).unwrap().cas_quotas;
        assert_eq!(limits.max_total_bytes, Some(1048576));
        assert_eq!(limits.max_space_objects, Some(1000));
        assert_eq!(limits.max_total_objects, None);
    }
    #[test]
    fn refuses_insecure_remote_and_url_credentials() {
        for url in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://example.com/path",
            "https://example.com?secret=x",
        ] {
            let mut values = env();
            values.insert("LAYRS_PUBLIC_URL".into(), url.into());
            assert!(ServerConfig::parse(&values).is_err());
        }
    }
    #[test]
    fn loopback_http_requires_explicit_opt_in_and_loopback_listener() {
        let mut values = env();
        values.insert("LAYRS_PUBLIC_URL".into(), "http://127.0.0.1:8787".into());
        assert!(ServerConfig::parse(&values).is_err());
        values.insert("LAYRS_ALLOW_LOOPBACK_HTTP".into(), "true".into());
        assert!(ServerConfig::parse(&values).is_ok());
        values.insert("LAYRS_SERVER_ADDR".into(), "0.0.0.0:8787".into());
        assert!(ServerConfig::parse(&values).is_err());
    }
    #[test]
    fn refuses_ambiguous_secrets_and_relative_cas() {
        let mut values = env();
        values.insert("LAYRS_DATABASE_URL_FILE".into(), "missing".into());
        assert!(ServerConfig::parse(&values).is_err());
        let mut values = env();
        values.insert("LAYRS_CAS_ROOT".into(), "cas".into());
        assert!(ServerConfig::parse(&values).is_err());
    }
}
