use chrono::NaiveDateTime;
use exif::{In, Reader, Tag, Value};
use log::{error, info, warn};
use sqlx::SqlitePool;
use std::time::Duration;

/// 从文件 EXIF 中提取拍摄时间（Unix 秒），无 EXIF 或解析失败返回 None
fn extract_exif_taken_at(path: &str) -> Option<i64> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(&file);
    let exif_reader = Reader::new();
    let exif = exif_reader.read_from_container(&mut reader).ok()?;

    // 依次尝试：拍摄时间 → 数字化时间 → 最后修改时间（EXIF 内的 DateTime）
    for tag in [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime] {
        if let Some(field) = exif.get_field(tag, In::PRIMARY) {
            if let Value::Ascii(v) = &field.value {
                if let Some(raw) = v.first() {
                    let s = String::from_utf8_lossy(raw);
                    if let Some(ts) = parse_exif_time(s.trim()) {
                        return Some(ts);
                    }
                }
            }
        }
    }
    None
}

/// 解析 EXIF 时间字符串（"YYYY:MM:DD HH:MM:SS" 或 "YYYY-MM-DD HH:MM:SS"）。
/// EXIF 时间不含时区，这里按 UTC 处理，对排序影响可忽略。
fn parse_exif_time(s: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

/// 文件系统创建时间（无 EXIF 时的兜底），created() 失败回退 modified()
fn fallback_file_created_at(path: &str) -> i64 {
    let meta = std::fs::metadata(path).ok();
    let created = meta
        .as_ref()
        .and_then(|m| m.created().ok())
        .or_else(|| meta.as_ref().and_then(|m| m.modified().ok()));
    created
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
        .unwrap_or(0)
}

/// 计算文件的拍摄时间：EXIF 优先，无 EXIF 回退到文件系统创建时间
fn compute_taken_at(path: &str) -> i64 {
    extract_exif_taken_at(path).unwrap_or_else(|| fallback_file_created_at(path))
}

/// 解析单个文件的拍摄时间并写回数据库（幂等）
pub async fn parse_and_store_taken_at(db_pool: &SqlitePool, file_id: &str) {
    let file_path: String = match sqlx::query_scalar::<_, String>(
        "SELECT file_path FROM upload_file_meta WHERE file_id = ?",
    )
    .bind(file_id)
    .fetch_optional(db_pool)
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            error!("Failed to fetch file_path for {}: {}", file_id, e);
            return;
        }
    };

    if file_path.is_empty() {
        return;
    }

    let path = file_path.clone();
    let taken_at = tokio::task::spawn_blocking(move || compute_taken_at(&path))
        .await
        .unwrap_or(0);

    if let Err(e) = crate::upload_dao::update_file_taken_at(db_pool, file_id, taken_at).await {
        error!("Failed to update taken_at for {}: {}", file_id, e);
    }
}

/// 扫描并解析所有已完成但尚未解析过 EXIF 的文件
async fn scan_and_parse(db_pool: &SqlitePool) {
    let pending = match crate::upload_dao::fetch_files_needing_exif_parse(db_pool).await {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to fetch files needing exif parse: {}", e);
            return;
        }
    };

    if pending.is_empty() {
        return;
    }

    info!("EXIF parser: {} file(s) pending", pending.len());
    for (file_id, file_path) in pending {
        let path = file_path.clone();
        let taken_at = tokio::task::spawn_blocking(move || compute_taken_at(&path))
            .await
            .unwrap_or(0);
        if let Err(e) = crate::upload_dao::update_file_taken_at(db_pool, &file_id, taken_at).await {
            error!("Failed to update taken_at for {}: {}", file_id, e);
        }
    }
}

/// 启动 EXIF 解析后台任务：启动时立即扫一遍存量，之后每隔 interval 兜底扫描
/// 增量文件由上传完成处的 `parse_and_store_taken_at` 即时触发，这里负责存量与遗漏。
pub fn start_exif_parser_worker(db_pool: SqlitePool) {
    tokio::spawn(async move {
        // 首次立即扫一遍存量
        scan_and_parse(&db_pool).await;

        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            scan_and_parse(&db_pool).await;
        }
    });
}
