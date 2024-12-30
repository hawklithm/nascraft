-- 创建数据库
CREATE DATABASE IF NOT EXISTS nascraft;

-- 使用数据库
USE nascraft;
-- 创建系统配置表
CREATE TABLE IF NOT EXISTS system_config (
    id INT AUTO_INCREMENT PRIMARY KEY,
    config_key VARCHAR(255) NOT NULL UNIQUE,
    config_value VARCHAR(2048) NOT NULL
);

-- 初始化系统配置表
INSERT IGNORE INTO system_config (config_key, config_value) VALUES
('system_initialized', 'false'); 

-- 添加分片大小配置
INSERT INTO system_config (config_key, config_value) VALUES ('chunk_size', '1048576') -- 默认1MB
ON DUPLICATE KEY UPDATE config_value = '1048576'; 