//! `sherlock-web`: the process the container runs.

use std::net::SocketAddr;
use std::sync::Arc;

use sherlock_web::{api, AppState, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `--health` is what the container healthcheck runs: it answers without a listener,
    // so a wedged port does not look like a healthy process.
    if std::env::args().any(|arg| arg == "--health") {
        return match Config::from_env() {
            Ok(config) => {
                let library = sherlock_web::Library::new(config.library_dir);
                match library.pointer() {
                    Ok(pointer) => {
                        println!("ok revision={}", pointer.revision);
                        Ok(())
                    }
                    Err(error) => {
                        eprintln!("unhealthy: {error}");
                        std::process::exit(1);
                    }
                }
            }
            Err(error) => {
                eprintln!("unhealthy: {error}");
                std::process::exit(1);
            }
        };
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::from_env()?;
    let port = config.port;
    let state = AppState::new(config);
    let router = api::app(Arc::clone(&state));

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    log::info!("sherlock-web listening on {address}");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    log::info!("shutting down");
}
