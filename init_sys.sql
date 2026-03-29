-- 创建系统配置表
CREATE TABLE IF NOT EXISTS system_config (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    config_key TEXT NOT NULL UNIQUE,
    config_value TEXT NOT NULL
);

-- 初始化系统配置表
INSERT OR IGNORE INTO system_config (config_key, config_value) VALUES
('system_initialized', 'false'); 

-- 添加分片大小配置
INSERT INTO system_config (config_key, config_value) VALUES ('chunk_size', '1048576') -- 默认1MB
ON CONFLICT(config_key) DO UPDATE SET config_value = '1048576';