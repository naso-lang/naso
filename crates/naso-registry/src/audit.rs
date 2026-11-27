//! Audit logging for the Naso package registry

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::*;
use crate::RegistryError;

pub struct AuditLogger {
    pool: PgPool,
}

impl AuditLogger {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    
    pub async fn log(&self, event: impl AuditEvent) -> Result<(), RegistryError> {
        let entry = event.to_audit_entry();
        
        sqlx::query(
            r#"
            INSERT INTO audit_log (id, event_type, package_name, version, user_id, ip_address, user_agent, metadata, timestamp)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#
        )
        .bind(entry.id)
        .bind(entry.event_type)
        .bind(entry.package_name)
        .bind(entry.version)
        .bind(entry.user_id)
        .bind(entry.ip_address)
        .bind(entry.user_agent)
        .bind(serde_json::to_value(&entry.metadata).unwrap())
        .bind(entry.timestamp)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn query(&self, params: AuditParams) -> Result<AuditResponse, RegistryError> {
        let mut query = "SELECT * FROM audit_log WHERE 1=1".to_string();
        let mut bind_count = 0;
        
        if params.start.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND timestamp >= ${}", bind_count));
        }
        if params.end.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND timestamp <= ${}", bind_count));
        }
        if params.event_type.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND event_type = ${}", bind_count));
        }
        
        query.push_str(" ORDER BY timestamp DESC");
        
        if let Some(limit) = params.limit {
            bind_count += 1;
            query.push_str(&format!(" LIMIT ${}", bind_count));
        }
        
        let mut q = sqlx::query(&query);
        
        if let Some(start) = params.start {
            q = q.bind(start);
        }
        if let Some(end) = params.end {
            q = q.bind(end);
        }
        if let Some(event_type) = params.event_type {
            q = q.bind(event_type);
        }
        if let Some(limit) = params.limit {
            q = q.bind(limit as i64);
        }
        
        let rows = q.fetch_all(&self.pool).await?;
        
        let entries = rows.into_iter().map(|row| AuditEntry {
            id: row.get("id"),
            event_type: row.get("event_type"),
            package_name: row.get("package_name"),
            version: row.get("version"),
            user_id: row.get("user_id"),
            ip_address: row.get("ip_address"),
            user_agent: row.get("user_agent"),
            metadata: row.get("metadata"),
            timestamp: row.get("timestamp"),
        }).collect();
        
        Ok(AuditResponse { entries })
    }
}

pub trait AuditEvent {
    fn to_audit_entry(&self) -> AuditEntry;
}

impl AuditEvent for PublishEvent {
    fn to_audit_entry(&self) -> AuditEntry {
        AuditEntry {
            id: Uuid::new_v4(),
            event_type: "publish".to_string(),
            package_name: Some(self.package.clone()),
            version: Some(self.version.clone()),
            user_id: Some(self.publisher),
            ip_address: None,
            user_agent: None,
            metadata: serde_json::json!({}),
            timestamp: self.timestamp,
        }
    }
}

impl AuditEvent for DeleteEvent {
    fn to_audit_entry(&self) -> AuditEntry {
        AuditEntry {
            id: Uuid::new_v4(),
            event_type: "delete_package".to_string(),
            package_name: Some(self.package.clone()),
            version: None,
            user_id: Some(self.admin),
            ip_address: None,
            user_agent: None,
            metadata: serde_json::json!({}),
            timestamp: self.timestamp,
        }
    }
}

impl AuditEvent for DeleteVersionEvent {
    fn to_audit_entry(&self) -> AuditEntry {
        AuditEntry {
            id: Uuid::new_v4(),
            event_type: "delete_version".to_string(),
            package_name: Some(self.package.clone()),
            version: Some(self.version.clone()),
            user_id: Some(self.admin),
            ip_address: None,
            user_agent: None,
            metadata: serde_json::json!({}),
            timestamp: self.timestamp,
        }
    }
}

impl AuditEvent for YankEvent {
    fn to_audit_entry(&self) -> AuditEntry {
        AuditEntry {
            id: Uuid::new_v4(),
            event_type: "yank".to_string(),
            package_name: Some(self.package.clone()),
            version: Some(self.version.clone()),
            user_id: Some(self.publisher),
            ip_address: None,
            user_agent: None,
            metadata: serde_json::json!({}),
            timestamp: self.timestamp,
        }
    }
}

impl AuditEvent for UnyankEvent {
    fn to_audit_entry(&self) -> AuditEntry {
        AuditEntry {
            id: Uuid::new_v4(),
            event_type: "unyank".to_string(),
            package_name: Some(self.package.clone()),
            version: Some(self.version.clone()),
            user_id: Some(self.publisher),
            ip_address: None,
            user_agent: None,
            metadata: serde_json::json!({}),
            timestamp: self.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Uuid,
    pub event_type: String,
    pub package_name: Option<String>,
    pub version: Option<String>,
    pub user_id: Option<Uuid>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub metadata: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct AuditParams {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub event_type: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct AuditResponse {
    pub entries: Vec<AuditEntry>,
}