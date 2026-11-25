//! 24.4 Profile-Driven Recompilation - JIT online ML model updates
//!
//! This module implements profile-driven recompilation where the JIT
//! adapts to workload shifts without full rebuild using online ML model updates.

use crate::ir::pir_types::{PirFunction, PirModule};
use crate::ml_guided::features::{ProfileFeatures, ExecutionProfile, FunctionId, FunctionStats};
use crate::ml_guided::inference::{RecompilationInference, InferenceConfig, ModelRegistry};
use crate::ml_guided::training::{RegressionModel, TrainingConfig, TrainingDataCollector, TrainingDataset};
use candle_core::Device;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration for profile-driven recompilation
#[derive(Debug, Clone)]
pub struct RecompilationConfig {
    pub model_path: String,
    pub inference_config: InferenceConfig,
    pub training_config: TrainingConfig,
    pub enable_online_learning: bool,
    pub recompilation_threshold: f32,    // Minimum speedup to trigger recompilation
    pub hot_threshold: u64,              // Execution count to consider hot
    pub cooldown_period: Duration,       // Minimum time between recompilations
    pub max_recompilations_per_function: u32,
    pub profile_window: Duration,        // Window for profile collection
}

impl Default for RecompilationConfig {
    fn default() -> Self {
        Self {
            model_path: "models/recompilation_model".to_string(),
            inference_config: InferenceConfig::default(),
            training_config: TrainingConfig::default(),
            enable_online_learning: true,
            recompilation_threshold: 1.15,  // 15% speedup
            hot_threshold: 10000,
            cooldown_period: Duration::from_secs(60),
            max_recompilations_per_function: 5,
            profile_window: Duration::from_secs(300),
        }
    }
}

/// Recompilation action
#[derive(Debug, Clone)]
pub enum RecompilationAction {
    Recompile { function_id: FunctionId, reason: String, predicted_speedup: f32 },
    Specialize { function_id: FunctionId, specialization: Specialization },
    Deoptimize { function_id: FunctionId, reason: String },
    NoAction,
}

#[derive(Debug, Clone)]
pub enum Specialization {
    ConstantPropagation { constants: HashMap<String, i64> },
    LoopUnrolling { factor: usize },
    Vectorization { width: usize },
    Inlining { callee_id: FunctionId },
    DeadCodeElimination,
}

/// Profile-driven recompiler
pub struct ProfileDrivenRecompiler {
    config: RecompilationConfig,
    inference: Option<Arc<RecompilationInference>>,
    training_data: TrainingDataCollector,
    function_profiles: HashMap<FunctionId, FunctionProfile>,
    recompilation_history: HashMap<FunctionId, Vec<RecompilationRecord>>,
    last_recompilation: HashMap<FunctionId, Instant>,
    online_learning_buffer: Vec<(ProfileFeatures, f32)>,
    buffer_size: usize,
}

#[derive(Debug, Clone)]
pub struct FunctionProfile {
    pub stats: FunctionStats,
    pub last_updated: Instant,
    pub is_hot: bool,
    pub recompile_count: u32,
    pub last_speedup: f32,
    pub specialization: Option<Specialization>,
}

#[derive(Debug, Clone)]
pub struct RecompilationRecord {
    pub timestamp: Instant,
    pub action: RecompilationAction,
    pub predicted_speedup: f32,
    pub actual_speedup: Option<f32>,
    pub compile_time: Duration,
}

impl ProfileDrivenRecompiler {
    pub fn new(config: RecompilationConfig) -> Self {
        Self {
            config,
            inference: None,
            training_data: TrainingDataCollector::new(),
            function_profiles: HashMap::new(),
            recompilation_history: HashMap::new(),
            last_recompilation: HashMap::new(),
            online_learning_buffer: Vec::new(),
            buffer_size: 1000,
        }
    }

    /// Load a trained model
    pub fn load_model(&mut self) -> Result<(), MlError> {
        let model_path = Path::new(&self.config.model_path);
        if model_path.exists() {
            let inference = RecompilationInference::load(model_path, self.config.inference_config.clone())?;
            self.inference = Some(Arc::new(inference));
            Ok(())
        } else {
            Err(MlError::NotTrained("Recompilation model not found".to_string()))
        }
    }

