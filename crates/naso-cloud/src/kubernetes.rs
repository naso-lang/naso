//! Kubernetes integration for Naso Cloud Runtime

#![cfg(feature = "k8s")]

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sConfig {
    pub api_server: Option<String>,
    pub token: Option<String>,
    pub ca_cert: Option<String>,
    pub namespace: Option<String>,
    pub qps: f32,
    pub burst: u32,
}

impl K8sConfig {
    pub fn in_cluster() -> Result<Self> {
        let token = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
            .map_err(|e| CloudError::K8sConfigNotFound(e.to_string()))?;
        let ca_cert = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
            .map_err(|e| CloudError::K8sConfigNotFound(e.to_string()))?;
        let namespace = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
            .unwrap_or_else(|_| "default".into());

        Ok(K8sConfig {
            api_server: Some("https://kubernetes.default.svc".into()),
            token: Some(token.trim().into()),
            ca_cert: Some(ca_cert),
            namespace: Some(namespace.trim().into()),
            qps: 50.0,
            burst: 100,
        })
    }

    pub fn from_kubeconfig(_path: &str) -> Result<Self> {
        Err(CloudError::NotImplemented("Kubeconfig parsing not implemented".into()))
    }

    pub fn api_server(mut self, server: &str) -> Self {
        self.api_server = Some(server.into());
        self
    }

    pub fn namespace(mut self, ns: &str) -> Self {
        self.namespace = Some(ns.into());
        self
    }

    pub fn qps(mut self, qps: f32) -> Self {
        self.qps = qps;
        self
    }

    pub fn burst(mut self, burst: u32) -> Self {
        self.burst = burst;
        self
    }
}

/// Kubernetes client for API interactions
pub struct K8sClient {
    config: K8sConfig,
    client: reqwest::Client,
}

impl K8sClient {
    pub fn new(config: K8sConfig) -> Result<Self> {
        let mut client_builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(true); // In-cluster uses self-signed certs

        if let Some(ca_cert) = &config.ca_cert {
            let cert = reqwest::Certificate::from_pem(ca_cert.as_bytes())
                .map_err(|e| CloudError::K8sClientCreationFailed(e.to_string()))?;
            client_builder = client_builder.add_root_certificate(cert);
        }

        let client = client_builder
            .build()
            .map_err(|e| CloudError::K8sClientCreationFailed(e.to_string()))?;

        Ok(K8sClient { config, client })
    }

    pub async fn pod_info(&self) -> Result<PodInfo> {
        let namespace = self.config.namespace.as_deref().unwrap_or("default");
        let url = format!("{}/api/v1/namespaces/{}/pods", 
            self.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            namespace);

        let response = self.request(reqwest::Method::GET, &url).await?;
        let pod_list: PodList = response.json().await?;

        // Return first pod (in practice, would filter by current pod)
        pod_list.items.into_iter().next()
            .ok_or_else(|| CloudError::K8sRequestFailed("No pods found".into()))
    }

    pub async fn discover_service(&self, name: &str, namespace: Option<&str>) -> Result<ServiceEndpoints> {
        let ns = namespace.unwrap_or(self.config.namespace.as_deref().unwrap_or("default"));
        let url = format!("{}/api/v1/namespaces/{}/endpoints/{}", 
            self.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            ns, name);

        let response = self.request(reqwest::Method::GET, &url).await?;
        let endpoints: Endpoints = response.json().await?;

        Ok(endpoints.into())
    }

    pub async fn get_configmap(&self, name: &str, namespace: Option<&str>) -> Result<ConfigMap> {
        let ns = namespace.unwrap_or(self.config.namespace.as_deref().unwrap_or("default"));
        let url = format!("{}/api/v1/namespaces/{}/configmaps/{}", 
            self.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            ns, name);

        let response = self.request(reqwest::Method::GET, &url).await?;
        let cm: K8sConfigMap = response.json().await?;

        Ok(cm.into())
    }

    pub async fn get_secret(&self, name: &str, namespace: Option<&str>) -> Result<Secret> {
        let ns = namespace.unwrap_or(self.config.namespace.as_deref().unwrap_or("default"));
        let url = format!("{}/api/v1/namespaces/{}/secrets/{}", 
            self.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            ns, name);

        let response = self.request(reqwest::Method::GET, &url).await?;
        let secret: K8sSecret = response.json().await?;

        Ok(secret.into())
    }

    pub async fn leader_election(&self, lock_name: &str, identity: &str, ttl_secs: u32) -> Result<LeaderLock> {
        // Create or acquire lease for leader election
        let namespace = self.config.namespace.as_deref().unwrap_or("default");
        let url = format!("{}/apis/coordination.k8s.io/v1/namespaces/{}/leases/{}", 
            self.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            namespace, lock_name);

        let lease = Lease {
            metadata: LeaseMetadata {
                name: lock_name.into(),
                namespace: namespace.into(),
            },
            spec: LeaseSpec {
                holder_identity: Some(identity.into()),
                lease_duration_seconds: Some(ttl_secs),
                renew_time: Some(chrono::Utc::now().to_rfc3339()),
            },
        };

        let response = self.client
            .put(&url)
            .bearer_auth(self.config.token.as_deref().unwrap_or(""))
            .json(&lease)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(LeaderLock {
                client: self.clone(),
                lock_name: lock_name.into(),
                identity: identity.into(),
                ttl_secs,
            })
        } else {
            Err(CloudError::LeaderElectionFailed(format!("Status: {}", response.status())))
        }
    }

    async fn request(&self, method: reqwest::Method, url: &str) -> Result<reqwest::Response> {
        let response = self.client
            .request(method, url)
            .bearer_auth(self.config.token.as_deref().unwrap_or(""))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(response)
        } else {
            Err(CloudError::K8sRequestFailed(format!("Status: {}", response.status())))
        }
    }
}

