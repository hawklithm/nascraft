use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::stream;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use log::error;
use crate::upload_dao::fetch_file_record;
use crate::AppContext;

const STREAM_CHUNK_SIZE: usize = 64 * 1024;

/// Range 请求解析结果
#[derive(Debug, PartialEq)]
enum RangeResult {
    /// 无 Range 或无法解析，回退为 200 全量
    Full,
    /// 单区间 (start, end)，end 为闭区间
    Partial(u64, u64),
    /// 起始位置越界，应返回 416
    Unsatisfiable,
}

/// 解析 HTTP Range 头（仅支持单区间 `bytes=a-b` / `bytes=a-` / `bytes=-N`）。
/// 多区间按 RFC 允许的做法回退为 200 全量；语法非法同样回退全量。
fn parse_range(value: &str, len: u64) -> RangeResult {
    let spec = match value.trim().strip_prefix("bytes=") {
        Some(s) => s.trim(),
        None => return RangeResult::Full,
    };
    if spec.is_empty() || spec.contains(',') {
        return RangeResult::Full;
    }

    let (start, end) = if let Some(suffix) = spec.strip_prefix('-') {
        // bytes=-N：最后 N 字节
        let n: u64 = match suffix.trim().parse() {
            Ok(n) => n,
            Err(_) => return RangeResult::Full,
        };
        if n == 0 || len == 0 {
            return RangeResult::Unsatisfiable;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let (first, last) = match spec.split_once('-') {
            Some(p) => p,
            None => return RangeResult::Full,
        };
        let start: u64 = match first.trim().parse() {
            Ok(v) => v,
            Err(_) => return RangeResult::Full,
        };
        // bytes=a- ：到文件末尾
        let end: u64 = match last.trim().parse() {
            Ok(v) => v,
            Err(_) => len.saturating_sub(1),
        };
        if start >= len {
            return RangeResult::Unsatisfiable;
        }
        // 末尾越界按 RFC 截断到 len-1
        (start, end.min(len - 1))
    };

    if start > end {
        return RangeResult::Unsatisfiable;
    }
    RangeResult::Partial(start, end)
}

pub async fn download_file(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(file_id_str): Path<String>,
) -> Response {
    let db_pool = &ctx.app_state.db_pool;

    let range_hdr = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    log::info!(
        "Download request: file_id={}, range={}, ua={}",
        file_id_str,
        range_hdr,
        ua
    );

    let (filename, _, _, _, file_path) = match fetch_file_record(db_pool, &file_id_str).await {
        Ok(record) => record,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let file_len = match tokio::fs::metadata(&file_path).await {
        Ok(m) => m.len(),
        Err(e) => {
            error!("Failed to stat file {}: {}", file_path, e);
            return error_response(StatusCode::NOT_FOUND, "File not found on disk");
        }
    };

    let content_type = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, file_len))
        .unwrap_or(RangeResult::Full);

    let (status, start, end) = match range {
        RangeResult::Partial(start, end) => (StatusCode::PARTIAL_CONTENT, start, end),
        RangeResult::Full => (StatusCode::OK, 0, file_len.saturating_sub(1)),
        RangeResult::Unsatisfiable => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(
                    header::CONTENT_RANGE,
                    format!("bytes */{}", file_len),
                )],
            )
                .into_response()
        }
    };
    let body_len = if file_len == 0 { 0 } else { end - start + 1 };

    let mut file = match File::open(&file_path).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open file {}: {}", file_path, e);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to open file");
        }
    };

    if start > 0 {
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
            error!("Failed to seek file {}: {}", file_path, e);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to seek file");
        }
    }

    // 分块流式读取，避免大文件占满内存
    let body_stream = stream::unfold(
        (file, body_len),
        |(mut file, mut remaining)| async move {
            if remaining == 0 {
                return None;
            }
            let buf_len = STREAM_CHUNK_SIZE.min(remaining as usize);
            let mut buf = vec![0u8; buf_len];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    remaining -= n as u64;
                    Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, remaining)))
                }
                Err(e) => Some((Err(e), (file, 0))),
            }
        },
    );

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, body_len.to_string())
        // DLNA 渲染器拉流后若连接保持 keep-alive，部分电视无法判断流已结束而卡在"接收中"；
        // 显式关闭连接，让设备传完即收到结束信号
        .header(header::CONNECTION, "close");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, file_len),
        );
    }

    match builder.body(Body::from_stream(body_stream)) {
        Ok(resp) => resp,
        Err(e) => {
            error!("Failed to build download response: {}", e);
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to build response")
        }
    }
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

pub async fn serve_thumbnail(
    State(ctx): State<AppContext>,
    Path(file_id_str): Path<String>,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;

    // Fetch the uploaded file to get thumbnail path
    match crate::upload_dao::fetch_uploaded_file_by_id(db_pool, &file_id_str).await {
        Ok(Some(file)) => {
            let thumbnail_path = match &file.thumbnail_path {
                Some(path) => path,
                None => {
                    return (StatusCode::NOT_FOUND, "No thumbnail for this file").into_response();
                }
            };

            // Open the thumbnail file
            let mut file = match tokio::fs::File::open(&thumbnail_path).await {
                Ok(f) => f,
                Err(e) => {
                    error!("Failed to open thumbnail: {}", e);
                    return (StatusCode::NOT_FOUND, "Thumbnail not found").into_response();
                }
            };

            // Read the file content
            let mut buffer = Vec::new();
            if let Err(e) = file.read_to_end(&mut buffer).await {
                error!("Failed to read thumbnail: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read thumbnail").into_response();
            }

            // Return with proper content type and cache headers
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "image/webp"),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                buffer,
            ).into_response()
        }
        Ok(None) => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => {
            error!("Failed to fetch file for thumbnail: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_full_when_no_or_invalid_header() {
        assert_eq!(parse_range("", 100), RangeResult::Full);
        assert_eq!(parse_range("items=0-5", 100), RangeResult::Full);
        assert_eq!(parse_range("bytes=", 100), RangeResult::Full);
        assert_eq!(parse_range("bytes=abc", 100), RangeResult::Full);
        assert_eq!(parse_range("bytes=0-5,10-20", 100), RangeResult::Full);
    }

    #[test]
    fn range_closed_interval() {
        assert_eq!(parse_range("bytes=0-0", 100), RangeResult::Partial(0, 0));
        assert_eq!(parse_range("bytes=10-19", 100), RangeResult::Partial(10, 19));
        assert_eq!(parse_range("bytes=0-999999", 100), RangeResult::Partial(0, 99));
    }

    #[test]
    fn range_open_ended() {
        assert_eq!(parse_range("bytes=50-", 100), RangeResult::Partial(50, 99));
        assert_eq!(parse_range("bytes=0-", 100), RangeResult::Partial(0, 99));
    }

    #[test]
    fn range_suffix() {
        assert_eq!(parse_range("bytes=-50", 100), RangeResult::Partial(50, 99));
        assert_eq!(parse_range("bytes=-1", 100), RangeResult::Partial(99, 99));
        assert_eq!(parse_range("bytes=-999", 100), RangeResult::Partial(0, 99));
        assert_eq!(parse_range("bytes=-0", 100), RangeResult::Unsatisfiable);
    }

    #[test]
    fn range_unsatisfiable() {
        assert_eq!(parse_range("bytes=100-", 100), RangeResult::Unsatisfiable);
        assert_eq!(parse_range("bytes=100-200", 100), RangeResult::Unsatisfiable);
        assert_eq!(parse_range("bytes=20-10", 100), RangeResult::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), RangeResult::Unsatisfiable);
    }
}
