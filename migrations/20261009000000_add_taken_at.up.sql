ALTER TABLE upload_file_meta ADD COLUMN taken_at INTEGER DEFAULT 0;
ALTER TABLE upload_file_meta ADD COLUMN exif_parsed INTEGER DEFAULT 0;
