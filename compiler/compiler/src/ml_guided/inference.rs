//! Inference infrastructure for ML-guided optimization
//!
//! This module provides fast inference for trained ML models
//! used in the compiler optimization pipeline.

use crate::ml_guided::features::{
    CircuitFeatures, InliningFeatures, InstructionFeatures, LoopFeatures, ProfileFeatures,
};
use crate::ml_guided::training::{ClassificationModel, MultiTaskModel, RegressionModel};
use candle_core::{Device, Result as CandleResult, Tensor};
use candle_nn::Module;
use ndarray::Array1;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Inference configuration
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    pub device: Device,
    pub batch_size: usize,
    pub use_quantization: bool,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            device: Device::Cpu,
            batch_size: 1,
            use_quantization: false,
        }
    }
}

/// Cost model inference for instruction selection
pub struct CostModelInference {
    model: RegressionModel,
    varmap: candle_nn::VarMap,
    config: InferenceConfig,
}

impl CostModelInference {
    pub fn load(model_path: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let varmap = candle_nn::VarMap::load(model_path)?;
        // Model architecture would be loaded from config
        let model = RegressionModel::new(
            32, // input_dim
            1,  // output_dim
            vec![
                crate::ml_guided::training::LayerConfig { size: 128, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 32, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            config.device.clone(),
        );

        Ok(Self { model, varmap, config })
    }

    pub fn predict(&self, features: &InstructionFeatures) -> CandleResult<f32> {
        let feature_vec = features.to_vector();
        let input = Tensor::from_vec(
            feature_vec.to_vec(),
            (1, feature_vec.len()),
            &self.config.device,
        )?;

        let model = self.model.build(&self.varmap)?;
        let output = model.forward(&input)?;
        let cost = output.to_scalar::<f32>()?;
        Ok(cost)
    }

    pub fn predict_batch(&self, features: &[InstructionFeatures]) -> CandleResult<Vec<f32>> {
        let batch_size = features.len();
        let feature_dim = features[0].to_vector().len();
        let input_data: Vec<f32> = features.iter()
            .flat_map(|f| f.to_vector().to_vec())
            .collect();

        let input = Tensor::from_vec(input_data, (batch_size, feature_dim), &self.config.device)?;
        let model = self.model.build(&self.varmap)?;
        let output = model.forward(&input)?;

        let results: Vec<f32> = (0..batch_size)
            .map(|i| output.i(i)?.to_scalar::<f32>())
            .collect::<CandleResult<Vec<_>>>()?;

        Ok(results)
    }
}

/// Inlining decision inference
pub struct InliningInference {
    model: ClassificationModel,
    varmap: candle_nn::VarMap,
    config: InferenceConfig,
}

impl InliningInference {
    pub fn load(model_path: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let varmap = candle_nn::VarMap::load(model_path)?;
        let model = ClassificationModel::new(
            11, // input_dim
            2,  // binary classification: inline / don't inline
            vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 32, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            config.device.clone(),
        );

        Ok(Self { model, varmap, config })
    }

    pub fn should_inline(&self, features: &InliningFeatures) -> CandleResult<bool> {
        let probs = self.predict_proba(features)?;
        Ok(probs[1] > probs[0]) // class 1 = inline
    }

    pub fn predict_proba(&self, features: &InliningFeatures) -> CandleResult<Vec<f32>> {
        let feature_vec = features.to_vector();
        let input = Tensor::from_vec(
            feature_vec.to_vec(),
            (1, feature_vec.len()),
            &self.config.device,
        )?;

        let model = self.model.build(&self.varmap)?;
        let output = model.forward(&input)?;
        let probs: Vec<f32> = (0..2).map(|i| output.i(i)?.to_scalar::<f32>()).collect::<CandleResult<Vec<_>>>()?;
        Ok(probs)
    }
}

/// Vectorization policy inference
pub struct VectorizationInference {
    model: ClassificationModel,
    varmap: candle_nn::VarMap,
    config: InferenceConfig,
    /// Available transformations
    transformations: Vec<VectorizationTransform>,
}

#[derive(Debug, Clone)]
pub struct VectorizationTransform {
    pub id: u32,
    pub name: String,
    pub description: String,
}

impl VectorizationInference {
    pub fn load(model_path: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let varmap = candle_nn::VarMap::load(model_path)?;
        let model = ClassificationModel::new(
            16, // input_dim
            10, // 10 possible transformations
            vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 32, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            config.device.clone(),
        );

        let transformations = vec![
            VectorizationTransform { id: 0, name: "none".to_string(), description: "No vectorization".to_string() },
            VectorizationTransform { id: 1, name: "simd_128".to_string(), description: "128-bit SIMD".to_string() },
            VectorizationTransform { id: 2, name: "simd_256".to_string(), description: "256-bit SIMD".to_string() },
            VectorizationTransform { id: 3, name: "simd_512".to_string(), description: "512-bit SIMD".to_string() },
            VectorizationTransform { id: 4, name: "loop_unroll_2".to_string(), description: "Unroll by 2".to_string() },
            VectorizationTransform { id: 5, name: "loop_unroll_4".to_string(), description: "Unroll by 4".to_string() },
            VectorizationTransform { id: 6, name: "loop_tiling".to_string(), description: "Loop tiling".to_string() },
            VectorizationTransform { id: 7, name: "loop_interchange".to_string(), description: "Loop interchange".to_string() },
            VectorizationTransform { id: 8, name: "loop_fusion".to_string(), description: "Loop fusion".to_string() },
            VectorizationTransform { id: 9, name: "scatter_gather".to_string(), description: "Scatter/gather vectorization".to_string() },
        ];

        Ok(Self { model, varmap, config, transformations })
    }

    pub fn predict_transformations(&self, features: &LoopFeatures) -> CandleResult<Vec<VectorizationTransform>> {
        let feature_vec = features.to_vector();
        let input = Tensor::from_vec(
            feature_vec.to_vec(),
            (1, feature_vec.len()),
            &self.config.device,
        )?;

        let model = self.model.build(&self.varmap)?;
        let output = model.forward(&input)?;
        let probs: Vec<f32> = (0..10).map(|i| output.i(i)?.to_scalar::<f32>()).collect::<CandleResult<Vec<_>>>()?;

        // Return transformations with probability > threshold
        let threshold = 0.3;
        let mut selected: Vec<_> = probs.iter()
            .enumerate()
            .filter(|(_, &p)| p > threshold)
            .map(|(i, _)| self.transformations[i].clone())
            .collect();

        // Sort by probability
        selected.sort_by(|a, b| {
            let pa = probs[a.id as usize];
            let pb = probs[b.id as usize];
            pb.partial_cmp(&pa).unwrap()
        });

        Ok(selected)
    }
}

/// Recompilation decision inference
pub struct RecompilationInference {
    model: RegressionModel,
    varmap: candle_nn::VarMap,
    config: InferenceConfig,
}

impl RecompilationInference {
    pub fn load(model_path: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let varmap = candle_nn::VarMap::load(model_path)?;
        let model = RegressionModel::new(
            15, // input_dim
            1,  // output: expected speedup
            vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 32, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            config.device.clone(),
        );

        Ok(Self { model, varmap, config })
    }

    pub fn predict_speedup(&self, features: &ProfileFeatures) -> CandleResult<f32> {
        let feature_vec = features.to_vector();
        let input = Tensor::from_vec(
            feature_vec.to_vec(),
            (1, feature_vec.len()),
            &self.config.device,
        )?;

        let model = self.model.build(&self.varmap)?;
        let output = model.forward(&input)?;
        let speedup = output.to_scalar::<f32>()?;
        Ok(speedup.max(1.0)) // At minimum, no slowdown
    }

    pub fn should_recompile(&self, features: &ProfileFeatures, threshold: f32) -> CandleResult<bool> {
        let speedup = self.predict_speedup(features)?;
        Ok(speedup > threshold)
    }
}

/// Quantum circuit optimization inference
pub struct QuantumOptimizationInference {
    model: MultiTaskModel,
    varmap: candle_nn::VarMap,
    config: InferenceConfig,
}

impl QuantumOptimizationInference {
    pub fn load(model_path: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let varmap = candle_nn::VarMap::load(model_path)?;
        let mut model = MultiTaskModel::new(32, config.device.clone()); // 12 base + ~20 gate distribution

        // Add task heads
        model.add_task("depth".to_string(), crate::ml_guided::training::TaskHead {
            layers: vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            output_dim: 1,
            task_type: crate::ml_guided::training::TaskType::Regression,
        });
        model.add_task("gate_count".to_string(), crate::ml_guided::training::TaskHead {
            layers: vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            output_dim: 1,
            task_type: crate::ml_guided::training::TaskType::Regression,
        });
        model.add_task("error_rate".to_string(), crate::ml_guided::training::TaskHead {
            layers: vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            output_dim: 1,
            task_type: crate::ml_guided::training::TaskType::Regression,
        });
        model.add_task("routing".to_string(), crate::ml_guided::training::TaskHead {
            layers: vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            output_dim: 1,
            task_type: crate::ml_guided::training::TaskType::Regression,
        });

        Ok(Self { model, varmap, config })
    }

    pub fn predict_optimization(&self, features: &CircuitFeatures) -> CandleResult<QuantumOptimizationPrediction> {
        let feature_vec = features.to_vector();
        let input = Tensor::from_vec(
            feature_vec.to_vec(),
            (1, feature_vec.len()),
            &self.config.device,
        )?;

        let heads = self.model.build(&self.varmap)?;

        let mut predictions = HashMap::new();
        for (name, head) in heads {
            let output = head.forward(&input)?;
            let value = output.to_scalar::<f32>()?;
            predictions.insert(name, value);
        }

        Ok(QuantumOptimizationPrediction {
            optimized_depth: predictions.get("depth").copied().unwrap_or(0.0) as u32,
            optimized_gate_count: predictions.get("gate_count").copied().unwrap_or(0.0) as u32,
            optimized_error_rate: predictions.get("error_rate").copied().unwrap_or(0.0),
            routing_overhead: predictions.get("routing").copied().unwrap_or(0.0) as u32,
        })
    }
}

#[derive(Debug, Clone)]
pub struct QuantumOptimizationPrediction {
    pub optimized_depth: u32,
    pub optimized_gate_count: u32,
    pub optimized_error_rate: f32,
    pub routing_overhead: u32,
}

/// Model registry for managing multiple models
pub struct ModelRegistry {
    cost_model: Option<Arc<CostModelInference>>,
    inlining_model: Option<Arc<InliningInference>>,
    vectorization_model: Option<Arc<VectorizationInference>>,
    recompilation_model: Option<Arc<RecompilationInference>>,
    quantum_model: Option<Arc<QuantumOptimizationInference>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            cost_model: None,
            inlining_model: None,
            vectorization_model: None,
            recompilation_model: None,
            quantum_model: None,
        }
    }

