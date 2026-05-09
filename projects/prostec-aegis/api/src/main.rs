use anyhow::Result;
use tracing::info;

mod config;
mod crypto;
mod db;
mod error;
mod middleware;
mod models;
mod routes;
mod state;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = config::Config::load()?;
    let port = cfg.port;
    let aws_cfg = aws_config::load_from_env().await;
    let app_state = state::AppState::new(cfg, &aws_cfg).await?;
    let app = routes::router(app_state);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr, "listening");

    axum::serve(listener, app).await?;
    Ok(())
}