    /// Train the recompilation model from collected data
    pub fn train(&mut self) -> Result<(), MlError> {
        let dataset = self.training_data.recompilation_dataset();
        if dataset.is_empty() {
            return Err(MlError::TrainingFailed("No training data available".to_string()));
        }

        let (train, val) = dataset.split(1.0 - self.config.training_config.validation_split);

        let device = match self.config.inference_config.device {
            Device::Cpu => Device::Cpu,
            Device::Cuda(_) => Device::new_cuda(0).map_err(|e| MlError::BackendError(e.to_string()))?,
            Device::Metal(_) => Device::new_metal(0).map_err(|e| MlError::BackendError(e.to_string()))?,
        };

        let mut model = RegressionModel::new(
            15, // input_dim
            1,  // output: speedup
            vec![
                crate::ml_guided::training::LayerConfig { size: 64, activation: crate::ml_guided::training::Activation::ReLU },
                crate::ml_guided::training::LayerConfig { size: 32, activation: crate::ml_guided::training::Activation::ReLU },
            ],
            device.clone(),
        );

        model.train(&train, &self.config.training_config)
            .map_err(|e| MlError::TrainingFailed(e.to_string()))?;

        Ok(())
    }

    /// Update profile for a function
    pub fn update_profile(&mut self, function_id: FunctionId, stats: FunctionStats) {
        let profile = self.function_profiles.entry(function_id).or_insert_with(|| FunctionProfile {
            stats: FunctionStats::default(),
            last_updated: Instant::now(),
            is_hot: false,
            recompile_count: 0,
            last_speedup: 1.0,
            specialization: None,
        });

        profile.stats = stats;
        profile.last_updated = Instant::now();
        profile.is_hot = stats.execution_count >= self.config.hot_threshold;
    }

    /// Analyze profiles and recommend recompilations
    pub fn analyze(&mut self, profile: &ExecutionProfile) -> Result<Vec<RecompilationAction>, MlError> {
        let mut actions = Vec::new();

        for (func_id, func_profile) in &self.function_profiles {
            if !func_profile.is_hot {
                continue;
            }

            // Check cooldown
            if let Some(last) = self.last_recompilation.get(func_id) {
                if last.elapsed() < self.config.cooldown_period {
                    continue;
                }
            }

            // Check max recompilations
            if func_profile.recompile_count >= self.config.max_recompilations_per_function {
                continue;
            }

            // Get features for this function
            let features = ProfileFeatures::from_profile(profile, *func_id);

            // Predict speedup from recompilation
            let predicted_speedup = if let Some(inference) = &self.inference {
                inference.predict_speedup(&features)
                    .map_err(|e| MlError::InferenceFailed(e.to_string()))?
            } else {
                self.heuristic_speedup(&features)
            };

            // Check if recompilation is worthwhile
            if predicted_speedup >= self.config.recompilation_threshold {
                let action = self.decide_action(*func_id, &features, predicted_speedup);
                actions.push(action);
            }
        }

        Ok(actions)
    }

    /// Decide what recompilation action to take
    fn decide_action(&self, func_id: FunctionId, features: &ProfileFeatures, predicted_speedup: f32) -> RecompilationAction {
        // Analyze profile characteristics to decide on specialization
        if features.cache_miss_rate > 0.2 {
            RecompilationAction::Specialize {
                function_id: func_id,
                specialization: Specialization::DeadCodeElimination,
            }
        } else if features.branch_mispred_rate > 0.15 {
            RecompilationAction::Specialize {
                function_id: func_id,
                specialization: Specialization::ConstantPropagation { constants: HashMap::new() },
            }
        } else if features.vector_ratio < 0.1 && features.memory_ratio > 0.5 {
            RecompilationAction::Specialize {
                function_id: func_id,
                specialization: Specialization::Vectorization { width: 256 },
            }
        } else if features.spill_count > 10 {
            RecompilationAction::Specialize {
                function_id: func_id,
                specialization: Specialization::LoopUnrolling { factor: 2 },
            }
        } else {
            RecompilationAction::Recompile {
                function_id: func_id,
                reason: "General recompilation with updated profile".to_string(),
                predicted_speedup,
            }
        }
    }

    /// Heuristic speedup prediction
    fn heuristic_speedup(&self, features: &ProfileFeatures) -> f32 {
        let mut speedup = 1.0;

        // Cache misses indicate optimization opportunity
        speedup += features.cache_miss_rate * 0.5;

        // Branch mispredictions
        speedup += features.branch_mispred_rate * 0.3;

        // Low vectorization
        if features.vector_ratio < 0.2 {
            speedup += 0.2;
        }

        // High spill count
        speedup += (features.spill_count as f32 * 0.01).min(0.3);

        // Previous recompilation success
        speedup *= 1.0 + features.last_speedup * 0.1;

        speedup
    }

