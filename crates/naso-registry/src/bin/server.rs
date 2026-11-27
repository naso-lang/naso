//! Naso Package Registry Server Binary

use clap::Parser;
use naso_registry::{run_server, RegistryConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "naso-registry", version, about = "Naso Package Registry Server")]
struct Args {
    #[arg(long, env = "NASO_DB_URL", default_value = "postgresql://postgres:postgres@localhost/naso_registry")]
    database_url: String,
    
    #[arg(long, env = "NASO_MAX_PACKAGE_SIZE", default_value = "104857600")]
    max_package_size: usize,
    
    #[arg(long, env = "NASO_MAX_VERSIONS", default_value = "1000")]
    max_versions_per_package: usize,
    
    #[arg(long, env = "NASO_RETENTION_DAYS", default_value = "3650")]
    retention_days: i64,
    
    #[arg(long, env = "NASO_ENABLE_MIRRORS", default_value = "true")]
    enable_mirrors: bool,
    
    #[arg(long, env = "NASO_CDN_URL")]
    cdn_base_url: Option<String>,
    
    #[arg(long, env = "NASO_REQUIRE_SIGNED", default_value = "false")]
    require_signed: bool,
    
    #[arg(long, env = "NASO_SBOM_REQUIRED", default_value = "true")]
    sbom_required: bool,
    
    #[arg(long, env = "NASO_JWT_SECRET")]
    jwt_secret: Option<String>,
    
    #[arg(long, default_value = "0.0.0.0:8080")]
    bind: String,
    
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&args.log_level))
        .with(tracing_subscriber::fmt::layer().json())
        .init();
    
    if let Some(secret) = args.jwt_secret {
        std::env::set_var("NASO_JWT_SECRET", secret);
    }
    
    let config = RegistryConfig {
        max_package_size: args.max_package_size,
        max_versions_per_package: args.max_versions_per_package,
        retention_days: args.retention_days,
        enable_mirrors: args.enable_mirrors,
        cdn_base_url: args.cdn_base_url,
        require_signed: args.require_signed,
        sbom_required: args.sbom_required,
    };
    
    tracing::info!("Starting Naso Package Registry");
    tracing::info!("Database: {}", args.database_url);
    tracing::info!("Bind: {}", args.bind);
    
    run_server(config, &args.database_url).await?;
    
    Ok(())
}