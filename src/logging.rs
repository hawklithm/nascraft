use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// 单个日志文件达到该大小后触发滚动（分片），单位字节，默认 10 MB。
const MAX_LOG_SIZE_BYTES: u64 = 10 * 1024 * 1024;
/// 日志文件保留天数，超过该天数的滚动日志将被自动清理，默认 10 天。
const LOG_RETENTION_DAYS: i64 = 10;

static TRACING_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static LOGGING_INITIALIZED: OnceLock<()> = OnceLock::new();

/// 当前活动日志文件的状态。旋转时需先关闭文件句柄（Windows 不允许重命名已打开的文件）。
struct LogFileState {
    file: Option<File>,
    current_size: u64,
    file_path: PathBuf,
    rotated_dir: PathBuf,
    stem: String,
}

impl LogFileState {
    fn open(log_dir: &Path, file_name: &str) -> io::Result<Self> {
        let file_path = log_dir.join(file_name);
        let file = OpenOptions::new().create(true).append(true).open(&file_path)?;
        let current_size = file.metadata()?.len();
        let stem = Path::new(file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| file_name.to_string());
        Ok(Self {
            file: Some(file),
            current_size,
            file_path,
            rotated_dir: log_dir.to_path_buf(),
            stem,
        })
    }

    /// 将当前日志文件重命名为带时间戳的归档名，并打开一个新的空文件继续写入。
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        // 关闭句柄，否则 Windows 上 rename 会失败。
        self.file = None;

        let ts = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S");
        let rotated_name = format!("{}.{}.log", self.stem, ts);
        let rotated_path = self.rotated_dir.join(&rotated_name);

        fs::rename(&self.file_path, &rotated_path)?;

        self.file = Some(OpenOptions::new().create(true).append(true).open(&self.file_path)?);
        self.current_size = 0;
        Ok(())
    }
}

/// 按大小滚动的日志 writer：写满 `MAX_LOG_SIZE_BYTES` 后自动归档并新建文件。
///
/// 该 writer 仅由 `tracing_appender::non_blocking` 的后台工作线程独占访问，
/// 因此无需内部锁。
struct SizeRollingWriter {
    state: LogFileState,
}

impl SizeRollingWriter {
    fn new(log_dir: &Path, file_name: &str) -> io::Result<Self> {
        Ok(Self {
            state: LogFileState::open(log_dir, file_name)?,
        })
    }
}

impl io::Write for SizeRollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.state.current_size >= MAX_LOG_SIZE_BYTES {
            self.state.rotate()?;
        }
        let file = self.state.file.as_mut().expect("log file is not open");
        file.write_all(buf)?;
        self.state.current_size += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.state.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }
}

/// 清理超过 `LOG_RETENTION_DAYS` 天的滚动日志文件（只匹配以 `{stem}.` 开头的 .log 文件，
/// 不影响当前正在写入的活动日志文件，也不碰目录里的其它文件）。
fn cleanup_old_logs(log_dir: &Path, stem: &str) {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(LOG_RETENTION_DAYS);
    let entries = match fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(e) => {
            error!(
                "Failed to read log directory {} for cleanup: {}",
                log_dir.display(),
                e
            );
            return;
        }
    };

    let prefix = format!("{}.", stem);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // 只清理本服务的滚动日志，避免误删目录里其它文件。
        if !name.starts_with(&prefix) || !name.ends_with(".log") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mtime_dt = chrono::DateTime::<chrono::Utc>::from(mtime);
        if mtime_dt < cutoff {
            if let Err(e) = fs::remove_file(&path) {
                error!("Failed to remove old log file {}: {}", path.display(), e);
            } else {
                info!(
                    "Removed log file older than {} days: {}",
                    LOG_RETENTION_DAYS,
                    path.display()
                );
            }
        }
    }
}

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

    let stem = Path::new(&file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.clone());

    cleanup_old_logs(&log_dir, &stem);

    let writer = SizeRollingWriter::new(&log_dir, &file_name)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);
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
    info!(
        "Logging initialized (size-based rolling: {} MB, retention: {} days)",
        MAX_LOG_SIZE_BYTES / 1024 / 1024,
        LOG_RETENTION_DAYS
    );

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
