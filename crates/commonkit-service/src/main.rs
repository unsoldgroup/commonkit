use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use clap::Parser;
use commonkit_platform::AppPaths;
use commonkit_service::{BoundServer, ControlToken, EventHub, OverallState, ServiceStatus};
use serde_json::json;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(name = "commonkitd", version, about = "CommonKit local control daemon")]
struct Args {
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long, default_value_t = default_relay_port())]
    relay_port: u16,
}

fn default_relay_port() -> u16 {
    relay_port_from_env(std::env::var("COMMONKIT_RELAY_PORT").ok().as_deref())
}

fn relay_port_from_env(value: Option<&str>) -> u16 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Args::parse()).await {
        eprintln!("commonkitd: {error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let paths = AppPaths::discover()?;
    paths.create_private_roots()?;
    let token = ControlToken::load_or_create(&paths.config.join("control.token"))?;
    let status = Arc::new(RwLock::new(ServiceStatus {
        state: OverallState::Healthy,
        ..ServiceStatus::default()
    }));
    let events = EventHub::new(256);
    events.publish("status.changed", json!({"state": "healthy"}));
    let server = BoundServer::bind_with_relay_address(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.relay_port),
        token,
        status,
        events,
        paths.state.join("daemon.json"),
    )
    .await?;
    server
        .run_until(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::relay_port_from_env;

    #[test]
    fn unattended_daemon_uses_a_collision_safe_relay_port_unless_explicitly_configured() {
        assert_eq!(relay_port_from_env(None), 0);
        assert_eq!(relay_port_from_env(Some("")), 0);
        assert_eq!(relay_port_from_env(Some("not-a-port")), 0);
        assert_eq!(relay_port_from_env(Some("3764")), 3764);
    }
}
