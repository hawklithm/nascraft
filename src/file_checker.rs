use sqlx::{SqlitePool, Row};
use log::{error, info, warn};
use tokio::fs;
use tokio::io::AsyncReadExt;
use md5::{Md5, Digest};
use std::time::Duration;
use crate::upload_dao::update_file_meta_info;
use crate::thumbnail::{is_image_file, is_video_file, generate_thumbnail, generate_video_thumbnail, ThumbnailConfig};
use crate::upload_dao::update_file_thumbnail_path;

/// 定期检查文件元信息是否发生变化
/// 每隔10分钟检查一次uploads目录下的所有文件
/// 优化：先检查文件元信息（mtime, ctime, ino），只有变化时才计算MD5
pub fn start_file_integrity_checker(db_pool: SqlitePool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(600)); // 10分钟

        loop {
            interval.tick().await;
            info!("Starting periodic file integrity check (optimized with meta info)...");

            if let Err(e) = check_and_update_file_integrity(&db_pool).await {
                error!("File integrity check failed: {}", e);
            }

            // 清理过期未完成的上传僵尸记录（DB 行 + 磁盘残留分片）
            cleanup_zombie_upload_records(&db_pool).await;
        }
    });
}

/// 文件元信息（用于快速检测文件是否变化）
#[derive(Debug, Clone, PartialEq)]
struct FileSystemMeta {
    mtime: i64,    // 修改时间
    ctime: i64,    // 创建时间
    size: i64,     // 文件大小
    ino: Option<i64>, // inode号（仅Unix-like系统可用）
}

