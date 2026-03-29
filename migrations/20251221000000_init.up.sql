CREATE TABLE IF NOT EXISTS system_config (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    config_key TEXT NOT NULL UNIQUE,
    config_value TEXT NOT NULL
);

INSERT OR IGNORE INTO system_config (config_key, config_value) VALUES
('system_initialized', 'false');

INSERT INTO system_config (config_key, config_value) VALUES ('chunk_size', '1048576')
ON CONFLICT(config_key) DO UPDATE SET config_value = '1048576';

CREATE TABLE IF NOT EXISTS upload_file_meta (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id TEXT NOT NULL,
    filename TEXT NOT NULL,
    total_size INTEGER NOT NULL,
    checksum TEXT NOT NULL,
    status INT DEFAULT 0,
    file_path TEXT NOT NULL,
    last_updated INTEGER DEFAULT 0,
    UNIQUE (file_id)
);

CREATE TABLE IF NOT EXISTS upload_progress (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id TEXT NOT NULL,
    checksum TEXT NOT NULL,
    filename TEXT NOT NULL,
    total_size INTEGER NOT NULL,
    uploaded_size INTEGER NOT NULL,
    start_offset INTEGER NOT NULL,
    end_offset INTEGER NOT NULL,
    last_updated INTEGER DEFAULT 0,
    FOREIGN KEY (file_id) REFERENCES upload_file_meta(file_id)
);

CREATE INDEX IF NOT EXISTS idx_upload_progress_file_id ON upload_progress(file_id);
