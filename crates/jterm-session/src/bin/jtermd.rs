//! jtermd — the jterm session daemon.

use jterm_session::daemon::Daemon;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    log::info!("jtermd starting...");

    match Daemon::new() {
        Ok(daemon) => {
            if let Err(e) = daemon.run().await {
                log::error!("daemon error: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            log::error!("failed to start daemon: {e}");
            std::process::exit(1);
        }
    }
}