/// 检查并更新文件完整性（优化版本：先检查元信息）
async fn check_and_update_file_integrity(db_pool: &SqlitePool) -> Result<(), String> {
    // 获取所有已完成状态(status=2)且文件路径不为空的文件记录
    let files = match sqlx::query(
        "SELECT file_id, filename, checksum, file_path, total_size, file_mtime, file_ctime, file_ino, thumbnail_path FROM upload_file_meta WHERE status = 2 AND file_path IS NOT NULL AND file_path != ''"
    )
    .fetch_all(db_pool)
    .await
    {
        Ok(files) => files,
        Err(e) => {
            error!("Failed to fetch files for integrity check: {}", e);
            return Err("Failed to fetch files".to_string());
        }
    };

    let mut meta_updated_count = 0;
    let mut checksum_updated_count = 0;
    let mut missing_count = 0;
    let mut unchanged_count = 0;
    let mut thumbnails_generated_count = 0;
    let files_count = files.len();
    let thumbnail_config = ThumbnailConfig::default();

    for row in &files {
        let file_id: String = row.get("file_id");
        let filename: String = row.get("filename");
        let stored_checksum: String = row.get("checksum");
        let file_path: String = row.get("file_path");
        let stored_size: i64 = row.get("total_size");
        let stored_mtime: i64 = row.get("file_mtime");
        let stored_ctime: i64 = row.get("file_ctime");
        let stored_ino: i64 = row.get("file_ino");
        let stored_thumbnail_path: Option<String> = row.try_get("thumbnail_path").ok();
        // NULL 或空字符串都视为「无缩略图」：旧数据可能存在 thumbnail_path='' 的记录，
        // 若只用 is_none() 判断，'' 会被误判为已有缩略图而跳过补生成。
        let needs_thumbnail = stored_thumbnail_path
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);

        // 检查文件是否存在
        if !fs::try_exists(&file_path).await.unwrap_or(false) {
            warn!("File not found: {} (file_id: {})", file_path, file_id);
            missing_count += 1;
            continue;
        }

        // 补生成缺失的缩略图（与文件元信息是否变化无关）。
        // 视频上传时缩略图是后台生成，若当时 ffmpeg 未就绪会静默失败，
        // 这里在每次巡检中对缺失缩略图的图片/视频无条件补一次。
        if needs_thumbnail {
            if is_image_file(&filename) {
                info!("Generating thumbnail for existing image: {} (file_id: {})", filename, file_id);
                if let Some(thumbnail_path) = generate_thumbnail(&thumbnail_config, &file_path, &stored_checksum).await {
                    if let Err(e) = update_file_thumbnail_path(db_pool, &file_id, &thumbnail_path).await {
                        error!("Failed to save thumbnail path for existing image: {}", e);
                    } else {
                        info!("Thumbnail generated for existing image: {} (file_id: {})", filename, file_id);
                        thumbnails_generated_count += 1;
                    }
                }
            } else if is_video_file(&filename) {
                info!("Generating thumbnail for existing video: {} (file_id: {})", filename, file_id);
                if let Some(thumbnail_path) = generate_video_thumbnail(&thumbnail_config, &file_path, &stored_checksum).await {
                    if let Err(e) = update_file_thumbnail_path(db_pool, &file_id, &thumbnail_path).await {
                        error!("Failed to save thumbnail path for existing video: {}", e);
                    } else {
                        info!("Thumbnail generated for existing video: {} (file_id: {})", filename, file_id);
                        thumbnails_generated_count += 1;
                    }
                }
            }
        }

        // 获取当前文件的元信息
        let current_meta = match get_filesystem_meta(&file_path).await {
            Ok(meta) => meta,
            Err(e) => {
                error!("Failed to get file metadata for {}: {}", file_path, e);
                continue;
            }
        };

        // 如果数据库中没有存储元信息（旧数据），需要更新
        let needs_meta_update = stored_mtime == 0 && stored_ctime == 0;
        if needs_meta_update {
            info!("Updating missing meta info for: {} (file_id: {})", filename, file_id);
            if let Err(e) = update_file_meta_info(
                db_pool,
                &file_id,
                current_meta.mtime,
                current_meta.ctime,
                current_meta.ino.unwrap_or(0)
            ).await {
                error!("Failed to update file meta info: {}", e);
            } else {
                meta_updated_count += 1;
            }
            // 补充元信息后，继续检查MD5以确保数据一致性
        }

        // 比较元信息：如果都相同，认为文件未变化，跳过MD5计算
        let stored_meta = FileSystemMeta {
            mtime: stored_mtime,
            ctime: stored_ctime,
            size: stored_size,
            ino: if stored_ino > 0 { Some(stored_ino) } else { None },
        };

        if !needs_meta_update && current_meta == stored_meta {
            unchanged_count += 1;
            continue; // 元信息相同且非新补充数据，文件未变化
        }

        // 元信息不同，需要计算MD5验证
        info!(
            "File meta changed, checking MD5: {} (file_id: {}), mtime: {}->{}, size: {}->{}",
            filename, file_id, stored_mtime, current_meta.mtime, stored_size, current_meta.size
        );

        let current_checksum = match calculate_file_md5(&file_path).await {
            Ok(checksum) => checksum,
            Err(e) => {
                error!("Failed to calculate MD5 for {}: {}", file_path, e);
                continue;
            }
        };

        // 如果MD5也变化了，更新所有信息
        if current_checksum != stored_checksum {
            info!(
                "File content changed: {} (file_id: {}), MD5: {}->{}",
                filename, file_id, stored_checksum, current_checksum
            );

            // 更新数据库中的MD5和元信息
            if let Err(e) = update_file_hash_and_meta(
                db_pool,
                &file_id,
                &current_checksum,
                current_meta.size,
                current_meta.mtime,
                current_meta.ctime,
                current_meta.ino.unwrap_or(0)
            ).await {
                error!("Failed to update file hash and meta: {}", e);
            } else {
                info!("Updated file hash and meta for: {} (file_id: {})", filename, file_id);
                checksum_updated_count += 1;
            }
        } else {
            // MD5相同但元信息不同（可能是文件被移动或复制），更新元信息
            info!(
                "File content unchanged but meta changed (likely file moved/copied): {} (file_id: {}), updating meta",
                filename, file_id
            );

            if let Err(e) = update_file_meta_info(
                db_pool,
                &file_id,
                current_meta.mtime,
                current_meta.ctime,
                current_meta.ino.unwrap_or(0)
            ).await {
                error!("Failed to update file meta info: {}", e);
            } else {
                info!("Updated meta info for unchanged file: {} (file_id: {})", filename, file_id);
                meta_updated_count += 1;
            }
        }
    }

    info!(
        "File integrity check completed: {} total, {} unchanged, {} meta-updated, {} checksum-updated, {} missing, {} thumbnails generated",
        files_count,
        unchanged_count,
        meta_updated_count,
        checksum_updated_count,
        missing_count,
        thumbnails_generated_count
    );

    Ok(())
}

