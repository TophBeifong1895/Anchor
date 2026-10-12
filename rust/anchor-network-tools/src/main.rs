use anchor_network_tools::NetworkServer;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 2 && arguments[0] == "package-plugin" {
        anchor_network_tools::package_plugin(&arguments[1])?;
        return Ok(());
    }
    if !arguments.is_empty() {
        return Err("usage: anchor-network-tools [package-plugin <destination>]".into());
    }
    let workspace = std::env::var_os("ANCHOR_NETWORK_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    NetworkServer::new(workspace)?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
