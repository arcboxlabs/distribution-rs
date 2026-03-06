use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default = "default_server")]
    pub server: ServerConfig,
    #[serde(default = "default_storage")]
    pub storage: StorageConfig,
    #[serde(default = "default_database")]
    pub database: DatabaseConfig,
    #[serde(default)]
    #[allow(dead_code)]
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StorageConfig {
    Filesystem { root_dir: PathBuf },
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_url")]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub anonymous_pull: bool,
    /// HMAC secret for HS256 JWT validation (base64 or raw string).
    /// If set, Bearer tokens are validated as JWTs signed with this secret.
    #[serde(default)]
    pub jwt_secret: Option<String>,
    /// Static Basic auth credentials as "username:password".
    /// Multiple entries supported via a Vec.
    #[serde(default)]
    pub basic_credentials: Vec<String>,
}

fn default_server() -> ServerConfig {
    ServerConfig::default()
}

fn default_storage() -> StorageConfig {
    StorageConfig::default()
}

fn default_database() -> DatabaseConfig {
    DatabaseConfig::default()
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

const fn default_port() -> u16 {
    5000
}

const fn default_shutdown_timeout_secs() -> u64 {
    30
}

fn default_database_url() -> String {
    "sqlite://./data/registry.db?mode=rwc".to_string()
}

const fn default_true() -> bool {
    true
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self::Filesystem {
            root_dir: PathBuf::from("./data/storage"),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_database_url(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            anonymous_pull: true,
            jwt_secret: None,
            basic_credentials: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        match path {
            Some(path) => {
                let content = std::fs::read_to_string(path)?;
                let config: Self = toml::from_str(&content)?;
                Ok(config)
            }
            None => Ok(Self::default()),
        }
    }

    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.server.host.parse().expect("invalid host address"),
            self.server.port,
        )
    }
}
