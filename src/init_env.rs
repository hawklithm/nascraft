use sqlx::{SqlitePool, Row, Executor};
use log::{info, error};
use std::fs;
use dotenv::dotenv;
use std::env;
use std::borrow::Cow;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;

pub async fn init_db_pool() -> Result<SqlitePool, sqlx::Error> {
    dotenv().ok(); // Load .env file

    let database_url = env::var("DATABASE_URL").map_err(|e| {
        error!("DATABASE_URL must be set: {}", e);
        sqlx::Error::Configuration(e.into())
    })?;

    ensure_sqlite_db_parent_dir(&database_url)?;

    let options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(options).await?;

    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

fn ensure_sqlite_db_parent_dir(database_url: &str) -> Result<(), sqlx::Error> {
    // Best-effort: if url is like sqlite://path/to/file.db or sqlite:path/to/file.db
    // create the parent directory so sqlite can create the db file.
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"));

    if let Some(p) = path {
        let p = p.trim_start_matches('/');
        if !p.is_empty() && p != ":memory:" {
            let db_path = std::path::Path::new(p);
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| sqlx::Error::Io(e))?;
                }
            }
        }
    }
    Ok(())
}

async fn ensure_system_config(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    execute_sql_script(pool, "init_sys.sql").await?;
    info!("System configuration ensured.");
    Ok(())
}

async fn execute_sql_script(pool: &SqlitePool, script_path: &str) -> Result<(), sqlx::Error> {
    let sql = fs::read_to_string(script_path).map_err(|e| {
        error!("Failed to read {}: {}", script_path, e);
        sqlx::Error::Io(e)
    })?;

    for statement in sql.split(';') {
        let stmt = statement.trim();
        if stmt.is_empty() {
            continue;
        }
        pool.execute(stmt).await?;
    }
    info!("Executed SQL script: {}", script_path);
    Ok(())
}

pub async fn check_table_structure(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
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

async fn check_table(pool: &SqlitePool, table_name: &str, expected_columns: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    let query = format!("PRAGMA table_info({})", table_name);
    let rows = sqlx::query(&query).fetch_all(pool).await?;

    if rows.is_empty() {
        error!("Table '{}' does not exist", table_name);
        return Err(sqlx::Error::RowNotFound);
    }

    // First check for type mismatches / unexpected columns
    for row in &rows {
        let field: Cow<str> = match row.try_get::<Cow<str>, _>("name") {
            Ok(val) => val,
            Err(e) => {
                error!("Failed to get 'name' from row: {}", e);
                return Err(e);
            }
        };

        let field_type: Cow<str> = match row.try_get::<Cow<str>, _>("type") {
            Ok(val) => val,
            Err(e) => {
                error!("Failed to get 'type' from row: {}", e);
                return Err(e);
            }
        };

        if let Some((_, expected_type)) = expected_columns.iter().find(|(name, _)| name == &field) {
            if !field_type.to_lowercase().contains(&expected_type.to_lowercase()) {
                error!(
                    "Column type mismatch for table '{}', field '{}': expected contains '{}', found '{}'",
                    table_name,
                    field,
                    expected_type,
                    field_type
                );
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
            let field: Cow<str> = row.get("name");
            field == *expected_field
        }) {
            error!("Missing column '{}' in table '{}'", expected_field, table_name);
            return Err(sqlx::Error::RowNotFound);
        }
    }

    info!("Table '{}' structure is as expected.", table_name);
    Ok(())
}

pub async fn ensure_table_structure(pool: &SqlitePool) -> Result<(), sqlx::Error> {
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

pub async fn set_system_initialized(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE system_config SET config_value = 'success' WHERE config_key = 'system_initialized'")
        .execute(pool)
        .await?;
    info!("System initialized status set to success.");
    Ok(())
}

pub async fn check_system_initialized(pool: &SqlitePool) -> Result<(), bool> {
    // let row = sqlx::query("SELECT config_value FROM system_config WHERE config_key = 'system_initialized'")
    //     .fetch_one(pool)
    //     .await
    //     .map_err(|e| {
    //         error!("Failed to fetch system_initialized status: {}", e);
    //         false
    //     })?;

    // let config_value: String = row.get("config_value");
    // if config_value != "success" {
    //     error!("System not initialized");
    //     return Err(false);
    // }

    Ok(())
} 