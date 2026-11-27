//! VM metadata for Naso Cloud Runtime

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// VM metadata provider
pub struct VmMetadata;

impl VmMetadata {
    /// Get metadata value by key
    pub async fn get(key: &str) -> Result<String> {
        match Self::detect_provider() {
            CloudProvider::Aws => Self::aws_metadata(key).await,
            CloudProvider::Gcp => Self::gcp_metadata(key).await,
            CloudProvider::Azure => Self::azure_metadata(key).await,
            CloudProvider::DigitalOcean => Self::do_metadata(key).await,
            CloudProvider::Hetzner => Self::hetzner_metadata(key).await,
            _ => Err(CloudError::VmMetadataNotFound(key.into())),
        }
    }

    /// Get instance ID
    pub async fn instance_id() -> Result<String> {
        match Self::detect_provider() {
            CloudProvider::Aws => Self::aws_metadata("instance-id").await,
            CloudProvider::Gcp => Self::gcp_metadata("instance/id").await,
            CloudProvider::Azure => Self::azure_metadata("instance/compute/vmId").await,
            CloudProvider::DigitalOcean => Self::do_metadata("id").await,
            CloudProvider::Hetzner => Self::hetzner_metadata("id").await,
            _ => Err(CloudError::VmMetadataNotFound("instance-id".into())),
        }
    }

    /// Get availability zone
    pub async fn zone() -> Result<String> {
        match Self::detect_provider() {
            CloudProvider::Aws => Self::aws_metadata("placement/availability-zone").await,
            CloudProvider::Gcp => Self::gcp_metadata("instance/zone").await,
            CloudProvider::Azure => Self::azure_metadata("instance/compute/zone").await,
            CloudProvider::DigitalOcean => Self::do_metadata("region").await,
            CloudProvider::Hetzner => Self::hetzner_metadata("datacenter").await,
            _ => Err(CloudError::VmMetadataNotFound("zone".into())),
        }
    }

    /// Get instance tags/labels
    pub async fn tags() -> Result<HashMap<String, String>> {
        match Self::detect_provider() {
            CloudProvider::Aws => {
                // AWS tags require separate API call
                Ok(HashMap::new())
            }
            CloudProvider::Gcp => {
                let _metadata = Self::gcp_metadata("instance/attributes").await?;
                // Parse attributes
                Ok(HashMap::new())
            }
            CloudProvider::Azure => {
                let _metadata = Self::azure_metadata("instance/compute/tags").await?;
                // Parse tags
                Ok(HashMap::new())
            }
            _ => Ok(HashMap::new()),
        }
    }

    /// Get all metadata as JSON
    pub async fn all() -> Result<serde_json::Value> {
        let client = reqwest::Client::new();
        
        match Self::detect_provider() {
            CloudProvider::Aws => {
                let resp = client.get("http://169.254.169.254/latest/dynamic/instance-identity/document")
                    .send()
                    .await
                    .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
                resp.json().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
            }
            CloudProvider::Gcp => {
                let resp = client.get("http://metadata.google.internal/computeMetadata/v1/instance/?recursive=true")
                    .header("Metadata-Flavor", "Google")
                    .send()
                    .await
                    .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
                resp.json().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
            }
            CloudProvider::Azure => {
                let resp = client.get("http://169.254.169.254/metadata/instance?api-version=2021-02-01")
                    .header("Metadata", "true")
                    .send()
                    .await
                    .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
                resp.json().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
            }
            _ => Err(CloudError::VmMetadataNotFound("all".into())),
        }
    }

    fn detect_provider() -> CloudProvider {
        // Check for cloud metadata endpoints
        // In practice, would do quick HTTP HEAD requests
        if std::path::Path::new("/sys/hypervisor/uuid").exists() {
            if let Ok(uuid) = std::fs::read_to_string("/sys/hypervisor/uuid") {
                if uuid.starts_with("ec2") {
                    return CloudProvider::Aws;
                }
            }
        }

        // Default to unknown - would need actual HTTP checks
        CloudProvider::Unknown
    }

    async fn aws_metadata(key: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let url = format!("http://169.254.169.254/latest/meta-data/{}", key);
        let resp = client.get(&url)
            .send()
            .await
            .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
        resp.text().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
    }

    async fn gcp_metadata(key: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let url = format!("http://metadata.google.internal/computeMetadata/v1/{}", key);
        let resp = client.get(&url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
        resp.text().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
    }

    async fn azure_metadata(key: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let url = format!("http://169.254.169.254/metadata/{}?api-version=2021-02-01&format=text", key);
        let resp = client.get(&url)
            .header("Metadata", "true")
            .send()
            .await
            .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
        resp.text().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
    }

    async fn do_metadata(key: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let url = format!("http://169.254.169.254/metadata/v1/{}", key);
        let resp = client.get(&url)
            .send()
            .await
            .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
        resp.text().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
    }

    async fn hetzner_metadata(key: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let url = format!("http://169.254.169.254/hetzner/v1/{}", key);
        let resp = client.get(&url)
            .send()
            .await
            .map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))?;
        resp.text().await.map_err(|e| CloudError::VmMetadataNotFound(e.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

/// Instance metadata for cloud providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMetadata {
    pub instance_id: String,
    pub instance_type: Option<String>,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub provider: CloudProvider,
    pub tags: HashMap<String, String>,
    pub private_ip: Option<String>,
    pub public_ip: Option<String>,
    pub mac_addresses: Vec<String>,
}

impl InstanceMetadata {
    pub async fn fetch() -> Result<Self> {
        let provider = VmMetadata::detect_provider();
        let instance_id = VmMetadata::instance_id().await?;
        let zone = VmMetadata::zone().await.ok();
        let tags = VmMetadata::tags().await.unwrap_or_default();

        Ok(InstanceMetadata {
            instance_id,
            instance_type: None,
            region: None,
            zone,
            provider,
            tags,
            private_ip: None,
            public_ip: None,
            mac_addresses: Vec::new(),
        })
    }
}