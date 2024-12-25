-- 创建数据库
CREATE DATABASE IF NOT EXISTS nascraft;

-- 使用数据库
USE nascraft;

-- 创建上传文件元数据表
CREATE TABLE IF NOT EXISTS upload_file_meta (
    id VARCHAR(36) PRIMARY KEY,
    filename VARCHAR(255) NOT NULL,
    total_size BIGINT NOT NULL,
    checksum VARCHAR(64) NOT NULL,
    status INT DEFAULT 0
);

-- 创建上传进度表
CREATE TABLE IF NOT EXISTS upload_progress (
    checksum VARCHAR(64) PRIMARY KEY,
    filename VARCHAR(255) NOT NULL,
    total_size BIGINT NOT NULL,
    uploaded_size BIGINT NOT NULL,
    start_offset BIGINT NOT NULL,
    end_offset BIGINT NOT NULL,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
); 