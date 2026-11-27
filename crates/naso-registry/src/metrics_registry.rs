//! Metrics for the Naso package registry

use metrics::{counter, gauge, histogram, register_counter, register_gauge, register_histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::OnceLock;

static METRICS_INIT: OnceLock<()> = OnceLock::new();

pub fn init_metrics() {
    METRICS_INIT.get_or_init(|| {
        // Register metrics
        register_counter!("naso_registry_requests_total", "Total number of requests");
        register_counter!("naso_registry_publish_total", "Total number of packages published");
        register_counter!("naso_registry_download_total", "Total number of package downloads");
        register_counter!("naso_registry_errors_total", "Total number of errors");
        register_counter!("naso_registry_auth_failures_total", "Total number of authentication failures");
        
        register_gauge!("naso_registry_packages_total", "Total number of packages");
        register_gauge!("naso_registry_versions_total", "Total number of package versions");
        register_gauge!("naso_registry_storage_bytes", "Total storage used in bytes");
        register_gauge!("naso_registry_active_mirrors", "Number of active mirrors");
        
        register_histogram!("naso_registry_request_duration_seconds", "Request duration in seconds");
        register_histogram!("naso_registry_package_size_bytes", "Package size in bytes");
        register_histogram!("naso_registry_resolve_duration_seconds", "Dependency resolution duration in seconds");
        
        // Initialize Prometheus exporter
        let _ = PrometheusBuilder::new()
            .with_http_listener(([0, 0, 0, 0], 9090).into())
            .install();
    });
}

pub fn record_request(method: &str, path: &str, status: u16, duration: f64) {
    counter!("naso_registry_requests_total", "method" => method.to_string(), "path" => path.to_string(), "status" => status.to_string()).increment(1);
    histogram!("naso_registry_request_duration_seconds", "method" => method.to_string(), "path" => path.to_string()).record(duration);
}

pub fn record_publish(package: &str, success: bool) {
    counter!("naso_registry_publish_total", "package" => package.to_string(), "success" => success.to_string()).increment(1);
}

pub fn record_download(package: &str, version: &str, size: u64) {
    counter!("naso_registry_download_total", "package" => package.to_string(), "version" => version.to_string()).increment(1);
    histogram!("naso_registry_package_size_bytes").record(size as f64);
}

pub fn record_error(error_type: &str) {
    counter!("naso_registry_errors_total", "type" => error_type.to_string()).increment(1);
}

pub fn record_auth_failure(reason: &str) {
    counter!("naso_registry_auth_failures_total", "reason" => reason.to_string()).increment(1);
}

pub fn update_package_count(count: u64) {
    gauge!("naso_registry_packages_total").set(count as f64);
}

pub fn update_version_count(count: u64) {
    gauge!("naso_registry_versions_total").set(count as f64);
}

pub fn update_storage_bytes(bytes: u64) {
    gauge!("naso_registry_storage_bytes").set(bytes as f64);
}

pub fn update_active_mirrors(count: u64) {
    gauge!("naso_registry_active_mirrors").set(count as f64);
}

pub fn record_resolve_duration(duration: f64) {
    histogram!("naso_registry_resolve_duration_seconds").record(duration);
}

pub struct MetricsRegistry;

impl MetricsRegistry {
    pub fn new() -> Self {
        init_metrics();
        Self
    }
    
    pub fn render(&self) -> String {
        // In a real implementation, this would use the Prometheus encoder
        // For now, return a placeholder
        "# Metrics endpoint - use Prometheus exporter on port 9090\n".to_string()
    }
}

pub fn create() -> MetricsRegistry {
    MetricsRegistry::new()
}