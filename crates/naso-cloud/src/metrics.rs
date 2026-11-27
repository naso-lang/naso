//! Metrics for Naso Cloud Runtime

use super::{CloudError, Result};
use prometheus::{Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry, Encoder, TextEncoder};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Metrics registry wrapper
pub struct MetricsRegistry {
    registry: Registry,
    counters: HashMap<String, CounterVec>,
    gauges: HashMap<String, GaugeVec>,
    histograms: HashMap<String, HistogramVec>,
}

impl MetricsRegistry {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();
        Ok(MetricsRegistry {
            registry,
            counters: HashMap::new(),
            gauges: HashMap::new(),
            histograms: HashMap::new(),
        })
    }

    pub fn counter(&mut self, name: &str, help: &str, labels: &[(&str, &str)]) -> Result<CounterVec> {
        let label_names: Vec<&str> = labels.iter().map(|(k, _)| *k).collect();
        let const_labels: HashMap<String, String> = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        
        let opts = Opts::new(name, help)
            .const_labels(const_labels);
        let counter = CounterVec::new(opts, &label_names)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.registry.register(Box::new(counter.clone()))
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.counters.insert(name.into(), counter.clone());
        Ok(counter)
    }

    pub fn gauge(&mut self, name: &str, help: &str, labels: &[(&str, &str)]) -> Result<GaugeVec> {
        let label_names: Vec<&str> = labels.iter().map(|(k, _)| *k).collect();
        let const_labels: HashMap<String, String> = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        
        let opts = Opts::new(name, help)
            .const_labels(const_labels);
        let gauge = GaugeVec::new(opts, &label_names)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.registry.register(Box::new(gauge.clone()))
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.gauges.insert(name.into(), gauge.clone());
        Ok(gauge)
    }

    pub fn histogram(&mut self, name: &str, help: &str, labels: &[(&str, &str)], buckets: &[f64]) -> Result<HistogramVec> {
        let label_names: Vec<&str> = labels.iter().map(|(k, _)| *k).collect();
        let const_labels: HashMap<String, String> = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        
        let opts = HistogramOpts::new(name, help)
            .buckets(buckets.to_vec())
            .const_labels(const_labels);
        let histogram = HistogramVec::new(opts, &label_names)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.registry.register(Box::new(histogram.clone()))
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.histograms.insert(name.into(), histogram.clone());
        Ok(histogram)
    }

    pub fn gather(&self) -> Result<String> {
        let metric_families = self.registry.gather();
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        String::from_utf8(buffer).map_err(|e| CloudError::SerializationError(e.to_string()))
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

/// RED metrics (Rate, Errors, Duration)
pub struct RedMetrics {
    requests_total: CounterVec,
    requests_duration: HistogramVec,
    errors_total: CounterVec,
}

impl RedMetrics {
    pub fn new(registry: &mut MetricsRegistry, subsystem: &str) -> Result<Self> {
        let requests_total = registry.counter(
            &format!("{}_requests_total", subsystem),
            "Total number of requests",
            &[("method", ""), ("endpoint", ""), ("status", "")],
        )?;

        let requests_duration = registry.histogram(
            &format!("{}_request_duration_seconds", subsystem),
            "Request duration in seconds",
            &[("method", ""), ("endpoint", "")],
            &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
        )?;

        let errors_total = registry.counter(
            &format!("{}_errors_total", subsystem),
            "Total number of errors",
            &[("method", ""), ("endpoint", ""), ("error_type", "")],
        )?;

        Ok(RedMetrics {
            requests_total,
            requests_duration,
            errors_total,
        })
    }

    pub fn record_request(&self, method: &str, endpoint: &str, status: u16, duration_secs: f64) {
        self.requests_total.with_label_values(&[method, endpoint, &status.to_string()]).inc();
        self.requests_duration.with_label_values(&[method, endpoint]).observe(duration_secs);

        if status >= 400 {
            let error_type = if status >= 500 { "server_error" } else { "client_error" };
            self.errors_total.with_label_values(&[method, endpoint, error_type]).inc();
        }
    }
}

/// USE metrics (Utilization, Saturation, Errors)
pub struct UseMetrics {
    utilization: GaugeVec,
    saturation: GaugeVec,
    errors: CounterVec,
}

impl UseMetrics {
    pub fn new(registry: &mut MetricsRegistry, resource: &str) -> Result<Self> {
        let utilization = registry.gauge(
            &format!("{}_utilization", resource),
            "Resource utilization (0-1)",
            &[],
        )?;

        let saturation = registry.gauge(
            &format!("{}_saturation", resource),
            "Resource saturation (queue length, etc.)",
            &[],
        )?;

        let errors = registry.counter(
            &format!("{}_errors_total", resource),
            "Total number of resource errors",
            &[("error_type", "")],
        )?;

        Ok(UseMetrics {
            utilization,
            saturation,
            errors,
        })
    }

    pub fn set_utilization(&self, value: f64) {
        self.utilization.with_label_values(&[]).set(value);
    }

    pub fn set_saturation(&self, value: f64) {
        self.saturation.with_label_values(&[]).set(value);
    }

    pub fn record_error(&self, error_type: &str) {
        self.errors.with_label_values(&[error_type]).inc();
    }
}

/// HTTP metrics middleware
pub struct HttpMetrics {
    red: RedMetrics,
}

impl HttpMetrics {
    pub fn new(registry: &mut MetricsRegistry) -> Result<Self> {
        Ok(HttpMetrics {
            red: RedMetrics::new(registry, "http")?,
        })
    }

    pub fn record(&self, method: &str, path: &str, status: u16, duration: Duration) {
        self.red.record_request(method, path, status, duration.as_secs_f64());
    }
}

/// Runtime metrics
pub struct RuntimeMetrics {
    tasks_spawned: CounterVec,
    tasks_completed: CounterVec,
    tasks_failed: CounterVec,
    worker_count: GaugeVec,
    queue_depth: GaugeVec,
    cpu_usage: GaugeVec,
    memory_usage: GaugeVec,
}

impl RuntimeMetrics {
    pub fn new(registry: &mut MetricsRegistry) -> Result<Self> {
        let tasks_spawned = registry.counter("naso_tasks_spawned_total", "Total tasks spawned", &[])?;
        let tasks_completed = registry.counter("naso_tasks_completed_total", "Total tasks completed", &[])?;
        let tasks_failed = registry.counter("naso_tasks_failed_total", "Total tasks failed", &[])?;
        let worker_count = registry.gauge("naso_worker_count", "Number of worker threads", &[])?;
        let queue_depth = registry.gauge("naso_queue_depth", "Task queue depth", &[])?;
        let cpu_usage = registry.gauge("naso_cpu_usage_percent", "CPU usage percent", &[])?;
        let memory_usage = registry.gauge("naso_memory_usage_bytes", "Memory usage in bytes", &[])?;

        Ok(RuntimeMetrics {
            tasks_spawned,
            tasks_completed,
            tasks_failed,
            worker_count,
            queue_depth,
            cpu_usage,
            memory_usage,
        })
    }

    pub fn task_spawned(&self) {
        self.tasks_spawned.with_label_values(&[]).inc();
    }

    pub fn task_completed(&self) {
        self.tasks_completed.with_label_values(&[]).inc();
    }

    pub fn task_failed(&self) {
        self.tasks_failed.with_label_values(&[]).inc();
    }

    pub fn set_workers(&self, count: usize) {
        self.worker_count.with_label_values(&[]).set(count as f64);
    }

    pub fn set_queue_depth(&self, depth: usize) {
        self.queue_depth.with_label_values(&[]).set(depth as f64);
    }

    pub fn set_cpu_usage(&self, percent: f64) {
        self.cpu_usage.with_label_values(&[]).set(percent);
    }

    pub fn set_memory_usage(&self, bytes: u64) {
        self.memory_usage.with_label_values(&[]).set(bytes as f64);
    }
}

/// Metrics exporter for Prometheus
pub struct PrometheusExporter {
    registry: Registry,
    bind_address: String,
}

impl PrometheusExporter {
    pub fn new(registry: Registry, bind_address: &str) -> Self {
        PrometheusExporter {
            registry,
            bind_address: bind_address.into(),
        }
    }

    pub async fn serve(&self) -> Result<()> {
        // Would start HTTP server on /metrics
        Ok(())
    }
}