//! Mirror management for the Naso package registry

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::models::Mirror;
use crate::storage::PackageStorage;
use crate::RegistryError;

pub struct MirrorManager {
    storage: Arc<dyn PackageStorage>,
    pool: PgPool,
    mirrors: RwLock<HashMap<Uuid, Mirror>>,
    sync_handles: RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>,
}

impl MirrorManager {
    pub fn new(storage: Arc<dyn PackageStorage>, pool: PgPool) -> Self {
        Self {
            storage,
            pool,
            mirrors: RwLock::new(HashMap::new()),
            sync_handles: RwLock::new(HashMap::new()),
        }
    }
    
    pub async fn initialize(&self) -> Result<(), RegistryError> {
        let rows = sqlx::query("SELECT * FROM mirrors WHERE status != 'disabled'")
            .fetch_all(&self.pool)
            .await?;
        
        let mut mirrors = self.mirrors.write().await;
        for row in rows {
            let mirror = Mirror {
                id: row.get("id"),
                url: row.get("url"),
                region: row.get("region"),
                priority: row.get("priority"),
                status: match row.get::<String, _>("status").as_str() {
                    "active" => crate::models::MirrorStatus::Active,
                    "syncing" => crate::models::MirrorStatus::Syncing,
                    "error" => crate::models::MirrorStatus::Error(row.get("error_message").unwrap_or_default()),
                    "disabled" => crate::models::MirrorStatus::Disabled,
                    _ => crate::models::MirrorStatus::Active,
                },
                last_sync: row.get("last_sync"),
                bandwidth_limit: row.get("bandwidth_limit"),
            };
            mirrors.insert(mirror.id, mirror);
        }
        
        // Start sync loops for active mirrors
        for mirror in mirrors.values() {
            if matches!(mirror.status, crate::models::MirrorStatus::Active) {
                self.start_sync_loop(mirror.clone());
            }
        }
        
        Ok(())
    }
    
    pub async fn list_mirrors(&self) -> Vec<Mirror> {
        self.mirrors.read().await.values().cloned().collect()
    }
    
    pub async fn add_mirror(&self, request: AddMirrorRequest) -> Result<Mirror, RegistryError> {
        let mirror = Mirror {
            id: Uuid::new_v4(),
            url: request.url,
            region: request.region,
            priority: request.priority,
            status: crate::models::MirrorStatus::Active,
            last_sync: None,
            bandwidth_limit: None,
        };
        
        sqlx::query(
            "INSERT INTO mirrors (id, url, region, priority, status) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(mirror.id)
        .bind(&mirror.url)
        .bind(&mirror.region)
        .bind(mirror.priority as i32)
        .bind("active")
        .execute(&self.pool)
        .await?;
        
        self.mirrors.write().await.insert(mirror.id, mirror.clone());
        self.start_sync_loop(mirror.clone());
        
        Ok(mirror)
    }
    
    pub async fn remove_mirror(&self, id: Uuid) -> Result<(), RegistryError> {
        sqlx::query("UPDATE mirrors SET status = 'disabled' WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        
        self.mirrors.write().await.remove(&id);
        
        if let Some(handle) = self.sync_handles.write().await.remove(&id) {
            handle.abort();
        }
        
        Ok(())
    }
    
    pub async fn trigger_sync(&self, package_name: &str) {
        let mirrors = self.mirrors.read().await;
        for mirror in mirrors.values() {
            if matches!(mirror.status, crate::models::MirrorStatus::Active) {
                self.sync_package_to_mirror(mirror.clone(), package_name).await;
            }
        }
    }
    
    async fn sync_package_to_mirror(&self, mirror: Mirror, package_name: &str) {
        // Update mirror status to syncing
        {
            let mut mirrors = self.mirrors.write().await;
            if let Some(m) = mirrors.get_mut(&mirror.id) {
                m.status = crate::models::MirrorStatus::Syncing;
            }
        }
        
        sqlx::query("UPDATE mirrors SET status = 'syncing' WHERE id = $1")
            .bind(mirror.id)
            .execute(&self.pool)
            .await.ok();
        
        // In a real implementation, this would:
        // 1. Fetch package data from primary storage
        // 2. Push to mirror via HTTP PUT
        // 3. Update mirror status
        
        // Simulate sync
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Update mirror status back to active
        {
            let mut mirrors = self.mirrors.write().await;
            if let Some(m) = mirrors.get_mut(&mirror.id) {
                m.status = crate::models::MirrorStatus::Active;
                m.last_sync = Some(Utc::now());
            }
        }
        
        sqlx::query("UPDATE mirrors SET status = 'active', last_sync = NOW() WHERE id = $1")
            .bind(mirror.id)
            .execute(&self.pool)
            .await.ok();
    }
    
    fn start_sync_loop(&self, mirror: Mirror) {
        let storage = self.storage.clone();
        let pool = self.pool.clone();
        let mirrors = self.mirrors.clone();
        let mirror_id = mirror.id;
        
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 min
            
            loop {
                interval.tick().await;
                
                // Check if mirror still exists and is active
                let should_continue = {
                    let mirrors_guard = mirrors.read().await;
                    mirrors_guard.get(&mirror_id).map(|m| matches!(m.status, crate::models::MirrorStatus::Active)).unwrap_or(false)
                };
                
                if !should_continue {
                    break;
                }
                
                // Sync all packages
                match storage.list_packages(Some(1000), Some(0)).await {
                    Ok(packages) => {
                        for pkg in packages {
                            // In real implementation, check if package changed since last sync
                            // and sync if needed
                        }
                    }
                    Err(e) => {
                        tracing::error!("Mirror sync failed for {}: {}", mirror_id, e);
                        let mut mirrors_guard = mirrors.write().await;
                        if let Some(m) = mirrors_guard.get_mut(&mirror_id) {
                            m.status = crate::models::MirrorStatus::Error(e.to_string());
                        }
                        sqlx::query("UPDATE mirrors SET status = 'error', error_message = $1 WHERE id = $2")
                            .bind(e.to_string())
                            .bind(mirror_id)
                            .execute(&pool)
                            .await.ok();
                    }
                }
            }
        });
        
        self.sync_handles.write().await.insert(mirror_id, handle);
    }
}

#[derive(Deserialize)]
pub struct AddMirrorRequest {
    pub url: String,
    pub region: String,
    pub priority: u32,
}