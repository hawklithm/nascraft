use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    message: String,
    status: i32,
    code: String,
    data: Option<T>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            message: "Success".to_string(),
            status: 1,
            code: "0".to_string(),
            data: Some(data),
        }
    }

    pub fn error(code: String, message: String) -> Self {
        Self {
            message,
            status: 0,
            code,
            data: None,
        }
    }
} 