use layrs_server::web::{WebServerConfig, serve};
use std::env;

const DEFAULT_ADDR: &str = "127.0.0.1:8787";
const DEFAULT_STUDIO_URL: &str = "http://127.0.0.1:5173";
const DEFAULT_DEPLOYMENT_ID: &str = "local-dev";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig::from_env_and_args(env::args().skip(1));

    if config.help {
        print_help();
        return Ok(());
    }

    serve(config.web_config()).await?;
    Ok(())
}

#[derive(Clone, Debug)]
struct ServerConfig {
    addr: String,
    studio_url: String,
    database_url: String,
    deployment_id: String,
    cookie_secure: bool,
    help: bool,
}

impl ServerConfig {
    fn from_env_and_args(args: impl Iterator<Item = String>) -> Self {
        let mut addr = env::var("LAYRS_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let mut studio_url =
            env::var("LAYRS_STUDIO_WEB_URL").unwrap_or_else(|_| DEFAULT_STUDIO_URL.to_string());
        let mut database_url = env::var("LAYRS_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "postgres://layrs:layrs@127.0.0.1:15432/layrs".to_string());
        let mut deployment_id =
            env::var("LAYRS_DEPLOYMENT_ID").unwrap_or_else(|_| DEFAULT_DEPLOYMENT_ID.to_string());
        let mut cookie_secure = env::var("LAYRS_COOKIE_SECURE")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|_| !is_local_url(&studio_url));
        let mut help = false;

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => {}
                "--addr" => {
                    if let Some(value) = args.next() {
                        addr = value;
                    }
                }
                "--studio-url" => {
                    if let Some(value) = args.next() {
                        cookie_secure = !is_local_url(&value);
                        studio_url = value;
                    }
                }
                "--database-url" => {
                    if let Some(value) = args.next() {
                        database_url = value;
                    }
                }
                "--deployment-id" => {
                    if let Some(value) = args.next() {
                        deployment_id = value;
                    }
                }
                "--secure-cookies" => cookie_secure = true,
                "--insecure-cookies" => cookie_secure = false,
                "-h" | "--help" => help = true,
                _ => {}
            }
        }

        Self {
            addr,
            studio_url,
            database_url,
            deployment_id,
            cookie_secure,
            help,
        }
    }

    fn web_config(&self) -> WebServerConfig {
        WebServerConfig {
            addr: self.addr.clone(),
            studio_url: self.studio_url.clone(),
            database_url: self.database_url.clone(),
            deployment_id: self.deployment_id.clone(),
            cookie_secure: self.cookie_secure,
        }
    }
}

fn is_local_url(value: &str) -> bool {
    value.contains("127.0.0.1") || value.contains("localhost") || value.contains("[::1]")
}

fn print_help() {
    println!(
        "Layrs Server\n\nUsage:\n  layrs-server [serve] [--addr 127.0.0.1:8787] [--studio-url http://127.0.0.1:5173] [--database-url postgres://...] [--deployment-id local-dev]\n\nEnvironment:\n  LAYRS_SERVER_ADDR\n  LAYRS_STUDIO_WEB_URL\n  LAYRS_DEPLOYMENT_ID\n  LAYRS_DATABASE_URL or DATABASE_URL\n  LAYRS_COOKIE_SECURE"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_and_args_prefer_explicit_values() {
        let config = ServerConfig::from_env_and_args(
            [
                "serve".to_string(),
                "--addr".to_string(),
                "127.0.0.1:9999".to_string(),
                "--studio-url".to_string(),
                "http://127.0.0.1:1111".to_string(),
                "--database-url".to_string(),
                "postgres://example".to_string(),
                "--deployment-id".to_string(),
                "test".to_string(),
            ]
            .into_iter(),
        );

        assert_eq!(config.addr, "127.0.0.1:9999");
        assert_eq!(config.studio_url, "http://127.0.0.1:1111");
        assert_eq!(config.database_url, "postgres://example");
        assert_eq!(config.deployment_id, "test");
        assert!(!config.cookie_secure);
    }
}
