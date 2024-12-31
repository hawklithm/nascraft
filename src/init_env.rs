use sqlx::{MySqlPool, Row, Executor};
use log::{info, error};
use std::fs;
use dotenv::dotenv;
use std::env;
use actix_web::{web, HttpResponse};
use serde::Serialize;
use std::borrow::Cow;

#[derive(Debug, Serialize)]
struct ApiResponse<T> {
    message: String,
    status: i32,
    code: String,
    data: Option<T>,
}

impl<T> ApiResponse<T> {
    fn success(message: &str, data: Option<T>) -> Self {
        Self {
            message: message.to_string(),
            status: 1,
            code: "0".to_string(),
            data,
        }
    }

    fn error(message: &str, code: &str, data: Option<T>) -> Self {
        Self {
            message: message.to_string(),
            status: 0,
            code: code.to_string(),
            data,
        }
    }
}

pub async fn init_db_pool() -> Result<MySqlPool, sqlx::Error> {
    dotenv().ok(); // Load .env file

    let database_url = env::var("DATABASE_URL").map_err(|e| {
        error!("DATABASE_URL must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;

    let pool = MySqlPool::connect(&database_url).await?;

    // Ensure system configuration table
    ensure_system_config(&pool).await?;

    Ok(pool)
}

async fn ensure_system_config(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    execute_sql_script(pool, "init_sys.sql").await?;
    info!("System configuration ensured.");
    Ok(())
}

async fn execute_sql_script(pool: &MySqlPool, script_path: &str) -> Result<(), sqlx::Error> {
    let sql = fs::read_to_string(script_path).map_err(|e| {
        error!("Failed to read {}: {}", script_path, e);
        sqlx::Error::Io(e)
    })?;
    pool.execute(sql.as_str()).await?;
    info!("Executed SQL script: {}", script_path);
    Ok(())
}

pub async fn check_table_structure(pool: &MySqlPool) -> Result<Vec<String>, sqlx::Error> {
    dotenv().ok(); // Ensure environment variables are loaded

    let mut errors = Vec::new(); // Collect error messages

    // Check upload_file_meta table
    let expected_columns_upload_file_meta_str = env::var("EXPECTED_COLUMNS_UPLOAD_FILE_META").map_err(|e| {
        error!("EXPECTED_COLUMNS_UPLOAD_FILE_META must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;
    let expected_columns_upload_file_meta: Vec<(&str, &str)> = expected_columns_upload_file_meta_str
        .split(',')
        .filter_map(|s| {
            let mut parts = s.split(':');
            match (parts.next(), parts.next()) {
                (Some(name), Some(type_)) => Some((name, type_)),
                _ => None,
            }
        })
        .collect();

    if let Err(e) = check_table(pool, "upload_file_meta", &expected_columns_upload_file_meta).await {
        errors.push(format!("Error checking 'upload_file_meta': {}", e));
    }

    // Check upload_progress table
    let expected_columns_upload_progress_str = env::var("EXPECTED_COLUMNS_UPLOAD_PROGRESS").map_err(|e| {
        error!("EXPECTED_COLUMNS_UPLOAD_PROGRESS must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;
    let expected_columns_upload_progress: Vec<(&str, &str)> = expected_columns_upload_progress_str
        .split(',')
        .filter_map(|s| {
            let mut parts = s.split(':');
            match (parts.next(), parts.next()) {
                (Some(name), Some(type_)) => Some((name, type_)),
                _ => None,
            }
        })
        .collect();

    if let Err(e) = check_table(pool, "upload_progress", &expected_columns_upload_progress).await {
        errors.push(format!("Error checking 'upload_progress': {}", e));
    }

    if errors.is_empty() {
        info!("All table structures are as expected.");
        Ok(vec![]) // Return empty error list
    } else {
        Ok(errors)
    }
}

async fn check_table(pool: &MySqlPool, table_name: &str, expected_columns: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    let query = format!("SHOW COLUMNS FROM {}", table_name);
    let rows = match sqlx::query(&query).fetch_all(pool).await {
        Ok(rows) => rows,
        Err(e) => {
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.code() == Some(std::borrow::Cow::Borrowed("42S02")) { // MySQL error code for table not found
                    error!("Table '{}' does not exist", table_name);
                    return Err(sqlx::Error::RowNotFound);
                }
            }
            return Err(e);
        }
    };

    // First check for type mismatches
    for row in &rows {
        let field: Cow<str> = match row.try_get::<Cow<str>, _>("Field") {
            Ok(val) => val,
            Err(e) => {
                error!("Failed to get 'Field' from row: {}", e);
                return Err(e);
            }
        };
        
        let field_type: Cow<str> = match row.try_get::<Vec<u8>, _>("Type") {
            Ok(val) => String::from_utf8(val).map(Cow::Owned).map_err(|e| {
                error!("Failed to convert 'Type' from Vec<u8> to String: {}", e);
                sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?,
            Err(e) => {
                error!("Failed to get 'Type' from row: {}", e);
                return Err(e);
            }
        };

        if let Some((_, expected_type)) = expected_columns.iter().find(|(name, _)| name == &field) {
            if !field_type.starts_with(&expected_type.to_lowercase()) {
                error!("Column type mismatch for table '{}', field '{}': expected '{}', found '{}'", 
                    table_name, field, expected_type, field_type);
                return Err(sqlx::Error::RowNotFound);
            } else {
                info!("Column '{}' in table '{}' is valid with type '{}'", field, table_name, field_type);
            }
        } else {
            error!("Unexpected column in table '{}': '{}'", table_name, field);
            return Err(sqlx::Error::RowNotFound);
        }
    }

    // Then check for missing columns
    for (expected_field, _) in expected_columns {
        if !rows.iter().any(|row| {
            let field: Cow<str> = row.get("Field");
            field == *expected_field
        }) {
            error!("Missing column '{}' in table '{}'", expected_field, table_name);
            return Err(sqlx::Error::RowNotFound);
        }
    }

    info!("Table '{}' structure is as expected.", table_name);
    Ok(())
}

pub async fn ensure_table_structure(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    match check_table_structure(pool).await {
        Ok(errors) => {
            if !errors.is_empty() {
                info!("Table structure is incorrect. Attempting to create the correct structure using init.sql.");
                execute_sql_script(pool, "init.sql").await?;
            } else {
                info!("Table structure is correct.");
            }
            Ok(())
        }
        Err(_) => {
            info!("Table structure check failed. Attempting to create the correct structure using init.sql.");
            execute_sql_script(pool, "init.sql").await?;
            Ok(())
        }
    }
}

pub async fn set_system_initialized(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE system_config SET config_value = 'success' WHERE config_key = 'system_initialized'"
    )
    .execute(pool)
    .await?;
    info!("System initialized status set to success.");
    Ok(())
}

pub async fn check_table_structure_endpoint(
    data: web::Data<MySqlPool>,
) -> HttpResponse {
    match check_table_structure(&data).await {
        Ok(errors) => {
            if errors.is_empty() {
                if let Err(e) = set_system_initialized(&data).await {
                    error!("Failed to update system_initialized status: {}", e);
                    return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
                        &format!("Failed to update system_initialized status: {}", e),
                        "SYSTEM_INIT_ERROR",
                        None
                    ));
                }
                HttpResponse::Ok().json(ApiResponse::<()>::success(
                    "Table structure is as expected and system initialized status set to success.",
                    None
                ))
            } else {
                HttpResponse::Ok().json(ApiResponse::<Vec<String>>::error(
                    "Table structure check failed with errors.",
                    "TABLE_STRUCTURE_ERROR",
                    Some(errors)
                ))
            }
        },
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &format!("Table structure check failed: {}", e),
            "TABLE_STRUCTURE_ERROR",
            None
        )),
    }
}

pub async fn ensure_table_structure_endpoint(
    data: web::Data<MySqlPool>,
) -> HttpResponse {
    match ensure_table_structure(&data).await {
        Ok(_) => {
            if let Err(e) = set_system_initialized(&data).await {
                error!("Failed to update system_initialized status: {}", e);
                return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
                    &format!("Failed to update system_initialized status: {}", e),
                    "SYSTEM_INIT_ERROR",
                    None
                ));
            }
            HttpResponse::Ok().json(ApiResponse::<()>::success(
                "Table structure is ensured using init.sql.",
                None
            ))
        },
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &format!("Failed to ensure table structure: {}", e),
            "TABLE_STRUCTURE_ERROR",
            None
        )),
    }
}

pub async fn check_system_initialized(pool: &MySqlPool) -> Result<(), HttpResponse> {
    let row = sqlx::query!("SELECT config_value FROM system_config WHERE `config_key` = 'system_initialized'")
        .fetch_one(pool)
        .await
        .map_err(|e| {
            error!("Failed to fetch system_initialized status: {}", e);
            HttpResponse::InternalServerError().body("Failed to fetch system status")
        })?;

    if row.config_value != "success" {
        error!("System not initialized");
        return Err(HttpResponse::BadRequest().body("System needs initialization"));
    }

    Ok(())
} 