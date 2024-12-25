-- 创建数据库
CREATE DATABASE IF NOT EXISTS nascraft;

-- 使用数据库
USE nascraft;

-- 创建上传状态表
CREATE TABLE IF NOT EXISTS upload_states (
    id VARCHAR(36) PRIMARY KEY,
    filename VARCHAR(255) NOT NULL,
    total_size BIGINT NOT NULL,
    uploaded_size BIGINT NOT NULL,
    checksum VARCHAR(64) NOT NULL
); 