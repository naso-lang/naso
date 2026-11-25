//! Training infrastructure for ML-guided optimization models
//!
//! This module provides training pipelines for the various ML models
//! used in the compiler optimization pipeline.

use crate::ml_guided::features::{
    CircuitFeatures, InliningFeatures, InstructionFeatures, LoopFeatures, ProfileFeatures,
};
use candle_core::{Device, Result as CandleResult, Tensor};
use candle_nn::{Module, VarBuilder, VarMap};
use ndarray::Array1;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Training configuration for ML models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingConfig {
    /// Learning rate
    pub learning_rate: f64,
    /// Batch size
    pub batch_size: usize,
    /// Number of epochs
    pub epochs: usize,
    /// Device to use (cpu/cuda)
    pub device: String,
    /// Model save path
    pub model_path: String,
    /// Whether to use mixed precision
    pub mixed_precision: bool,
    /// Early stopping patience
    pub early_stopping_patience: usize,
    /// Validation split ratio
    pub validation_split: f32,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            learning_rate: 1e-3,
            batch_size: 32,
            epochs: 100,
            device: "cpu".to_string(),
            model_path: "models/".to_string(),
            mixed_precision: false,
            early_stopping_patience: 10,
            validation_split: 0.2,
        }
    }
}

/// Generic training dataset
#[derive(Debug, Clone)]
pub struct TrainingDataset<F, T> {
    pub features: Vec<F>,
    pub targets: Vec<T>,
    pub feature_dim: usize,
    pub target_dim: usize,
}

impl<F, T> TrainingDataset<F, T>
where
    F: Clone + Send + Sync,
    T: Clone + Send + Sync,
{
    pub fn new(features: Vec<F>, targets: Vec<T>, feature_dim: usize, target_dim: usize) -> Self {
        assert_eq!(features.len(), targets.len());
        Self {
            features,
            targets,
            feature_dim,
            target_dim,
        }
    }

    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    pub fn split(self, ratio: f32) -> (Self, Self) {
        let split_idx = (self.len() as f32 * ratio) as usize;
        let (train_features, val_features) = self.features.split_at(split_idx);
        let (train_targets, val_targets) = self.targets.split_at(split_idx);

        (
            Self::new(train_features.to_vec(), train_targets.to_vec(), self.feature_dim, self.target_dim),
            Self::new(val_features.to_vec(), val_targets.to_vec(), self.feature_dim, self.target_dim),
        )
    }
}

