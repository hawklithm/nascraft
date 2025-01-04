# Nascraft

The repository of the corresponding front-end page is [here](https://github.com/hawklithm/nascraft-webui).

Nascraft is a web application designed to handle file uploads efficiently using Rust and Actix-web. It supports chunked file uploads, allowing large files to be uploaded in smaller parts, which are then reassembled on the server. This approach is particularly useful for handling unreliable network connections or large file sizes.

## Features

- **Chunked File Uploads**: Upload large files in smaller chunks to improve reliability and performance.
- **File Metadata Management**: Store and manage metadata for each uploaded file, including filename, total size, and checksum.
- **Upload Progress Tracking**: Track the progress of each file upload, ensuring that all parts are received before final assembly.
- **Database Integration**: Use MySQL for storing file metadata and upload progress, with support for database initialization and structure checks.
- **Asynchronous Processing**: Leverage Rust's asynchronous capabilities for efficient file handling and database operations.
- **Query Uploaded Files**: Retrieve a list of uploaded files with support for pagination, filtering by status, sorting, and total count.

## Frontend Repository

The frontend code for Nascraft is available in a separate repository. You can find it here: [Nascraft Web UI](https://github.com/hawklithm/nascraft-webui).

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
   EXPECTED_COLUMNS_UPLOAD_FILE_META=id:bigint,file_id:varchar,filename:varchar,total_size:bigint,checksum:varchar,status:int,file_path:varchar
   EXPECTED_COLUMNS_UPLOAD_PROGRESS=id:bigint,file_id:varchar,checksum:varchar,filename:varchar,total_size:bigint,uploaded_size:bigint,start_offset:bigint,end_offset:bigint,last_updated:bigint
   ```

4. Build and run the application:

   ```bash
   cargo build
   cargo run
   ```

5. Access the application at `http://127.0.0.1:8080`.

### API Endpoints

#### `/uploaded_files`

**Description**: Retrieve a list of uploaded files with pagination, filtering by status, sorting options, and total count.

**Request**:
- Method: GET
- Query Parameters:
  - `page`: The page number to retrieve (default is 1).
  - `page_size`: The number of items per page (default is 10).
  - `status`: Optional. Filter files by their status.
  - `sort_by`: Optional. Sort files by `size`, `date`, or `id` (default is `id`).
  - `order`: Optional. Sort order, either `asc` or `desc` (default is `asc`).

**Success Response**:
```json
{
    "message": "Fetched uploaded files successfully",
    "status": 1,
    "code": "0",
    "data": {
        "total_files": 100,
        "files": [
            {
                "file_id": "550e8400-e29b-41d4-a716-446655440000",
                "filename": "example.txt",
                "total_size": 10485760,
                "checksum": "abc123...",
                "status": 2
            },
            // More files...
        ]
    }
}
```

**Example Usage**:
```bash
curl -X GET "http://localhost:8080/uploaded_files?page=1&page_size=10&status=2&sort_by=size&order=desc"
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