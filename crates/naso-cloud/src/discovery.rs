//! Service discovery for Naso Cloud Runtime

#![cfg(feature = "k8s")]

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceDiscoveryBackend {
    Kubernetes,
    Consul,
    Etcd,
    Dns,
    Static,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstance {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, String>,
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    pub http: Option<String>,
    pub tcp: Option<String>,
    pub interval: Duration,
    pub timeout: Duration,
    pub deregister_after: Duration,
}

pub struct ServiceDiscovery {
    backend: ServiceDiscoveryBackend,
    k8s_client: Option<crate::kubernetes::K8sClient>,
    consul_addr: Option<String>,
    etcd_endpoints: Vec<String>,
    static_services: HashMap<String, Vec<ServiceInstance>>,
}

impl ServiceDiscovery {
    pub fn kubernetes(client: crate::kubernetes::K8sClient) -> Self {
        ServiceDiscovery {
            backend: ServiceDiscoveryBackend::Kubernetes,
            k8s_client: Some(client),
            consul_addr: None,
            etcd_endpoints: Vec::new(),
            static_services: HashMap::new(),
        }
    }

    pub fn consul(addr: &str) -> Self {
        ServiceDiscovery {
            backend: ServiceDiscoveryBackend::Consul,
            k8s_client: None,
            consul_addr: Some(addr.into()),
            etcd_endpoints: Vec::new(),
            static_services: HashMap::new(),
        }
    }

    pub fn etcd(endpoints: &[&str]) -> Self {
        ServiceDiscovery {
            backend: ServiceDiscoveryBackend::Etcd,
            k8s_client: None,
            consul_addr: None,
            etcd_endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            static_services: HashMap::new(),
        }
    }

    pub fn static_services(mut self, services: HashMap<String, Vec<ServiceInstance>>) -> Self {
        self.static_services = services;
        self
    }

    pub async fn discover(&self, service: &str) -> Result<Vec<ServiceInstance>> {
        match self.backend {
            ServiceDiscoveryBackend::Kubernetes => {
                if let Some(client) = &self.k8s_client {
                    let endpoints = client.discover_service(service, None).await?;
                    Ok(endpoints.endpoints.into_iter().map(|e| ServiceInstance {
                        id: e.ip.clone(),
                        name: service.into(),
                        address: e.ip,
                        port: endpoints.ports.first().map(|p| p.port).unwrap_or(80),
                        tags: vec![],
                        metadata: HashMap::new(),
                        health_check: None,
                    }).collect())
                } else {
                    Err(CloudError::NotConfigured("Kubernetes client required".into()))
                }
            }
            ServiceDiscoveryBackend::Static => {
                Ok(self.static_services.get(service).cloned().unwrap_or_default())
            }
            _ => Err(CloudError::NotImplemented("Service discovery backend not implemented".into())),
        }
    }

    pub async fn register(&self, instance: ServiceInstance) -> Result<()> {
        match self.backend {
            ServiceDiscoveryBackend::Consul => {
                // Would register with Consul
                Ok(())
            }
            ServiceDiscoveryBackend::Etcd => {
                // Would register with etcd
                Ok(())
            }
            _ => Err(CloudError::NotImplemented("Registration not supported for this backend".into())),
        }
    }

    pub async fn deregister(&self, instance_id: &str) -> Result<()> {
        // Would deregister from backend
        Ok(())
    }

    pub async fn health_check(&self, instance_id: &str) -> Result<bool> {
        // Would check health
        Ok(true)
    }
}

/// Service registry for client-side load balancing
pub struct ServiceRegistry {
    discovery: ServiceDiscovery,
    cache: HashMap<String, Vec<ServiceInstance>>,
    ttl: Duration,
}

impl ServiceRegistry {
    pub fn new(discovery: ServiceDiscovery) -> Self {
        ServiceRegistry {
            discovery,
            cache: HashMap::new(),
            ttl: Duration::from_secs(30),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub async fn get_instances(&mut self, service: &str) -> Result<Vec<ServiceInstance>> {
        // Check cache
        if let Some(instances) = self.cache.get(service) {
            return Ok(instances.clone());
        }

        // Discover
        let instances = self.discovery.discover(service).await?;
        self.cache.insert(service.into(), instances.clone());
        Ok(instances)
    }

    pub async fn pick_instance(&mut self, service: &str) -> Result<Option<ServiceInstance>> {
        let instances = self.get_instances(service).await?;
        // Simple round-robin or random selection
        instances.into_iter().next().map(|i| Some(i)).unwrap_or(None)
    }
}