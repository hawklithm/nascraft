-- 添加文件元信息字段，优化文件变更检测
ALTER TABLE upload_file_meta ADD COLUMN file_mtime INTEGER DEFAULT 0;
ALTER TABLE upload_file_meta ADD COLUMN file_ctime INTEGER DEFAULT 0;
ALTER TABLE upload_file_meta ADD COLUMN file_ino INTEGER DEFAULT 0;
