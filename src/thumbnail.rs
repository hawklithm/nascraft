use image::{ImageFormat, io::Reader as ImageReader, GenericImageView};
use log::{info, error};
use std::path::Path;
use std::process::Command;
use std::env;
use tokio::fs;

/// Configuration for thumbnail generation
#[derive(Debug, Clone)]
pub struct ThumbnailConfig {
    /// Maximum dimensions for thumbnail (width x height)
    pub max_size: u32,
    /// Thumbnail storage directory
    pub thumbnails_dir: String,
}

impl Default for ThumbnailConfig {
    fn default() -> Self {
        Self {
            max_size: 200,
            thumbnails_dir: "thumbnails".to_string(),
        }
    }
}

/// Check if a file is an image based on file extension
pub fn is_image_file(filename: &str) -> bool {
    let extensions = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];
    filename
        .to_lowercase()
        .rsplit('.')
        .next()
        .map(|ext| extensions.contains(&ext))
        .unwrap_or(false)
}

/// Check if a file is a video based on file extension
pub fn is_video_file(filename: &str) -> bool {
    let extensions = ["mp4", "webm", "mkv", "avi", "mov", "flv", "wmv", "m4v"];
    filename
        .to_lowercase()
        .rsplit('.')
        .next()
        .map(|ext| extensions.contains(&ext))
        .unwrap_or(false)
}

/// Generate a thumbnail for an image file
/// Returns the relative path to the thumbnail on success
pub async fn generate_thumbnail(
    config: &ThumbnailConfig,
    original_path: &str,
    checksum: &str,
) -> Option<String> {
    // Ensure thumbnails directory exists
    if let Err(e) = fs::create_dir_all(&config.thumbnails_dir).await {
        error!("Failed to create thumbnails directory: {}", e);
        return None;
    }

    let thumbnail_path = format!("{}/{}.webp", config.thumbnails_dir, checksum);

    // Generate thumbnail synchronously (CPU-bound work)
    let original_path = original_path.to_string();
    let max_size = config.max_size;
    let result = tokio::task::spawn_blocking(move || {
        generate_thumbnail_sync(&original_path, max_size)
    }).await;

    match result {
        Ok(Ok(image)) => {
            // Save as WebP
            match fs::write(&thumbnail_path, image).await {
                Ok(_) => {
                    info!("Thumbnail generated successfully: {}", thumbnail_path);
                    Some(thumbnail_path)
                }
                Err(e) => {
                    error!("Failed to write thumbnail: {}", e);
                    None
                }
            }
        }
        Ok(Err(e)) => {
            error!("Failed to generate thumbnail: {}", e);
            None
        }
        Err(e) => {
            error!("Thumbnail task panicked: {}", e);
            None
        }
    }
}

/// Generate thumbnail synchronously (runs in blocking thread)
fn generate_thumbnail_sync(original_path: &str, max_size: u32) -> Result<Vec<u8>, String> {
    let path = Path::new(original_path);
    let img = ImageReader::open(path)
        .map_err(|e| format!("Failed to open image: {}", e))?
        .decode()
        .map_err(|e| format!("Failed to decode image: {}", e))?;

    let (width, height) = img.dimensions();

    // Calculate new dimensions preserving aspect ratio
    let (new_width, new_height) = if width > height {
        let ratio = max_size as f32 / width as f32;
        (max_size, (height as f32 * ratio) as u32)
    } else {
        let ratio = max_size as f32 / height as f32;
        ((width as f32 * ratio) as u32, max_size)
    };

    let resized = img.resize(new_width, new_height, image::imageops::Lanczos3);

    // Encode to WebP in memory
    let mut buffer = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buffer);
    resized.write_to(&mut cursor, ImageFormat::WebP)
        .map_err(|e| format!("Failed to encode WebP: {}", e))?;

    Ok(buffer)
}

/// Check if a thumbnail already exists
pub async fn thumbnail_exists(config: &ThumbnailConfig, checksum: &str) -> bool {
    let thumbnail_path = format!("{}/{}.webp", config.thumbnails_dir, checksum);
    fs::metadata(&thumbnail_path).await.is_ok()
}

/// 提取视频第一帧作为缩略图（依赖系统 ffmpeg）
/// ffmpeg 路径可通过环境变量 FFMPEG_PATH 覆盖，默认使用 PATH 中的 "ffmpeg"。
/// 输出与图片缩略图一致的 WebP 格式（供 serve_thumbnail 以 image/webp 提供）。
/// ffmpeg 缺失或失败时返回 None（视频无缩略图，客户端回退为图标），不影响上传。
pub async fn generate_video_thumbnail(
    config: &ThumbnailConfig,
    original_path: &str,
    checksum: &str,
) -> Option<String> {
    // Ensure thumbnails directory exists
    if let Err(e) = fs::create_dir_all(&config.thumbnails_dir).await {
        error!("Failed to create thumbnails directory: {}", e);
        return None;
    }

    let thumbnail_path = format!("{}/{}.webp", config.thumbnails_dir, checksum);

    let ffmpeg_path = env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string());
    let ffmpeg_path_log = ffmpeg_path.clone();
    let input = original_path.to_string();
    let output = thumbnail_path.clone();
    // 宽度不超过 max_size 且不放大（min），高度按比例取偶（-2）
    let scale_filter = format!("scale='min({},iw)':-2", config.max_size);

    let result = tokio::task::spawn_blocking(move || {
        Command::new(&ffmpeg_path)
            .arg("-y")
            .arg("-t").arg("1")
            .arg("-i").arg(&input)
            .arg("-frames:v").arg("1")
            .arg("-vf").arg(&scale_filter)
            .arg("-f").arg("webp")
            .arg(&output)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    }).await;

    match result {
        Ok(Ok(status)) if status.success() => {
            if fs::metadata(&thumbnail_path).await.is_ok() {
                info!("Video thumbnail generated: {}", thumbnail_path);
                Some(thumbnail_path)
            } else {
                error!("ffmpeg succeeded but thumbnail file missing: {}", thumbnail_path);
                None
            }
        }
        Ok(Ok(status)) => {
            error!("ffmpeg exited with status {} for {}", status, original_path);
            None
        }
        Ok(Err(e)) => {
            error!("Failed to run ffmpeg (is it installed? FFMPEG_PATH={}): {}", ffmpeg_path_log, e);
            None
        }
        Err(e) => {
            error!("Video thumbnail task panicked: {}", e);
            None
        }
    }
}
