-- 创建系统配置表
CREATE TABLE IF NOT EXISTS system_config (
    id INT AUTO_INCREMENT PRIMARY KEY,
    config_key VARCHAR(255) NOT NULL UNIQUE,
    config_value TEXT NOT NULL
);

-- 初始化系统配置表
INSERT IGNORE INTO system_config (config_key, config_value) VALUES
('system_initialized', 'false'); 