/// Regression model for cost prediction
#[derive(Debug, Clone)]
pub struct RegressionModel {
    pub layers: Vec<LayerConfig>,
    pub input_dim: usize,
    pub output_dim: usize,
    pub device: Device,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerConfig {
    pub size: usize,
    pub activation: Activation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Activation {
    ReLU,
    Tanh,
    Sigmoid,
    Linear,
    GELU,
}

impl RegressionModel {
    pub fn new(input_dim: usize, output_dim: usize, layers: Vec<LayerConfig>, device: Device) -> Self {
        Self {
            layers,
            input_dim,
            output_dim,
            device,
        }
    }

    /// Build the model using candle
    pub fn build(&self, varmap: &VarMap) -> CandleResult<candle_nn::Module> {
        let vb = VarBuilder::from_varmap(varmap, candle_core::DType::F32, &self.device);

        let mut modules: Vec<Box<dyn Module>> = Vec::new();
        let mut prev_dim = self.input_dim;

        for (i, layer) in self.layers.iter().enumerate() {
            let linear = candle_nn::linear(prev_dim, layer.size, vb.pp(&format!("layer_{}", i)))?;
            modules.push(Box::new(linear));

            match layer.activation {
                Activation::ReLU => modules.push(Box::new(candle_nn::Activation::Relu)),
                Activation::Tanh => modules.push(Box::new(candle_nn::Activation::Tanh)),
                Activation::Sigmoid => modules.push(Box::new(candle_nn::Activation::Sigmoid)),
                Activation::Linear => {}
                Activation::GELU => modules.push(Box::new(candle_nn::Activation::Gelu)),
            }
            prev_dim = layer.size;
        }

        // Output layer
        let output_linear = candle_nn::linear(prev_dim, self.output_dim, vb.pp("output"))?;
        modules.push(Box::new(output_linear));

        Ok(candle_nn::Sequential::new(modules))
    }

    /// Train the model on a dataset
    pub fn train<F, T>(
        &mut self,
        dataset: &TrainingDataset<F, T>,
        config: &TrainingConfig,
    ) -> CandleResult<()>
    where
        F: AsRef<Array1<f32>>,
        T: AsRef<Array1<f32>>,
    {
        let varmap = VarMap::new();
        let model = self.build(&varmap)?;
        let mut opt = candle_nn::AdamW::new(varmap.all_vars(), config.learning_rate)?;

        let device = &self.device;

        for epoch in 0..config.epochs {
            let mut total_loss = 0.0f32;
            let mut batches = 0;

            for i in (0..dataset.len()).step_by(config.batch_size) {
                let end = (i + config.batch_size).min(dataset.len());
                let batch_features = &dataset.features[i..end];
                let batch_targets = &dataset.targets[i..end];

                // Convert to tensors
                let feature_data: Vec<f32> = batch_features.iter()
                    .flat_map(|f| f.as_ref().iter().cloned())
                    .collect();
                let target_data: Vec<f32> = batch_targets.iter()
                    .flat_map(|t| t.as_ref().iter().cloned())
                    .collect();

                let features_tensor = Tensor::from_vec(feature_data, (batch_features.len(), self.input_dim), device)?;
                let targets_tensor = Tensor::from_vec(target_data, (batch_targets.len(), self.output_dim), device)?;

                let predictions = model.forward(&features_tensor)?;
                let loss = predictions.mse_loss(&targets_tensor)?;

                opt.backward_step(&loss)?;
                total_loss += loss.to_scalar::<f32>()?;
                batches += 1;
            }

            let avg_loss = total_loss / batches as f32;
            println!("Epoch {}: loss = {:.6}", epoch, avg_loss);

            // Early stopping would go here
        }

        // Save model
        varmap.save(&config.model_path)?;

        Ok(())
    }
}

/// Classification model for inlining decisions
#[derive(Debug, Clone)]
pub struct ClassificationModel {
    pub layers: Vec<LayerConfig>,
    pub input_dim: usize,
    pub num_classes: usize,
    pub device: Device,
}

impl ClassificationModel {
    pub fn new(input_dim: usize, num_classes: usize, layers: Vec<LayerConfig>, device: Device) -> Self {
        Self {
            layers,
            input_dim,
            num_classes,
            device,
        }
    }

    pub fn build(&self, varmap: &VarMap) -> CandleResult<candle_nn::Module> {
        let vb = VarBuilder::from_varmap(varmap, candle_core::DType::F32, &self.device);

        let mut modules: Vec<Box<dyn Module>> = Vec::new();
        let mut prev_dim = self.input_dim;

        for (i, layer) in self.layers.iter().enumerate() {
            let linear = candle_nn::linear(prev_dim, layer.size, vb.pp(&format!("layer_{}", i)))?;
            modules.push(Box::new(linear));

            match layer.activation {
                Activation::ReLU => modules.push(Box::new(candle_nn::Activation::Relu)),
                Activation::Tanh => modules.push(Box::new(candle_nn::Activation::Tanh)),
                Activation::Sigmoid => modules.push(Box::new(candle_nn::Activation::Sigmoid)),
                Activation::Linear => {}
                Activation::GELU => modules.push(Box::new(candle_nn::Activation::Gelu)),
            }
            prev_dim = layer.size;
        }

        let output_linear = candle_nn::linear(prev_dim, self.num_classes, vb.pp("output"))?;
        modules.push(Box::new(output_linear));
        modules.push(Box::new(candle_nn::Activation::Softmax));

        Ok(candle_nn::Sequential::new(modules))
    }
}

/// Multi-task model for quantum circuit optimization
#[derive(Debug, Clone)]
pub struct MultiTaskModel {
    pub shared_layers: Vec<LayerConfig>,
    pub task_heads: HashMap<String, TaskHead>,
    pub input_dim: usize,
    pub device: Device,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHead {
    pub layers: Vec<LayerConfig>,
    pub output_dim: usize,
    pub task_type: TaskType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    Regression,
    Classification { num_classes: usize },
}

impl MultiTaskModel {
    pub fn new(input_dim: usize, device: Device) -> Self {
        Self {
            shared_layers: vec![],
            task_heads: HashMap::new(),
            input_dim,
            device,
        }
    }

    pub fn add_task(&mut self, name: String, head: TaskHead) {
        self.task_heads.insert(name, head);
    }

    pub fn build(&self, varmap: &VarMap) -> CandleResult<HashMap<String, candle_nn::Module>> {
        let vb = VarBuilder::from_varmap(varmap, candle_core::DType::F32, &self.device);

        // Build shared trunk
        let mut shared_modules: Vec<Box<dyn Module>> = Vec::new();
        let mut prev_dim = self.input_dim;

        for (i, layer) in self.shared_layers.iter().enumerate() {
            let linear = candle_nn::linear(prev_dim, layer.size, vb.pp(&format!("shared_{}", i)))?;
            shared_modules.push(Box::new(linear));

            match layer.activation {
                Activation::ReLU => shared_modules.push(Box::new(candle_nn::Activation::Relu)),
                Activation::Tanh => shared_modules.push(Box::new(candle_nn::Activation::Tanh)),
                Activation::Sigmoid => shared_modules.push(Box::new(candle_nn::Activation::Sigmoid)),
                Activation::Linear => {}
                Activation::GELU => shared_modules.push(Box::new(candle_nn::Activation::Gelu)),
            }
            prev_dim = layer.size;
        }

        let shared = candle_nn::Sequential::new(shared_modules);

        // Build task heads
        let mut heads = HashMap::new();
        for (name, head) in &self.task_heads {
            let mut head_modules: Vec<Box<dyn Module>> = Vec::new();
            let mut head_prev_dim = prev_dim;

            for (i, layer) in head.layers.iter().enumerate() {
                let linear = candle_nn::linear(head_prev_dim, layer.size, vb.pp(&format!("head_{}_{}", name, i)))?;
                head_modules.push(Box::new(linear));

                match layer.activation {
                    Activation::ReLU => head_modules.push(Box::new(candle_nn::Activation::Relu)),
                    Activation::Tanh => head_modules.push(Box::new(candle_nn::Activation::Tanh)),
                    Activation::Sigmoid => head_modules.push(Box::new(candle_nn::Activation::Sigmoid)),
                    Activation::Linear => {}
                    Activation::GELU => head_modules.push(Box::new(candle_nn::Activation::Gelu)),
                }
                head_prev_dim = layer.size;
            }

            let output_linear = candle_nn::linear(head_prev_dim, head.output_dim, vb.pp(&format!("head_{}_output", name)))?;
            head_modules.push(Box::new(output_linear));

            match head.task_type {
                TaskType::Classification { num_classes: _ } => {
                    head_modules.push(Box::new(candle_nn::Activation::Softmax));
                }
                TaskType::Regression => {}
            }

            heads.insert(name.clone(), candle_nn::Sequential::new(head_modules));
        }

        Ok(heads)
    }
}

/// Training data collector for online learning
pub struct TrainingDataCollector {
    instruction_data: Vec<(InstructionFeatures, f32)>, // features, actual_cost
    inlining_data: Vec<(InliningFeatures, bool)>,      // features, should_inline
    vectorization_data: Vec<(LoopFeatures, Vec<u32>)>, // features, transformation_ids
    recompilation_data: Vec<(ProfileFeatures, f32)>,   // features, speedup
    quantum_data: Vec<(CircuitFeatures, CircuitOptimizationTarget)>,
}

#[derive(Debug, Clone)]
pub struct CircuitOptimizationTarget {
    pub optimized_depth: u32,
    pub optimized_gate_count: u32,
    pub optimized_error_rate: f32,
    pub routing_overhead: u32,
}

impl TrainingDataCollector {
    pub fn new() -> Self {
        Self {
            instruction_data: Vec::new(),
            inlining_data: Vec::new(),
            vectorization_data: Vec::new(),
            recompilation_data: Vec::new(),
            quantum_data: Vec::new(),
        }
    }

    pub fn record_instruction_cost(&mut self, features: InstructionFeatures, actual_cost: f32) {
        self.instruction_data.push((features, actual_cost));
    }

    pub fn record_inlining_decision(&mut self, features: InliningFeatures, should_inline: bool) {
        self.inlining_data.push((features, should_inline));
    }

    pub fn record_vectorization(&mut self, features: LoopFeatures, transformations: Vec<u32>) {
        self.vectorization_data.push((features, transformations));
    }

    pub fn record_recompilation(&mut self, features: ProfileFeatures, speedup: f32) {
        self.recompilation_data.push((features, speedup));
    }

    pub fn record_quantum_optimization(&mut self, features: CircuitFeatures, target: CircuitOptimizationTarget) {
        self.quantum_data.push((features, target));
    }

    pub fn instruction_dataset(&self) -> TrainingDataset<InstructionFeatures, Array1<f32>> {
        let features = self.instruction_data.iter().map(|(f, _)| f.clone()).collect();
        let targets = self.instruction_data.iter().map(|(_, t)| Array1::from_vec(vec![*t])).collect();
        TrainingDataset::new(features, targets, 32, 1)
    }

    pub fn inlining_dataset(&self) -> TrainingDataset<InliningFeatures, Array1<f32>> {
        let features = self.inlining_data.iter().map(|(f, _)| f.clone()).collect();
        let targets = self.inlining_data.iter().map(|(_, t)| Array1::from_vec(vec![if *t { 1.0 } else { 0.0 }])).collect();
        TrainingDataset::new(features, targets, 11, 1)
    }

    pub fn vectorization_dataset(&self) -> TrainingDataset<LoopFeatures, Array1<f32>> {
        let features = self.vectorization_data.iter().map(|(f, _)| f.clone()).collect();
        let targets = self.vectorization_data.iter().map(|(_, t)| {
            let mut v = vec![0.0; 10];
            for &idx in t {
                if idx < 10 { v[idx as usize] = 1.0; }
            }
            Array1::from_vec(v)
        }).collect();
        TrainingDataset::new(features, targets, 16, 10)
    }

    pub fn recompilation_dataset(&self) -> TrainingDataset<ProfileFeatures, Array1<f32>> {
        let features = self.recompilation_data.iter().map(|(f, _)| f.clone()).collect();
        let targets = self.recompilation_data.iter().map(|(_, t)| Array1::from_vec(vec![*t])).collect();
        TrainingDataset::new(features, targets, 15, 1)
    }

    pub fn quantum_dataset(&self) -> TrainingDataset<CircuitFeatures, Array1<f32>> {
        let features = self.quantum_data.iter().map(|(f, _)| f.clone()).collect();
        let targets = self.quantum_data.iter().map(|(_, t)| Array1::from_vec(vec![
            t.optimized_depth as f32,
            t.optimized_gate_count as f32,
            t.optimized_error_rate,
            t.routing_overhead as f32,
        ])).collect();
        TrainingDataset::new(features, targets, 12 + 20, 4) // 12 base + gate_distribution
    }
}

impl Default for TrainingDataCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Model persistence utilities
pub mod persistence {
    use super::*;
    use candle_core::Result as CandleResult;

    pub fn save_varmap(varmap: &VarMap, path: &Path) -> CandleResult<()> {
        varmap.save(path)
    }

    pub fn load_varmap(path: &Path) -> CandleResult<VarMap> {
        VarMap::load(path)
    }

    pub fn save_config<T: Serialize>(config: &T, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(config)?;
        std::fs::write(path, json)
    }

    pub fn load_config<T: for<'de> Deserialize<'de>>(path: &Path) -> std::io::Result<T> {
        let json = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_training_config_default() {
        let config = TrainingConfig::default();
        assert_eq!(config.learning_rate, 1e-3);
        assert_eq!(config.batch_size, 32);
    }

    #[test]
    fn test_dataset_split() {
        let features = vec![1, 2, 3, 4, 5];
        let targets = vec![10, 20, 30, 40, 50];
        let dataset = TrainingDataset::new(features, targets, 1, 1);
        let (train, val) = dataset.split(0.6);
        assert_eq!(train.len(), 3);
        assert_eq!(val.len(), 2);
    }
}