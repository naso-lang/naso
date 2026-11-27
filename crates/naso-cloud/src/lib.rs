//! Naso Cloud Runtime
//!
//! Cloud-native runtime for Naso applications running on Kubernetes, VMs, and serverless platforms.
//! Provides service discovery, configuration management, health checks, graceful shutdown,
//! and enterprise-grade observability.

pub mod config;
#[cfg(feature = "k8s")]
pub mod discovery;
pub mod health;
#[cfg(feature = "k8s")]
pub mod kubernetes;
pub mod metrics;
pub mod serverless;
pub mod shutdown;
pub mod vm;

use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CloudError {
    #[error("Kubernetes config not found: {0}")]
    K8sConfigNotFound(String),

    #[error("Kubernetes client creation failed: {0}")]
    K8sClientCreationFailed(String),

    #[error("Kubernetes API request failed: {0}")]
    K8sRequestFailed(String),

    #[error("VM metadata not found: {0}")]
    VmMetadataNotFound(String),

    #[error("Configuration file not found: {0}")]
    ConfigFileNotFound(String),

    #[error("Required config missing: {0}")]
    ConfigMissing(String),

    #[error("Not configured: {0}")]
    NotConfigured(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Leader election failed: {0}")]
    LeaderElectionFailed(String),

    #[error("Shutdown registration failed: {0}")]
    ShutdownRegistrationFailed(String),

    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Service discovery failed: {0}")]
    ServiceDiscoveryFailed(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, CloudError>;

// Re-export key types
pub use config::{ConfigManager, ConfigSource};
#[cfg(feature = "k8s")]
pub use discovery::{ServiceDiscovery, ServiceInstance, ServiceDiscoveryBackend};
pub use health::{HealthRegistry, HealthCheck, HealthStatus, HealthReport};
#[cfg(feature = "k8s")]
pub use kubernetes::{K8sClient, K8sConfig, PodInfo, ServiceEndpoints, ConfigMap, Secret, LeaderLock};
pub use shutdown::{ShutdownManager, ShutdownSignal};
pub use vm::VmMetadata;
pub use serverless::{ServerlessRuntime, ServerlessPlatform, Request, Response};

/// Detect the current cloud provider
pub fn detect_cloud_provider() -> CloudProvider {
    // Check for Kubernetes first
    if std::path::Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token").exists() {
        return CloudProvider::Kubernetes;
    }

    // Check cloud metadata endpoints (would do actual HTTP requests)
    CloudProvider::Unknown
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    Kubernetes,
    Aws,
    Gcp,
    Azure,
    DigitalOcean,
    Hetzner,
    BareMetal,
    Unknown,
}

/// Cloud runtime builder for easy configuration
#[cfg(feature = "k8s")]
pub struct CloudRuntimeBuilder {
    k8s_config: Option<K8sConfig>,
    health_checks: Vec<HealthCheck>,
    shutdown_handlers: Vec<Box<dyn FnOnce() + Send + 'static>>,
    service_discovery: Option<ServiceDiscoveryBackend>,
    config_sources: Vec<ConfigSource>,
    metrics_exporter: Option<String>,
    tracing_exporter: Option<String>,
}

/// Cloud runtime builder for non-K8s environments
#[cfg(not(feature = "k8s"))]
pub struct CloudRuntimeBuilder {
    health_checks: Vec<HealthCheck>,
    shutdown_handlers: Vec<Box<dyn FnOnce() + Send + 'static>>,
    config_sources: Vec<ConfigSource>,
    metrics_exporter: Option<String>,
    tracing_exporter: Option<String>,
}

impl CloudRuntimeBuilder {
    pub fn new() -> Self {
        #[cfg(feature = "k8s")]
        return CloudRuntimeBuilder {
            k8s_config: None,
            health_checks: Vec::new(),
            shutdown_handlers: Vec::new(),
            service_discovery: None,
            config_sources: Vec::new(),
            metrics_exporter: None,
            tracing_exporter: None,
        };

        #[cfg(not(feature = "k8s"))]
        return CloudRuntimeBuilder {
            health_checks: Vec::new(),
            shutdown_handlers: Vec::new(),
            config_sources: Vec::new(),
            metrics_exporter: None,
            tracing_exporter: None,
        };
    }

    #[cfg(feature = "k8s")]
    pub fn kubernetes(mut self, config: K8sConfig) -> Self {
        self.k8s_config = Some(config);
        self
    }

    #[cfg(feature = "k8s")]
    pub fn kubernetes_in_cluster(mut self) -> Result<Self> {
        self.k8s_config = Some(K8sConfig::in_cluster()?);
        Ok(self)
    }

    pub fn health_check(mut self, check: HealthCheck) -> Self {
        self.health_checks.push(check);
        self
    }

    pub fn on_shutdown(mut self, handler: impl FnOnce() + Send + 'static) -> Self {
        self.shutdown_handlers.push(Box::new(handler));
        self
    }

    #[cfg(feature = "k8s")]
    pub fn service_discovery(mut self, backend: ServiceDiscoveryBackend) -> Self {
        self.service_discovery = Some(backend);
        self
    }

    pub fn config_source(mut self, source: ConfigSource) -> Self {
        self.config_sources.push(source);
        self
    }

    pub fn metrics_exporter(mut self, endpoint: String) -> Self {
        self.metrics_exporter = Some(endpoint);
        self
    }

    pub fn tracing_exporter(mut self, endpoint: String) -> Self {
        self.tracing_exporter = Some(endpoint);
        self
    }

    pub fn build(self) -> Result<CloudRuntime> {
        #[cfg(feature = "k8s")]
        {
            let k8s_client = if let Some(config) = self.k8s_config {
                Some(K8sClient::new(config)?)
            } else {
                None
            };

            let mut health_registry = HealthRegistry::new();
            for check in self.health_checks {
                health_registry.register(check);
            }

            let mut shutdown_manager = ShutdownManager::new();
            for handler in self.shutdown_handlers {
                shutdown_manager.on_shutdown(handler);
            }
            shutdown_manager.register_signals()?;

            let config_manager = {
                let mut mgr = ConfigManager::new();
                for source in self.config_sources {
                    mgr = mgr.add_source(source);
                }
                mgr.load()?;
                mgr
            };

            let service_discovery = if let Some(backend) = self.service_discovery {
                match backend {
                    ServiceDiscoveryBackend::Kubernetes => {
                        if let Some(client) = k8s_client.clone() {
                            Some(ServiceDiscovery::kubernetes(client))
                        } else {
                            return Err(CloudError::NotConfigured("Kubernetes client required for K8s service discovery".into()));
                        }
                    }
                    _ => return Err(CloudError::NotImplemented("Service discovery backend not implemented".into())),
                }
            } else {
                None
            };

            Ok(CloudRuntime {
                k8s_client,
                health_registry,
                shutdown_manager,
                config_manager,
                service_discovery,
                metrics_exporter: self.metrics_exporter,
                tracing_exporter: self.tracing_exporter,
            })
        }

        #[cfg(not(feature = "k8s"))]
        {
            let mut health_registry = HealthRegistry::new();
            for check in self.health_checks {
                health_registry.register(check);
            }

            let mut shutdown_manager = ShutdownManager::new();
            for handler in self.shutdown_handlers {
                shutdown_manager.on_shutdown(handler);
            }
            shutdown_manager.register_signals()?;

            let config_manager = {
                let mut mgr = ConfigManager::new();
                for source in self.config_sources {
                    mgr = mgr.add_source(source);
                }
                mgr.load()?;
                mgr
            };

            Ok(CloudRuntime {
                health_registry,
                shutdown_manager,
                config_manager,
                metrics_exporter: self.metrics_exporter,
                tracing_exporter: self.tracing_exporter,
            })
        }
    }
}

impl Default for CloudRuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Main cloud runtime
#[cfg(feature = "k8s")]
pub struct CloudRuntime {
    k8s_client: Option<K8sClient>,
    health_registry: HealthRegistry,
    shutdown_manager: ShutdownManager,
    config_manager: ConfigManager,
    service_discovery: Option<ServiceDiscovery>,
    metrics_exporter: Option<String>,
    tracing_exporter: Option<String>,
}

/// Main cloud runtime (non-K8s)
#[cfg(not(feature = "k8s"))]
pub struct CloudRuntime {
    health_registry: HealthRegistry,
    shutdown_manager: ShutdownManager,
    config_manager: ConfigManager,
    metrics_exporter: Option<String>,
    tracing_exporter: Option<String>,
}

impl CloudRuntime {
    pub fn builder() -> CloudRuntimeBuilder {
        CloudRuntimeBuilder::new()
    }

    #[cfg(feature = "k8s")]
    pub fn k8s_client(&self) -> Option<&K8sClient> {
        self.k8s_client.as_ref()
    }

    pub fn health_registry(&self) -> &HealthRegistry {
        &self.health_registry
    }

    pub fn health_registry_mut(&mut self) -> &mut HealthRegistry {
        &mut self.health_registry
    }

    pub fn shutdown_manager(&self) -> &ShutdownManager {
        &self.shutdown_manager
    }

    pub fn shutdown_manager_mut(&mut self) -> &mut ShutdownManager {
        &mut self.shutdown_manager
    }

    pub fn config_manager(&self) -> &ConfigManager {
        &self.config_manager
    }

    pub fn config_manager_mut(&mut self) -> &mut ConfigManager {
        &mut self.config_manager
    }

    #[cfg(feature = "k8s")]
    pub fn service_discovery(&self) -> Option<&ServiceDiscovery> {
        self.service_discovery.as_ref()
    }

    /// Run health checks and return report
    pub fn check_health(&self) -> HealthReport {
        self.health_registry.run_all()
    }

    /// Start health check HTTP server
    pub async fn serve_health(&self, addr: &str) -> Result<()> {
        self.health_registry.start_http_server(addr).await
    }

    /// Wait for shutdown signal
    pub async fn wait_for_shutdown(&self) {
        self.shutdown_manager.wait().await;
    }

    /// Execute shutdown handlers
    pub fn execute_shutdown(&mut self) {
        self.shutdown_manager.execute();
    }

    /// Get configuration value
    pub fn config(&self, key: &str) -> Option<&String> {
        self.config_manager.get(key)
    }

    #[cfg(feature = "k8s")]
    /// Discover service instances
    pub async fn discover(&self, service: &str) -> Result<Vec<ServiceInstance>> {
        if let Some(sd) = &self.service_discovery {
            sd.discover(service).await
        } else {
            Err(CloudError::NotConfigured("Service discovery not configured".into()))
        }
    }
}

/// Quick setup for Kubernetes in-cluster deployment
#[cfg(feature = "k8s")]
pub async fn kubernetes_runtime() -> Result<CloudRuntime> {
    CloudRuntime::builder()
        .kubernetes_in_cluster()?
        .health_check(HealthCheck::liveness("/health/live"))
        .health_check(HealthCheck::readiness("/health/ready"))
        .health_check(HealthCheck::startup("/health/startup"))
        .config_source(ConfigSource::Environment)
        .config_source(ConfigSource::K8sConfigMap("app-config".into(), None))
        .config_source(ConfigSource::K8sSecret("app-secrets".into(), None))
        .service_discovery(ServiceDiscoveryBackend::Kubernetes)
        .build()
}

/// Quick setup for VM deployment
pub async fn vm_runtime() -> Result<CloudRuntime> {
    CloudRuntime::builder()
        .health_check(HealthCheck::liveness("/health/live"))
        .health_check(HealthCheck::readiness("/health/ready"))
        .config_source(ConfigSource::Environment)
        .config_source(ConfigSource::File("/etc/app/config.toml".into()))
        .build()
}

/// Quick setup for serverless (Cloud Run, Lambda, etc.)
pub fn serverless_runtime(platform: ServerlessPlatform) -> ServerlessRuntime {
    ServerlessRuntime::new(platform)
}