impl Clone for K8sClient {
    fn clone(&self) -> Self {
        K8sClient {
            config: self.config.clone(),
            client: self.client.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub node_name: String,
    pub pod_ip: String,
    pub host_ip: String,
    pub phase: String,
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
    pub containers: Vec<ContainerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PodList {
    items: Vec<PodInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoints {
    pub name: String,
    pub namespace: String,
    pub ports: Vec<ServicePort>,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePort {
    pub name: String,
    pub port: u16,
    pub target_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub ip: String,
    pub node_name: Option<String>,
    pub target_ref: Option<String>,
    pub conditions: EndpointConditions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConditions {
    pub ready: bool,
    pub serving: bool,
    pub terminating: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Endpoints {
    metadata: ObjectMeta,
    subsets: Vec<EndpointSubset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndpointSubset {
    addresses: Vec<EndpointAddress>,
    ports: Vec<EndpointPort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndpointAddress {
    ip: String,
    node_name: Option<String>,
    target_ref: Option<ObjectReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndpointPort {
    name: String,
    port: u16,
    protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectReference {
    kind: String,
    name: String,
    namespace: String,
}

impl From<Endpoints> for ServiceEndpoints {
    fn from(e: Endpoints) -> Self {
        let mut endpoints = Vec::new();
        let mut ports = Vec::new();

        for subset in e.subsets {
            for addr in subset.addresses {
                endpoints.push(Endpoint {
                    ip: addr.ip,
                    node_name: addr.node_name,
                    target_ref: addr.target_ref.map(|r| r.name),
                    conditions: EndpointConditions {
                        ready: true,
                        serving: true,
                        terminating: false,
                    },
                });
            }
            for port in subset.ports {
                ports.push(ServicePort {
                    name: port.name,
                    port: port.port,
                    target_port: port.port,
                    protocol: port.protocol,
                });
            }
        }

        ServiceEndpoints {
            name: e.metadata.name,
            namespace: e.metadata.namespace.unwrap_or_default(),
            ports,
            endpoints,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMap {
    pub name: String,
    pub namespace: String,
    pub data: HashMap<String, String>,
    pub binary_data: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct K8sConfigMap {
    metadata: ObjectMeta,
    data: Option<HashMap<String, String>>,
    binary_data: Option<HashMap<String, String>>, // base64 encoded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectMeta {
    name: String,
    namespace: Option<String>,
    uid: Option<String>,
}

impl From<K8sConfigMap> for ConfigMap {
    fn from(cm: K8sConfigMap) -> Self {
        ConfigMap {
            name: cm.metadata.name,
            namespace: cm.metadata.namespace.unwrap_or_default(),
            data: cm.data.unwrap_or_default(),
            binary_data: cm.binary_data.unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, base64::decode(v).unwrap_or_default()))
                .collect(),
        }
    }
}

impl ConfigMap {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    pub namespace: String,
    pub data: HashMap<String, Vec<u8>>,
    pub string_data: HashMap<String, String>,
    pub secret_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct K8sSecret {
    metadata: ObjectMeta,
    data: Option<HashMap<String, String>>, // base64 encoded
    string_data: Option<HashMap<String, String>>,
    #[serde(rename = "type")]
    secret_type: String,
}

impl From<K8sSecret> for Secret {
    fn from(s: K8sSecret) -> Self {
        Secret {
            name: s.metadata.name,
            namespace: s.metadata.namespace.unwrap_or_default(),
            data: s.data.unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, base64::decode(v).unwrap_or_default()))
                .collect(),
            string_data: s.string_data.unwrap_or_default(),
            secret_type: s.secret_type,
        }
    }
}

impl Secret {
    pub fn get(&self, key: &str) -> Option<&Vec<u8>> {
        self.data.get(key)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.string_data.get(key).cloned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Lease {
    metadata: LeaseMetadata,
    spec: LeaseSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseMetadata {
    name: String,
    namespace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseSpec {
    holder_identity: Option<String>,
    lease_duration_seconds: Option<u32>,
    renew_time: Option<String>,
}

/// Leader election lock
pub struct LeaderLock {
    client: K8sClient,
    lock_name: String,
    identity: String,
    ttl_secs: u32,
}

impl LeaderLock {
    pub async fn renew(&self) -> Result<()> {
        self.client.leader_election(&self.lock_name, &self.identity, self.ttl_secs).await?;
        Ok(())
    }

    pub async fn resign(&self) -> Result<()> {
        // Delete the lease
        let namespace = self.client.config.namespace.as_deref().unwrap_or("default");
        let url = format!("{}/apis/coordination.k8s.io/v1/namespaces/{}/leases/{}", 
            self.client.config.api_server.as_deref().unwrap_or("https://kubernetes.default.svc"), 
            namespace, self.lock_name);

        let response = self.client.client
            .delete(&url)
            .bearer_auth(self.client.config.token.as_deref().unwrap_or(""))
            .send()
            .await?;

        if !response.status().is_success() && response.status() != 404 {
            return Err(CloudError::LeaderElectionFailed(format!("Status: {}", response.status())));
        }
        Ok(())
    }

    pub fn is_leader(&self) -> bool {
        true // Simplified - would check lease holder identity
    }
}

impl Drop for LeaderLock {
    fn drop(&mut self) {
        // Best effort resign
        let _ = self.resign();
    }
}