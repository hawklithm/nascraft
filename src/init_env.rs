use sqlx::{MySqlPool, Row, Executor};
use log::{info, error};
use std::fs;
use dotenv::dotenv;
use std::env;
use actix_web::{web, HttpResponse};

pub async fn init_db_pool() -> Result<MySqlPool, sqlx::Error> {
    dotenv().ok(); // 加载 .env 文件

    let database_url = env::var("DATABASE_URL").map_err(|e| {
        error!("DATABASE_URL must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;

    let pool = MySqlPool::connect(&database_url).await?;

    // 检查并初始化系统配置表
    ensure_system_config(&pool).await?;

    Ok(pool)
}

async fn ensure_system_config(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    let init_sys_sql = fs::read_to_string("init_sys.sql").map_err(|e| {
        error!("Failed to read init_sys.sql: {}", e);
        sqlx::Error::Io(e)
    })?;
    pool.execute(init_sys_sql.as_str()).await?;
    info!("System configuration ensured.");
    Ok(())
}

pub async fn check_table_structure(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    dotenv().ok(); // 确保环境变量已加载

    // 校验 upload_file_meta 表
    let expected_columns_upload_file_meta_str = env::var("EXPECTED_COLUMNS_UPLOAD_FILE_META").map_err(|e| {
        error!("EXPECTED_COLUMNS_UPLOAD_FILE_META must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;
    let expected_columns_upload_file_meta: Vec<(&str, &str)> = expected_columns_upload_file_meta_str
        .split(',')
        .map(|s| {
            let mut parts = s.split(':');
            (parts.next().unwrap(), parts.next().unwrap())
        })
        .collect();

    check_table(pool, "upload_file_meta", &expected_columns_upload_file_meta).await?;

    // 校验 upload_progress 表
    let expected_columns_upload_progress_str = env::var("EXPECTED_COLUMNS_UPLOAD_PROGRESS").map_err(|e| {
        error!("EXPECTED_COLUMNS_UPLOAD_PROGRESS must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;
    let expected_columns_upload_progress: Vec<(&str, &str)> = expected_columns_upload_progress_str
        .split(',')
        .map(|s| {
            let mut parts = s.split(':');
            (parts.next().unwrap(), parts.next().unwrap())
        })
        .collect();

    check_table(pool, "upload_progress", &expected_columns_upload_progress).await?;

    info!("All table structures are as expected.");
    Ok(())
}

async fn check_table(pool: &MySqlPool, table_name: &str, expected_columns: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    let query = format!("SHOW COLUMNS FROM {}", table_name);
    let rows = sqlx::query(&query)
        .fetch_all(pool)
        .await?;

    for row in rows {
        let field: &str = row.get("Field");
        let field_type: &str = row.get("Type");

        if let Some((_, expected_type)) = expected_columns.iter().find(|(name, _)| name == &field) {
            if expected_type != &field_type {
                error!("Column type mismatch for '{}.{}': expected '{}', found '{}'", table_name, field, expected_type, field_type);
                return Err(sqlx::Error::RowNotFound);
            }
        } else {
            error!("Unexpected column '{}.{}'", table_name, field);
            return Err(sqlx::Error::RowNotFound);
        }
    }

    info!("Table '{}' structure is as expected.", table_name);
    Ok(())
}

pub async fn ensure_table_structure(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    match check_table_structure(pool).await {
        Ok(_) => {
            info!("Table structure is correct.");
            Ok(())
        }
        Err(_) => {
            info!("Table structure is incorrect. Attempting to create the correct structure using init.sql.");
            let init_sql = fs::read_to_string("init.sql").map_err(|e| {
                error!("Failed to read init.sql: {}", e);
                sqlx::Error::Io(e)
            })?;
            pool.execute(init_sql.as_str()).await?;
            info!("Table structure created using init.sql.");
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
        Ok(_) => {
            if let Err(e) = set_system_initialized(&data).await {
                error!("Failed to update system_initialized status: {}", e);
                return HttpResponse::InternalServerError().body(format!("Failed to update system_initialized status: {}", e));
            }
            HttpResponse::Ok().json("Table structure is as expected and system initialized status set to success.")
        },
        Err(e) => HttpResponse::InternalServerError().body(format!("Table structure check failed: {}", e)),
    }
}

pub async fn ensure_table_structure_endpoint(
    data: web::Data<MySqlPool>,
) -> HttpResponse {
    match ensure_table_structure(&data).await {
        Ok(_) => {
            if let Err(e) = set_system_initialized(&data).await {
                error!("Failed to update system_initialized status: {}", e);
                return HttpResponse::InternalServerError().body(format!("Failed to update system_initialized status: {}", e));
            }
            HttpResponse::Ok().json("Table structure is ensured using init.sql.")
        },
        Err(e) => HttpResponse::InternalServerError().body(format!("Failed to ensure table structure: {}", e)),
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