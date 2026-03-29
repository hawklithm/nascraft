use axum::Router;
use log::error;
use log::info;

pub async fn serve_http(app: Router, server_port: u16) -> std::io::Result<()> {
    let bind_addr = format!("0.0.0.0:{}", server_port);
    info!("Binding HTTP listener: addr={}", bind_addr);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    tokio::spawn(async move {
        info!("HTTP server started");
        if let Err(e) = axum::serve(listener, app).await {
            error!("Main server error: {}", e);
        }
    });

    Ok(())
}
