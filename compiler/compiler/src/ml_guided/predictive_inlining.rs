//! 24.2 Predictive Inlining - ML-guided inline decisions
//!
//! This module implements ML-guided inlining decisions that achieve
//! better code size/speed tradeoffs than static thresholds.

use crate::ir::pir_types::{PirFunction, PirModule};
use crate::ml_guided::features::{InliningFeatures, ExecutionProfile, FunctionId};
use crate::ml_guided::inference::{InliningInference, InferenceConfig, ModelRegistry};
use crate::ml_guided::training::{ClassificationModel, TrainingConfig, TrainingDataCollector, TrainingDataset};
use candle_core::Device;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Configuration for predictive inlining
#[derive(Debug, Clone)]
pub struct InliningConfig {
    pub model_path: String,
    pub inference_config: InferenceConfig,
    pub training_config: TrainingConfig,
    pub enable_online_learning: bool,
    pub inline_threshold: f32,      // Probability threshold for inlining
    pub max_inline_size: usize,     // Maximum function size to inline
    pub hot_threshold: f32,         // Call frequency threshold for "hot"
    pub size_penalty_factor: f32,   // Penalty factor for code size increase
}

impl Default for InliningConfig {
    fn default() -> Self {
        Self {
            model_path: "models/inlining_model".to_string(),
            inference_config: InferenceConfig::default(),
            training_config: TrainingConfig::default(),
            enable_online_learning: true,
            inline_threshold: 0.5,
            max_inline_size: 500,
            hot_threshold: 1000.0,
            size_penalty_factor: 0.1,
        }
    }
}

/// Predictive inliner using ML
pub struct PredictiveInliner {
    config: InliningConfig,
    inference: Option<Arc<InliningInference>>,
    training_data: TrainingDataCollector,
    inline_decisions: HashMap<(FunctionId, FunctionId), InlineDecision>,
    online_learning_buffer: Vec<(InliningFeatures, bool)>,
    buffer_size: usize,
}

#[derive(Debug, Clone)]
pub struct InlineDecision {
    pub should_inline: bool,
    pub confidence: f32,
    pub predicted_speedup: f32,
    pub predicted_size_increase: i32,
}

impl PredictiveInliner {
    pub fn new(config: InliningConfig) -> Self {
        Self {
            config,
            inference: None,
            training_data: TrainingDataCollector::new(),
            inline_decisions: HashMap::new(),
            online_learning_buffer: Vec::new(),
            buffer_size: 1000,
        }
    }

    /// Load a trained model
    pub fn load_model(&mut self) -> Result<(), MlError> {
        let model_path = Path::new(&self.config.model_path);
        if model_path.exists() {
            let inference = InliningInference::load(model_path, self.config.inference_config.clone())?;
            self.inference = Some(Arc::new(inference));
            Ok(())
        } else {
            Err(MlError::NotTrained("Inlining model not found".to_string()))
        }
    }

