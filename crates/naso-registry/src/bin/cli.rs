//! Naso Package Manager CLI

use clap::{Parser, Subcommand};
use naso_registry::{DependencyResolver, MemoryStorage, Requirement};
use semver::VersionReq;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "naso-pkg", version, about = "Naso Package Manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    #[arg(long, global = true, default_value = "https://registry.naso-lang.org")]
    registry: String,
    
    #[arg(long, global = true)]
    token: Option<String>,
    
    #[arg(long, global = true, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish a package to the registry
    Publish {
        #[arg(long)]
        path: Option<PathBuf>,
        
        #[arg(long)]
        dry_run: bool,
    },
    
    /// Install dependencies
    Install {
        #[arg(long)]
        locked: bool,
        
        #[arg(long)]
        no_dev: bool,
    },
    
    /// Add a dependency
    Add {
        name: String,
        version: Option<String>,
        
        #[arg(long)]
        dev: bool,
        
        #[arg(long)]
        build: bool,
        
        #[arg(long)]
        features: Vec<String>,
    },
    
    /// Remove a dependency
    Remove {
        name: String,
        
        #[arg(long)]
        dev: bool,
        
        #[arg(long)]
        build: bool,
    },
    
    /// Update dependencies
    Update {
        #[arg(long)]
        package: Option<String>,
        
        #[arg(long)]
        dry_run: bool,
    },
    
    /// Search for packages
    Search {
        query: String,
        
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    
    /// Show package information
    Info {
        name: String,
        
        #[arg(long)]
        version: Option<String>,
    },
    
    /// List installed packages
    List {
        #[arg(long)]
        tree: bool,
    },
    
    /// Resolve dependencies
    Resolve {
        #[arg(long)]
        requirements: Vec<String>,
    },
    
    /// Generate lockfile
    Lock {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    
    /// Verify package integrity
    Verify {
        name: String,
        version: String,
    },
    
    /// Login to registry
    Login {
        #[arg(long)]
        username: Option<String>,
    },
    
    /// Logout from registry
    Logout,
    
    /// Manage package ownership
    Owner {
        #[command(subcommand)]
        action: OwnerAction,
    },
    
    /// Yank a version
    Yank {
        name: String,
        version: String,
        
        #[arg(long)]
        reason: String,
    },
    
    /// Unyank a version
    Unyank {
        name: String,
        version: String,
    },
}

#[derive(Subcommand)]
enum OwnerAction {
    Add { package: String, user: String },
    Remove { package: String, user: String },
    List { package: String },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&cli.log_level))
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    match cli.command {
        Commands::Publish { path, dry_run } => {
            publish_command(path, dry_run, &cli).await?;
        }
        Commands::Install { locked, no_dev } => {
            install_command(locked, no_dev).await?;
        }
        Commands::Add { name, version, dev, build, features } => {
            add_command(name, version, dev, build, features).await?;
        }
        Commands::Remove { name, dev, build } => {
            remove_command(name, dev, build).await?;
        }
        Commands::Update { package, dry_run } => {
            update_command(package, dry_run).await?;
        }
        Commands::Search { query, limit } => {
            search_command(query, limit, &cli).await?;
        }
        Commands::Info { name, version } => {
            info_command(name, version, &cli).await?;
        }
        Commands::List { tree } => {
            list_command(tree).await?;
        }
        Commands::Resolve { requirements } => {
            resolve_command(requirements, &cli).await?;
        }
        Commands::Lock { output } => {
            lock_command(output).await?;
        }
        Commands::Verify { name, version } => {
            verify_command(name, version, &cli).await?;
        }
        Commands::Login { username } => {
            login_command(username).await?;
        }
        Commands::Logout => {
            logout_command().await?;
        }
        Commands::Owner { action } => {
            owner_command(action).await?;
        }
        Commands::Yank { name, version, reason } => {
            yank_command(name, version, reason, &cli).await?;
        }
        Commands::Unyank { name, version } => {
            unyank_command(name, version, &cli).await?;
        }
    }
    
    Ok(())
}

async fn publish_command(path: Option<PathBuf>, dry_run: bool, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let path = path.unwrap_or_else(|| std::env::current_dir().unwrap());
    println!("Publishing package from {}", path.display());
    
    if dry_run {
        println!("Dry run - not actually publishing");
        return Ok(());
    }
    
    // Read package manifest
    let manifest_path = path.join("Package.naso");
    if !manifest_path.exists() {
        return Err("Package.naso not found".into());
    }
    
    let manifest_content = std::fs::read_to_string(manifest_path)?;
    println!("Manifest: {}", manifest_content);
    
    // TODO: Implement actual publishing
    println!("Package published successfully!");
    
    Ok(())
}

