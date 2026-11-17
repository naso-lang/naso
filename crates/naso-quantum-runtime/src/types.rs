//! Common Types for Quantum Runtime
//!
//! Shared types used across backend, job, and other modules to avoid circular dependencies.

use crate::circuit::QuantumCircuit;
use crate::execution::{ExecutionConfig, ExecutionResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;
use std::time::{SystemTime, UNIX_EPOCH};

/// A quantum job to be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumJob {
    pub id: String,
    pub circuit: QuantumCircuit,
    pub config: ExecutionConfig,
    pub backend_preference: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: u64,
}

impl QuantumJob {
    pub fn new(circuit: QuantumCircuit, config: ExecutionConfig) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            circuit,
            config,
            backend_preference: None,
            metadata: HashMap::new(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn with_backend(mut self, backend: String) -> Self {
        self.backend_preference = Some(backend);
        self
    }
}

/// Status of a quantum job
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed(String),
    Cancelled,
    Unknown,
}

/// Job handle for tracking submitted jobs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobHandle {
    pub id: String,
    pub backend: String,
    pub submitted_at: u64,
    pub estimated_completion: Option<u64>,
}

impl JobHandle {
    pub fn new(id: String, backend: String) -> Self {
        Self {
            id,
            backend,
            submitted_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            estimated_completion: None,
        }
    }
}