/// 清理上传僵尸记录：上传中途被遗弃、超过 7 天仍未完成（status 0/1）的记录，
/// 连同磁盘上残留的分片文件一起删除。
async fn cleanup_zombie_upload_records(db_pool: &SqlitePool) {
    const ZOMBIE_STALE_DAYS: i64 = 7;
    match crate::upload_dao::cleanup_zombie_uploads(db_pool, ZOMBIE_STALE_DAYS).await {
        Ok(filenames) => {
            let count = filenames.len();
            for filename in filenames {
                // 删除 uploads/{filename}_chunk_* 残留分片
                let dir = "uploads".to_string();
                let prefix = format!("{}_chunk_", filename);
                let result = tokio::task::spawn_blocking(move || {
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name();
                            if name.to_string_lossy().starts_with(&prefix) {
                                if let Err(e) = std::fs::remove_file(entry.path()) {
                                    error!("Failed to remove stale chunk file {}: {}", entry.path().display(), e);
                                }
                            }
                        }
                    }
                })
                .await;
                if let Err(e) = result {
                    error!("Failed to run chunk file cleanup task: {}", e);
                }
            }
            if count > 0 {
                info!("Cleaned up {} zombie upload record(s)", count);
            }
        }
        Err(e) => {
            warn!("Zombie upload cleanup failed: {}", e);
        }
    }
}

/// 获取文件的文件系统元信息
async fn get_filesystem_meta(file_path: &str) -> Result<FileSystemMeta, String> {
    let metadata = match fs::metadata(file_path).await {
        Ok(meta) => meta,
        Err(e) => return Err(format!("Failed to get metadata: {}", e)),
    };

    let size = metadata.len() as i64;
    let mtime = metadata.modified()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
        .unwrap_or(0);
    let ctime = metadata.created()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
        .unwrap_or_else(|_| mtime); // Windows可能不支持created()

    // 尝试获取inode（仅Unix-like系统；Windows无inode，返回None）
    #[cfg(unix)]
    let ino = std::fs::metadata(file_path)
        .ok()
        .and_then(|m| std::os::unix::fs::MetadataExt::ino(&m).try_into().ok());
    #[cfg(not(unix))]
    let ino: Option<i64> = None;

    Ok(FileSystemMeta {
        mtime,
        ctime,
        size,
        ino,
    })
}

/// 计算文件的MD5哈希值
async fn calculate_file_md5(file_path: &str) -> Result<String, String> {
    let mut file = match fs::File::open(file_path).await {
        Ok(f) => f,
        Err(e) => return Err(format!("Failed to open file: {}", e)),
    };

    let mut hasher = Md5::new();
    let mut buffer = [0u8; 8192]; // 8KB buffer

    loop {
        let n = match file.read(&mut buffer).await {
            Ok(n) if n == 0 => break,
            Ok(n) => n,
            Err(e) => return Err(format!("Failed to read file: {}", e)),
        };
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// 更新文件的MD5和元信息
async fn update_file_hash_and_meta(
    db_pool: &SqlitePool,
    file_id: &str,
    new_checksum: &str,
    new_size: i64,
    file_mtime: i64,
    file_ctime: i64,
    file_ino: i64,
) -> Result<(), String> {
    match sqlx::query(
        "UPDATE upload_file_meta SET checksum = ?, total_size = ?, file_mtime = ?, file_ctime = ?, file_ino = ?, last_updated = strftime('%s', 'now') WHERE file_id = ?"
    )
    .bind(new_checksum)
    .bind(new_size)
    .bind(file_mtime)
    .bind(file_ctime)
    .bind(file_ino)
    .bind(file_id)
    .execute(db_pool)
    .await
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to update file hash and meta: {}", e)),
    }
}
