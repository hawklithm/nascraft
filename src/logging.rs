use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

static TRACING_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static LOGGING_INITIALIZED: OnceLock<()> = OnceLock::new();

pub fn init_logging() -> std::io::Result<()> {
    if LOGGING_INITIALIZED.get().is_some() {
        return Ok(());
    }

    dotenv::dotenv().ok();

    let (log_dir, file_name) = match env::var("LOG_FILE_PATH") {
        Ok(p) if !p.trim().is_empty() => {
            let path = PathBuf::from(p);
            let dir = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "nascraft.log".to_string());
            (dir, name)
        }
        _ => {
            let dir = env::var("LOG_DIR").unwrap_or_else(|_| "logs".to_string());
            (PathBuf::from(dir), "nascraft.log".to_string())
        }
    };

    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        return Err(e);
    }

    let absolute_log_path = std::env::current_dir()?
        .join(&log_dir)
        .join(&file_name)
        .canonicalize()
        .unwrap_or_else(|_| log_dir.join(&file_name));

    let file_appender = tracing_appender::rolling::never(&log_dir, &file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let _ = TRACING_GUARD.set(guard);

    // Avoid panicking if another logger is already installed.
    if let Err(e) = tracing_log::LogTracer::init() {
        info!("LogTracer already initialized or failed to initialize: {}", e);
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Avoid panicking if a global subscriber was already installed.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .try_init();

    info!("Log file path: {}", absolute_log_path.display());
    info!("Logging initialized");

    let _ = LOGGING_INITIALIZED.set(());

    Ok(())
}

pub fn ensure_data_dirs() -> std::io::Result<()> {
    if let Err(e) = std::fs::create_dir_all("uploads") {
        error!("Failed to create uploads directory: {}", e);
        return Err(e);
    }

    info!("Ensured directory exists: uploads");

    if let Err(e) = std::fs::create_dir_all("media") {
        error!("Failed to create media directory: {}", e);
        return Err(e);
    }

    info!("Ensured directory exists: media");

    Ok(())
}