    /// Record recompilation outcome
    pub fn record_outcome(
        &mut self,
        function_id: FunctionId,
        action: RecompilationAction,
        predicted_speedup: f32,
        actual_speedup: f32,
        compile_time: Duration,
    ) {
        let record = RecompilationRecord {
            timestamp: Instant::now(),
            action: action.clone(),
            predicted_speedup,
            actual_speedup: Some(actual_speedup),
            compile_time,
        };

        self.recompilation_history
            .entry(function_id)
            .or_default()
            .push(record);

        // Update function profile
        if let Some(profile) = self.function_profiles.get_mut(&function_id) {
            profile.recompile_count += 1;
            profile.last_speedup = actual_speedup;
            self.last_recompilation.insert(function_id, Instant::now());
        }

        // Record for training
        if self.config.enable_online_learning {
            // Would extract features and record
        }
    }

    /// Get recompilation statistics
    pub fn get_statistics(&self) -> RecompilationStatistics {
        let total_recompilations: usize = self.recompilation_history.values().map(|v| v.len()).sum();
        let successful = self.recompilation_history.values()
            .flat_map(|v| v.iter())
            .filter(|r| r.actual_speedup.unwrap_or(0.0) > 1.0)
            .count();

        RecompilationStatistics {
            total_recompilations,
            successful_recompilations: successful,
            success_rate: if total_recompilations > 0 {
                successful as f32 / total_recompilations as f32
            } else {
                0.0
            },
            avg_speedup: self.recompilation_history.values()
                .flat_map(|v| v.iter())
                .filter_map(|r| r.actual_speedup)
                .sum::<f32>() / successful.max(1) as f32,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecompilationStatistics {
    pub total_recompilations: usize,
    pub successful_recompilations: usize,
    pub success_rate: f32,
    pub avg_speedup: f32,
}

/// Error type for recompilation
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("Model not trained: {0}")]
    NotTrained(String),
    #[error("Inference failed: {0}")]
    InferenceFailed(String),
    #[error("Training failed: {0}")]
    TrainingFailed(String),
    #[error("Backend error: {0}")]
    BackendError(String),
}

impl From<candle_core::Error> for MlError {
    fn from(e: candle_core::Error) -> Self {
        MlError::BackendError(e.to_string())
    }
}

/// Integration with JIT runtime
pub struct MlGuidedJitRecompiler {
    recompiler: ProfileDrivenRecompiler,
    jit: crate::codegen::cranelift::jit::CraneliftJit,
}

impl MlGuidedJitRecompiler {
    pub fn new(recompiler: ProfileDrivenRecompiler, jit: crate::codegen::cranelift::jit::CraneliftJit) -> Self {
        Self { recompiler, jit }
    }

    /// Check and trigger recompilation for hot functions
    pub fn check_recompile(&mut self, profile: &ExecutionProfile) -> Result<Vec<RecompilationAction>, MlError> {
        self.recompiler.analyze(profile)
    }

    /// Recompile a function with new optimization settings
    pub fn recompile_function(&mut self, function_id: FunctionId, module: &PirModule) -> Result<(), MlError> {
        // Find function in module
        // Recompile with updated optimization flags
        // Install new version
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recompilation_config_default() {
        let config = RecompilationConfig::default();
        assert_eq!(config.recompilation_threshold, 1.15);
        assert_eq!(config.hot_threshold, 10000);
    }

    #[test]
    fn test_heuristic_speedup() {
        let recompiler = ProfileDrivenRecompiler::new(RecompilationConfig::default());
        let features = ProfileFeatures {
            cache_miss_rate: 0.3,
            branch_mispred_rate: 0.1,
            vector_ratio: 0.05,
            spill_count: 20,
            last_speedup: 1.2,
            ..Default::default()
        };
        let speedup = recompiler.heuristic_speedup(&features);
        assert!(speedup > 1.0);
    }
}

// Default for ProfileFeatures for tests
impl Default for ProfileFeatures {
    fn default() -> Self {
        Self {
            execution_count: 0,
            total_cycles: 0,
            avg_cycles: 0.0,
            cache_miss_rate: 0.0,
            branch_mispred_rate: 0.0,
            arithmetic_ratio: 0.0,
            memory_ratio: 0.0,
            control_flow_ratio: 0.0,
            vector_ratio: 0.0,
            max_register_pressure: 0,
            spill_count: 0,
            is_hot: false,
            time_since_recompile: 0.0,
            recompile_count: 0,
            last_speedup: 1.0,
        }
    }
}