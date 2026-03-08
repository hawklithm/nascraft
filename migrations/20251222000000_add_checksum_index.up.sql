-- 为 checksum 字段添加索引以优化去重查询
CREATE INDEX IF NOT EXISTS idx_upload_file_meta_checksum ON upload_file_meta(checksum);
