//! Quantum Job Management
//!
//! Handles job submission, tracking, and result retrieval.

use crate::backend::QuantumBackend;
use crate::circuit::QuantumCircuit;
use crate::execution::{ExecutionConfig, ExecutionResult};
use crate::types::{JobHandle, JobStatus, QuantumJob};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use uuid::Uuid;

/// Job info for internal tracking
struct JobInfo {
    job: QuantumJob,
    handle: Option<JobHandle>,
    status: JobStatus,
    result: Option<ExecutionResult>,
    backend: Option<String>,
}

/// Job manager for tracking multiple jobs
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
    backends: Arc<Mutex<HashMap<String, Arc<dyn QuantumBackend>>>>,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            backends: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a backend
    pub fn register_backend(&self, name: String, backend: Arc<dyn QuantumBackend>) {
        self.backends.lock().unwrap().insert(name, backend);
    }

    /// Get a backend by name
    pub fn get_backend(&self, name: &str) -> Option<Arc<dyn QuantumBackend>> {
        self.backends.lock().unwrap().get(name).cloned()
    }

    /// Submit a job to a backend
    pub async fn submit_job(
        &self,
        job: QuantumJob,
        backend_name: Option<&str>,
    ) -> Result<JobHandle> {
        let backend_preference = job.backend_preference.clone();
        let backend_name = backend_name.unwrap_or(backend_preference.as_deref().unwrap_or("default"));
        let backend = self.get_backend(backend_name)
            .ok_or_else(|| anyhow!("Backend not found: {}", backend_name))?;

        let handle = backend.submit_job(job.clone()).await?;
        
        let job_info = JobInfo {
            job,
            handle: Some(handle.clone()),
            status: JobStatus::Queued,
            result: None,
            backend: Some(backend_name.to_string()),
        };
        
        self.jobs.lock().unwrap().insert(handle.id.clone(), job_info);
        Ok(handle)
    }

    /// Get job status
    pub async fn job_status(&self, handle: &JobHandle) -> Result<JobStatus> {
        let backend = if let Some(job_info) = self.jobs.lock().unwrap().get(&handle.id) {
            if let Some(backend_name) = &job_info.backend {
                self.get_backend(backend_name)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(backend) = backend {
            backend.job_status(handle).await
        } else {
            // Fallback to local status
            Ok(self.jobs.lock().unwrap()
                .get(&handle.id)
                .map(|j| j.status.clone())
                .unwrap_or(JobStatus::Unknown))
        }
    }

    /// Get job result (blocks until complete)
    pub async fn job_result(&self, handle: &JobHandle) -> Result<ExecutionResult> {
        let backend = if let Some(job_info) = self.jobs.lock().unwrap().get(&handle.id) {
            if let Some(backend_name) = &job_info.backend {
                self.get_backend(backend_name)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(backend) = backend {
            let result = backend.job_result(handle).await?;
            // Update local cache
            if let Some(job_info) = self.jobs.lock().unwrap().get_mut(&handle.id) {
                job_info.status = JobStatus::Completed;
                job_info.result = Some(result.clone());
            }
            Ok(result)
        } else {
            // Fallback to local cache
            self.jobs.lock().unwrap()
                .get(&handle.id)
                .and_then(|j| j.result.clone())
                .ok_or_else(|| anyhow!("Result not available"))
        }
    }

    /// Cancel a job
    pub async fn cancel_job(&self, handle: &JobHandle) -> Result<()> {
        if let Some(job_info) = self.jobs.lock().unwrap().get(&handle.id) {
            if let Some(backend_name) = &job_info.backend {
                if let Some(backend) = self.get_backend(backend_name) {
                    backend.cancel_job(handle).await?;
                }
            }
        }
        if let Some(job_info) = self.jobs.lock().unwrap().get_mut(&handle.id) {
            job_info.status = JobStatus::Cancelled;
        }
        Ok(())
    }

    /// List all jobs
    pub fn list_jobs(&self) -> Vec<(String, JobStatus)> {
        self.jobs.lock().unwrap()
            .iter()
            .map(|(id, info)| (id.clone(), info.status.clone()))
            .collect()
    }
}

/// Batch job for submitting multiple circuits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchJob {
    pub jobs: Vec<QuantumJob>,
    pub backend_preference: Option<String>,
}

/// Batch job result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub handles: Vec<JobHandle>,
    pub results: Vec<Option<ExecutionResult>>,
}