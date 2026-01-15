use log::error;
use simplelog::*;
use std::env;
use std::path::{Path, PathBuf};

pub fn init_logging() -> std::io::Result<()> {
    dotenv::dotenv().ok();

    let log_file_path = match env::var("LOG_FILE_PATH") {
        Ok(path) => path,
        Err(_) => {
            error!("LOG_FILE_PATH must be set");
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "LOG_FILE_PATH not set",
            ));
        }
    };

    let log_path = Path::new(&log_file_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let absolute_log_path = std::env::current_dir()?
        .join(log_path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&log_file_path));

    println!("Log file absolute path: {}", absolute_log_path.display());

    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Debug,
        Config::default(),
        std::fs::File::create(&log_file_path).unwrap_or_else(|e| {
            error!("Failed to create log file: {}", e);
            std::process::exit(1);
        }),
    )])
    .unwrap();

    Ok(())
}

pub fn ensure_data_dirs() -> std::io::Result<()> {
    if let Err(e) = std::fs::create_dir_all("uploads") {
        error!("Failed to create uploads directory: {}", e);
        return Err(e);
    }

    if let Err(e) = std::fs::create_dir_all("media") {
        error!("Failed to create media directory: {}", e);
        return Err(e);
    }

    Ok(())
}
