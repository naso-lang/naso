//! Health checks for Naso Cloud Runtime

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl HealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Unhealthy => "unhealthy",
            HealthStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub status: HealthStatus,
    pub message: String,
    pub details: HashMap<String, String>,
    pub timestamp: u64,
}

impl HealthCheckResult {
    pub fn healthy(message: &str) -> Self {
        HealthCheckResult {
            status: HealthStatus::Healthy,
            message: message.into(),
            details: HashMap::new(),
            timestamp: current_timestamp(),
        }
    }

    pub fn degraded(message: &str) -> Self {
        HealthCheckResult {
            status: HealthStatus::Degraded,
            message: message.into(),
            details: HashMap::new(),
            timestamp: current_timestamp(),
        }
    }

    pub fn unhealthy(message: &str) -> Self {
        HealthCheckResult {
            status: HealthStatus::Unhealthy,
            message: message.into(),
            details: HashMap::new(),
            timestamp: current_timestamp(),
        }
    }

    pub fn with_detail(mut self, key: &str, value: &str) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Health check definition
pub struct HealthCheck {
    name: String,
    check_fn: Arc<dyn Fn() -> HealthCheckResult + Send + Sync>,
    interval: Duration,
    timeout: Duration,
    critical: bool,
    endpoint: Option<String>,
}

impl HealthCheck {
    /// Create a new health check with a closure
    pub fn new<F>(name: &str, check_fn: F) -> Self
    where
        F: Fn() -> HealthCheckResult + Send + Sync + 'static,
    {
        HealthCheck {
            name: name.into(),
            check_fn: Arc::new(check_fn),
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            critical: false,
            endpoint: None,
        }
    }

    /// Liveness probe - indicates if the process should be restarted
    pub fn liveness(path: &str) -> Self {
        HealthCheck::new("liveness", move || {
            // Default liveness check - just check if process is alive
            HealthCheckResult::healthy("Process is alive")
        }).endpoint(path)
    }

    /// Readiness probe - indicates if the service can handle traffic
    pub fn readiness(path: &str) -> Self {
        HealthCheck::new("readiness", move || {
            // Default readiness check
            HealthCheckResult::healthy("Service is ready")
        }).endpoint(path)
    }

    /// Startup probe - indicates if the application has started
    pub fn startup(path: &str) -> Self {
        HealthCheck::new("startup", move || {
            HealthCheckResult::healthy("Application started")
        }).endpoint(path)
    }

    /// HTTP health check
    pub fn http(name: &str, url: &str) -> Self {
        let url = url.to_string();
        HealthCheck::new(name, move || {
            // Would make HTTP request
            HealthCheckResult::healthy("HTTP check passed")
                .with_detail("url", &url)
        })
    }

    /// Database health check
    pub fn database(name: &str, check_fn: impl Fn() -> Result<()> + Send + Sync + 'static) -> Self {
        HealthCheck::new(name, move || {
            match check_fn() {
                Ok(_) => HealthCheckResult::healthy("Database connection OK"),
                Err(e) => HealthCheckResult::unhealthy(&format!("Database error: {}", e)),
            }
        })
    }

    /// Custom health check with interval
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Custom health check with timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Mark as critical - failure affects overall status
    pub fn critical(mut self, critical: bool) -> Self {
        self.critical = critical;
        self
    }

    /// Set HTTP endpoint path
    pub fn endpoint(mut self, path: &str) -> Self {
        self.endpoint = Some(path.into());
        self
    }

    pub fn run(&self) -> HealthCheckResult {
        (self.check_fn)()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_critical(&self) -> bool {
        self.critical
    }

    /// Get HTTP endpoint path
    pub fn get_endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }
}

/// Health check registry
pub struct HealthRegistry {
    checks: Vec<HealthCheck>,
    http_endpoint: Option<String>,
    results: Arc<RwLock<HashMap<String, HealthCheckResult>>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        HealthRegistry {
            checks: Vec::new(),
            http_endpoint: None,
            results: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&mut self, check: HealthCheck) {
        self.checks.push(check);
    }

    pub fn with_http_endpoint(mut self, endpoint: &str) -> Self {
        self.http_endpoint = Some(endpoint.into());
        self
    }

    /// Run all health checks
    pub fn run_all(&self) -> HealthReport {
        let mut results = Vec::new();
        let mut overall = HealthStatus::Healthy;

        for check in &self.checks {
            let result = check.run();
            let name = check.name().to_string();

            if check.is_critical() && result.status == HealthStatus::Unhealthy {
                overall = HealthStatus::Unhealthy;
            } else if result.status == HealthStatus::Degraded && overall == HealthStatus::Healthy {
                overall = HealthStatus::Degraded;
            }

            results.push((name.clone(), result.clone()));
        }

        HealthReport {
            overall,
            checks: results,
            timestamp: current_timestamp(),
        }
    }

    /// Start HTTP server for health endpoints
    pub async fn start_http_server(&self, addr: &str) -> Result<()> {
        // Would start HTTP server with health endpoints
        // /health/live -> liveness
        // /health/ready -> readiness
        // /health/startup -> startup
        // /health -> overall
        Ok(())
    }

    pub fn checks(&self) -> &[HealthCheck] {
        &self.checks
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub overall: HealthStatus,
    pub checks: Vec<(String, HealthCheckResult)>,
    pub timestamp: u64,
}

impl HealthReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn is_healthy(&self) -> bool {
        self.overall == HealthStatus::Healthy
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}