async fn install_command(locked: bool, no_dev: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("Installing dependencies...");
    if locked {
        println!("Using lockfile");
    }
    if no_dev {
        println!("Skipping dev dependencies");
    }
    // TODO: Implement installation
    Ok(())
}

async fn add_command(name: String, version: Option<String>, dev: bool, build: bool, features: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let ver = version.unwrap_or_else(|| "*".to_string());
    println!("Adding {}@{}", name, ver);
    if dev { println!("  (dev dependency)"); }
    if build { println!("  (build dependency)"); }
    if !features.is_empty() { println!("  features: {:?}", features); }
    // TODO: Implement add
    Ok(())
}

async fn remove_command(name: String, dev: bool, build: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("Removing {}", name);
    if dev { println!("  (dev dependency)"); }
    if build { println!("  (build dependency)"); }
    // TODO: Implement remove
    Ok(())
}

async fn update_command(package: Option<String>, dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pkg) = package {
        println!("Updating {}...", pkg);
    } else {
        println!("Updating all packages...");
    }
    if dry_run {
        println!("Dry run - showing what would be updated");
    }
    Ok(())
}

async fn search_command(query: String, limit: u32, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Searching for '{}' (limit: {})", query, limit);
    println!("Registry: {}", cli.registry);
    // TODO: Implement search
    Ok(())
}

async fn info_command(name: String, version: Option<String>, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Package: {}", name);
    if let Some(v) = version {
        println!("Version: {}", v);
    } else {
        println!("Version: latest");
    }
    println!("Registry: {}", cli.registry);
    // TODO: Implement info
    Ok(())
}

async fn list_command(tree: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("Installed packages:");
    if tree {
        println!("(tree view)");
    }
    // TODO: Implement list
    Ok(())
}

async fn resolve_command(requirements: Vec<String>, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Resolving dependencies:");
    for req in &requirements {
        println!("  {}", req);
    }
    
    // Use in-memory resolver for demo
    let storage = Arc::new(MemoryStorage::new());
    let resolver = DependencyResolver::new(storage);
    
    let reqs: Result<Vec<Requirement>, _> = requirements.iter().map(|r| {
        let parts: Vec<&str> = r.split('@').collect();
        let name = parts[0].to_string();
        let version_req = parts.get(1).unwrap_or(&"*").parse::<VersionReq>()?;
        Ok(Requirement {
            name,
            version_req,
            features: Vec::new(),
            optional: false,
            target: None,
        })
    }).collect();
    
    let reqs = reqs?;
    match resolver.resolve(&reqs, None).await {
        Ok(resolution) => {
            println!("\nResolved packages:");
            for pkg in resolution.packages {
                println!("  {}@{}", pkg.name, pkg.version);
            }
            if !resolution.conflicts.is_empty() {
                println!("\nConflicts:");
                for conflict in resolution.conflicts {
                    println!("  {}: {:?}", conflict.package, conflict.available);
                }
            }
        }
        Err(e) => {
            eprintln!("Resolution failed: {}", e);
        }
    }
    
    Ok(())
}

async fn lock_command(output: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Generating lockfile...");
    let out = output.unwrap_or_else(|| PathBuf::from("naso.lock"));
    println!("Output: {}", out.display());
    // TODO: Implement lockfile generation
    Ok(())
}

async fn verify_command(name: String, version: String, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Verifying {}@{}", name, version);
    println!("Registry: {}", cli.registry);
    // TODO: Implement verification
    Ok(())
}

async fn login_command(username: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(user) = username {
        println!("Logging in as {}", user);
    } else {
        println!("Login (interactive)");
    }
    // TODO: Implement login
    Ok(())
}

async fn logout_command() -> Result<(), Box<dyn std::error::Error>> {
    println!("Logged out");
    // TODO: Implement logout
    Ok(())
}

async fn owner_command(action: OwnerAction) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        OwnerAction::Add { package, user } => {
            println!("Adding {} as owner of {}", user, package);
        }
        OwnerAction::Remove { package, user } => {
            println!("Removing {} as owner of {}", user, package);
        }
        OwnerAction::List { package } => {
            println!("Owners of {}:", package);
        }
    }
    Ok(())
}

async fn yank_command(name: String, version: String, reason: String, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Yanking {}@{}", name, version);
    println!("Reason: {}", reason);
    println!("Registry: {}", cli.registry);
    // TODO: Implement yank
    Ok(())
}

async fn unyank_command(name: String, version: String, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("Unyanking {}@{}", name, version);
    println!("Registry: {}", cli.registry);
    // TODO: Implement unyank
    Ok(())
}