    pub fn load_all(model_dir: &Path, config: InferenceConfig) -> CandleResult<Self> {
        let mut registry = Self::new();

        if model_dir.join("cost_model").exists() {
            registry.cost_model = Some(Arc::new(CostModelInference::load(&model_dir.join("cost_model"), config.clone())?));
        }
        if model_dir.join("inlining_model").exists() {
            registry.inlining_model = Some(Arc::new(InliningInference::load(&model_dir.join("inlining_model"), config.clone())?));
        }
        if model_dir.join("vectorization_model").exists() {
            registry.vectorization_model = Some(Arc::new(VectorizationInference::load(&model_dir.join("vectorization_model"), config.clone())?));
        }
        if model_dir.join("recompilation_model").exists() {
            registry.recompilation_model = Some(Arc::new(RecompilationInference::load(&model_dir.join("recompilation_model"), config.clone())?));
        }
        if model_dir.join("quantum_model").exists() {
            registry.quantum_model = Some(Arc::new(QuantumOptimizationInference::load(&model_dir.join("quantum_model"), config.clone())?));
        }

        Ok(registry)
    }

    pub fn cost_model(&self) -> Option<&Arc<CostModelInference>> {
        self.cost_model.as_ref()
    }

    pub fn inlining_model(&self) -> Option<&Arc<InliningInference>> {
        self.inlining_model.as_ref()
    }

    pub fn vectorization_model(&self) -> Option<&Arc<VectorizationInference>> {
        self.vectorization_model.as_ref()
    }

    pub fn recompilation_model(&self) -> Option<&Arc<RecompilationInference>> {
        self.recompilation_model.as_ref()
    }

    pub fn quantum_model(&self) -> Option<&Arc<QuantumOptimizationInference>> {
        self.quantum_model.as_ref()
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inference_config_default() {
        let config = InferenceConfig::default();
        assert_eq!(config.batch_size, 1);
        assert!(!config.use_quantization);
    }

    #[test]
    fn test_model_registry_new() {
        let registry = ModelRegistry::new();
        assert!(registry.cost_model().is_none());
    }
}