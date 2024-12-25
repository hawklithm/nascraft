# Nascraft

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
   DATABASE_URL=mysql://user:password@localhost/nascraft
   LOG_FILE_PATH=logs/nascraft.log
   ```

4. Build and run the application:

   ```bash
   cargo build
   cargo run
   ```

5. Access the application at `http://127.0.0.1:8080`.

### Testing

To run the tests, use the following command: