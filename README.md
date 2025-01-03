# Nascraft

The repository of the corresponding front-end page is [here](https://github.com/hawklithm/nascraft-webui).

Nascraft is a web application designed to handle file uploads efficiently using Rust and Actix-web. It supports chunked file uploads, allowing large files to be uploaded in smaller parts, which are then reassembled on the server. This approach is particularly useful for handling unreliable network connections or large file sizes.

## Features

- **Chunked File Uploads**: Upload large files in smaller chunks to improve reliability and performance.
- **File Metadata Management**: Store and manage metadata for each uploaded file, including filename, total size, and checksum.
- **Upload Progress Tracking**: Track the progress of each file upload, ensuring that all parts are received before final assembly.
- **Database Integration**: Use MySQL for storing file metadata and upload progress, with support for database initialization and structure checks.
- **Asynchronous Processing**: Leverage Rust's asynchronous capabilities for efficient file handling and database operations.

## Getting Started

### Prerequisites

- Rust (latest stable version)
- MySQL database
- Cargo (Rust package manager)

### Installation

1. Clone the repository:

   ```bash
   git clone https://github.com/yourusername/nascraft.git
   cd nascraft
   ```

2. Set up the MySQL database:

   - Create a database named `nascraft`.
   - Run the SQL scripts in `init.sql` and `init_sys.sql` to set up the necessary tables.

3. Configure environment variables:

   Create a `.env` file in the project root with the following variables:

   ```env
   # Database Configuration
   DATABASE_URL=mysql://user:password@localhost/nascraft
   LOG_FILE_PATH=logs/nascraft.log
   SQLX_OFFLINE=true

   # Table Structure Configuration
   EXPECTED_COLUMNS_UPLOAD_FILE_META=id:bigint,file_id:varchar,filename:varchar,total_size:bigint,checksum:varchar,status:int
   EXPECTED_COLUMNS_UPLOAD_PROGRESS=id:bigint,file_id:varchar,checksum:varchar,filename:varchar,total_size:bigint,uploaded_size:bigint,start_offset:bigint,end_offset:bigint,last_updated:timestamp
   ```

4. Build and run the application:

   ```bash
   cargo build
   cargo run
   ```

5. Access the application at `http://127.0.0.1:8080`.

### API Response Format

All API endpoints return responses in the following format:

```json
{
    "message": "Operation result message",
    "status": 1,  // 1 for success, 0 for failure
    "code": "0",  // "0" for success, error code string for failures
    "data": {     // Optional, contains the actual response data
        // Response data specific to each endpoint
    }
}
```

#### Status Codes
- `1`: Success
- `0`: Failure

#### Error Codes
- `"0"`: Success (no error)
- `"SYSTEM_NOT_INITIALIZED"`: System initialization required
- `"MISSING_FILE_ID"`: File ID not provided
- `"DB_SAVE_ERROR"`: Database operation failed
- `"MERGE_CHUNKS_ERROR"`: Error while merging file chunks
- `"UPLOAD_ERROR"`: General upload error
- `"SYSTEM_INIT_ERROR"`: Failed to initialize system status
- `"TABLE_STRUCTURE_ERROR"`: Database table structure error

### API Endpoints

#### `/submit_metadata`

**Description**: Submit metadata for a file before starting the upload process.

**Request**:
- Method: POST
- Content-Type: application/json
- Body:
```json
{
    "filename": "example.txt",
    "total_size": 10485760
}
```

**Success Response**:
```json
{
    "message": "Metadata submitted successfully",
    "status": 1,
    "code": "0",
    "data": {
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "total_size": 10485760,
        "chunk_size": 1048576,
        "total_chunks": 10,
        "chunks": [
            {
                "start_offset": 0,
                "end_offset": 1048575,
                "chunk_size": 1048576
            },
            {
                "start_offset": 1048576,
                "end_offset": 2097151,
                "chunk_size": 1048576
            }
        ]
    }
}
```

#### `/upload`

**Description**: Upload a chunk of a file.

**Request**:
- Method: POST
- Headers:
  - X-File-ID: Unique identifier returned from /submit_metadata
  - X-Start-Offset: Starting byte offset of the chunk
  - Content-Length: Length of the chunk in bytes
  - Content-Range: Range of bytes being uploaded (e.g., "bytes 0-1048575/10485760")
- Body: Binary file chunk data

**Success Response (Chunk Upload)**:
```json
{
    "message": "Chunk upload successful",
    "status": 1,
    "code": "0",
    "data": {
        "status": "range_success",
        "filename": "example.txt",
        "size": 1048576,
        "checksum": "abc123..."
    }
}
```

**Success Response (Final Chunk)**:
```json
{
    "message": "File upload completed successfully",
    "status": 1,
    "code": "0",
    "data": {
        "status": "success",
        "filename": "example.txt",
        "size": 10485760,
        "checksum": "xyz789..."
    }
}
```

#### `/check_table_structure`

**Description**: Check if the database tables have the correct structure.

**Request**:
- Method: GET

**Success Response**:
```json
{
    "message": "Table structure is as expected and system initialized status set to success.",
    "status": 1,
    "code": "0",
    "data": null
}
```

**Error Response**:
```json
{
    "message": "Table structure check failed with errors.",
    "status": 0,
    "code": "TABLE_STRUCTURE_ERROR",
    "data": [
        "Error checking 'upload_file_meta': Table 'upload_file_meta' does not exist",
        "Error checking 'upload_progress': Column type mismatch for 'upload_progress', field 'checksum': expected 'varchar', found 'text'"
    ]
}
```

#### `/ensure_table_structure`

**Description**: Ensure database tables are created with the correct structure.

**Request**:
- Method: GET

**Success Response**:
```json
{
    "message": "Table structure is ensured using init.sql.",
    "status": 1,
    "code": "0",
    "data": null
}
```

**Error Response**:
```json
{
    "message": "Failed to ensure table structure: [error details]",
    "status": 0,
    "code": "TABLE_STRUCTURE_ERROR",
    "data": null
}
```

### Example Usage

1. Submit file metadata:
```bash
curl -X POST http://localhost:8080/submit_metadata \
     -H "Content-Type: application/json" \
     -d '{
           "filename": "example.txt",
           "total_size": 10485760
         }'
```

2. Upload file chunks:
```bash
curl -X POST http://localhost:8080/upload \
     -H "X-File-ID: 550e8400-e29b-41d4-a716-446655440000" \
     -H "X-Start-Offset: 0" \
     -H "Content-Length: 1048576" \
     -H "Content-Range: bytes 0-1048575/10485760" \
     --data-binary @chunk1.bin
```

### Testing

To run the tests, use the following command:

```bash
cargo test
```

### Configuration

The application requires several environment variables to be set in a `.env` file:

#### Required Environment Variables

```env
# Database Configuration
DATABASE_URL=mysql://user:password@localhost/nascraft
LOG_FILE_PATH=logs/nascraft.log
SQLX_OFFLINE=true

# Table Structure Configuration
EXPECTED_COLUMNS_UPLOAD_FILE_META=id:bigint,file_id:varchar,filename:varchar,total_size:bigint,checksum:varchar,status:int
EXPECTED_COLUMNS_UPLOAD_PROGRESS=id:bigint,file_id:varchar,checksum:varchar,filename:varchar,total_size:bigint,uploaded_size:bigint,start_offset:bigint,end_offset:bigint,last_updated:timestamp
```

#### Environment Variables Description

- **Database Configuration**
  - `DATABASE_URL`: MySQL database connection string
  - `LOG_FILE_PATH`: Path where application logs will be written
  - `SQLX_OFFLINE`: Enable SQLx offline mode

- **Table Structure Configuration**
  - `EXPECTED_COLUMNS_UPLOAD_FILE_META`: Defines the expected structure of the `upload_file_meta` table
    - Required columns: `id`, `file_id`, `filename`, `total_size`, `checksum`, `status`
    - Each column is defined in format: `column_name:column_type`
  
  - `EXPECTED_COLUMNS_UPLOAD_PROGRESS`: Defines the expected structure of the `upload_progress` table
    - Required columns: `id`, `file_id`, `checksum`, `filename`, `total_size`, `uploaded_size`, `start_offset`, `end_offset`, `last_updated`
    - Each column is defined in format: `column_name:column_type`

The application will validate the database table structure against these configurations during startup and when the `/check_table_structure` endpoint is called.