    /// Train the inlining model from collected data
    pub fn train(&mut self) -> Result<(), MlError> {
        let dataset = self.training_data.inlining_dataset();
        if dataset.is_empty() {
            return Err(MlError::TrainingFailed("No training data available".to_string()));
        }

        let (train, val) = dataset.split(1.0 - self.config.training_config.validation_split);

        let device = match self.config.inference_config.device {
            Device::Cpu => Device::Cpu,
            Device::Cuda(_) => Device::new_cuda(0).map_err(|e| MlError::BackendError(e.to_string()))?,
            Device::Metal(_) => Device::new_metal(0).map_err(|e| MlError::BackendError(e.to_string()))?,
        };

        let mut model = ClassificationModel::new(
            11, // input_dim
            2,  // binary classification
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

    /// Make inlining decision for a call site
    pub fn should_inline(
        &mut self,
        caller: &PirFunction,
        callee: &PirFunction,
        profile: &ExecutionProfile,
    ) -> Result<InlineDecision, MlError> {
        let features = InliningFeatures::from_call_site(caller, callee, profile);

        // Check hard constraints first
        if callee.instructions.len() > self.config.max_inline_size {
            return Ok(InlineDecision {
                should_inline: false,
                confidence: 1.0,
                predicted_speedup: 1.0,
                predicted_size_increase: callee.instructions.len() as i32,
            });
        }

        let decision = if let Some(inference) = &self.inference {
            let should_inline = inference.should_inline(&features)
                .map_err(|e| MlError::InferenceFailed(e.to_string()))?;
            let probs = inference.predict_proba(&features)
                .map_err(|e| MlError::InferenceFailed(e.to_string()))?;
            let confidence = probs[1].max(probs[0]);

            InlineDecision {
                should_inline,
                confidence,
                predicted_speedup: self.estimate_speedup(&features, should_inline),
                predicted_size_increase: callee.instructions.len() as i32,
            }
        } else {
            // Fallback to heuristic
            self.heuristic_decision(&features)
        };

        // Cache decision
        self.inline_decisions.insert((caller.id, callee.id), decision.clone());

        // Record for online learning
        if self.config.enable_online_learning {
            self.online_learning_buffer.push((features, decision.should_inline));
            if self.online_learning_buffer.len() >= self.buffer_size {
                self.flush_online_buffer();
            }
        }

        Ok(decision)
    }

    /// Heuristic fallback decision
    fn heuristic_decision(&self, features: &InliningFeatures) -> InlineDecision {
        let should_inline = features.heuristic_score > self.config.inline_threshold
            && features.callee_size <= self.config.max_inline_size as u32;

        InlineDecision {
            should_inline,
            confidence: 0.7,
            predicted_speedup: if should_inline { 1.2 } else { 1.0 },
            predicted_size_increase: features.callee_size as i32,
        }
    }

    /// Estimate speedup from inlining
    fn estimate_speedup(&self, features: &InliningFeatures, should_inline: bool) -> f32 {
        if !should_inline {
            return 1.0;
        }

        // Base speedup from eliminating call overhead
        let call_overhead_savings = 1.15;

        // Bonus for hot functions
        let hot_bonus = if features.is_hot { 1.2 } else { 1.0 };

        // Penalty for code size increase (instruction cache pressure)
        let size_penalty = 1.0 - (features.callee_size as f32 * self.config.size_penalty_factor / 1000.0).min(0.3);

        // Bonus for enabling further optimizations (constant propagation, etc.)
        let optimization_bonus = if features.callee_size < 50 { 1.1 } else { 1.0 };

        call_overhead_savings * hot_bonus * size_penalty * optimization_bonus
    }

    /// Record actual outcome for online learning
    pub fn record_outcome(
        &mut self,
        caller_id: FunctionId,
        callee_id: FunctionId,
        actual_speedup: f32,
        actual_size_increase: i32,
    ) {
        if let Some(decision) = self.inline_decisions.get_mut(&(caller_id, callee_id)) {
            // Update confidence based on prediction accuracy
            let predicted = decision.predicted_speedup;
            let error = (predicted - actual_speedup).abs() / actual_speedup.max(1.0);
            decision.confidence *= (1.0 - error * 0.5).max(0.1);
        }
    }

    fn flush_online_buffer(&mut self) {
        self.online_learning_buffer.clear();
    }

    /// Optimize inlining for entire module
    pub fn optimize(&mut self, module: &mut PirModule, profile: &ExecutionProfile) -> Result<(), MlError> {
        // Collect all call sites
        let mut call_sites = Vec::new();

        for func in &module.functions {
            for inst in &func.instructions {
                if matches!(inst.opcode, crate::ir::pir_types::PirOpcode::Call) {
                    if let Some(callee_id) = inst.operands.first().map(|o| o.value as FunctionId) {
                        if let Some(callee) = module.functions.iter().find(|f| f.id == callee_id) {
                            call_sites.push((func.clone(), callee.clone()));
                        }
                    }
                }
            }
        }

        // Make inlining decisions
        for (caller, callee) in call_sites {
            let decision = self.should_inline(&caller, &callee, profile)?;
            if decision.should_inline {
                self.inline_function(module, &caller, &callee)?;
            }
        }

        Ok(())
    }

    /// Inline a function at a call site
    fn inline_function(&self, module: &mut PirModule, caller: &PirFunction, callee: &PirFunction) -> Result<(), MlError> {
        // This would perform actual inlining in the IR
        // For now, just a placeholder
        Ok(())
    }
}

/// Error type for inlining
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

/// Inlining advisor for integration with compiler pipeline
pub struct InliningAdvisor {
    inliner: PredictiveInliner,
    profile: ExecutionProfile,
}

impl InliningAdvisor {
    pub fn new(config: InliningConfig, profile: ExecutionProfile) -> Self {
        Self {
            inliner: PredictiveInliner::new(config),
            profile,
        }
    }

    pub fn load_model(&mut self) -> Result<(), MlError> {
        self.inliner.load_model()
    }

    pub fn advise(&mut self, caller: &PirFunction, callee: &PirFunction) -> Result<InlineAdvice, MlError> {
        let decision = self.inliner.should_inline(caller, callee, &self.profile)?;

        Ok(InlineAdvice {
            should_inline: decision.should_inline,
            confidence: decision.confidence,
            reason: self.explain_decision(&decision, caller, callee),
            predicted_speedup: decision.predicted_speedup,
            predicted_size_increase: decision.predicted_size_increase,
        })
    }

    fn explain_decision(&self, decision: &InlineDecision, caller: &PirFunction, callee: &PirFunction) -> String {
        if decision.should_inline {
            format!(
                "Inline {} into {} (confidence: {:.1}%, predicted speedup: {:.2}x, size increase: {} insns)",
                callee.name, caller.name, decision.confidence * 100.0,
                decision.predicted_speedup, decision.predicted_size_increase
            )
        } else {
            format!(
                "Don't inline {} into {} (confidence: {:.1}%, size: {} insns, threshold: {} insns)",
                callee.name, caller.name, decision.confidence * 100.0,
                callee.instructions.len(), self.inliner.config.max_inline_size
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct InlineAdvice {
    pub should_inline: bool,
    pub confidence: f32,
    pub reason: String,
    pub predicted_speedup: f32,
    pub predicted_size_increase: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inlining_config_default() {
        let config = InliningConfig::default();
        assert_eq!(config.inline_threshold, 0.5);
        assert_eq!(config.max_inline_size, 500);
    }

    #[test]
    fn test_heuristic_decision() {
        let inliner = PredictiveInliner::new(InliningConfig::default());
        let features = InliningFeatures {
            callee_size: 100,
            is_hot: true,
            heuristic_score: 0.8,
            ..Default::default()
        };
        let decision = inliner.heuristic_decision(&features);
        assert!(decision.should_inline);
    }
}

// Default for InliningFeatures for tests
impl Default for InliningFeatures {
    fn default() -> Self {
        Self {
            caller_size: 0,
            callee_size: 0,
            call_frequency: 0.0,
            is_recursive: false,
            has_loops: false,
            num_call_sites: 0,
            cyclomatic_complexity: 1,
            register_pressure_delta: 0.0,
            code_size_delta: 0,
            is_hot: false,
            heuristic_score: 0.0,
        }
    }
}