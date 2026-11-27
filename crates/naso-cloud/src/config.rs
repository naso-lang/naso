//! Configuration management for Naso Cloud Runtime

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Configuration source
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Environment variables
    Environment,
    /// File (JSON, YAML, TOML)
    File(String),
    /// Kubernetes ConfigMap
    K8sConfigMap { name: String, namespace: Option<String> },
    /// Kubernetes Secret
    K8sSecret { name: String, namespace: Option<String> },
    /// Consul
    Consul(String),
    /// Etcd
    Etcd(Vec<String>),
    /// Vault
    Vault(String),
    /// Remote HTTP endpoint
    Http { url: String, headers: HashMap<String, String> },
}

/// Configuration manager
pub struct ConfigManager {
    sources: Vec<ConfigSource>,
    cache: HashMap<String, String>,
    watchers: HashMap<String, Vec<Box<dyn Fn(String) + Send + Sync>>>,
}

impl ConfigManager {
    pub fn new() -> Self {
        ConfigManager {
            sources: Vec::new(),
            cache: HashMap::new(),
            watchers: HashMap::new(),
        }
    }

    pub fn add_source(mut self, source: ConfigSource) -> Self {
        self.sources.push(source);
        self
    }

    pub fn load(&mut self) -> Result<()> {
        let sources = self.sources.clone();
        for source in &sources {
            self.load_source(source)?;
        }
        Ok(())
    }

    fn load_source(&mut self, source: &ConfigSource) -> Result<()> {
        match source {
            ConfigSource::Environment => {
                for (key, value) in std::env::vars() {
                    self.cache.insert(key, value);
                }
            }
            ConfigSource::File(path) => {
                self.load_file(path)?;
            }
            ConfigSource::K8sConfigMap { name, namespace } => {
                // Would load from K8s API
            }
            ConfigSource::K8sSecret { name, namespace } => {
                // Would load from K8s API
            }
            _ => return Err(CloudError::NotImplemented(format!("Config source not implemented: {:?}", source))),
        }
        Ok(())
    }

    fn load_file(&mut self, path: &str) -> Result<()> {
        let content = std::fs::read_to_string(path)
            .map_err(|_| CloudError::ConfigFileNotFound(path.into()))?;

        let ext = Path::new(path).extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match ext {
            "json" => self.parse_json(&content)?,
            "yaml" | "yml" => self.parse_yaml(&content)?,
            "toml" => self.parse_toml(&content)?,
            _ => return Err(CloudError::ConfigFileNotFound(format!("Unsupported format: {}", path))),
        }
        Ok(())
    }

    fn parse_json(&mut self, content: &str) -> Result<()> {
        let value: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.flatten_json("", &value);
        Ok(())
    }

    fn parse_yaml(&mut self, content: &str) -> Result<()> {
        let value: serde_json::Value = serde_yaml::from_str(content)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.flatten_json("", &value);
        Ok(())
    }

    fn parse_toml(&mut self, content: &str) -> Result<()> {
        let value: toml::Value = content.parse()
            .map_err(|e: toml::de::Error| CloudError::SerializationError(e.to_string()))?;
        let json = serde_json::to_value(value)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        self.flatten_json("", &json);
        Ok(())
    }

    fn flatten_json(&mut self, prefix: &str, value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, val) in map {
                    let new_prefix = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    self.flatten_json(&new_prefix, val);
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let new_prefix = format!("{}.{}", prefix, i);
                    self.flatten_json(&new_prefix, val);
                }
            }
            _ => {
                self.cache.insert(prefix.into(), value.to_string());
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.cache.get(key)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.cache.get(key).cloned()
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.cache.get(key).and_then(|v| v.parse().ok())
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.cache.get(key).and_then(|v| v.parse().ok())
    }

    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.cache.get(key).and_then(|v| v.parse().ok())
    }

    pub fn get_duration(&self, key: &str) -> Option<Duration> {
        self.cache.get(key).and_then(|v| {
            humantime::parse_duration(v).ok()
        })
    }

    pub fn require(&self, key: &str) -> Result<String> {
        self.cache.get(key).cloned().ok_or(CloudError::ConfigMissing(key.into()))
    }

    pub fn require_int(&self, key: &str) -> Result<i64> {
        self.require(key)?.parse().map_err(|_| CloudError::ConfigMissing(key.into()))
    }

    pub fn watch(&mut self, key: &str, callback: impl Fn(String) + Send + Sync + 'static) {
        self.watchers.entry(key.into()).or_default().push(Box::new(callback));
    }

    pub fn all(&self) -> &HashMap<String, String> {
        &self.cache
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Typed configuration using serde
pub trait Configurable: for<'de> Deserialize<'de> {
    fn from_manager(manager: &ConfigManager) -> Result<Self> {
        let mut map = HashMap::new();
        for (k, v) in manager.all() {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        let value = serde_json::to_value(map)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        Ok(serde_json::from_value(value)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?)
    }

    fn from_manager_prefix(manager: &ConfigManager, prefix: &str) -> Result<Self> {
        let mut map = HashMap::new();
        for (k, v) in manager.all() {
            if k.starts_with(prefix) {
                let new_key = k[prefix.len()..].trim_start_matches('.').to_string();
                map.insert(new_key, serde_json::Value::String(v.clone()));
            }
        }
        let value = serde_json::to_value(map)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        Ok(serde_json::from_value(value)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
    pub max_connections: Option<u32>,
    pub ssl_mode: Option<String>,
}

impl DatabaseConfig {
    pub fn url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.database
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: Option<usize>,
    pub keep_alive: Option<Duration>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub database: Option<u8>,
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub metrics_enabled: bool,
    pub metrics_port: Option<u16>,
    pub tracing_enabled: bool,
    pub tracing_endpoint: Option<String>,
    pub logging_level: Option<String>,
    pub profiling_enabled: bool,
}