mod cli;
mod protocol;
mod proxy;
mod rtc;
mod serve;
mod signaling;

use clap::Parser;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use cli::TurnConfig;

const MAX_RETRIES: u32 = u32::MAX; // Retry forever
const INITIAL_BACKOFF_SECS: u64 = 2;
const MAX_BACKOFF_SECS: u64 = 60; // Cap backoff at 60 seconds

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = cli::Cli::parse();

    match cli.command {
        cli::Command::Serve {
            signal,
            room,
            upstream,
            advertise,
            turn,
            turn_user,
            turn_pass,
        } => {
            let turn_config = TurnConfig {
                url: turn,
                username: turn_user,
                password: turn_pass,
            };
            info!(
                "starting serve mode: signal={}, room={}, upstream={}, advertise={}",
                signal, room, upstream, advertise
            );
            if let Some(ref url) = turn_config.url {
                info!("TURN server configured: {}", url);
            }

            run_with_retry("serve", MAX_RETRIES, || {
                let signal = signal.clone();
                let room = room.clone();
                let upstream = upstream.clone();
                let advertise = advertise.clone();
                let turn_config = turn_config.clone();
                async move {
                    let (_pc, dc_pair, _signaling) =
                        rtc::connect(&signal, &room, &turn_config).await?;
                    info!("WebRTC connected, starting serve...");
                    serve::run_serve(dc_pair, upstream, advertise).await
                }
            })
            .await?;
        }

        cli::Command::Proxy {
            signal,
            room,
            listen,
            turn,
            turn_user,
            turn_pass,
        } => {
            let turn_config = TurnConfig {
                url: turn,
                username: turn_user,
                password: turn_pass,
            };
            info!(
                "starting proxy mode: signal={}, room={}, listen={}",
                signal, room, listen
            );
            if let Some(ref url) = turn_config.url {
                info!("TURN server configured: {}", url);
            }

            run_with_retry("proxy", MAX_RETRIES, || {
                let signal = signal.clone();
                let room = room.clone();
                let listen = listen.clone();
                let turn_config = turn_config.clone();
                async move {
                    let (_pc, dc_pair, _signaling) =
                        rtc::connect(&signal, &room, &turn_config).await?;
                    info!("WebRTC connected, starting proxy...");
                    proxy::run_proxy(dc_pair, listen).await
                }
            })
            .await?;
        }
    }

    Ok(())
}

/// Run a tunnel function with retry logic and exponential backoff.
/// On failure, waits and retries up to `max_retries` times.
/// Ctrl+C (tokio shutdown) will break out of the retry loop.
async fn run_with_retry<F, Fut>(mode: &str, max_retries: u32, make_future: F) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let mut attempt = 0u32;

    loop {
        match make_future().await {
            Ok(()) => {
                info!("{} completed normally", mode);
                return Ok(());
            }
            Err(e) => {
                attempt += 1;
                if attempt > max_retries {
                    error!(
                        "{} failed after {} attempts, giving up: {}",
                        mode, max_retries, e
                    );
                    return Err(e);
                }

                let backoff = (INITIAL_BACKOFF_SECS * 2u64.pow(attempt.min(10) - 1)).min(MAX_BACKOFF_SECS);
                warn!(
                    "{} failed (attempt {}): {}. Retrying in {}s...",
                    mode, attempt, e, backoff
                );

                // Use tokio::select to allow Ctrl+C to interrupt the backoff sleep
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff)) => {}
                    _ = tokio::signal::ctrl_c() => {
                        info!("received Ctrl+C during retry backoff, exiting");
                        return Err(anyhow::anyhow!("interrupted by user"));
                    }
                }
            }
        }
    }
}
