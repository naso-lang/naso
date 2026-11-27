//! Naso ML Framework
//!
//! Machine learning framework for Naso with:
//! - Neural networks (candle-core backend)
//! - Tensor operations (ndarray)
//! - Quantum ML support
//! - Model training and inference
//! - Data loading and preprocessing
//! - Model serialization

use candle_core::{Device, Result as CandleResult, Tensor};
use candle_nn::{Module, VarBuilder, VarMap};
use ndarray::{Array, Array1, Array2, Array3, Axis};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, instrument};
use thiserror::Error;

pub mod tensor;
pub mod nn;
pub mod optim;
pub mod data;
pub mod model;
pub mod training;
pub mod inference;
pub mod quantum;
pub mod quantization;
pub mod export;
pub mod cli;

use tensor::NasoTensor;
use nn::NasoModule;
use optim::Optimizer;
use data::DataLoader;
use model::NasoModel;
use training::Trainer;
use inference::InferenceEngine;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MLConfig {
    pub device: DeviceType,
    pub seed: u64,
    pub num_threads: usize,
    pub memory_limit: Option<usize>,
    pub enable_quantum: bool,
    pub default_dtype: DType,
    pub cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceType {
    Cpu,
    Cuda { device_id: i32 },
    Metal,
    Vulkan,
    Quantum,
}

impl Default for MLConfig {
    fn default() -> Self {
        Self {
            device: DeviceType::Cpu,
            seed: 42,
            num_threads: num_cpus::get(),
            memory_limit: None,
            enable_quantum: false,
            default_dtype: DType::F32,
            cache_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DType {
    F16,
    F32,
    F64,
    I8,
    I16,
    I32,
    I64,
    U8,
    Bool,
}

#[derive(Debug, Clone)]
pub struct MLFramework {
    config: MLConfig,
    device: Device,
    var_map: Arc<RwLock<VarMap>>,
    models: Arc<RwLock<HashMap<String, Arc<dyn NasoModel>>>>,
}

impl MLFramework {
    pub fn new(config: MLConfig) -> CandleResult<Self> {
        let device = match &config.device {
            DeviceType::Cpu => Device::Cpu,
            DeviceType::Cuda { device_id } => Device::new_cuda(*device_id)?,
            DeviceType::Metal => Device::new_metal()?,
            DeviceType::Vulkan => Device::new_vulkan(0)?,
            DeviceType::Quantum => {
                // Quantum device - would interface with quantum backend
                Device::Cpu
            }
        };

        Ok(Self {
            config: config.clone(),
            device,
            var_map: Arc::new(RwLock::new(VarMap::new())),
            models: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn config(&self) -> &MLConfig {
        &self.config
    }

    pub async fn register_model<M: NasoModel + 'static>(&self, name: &str, model: M) {
        self.models.write().await.insert(name.to_string(), Arc::new(model));
    }

    pub async fn get_model(&self, name: &str) -> Option<Arc<dyn NasoModel>> {
        self.models.read().await.get(name).cloned()
    }

    pub async fn list_models(&self) -> Vec<String> {
        self.models.read().await.keys().cloned().collect()
    }

    pub fn create_tensor(&self, data: Vec<f32>, shape: &[usize]) -> CandleResult<NasoTensor> {
        let tensor = Tensor::from_vec(data, shape, &self.device)?;
        Ok(NasoTensor::new(tensor))
    }

    pub fn zeros(&self, shape: &[usize]) -> CandleResult<NasoTensor> {
        let tensor = Tensor::zeros(shape, self.config.default_dtype.into(), &self.device)?;
        Ok(NasoTensor::new(tensor))
    }

    pub fn ones(&self, shape: &[usize]) -> CandleResult<NasoTensor> {
        let tensor = Tensor::ones(shape, self.config.default_dtype.into(), &self.device)?;
        Ok(NasoTensor::new(tensor))
    }

    pub fn randn(&self, shape: &[usize]) -> CandleResult<NasoTensor> {
        let tensor = Tensor::randn(0.0, 1.0, shape, &self.device)?;
        Ok(NasoTensor::new(tensor))
    }

    pub fn var_builder(&self) -> CandleResult<VarBuilder> {
        let var_map = self.var_map.read().await.clone();
        Ok(VarBuilder::from_varmap(&var_map, self.config.default_dtype.into(), &self.device))
    }

    pub async fn save_model(&self, name: &str, path: &PathBuf) -> Result<(), MLError> {
        let model = self.get_model(name).await
            .ok_or_else(|| MLError::ModelNotFound(name.to_string()))?;
        
        let state = model.state_dict();
        let var_map = VarMap::new();
        let vb = VarBuilder::from_varmap(&var_map, self.config.default_dtype.into(), &self.device);
        
        // Save would serialize the var_map
        let mut file = std::fs::File::create(path)?;
        // serialization logic
        Ok(())
    }

    pub async fn load_model(&self, name: &str, path: &PathBuf) -> Result<(), MLError> {
        let _ = path;
        // Load model from file
        Ok(())
    }
}

impl Default for MLFramework {
    fn default() -> Self {
        Self::new(MLConfig::default()).unwrap()
    }
}

#[derive(Error, Debug)]
pub enum MLError {
    #[error("Tensor error: {0}")]
    Tensor(#[from] candle_core::Error),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Training error: {0}")]
    Training(String),
    #[error("Inference error: {0}")]
    Inference(String),
    #[error("Data error: {0}")]
    Data(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Quantum error: {0}")]
    Quantum(String),
}

impl From<candle_core::Error> for MLError {
    fn from(e: candle_core::Error) -> Self {
        MLError::Tensor(e)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingMetrics {
    pub epoch: u32,
    pub loss: f64,
    pub accuracy: Option<f64>,
    pub val_loss: Option<f64>,
    pub val_accuracy: Option<f64>,
    pub duration_secs: f64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
    pub hidden_layers: Vec<usize>,
    pub activation: Activation,
    pub dropout: f32,
    pub batch_norm: bool,
    pub weight_decay: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Activation {
    Relu,
    Sigmoid,
    Tanh,
    Softmax,
    LeakyRelu { alpha: f32 },
    Gelu,
    Swish,
    Linear,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            input_shape: vec![784],
            output_shape: vec![10],
            hidden_layers: vec![256, 128],
            activation: Activation::Relu,
            dropout: 0.1,
            batch_norm: true,
            weight_decay: 0.0001,
        }
    }
}

pub async fn run_inference(
    framework: &MLFramework,
    model_name: &str,
    inputs: &[NasoTensor],
) -> CandleResult<Vec<NasoTensor>> {
    let model = framework.get_model(model_name).await
        .ok_or_else(|| candle_core::Error::Msg("Model not found".to_string()))?;
    
    model.forward(inputs).await
}