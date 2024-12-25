use sqlx::{MySqlPool, Row, Executor};
use log::{info, error};
use std::fs;
use dotenv::dotenv;
use std::env;

pub async fn init_db_pool() -> MySqlPool {
    dotenv().ok(); // 加载 .env 文件

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = MySqlPool::connect(&database_url).await.unwrap();

    // 检查并初始化系统配置表
    ensure_system_config(&pool).await.expect("Failed to ensure system config");

    pool
}

async fn ensure_system_config(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    let init_sys_sql = fs::read_to_string("init_sys.sql").expect("Failed to read init_sys.sql");
    pool.execute(init_sys_sql.as_str()).await?;
    info!("System configuration ensured.");
    Ok(())
}

pub async fn check_table_structure(pool: &MySqlPool) -> Result<(), sqlx::Error> {
    dotenv().ok(); // 确保环境变量已加载

    let expected_columns_str = env::var("EXPECTED_COLUMNS").expect("EXPECTED_COLUMNS must be set");
    let expected_columns: Vec<(&str, &str)> = expected_columns_str
        .split(',')
        .map(|s| {
            let mut parts = s.split(':');
            (parts.next().unwrap(), parts.next().unwrap())
        })
        .collect();

    let rows = sqlx::query("SHOW COLUMNS FROM upload_states")
        .fetch_all(pool)
        .await?;

    for row in rows {
        let field: &str = row.get("Field");
        let field_type: &str = row.get("Type");

        if let Some((_, expected_type)) = expected_columns.iter().find(|(name, _)| name == &field) {
            if expected_type != &field_type {
                error!("Column type mismatch for '{}': expected '{}', found '{}'", field, expected_type, field_type);
                return Err(sqlx::Error::RowNotFound);
            }
        } else {
            error!("Unexpected column '{}'", field);
            return Err(sqlx::Error::RowNotFound);
        }
    }

    info!("Table structure is as expected.");
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
            let init_sql = fs::read_to_string("init.sql").expect("Failed to read init.sql");
            pool.execute(init_sql.as_str()).await?;
            info!("Table structure created using init.sql.");
            Ok(())
        }
    }
} 