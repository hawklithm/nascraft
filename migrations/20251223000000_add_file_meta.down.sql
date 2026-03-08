-- 回滚：删除添加的元信息字段
ALTER TABLE upload_file_meta DROP COLUMN file_ino;
ALTER TABLE upload_file_meta DROP COLUMN file_ctime;
ALTER TABLE upload_file_meta DROP COLUMN file_mtime;
