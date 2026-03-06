use distribution::config::{Config, StorageConfig};
use distribution::registry::Registry;
use distribution::storage::filesystem::FilesystemStorage;

use anyhow::Context;
use sea_orm::ConnectionTrait;
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::MigratorTrait;
use std::sync::Arc;
use tokio::signal;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load configuration.
    let config_path = std::env::args().nth(1);
    let config = Config::load(config_path.as_deref()).context("failed to load config")?;
    info!(?config, "loaded configuration");

    // Initialize database.
    let mut db_opts = ConnectOptions::new(&config.database.url);
    db_opts
        .sqlx_logging(false)
        .max_connections(50)
        .min_connections(1);
    let db = Database::connect(db_opts)
        .await
        .context("failed to connect to database")?;

    // Enable SQLite pragmas.
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .context("failed to enable foreign keys")?;
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .context("failed to set WAL mode")?;

    // Run migrations.
    distribution::migration::Migrator::up(&db, None)
        .await
        .context("failed to run migrations")?;
    info!("database migrations complete");

    // Initialize storage backend.
    let storage: Arc<dyn distribution::storage::Storage> = match &config.storage {
        StorageConfig::Filesystem { root_dir } => Arc::new(
            FilesystemStorage::new(root_dir.clone())
                .await
                .context("failed to init storage")?,
        ),
    };
    info!("storage backend initialized");

    // Build app state and router.
    let registry = Arc::new(Registry::new(db, storage));

    // Clean up orphan files from previous runs.
    registry
        .startup_cleanup()
        .await
        .context("startup cleanup failed")?;

    let state = distribution::api::AppState {
        registry,
        auth_config: config.auth.clone(),
    };
    let app = distribution::api::router(state);

    // Bind and serve.
    let addr = config.socket_addr();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("failed to bind address")?;
    info!(%addr, "server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(config.server.shutdown_timeout_secs))
        .await
        .context("server error")?;

    info!("server shut down");
    Ok(())
}

async fn shutdown_signal(_timeout_secs: u64) {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to listen for Ctrl+C");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!("received Ctrl+C"),
        () = terminate => info!("received SIGTERM"),
    }
}
