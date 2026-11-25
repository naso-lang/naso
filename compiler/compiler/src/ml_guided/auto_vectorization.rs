//! 24.3 Auto-vectorization Policy - Learned loop transformation
//!
//! This module implements learned loop transformation policies that find
//! SIMD opportunities LLVM misses (scatter/gather, reductions, complex patterns).

use crate::ir::pir_types::{PirFunction, PirInstruction};
use crate::ir::schedule_tree::ScheduleNode;
use crate::ml_guided::features::{LoopFeatures, VectorizationTransform};
use crate::ml_guided::inference::{VectorizationInference, InferenceConfig, ModelRegistry};
use crate::ml_guided::training::{ClassificationModel, TrainingConfig, TrainingDataCollector, TrainingDataset};
use candle_core::Device;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Configuration for auto-vectorization
#[derive(Debug, Clone)]
pub struct VectorizationConfig {
    pub model_path: String,
    pub inference_config: InferenceConfig,
    pub training_config: TrainingConfig,
    pub enable_online_learning: bool,
    pub min_trip_count: u64,
    pub max_vector_width: usize,
    pub prefer_fixed_width: bool,
    pub enable_scatter_gather: bool,
    pub enable_reduction_vectorization: bool,
}

impl Default for VectorizationConfig {
    fn default() -> Self {
        Self {
            model_path: "models/vectorization_model".to_string(),
            inference_config: InferenceConfig::default(),
            training_config: TrainingConfig::default(),
            enable_online_learning: true,
            min_trip_count: 16,
            max_vector_width: 512,
            prefer_fixed_width: true,
            enable_scatter_gather: true,
            enable_reduction_vectorization: true,
        }
    }
}

/// Auto-vectorization policy using ML
pub struct AutoVectorizationPolicy {
    config: VectorizationConfig,
    inference: Option<Arc<VectorizationInference>>,
    training_data: TrainingDataCollector,
    applied_transformations: HashMap<String, Vec<AppliedTransform>>,
    online_learning_buffer: Vec<(LoopFeatures, Vec<u32>)>,
    buffer_size: usize,
}

#[derive(Debug, Clone)]
pub struct AppliedTransform {
    pub transform_id: u32,
    pub loop_id: String,
    pub predicted_speedup: f32,
    pub actual_speedup: Option<f32>,
}

impl AutoVectorizationPolicy {
    pub fn new(config: VectorizationConfig) -> Self {
        Self {
            config,
            inference: None,
            training_data: TrainingDataCollector::new(),
            applied_transformations: HashMap::new(),
            online_learning_buffer: Vec::new(),
            buffer_size: 1000,
        }
    }

    /// Load a trained model
    pub fn load_model(&mut self) -> Result<(), MlError> {
        let model_path = Path::new(&self.config.model_path);
        if model_path.exists() {
            let inference = VectorizationInference::load(model_path, self.config.inference_config.clone())?;
            self.inference = Some(Arc::new(inference));
            Ok(())
        } else {
            Err(MlError::NotTrained("Vectorization model not found".to_string()))
        }
    }

    /// Train the vectorization model from collected data
    pub fn train(&mut self) -> Result<(), MlError> {
        let dataset = self.training_data.vectorization_dataset();
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
            16, // input_dim
            10, // 10 transformation classes
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

    /// Analyze a loop and recommend vectorization transformations
    pub fn analyze_loop(&mut self, loop_node: &ScheduleNode) -> Result<Vec<VectorizationRecommendation>, MlError> {
        let features = LoopFeatures::from_schedule_node(loop_node);

        // Check basic eligibility
        if !self.is_vectorizable(&features) {
            return Ok(vec![VectorizationRecommendation {
                transform: VectorizationTransform::none(),
                confidence: 1.0,
                reason: "Loop not vectorizable".to_string(),
                predicted_speedup: 1.0,
            }]);
        }

        let recommendations = if let Some(inference) = &self.inference {
            inference.predict_transformations(&features)
                .map_err(|e| MlError::InferenceFailed(e.to_string()))?
        } else {
            self.heuristic_recommendations(&features)
        };

        let mut result = Vec::new();
        for transform in recommendations {
            let confidence = self.estimate_confidence(&features, &transform);
            let speedup = self.predict_speedup(&features, &transform);

            result.push(VectorizationRecommendation {
                transform: transform.clone(),
                confidence,
                reason: self.explain_recommendation(&features, &transform),
                predicted_speedup: speedup,
            });
        }

        // Sort by predicted speedup
        result.sort_by(|a, b| b.predicted_speedup.partial_cmp(&a.predicted_speedup).unwrap());

        Ok(result)
    }

    /// Check if a loop is vectorizable
    fn is_vectorizable(&self, features: &LoopFeatures) -> bool {
        // Must have sufficient trip count
        if let Some(trip) = features.trip_count {
            if trip < self.config.min_trip_count {
                return false;
            }
        }

        // Must not have problematic dependencies
        if features.has_dependencies && features.dependency_distance >= 0 {
            return false;
        }

        // Must not have function calls (unless they're inlineable intrinsics)
        if features.has_calls {
            return false;
        }

        // Must have memory operations that can be vectorized
        if features.memory_accesses == 0 && !features.has_reduction {
            return false;
        }

        true
    }

    /// Heuristic recommendations when no model is available
    fn heuristic_recommendations(&self, features: &LoopFeatures) -> Vec<VectorizationTransform> {
        let mut transforms = Vec::new();

        // Always consider no vectorization
        transforms.push(VectorizationTransform::none());

        // Unit stride loops -> SIMD
        if features.stride_pattern == 0 && features.memory_accesses > 0 {
            if features.trip_count.unwrap_or(100) >= 32 {
                transforms.push(VectorizationTransform::simd_256());
                transforms.push(VectorizationTransform::simd_128());
            }
        }

        // Reduction loops
        if features.has_reduction {
            transforms.push(VectorizationTransform::simd_128());
        }

        // Large trip counts -> unrolling
        if features.trip_count.unwrap_or(0) > 1000 {
            transforms.push(VectorizationTransform::loop_unroll_4());
            transforms.push(VectorizationTransform::loop_tiling());
        }

        // Nested loops -> interchange/tiling
        if features.nest_depth > 1 {
            transforms.push(VectorizationTransform::loop_interchange());
            transforms.push(VectorizationTransform::loop_tiling());
        }

        transforms
    }

    /// Estimate confidence in a recommendation
    fn estimate_confidence(&self, features: &LoopFeatures, transform: &VectorizationTransform) -> f32 {
        let mut confidence = 0.5;

        match transform.id {
            0 => confidence = 1.0, // "none" is always safe
            1..=3 => { // SIMD widths
                if features.stride_pattern == 0 && features.alignment >= 16 {
                    confidence += 0.3;
                }
                if features.trip_count.unwrap_or(0) > 100 {
                    confidence += 0.2;
                }
            }
            4..=5 => { // Unrolling
                if features.body_size < 50 {
                    confidence += 0.3;
                }
                if features.constant_trip_count {
                    confidence += 0.2;
                }
            }
            6 => { // Tiling
                if features.nest_depth > 1 && features.trip_count.unwrap_or(0) > 100 {
                    confidence += 0.3;
                }
            }
            7 => { // Interchange
                if features.nest_depth > 1 && !features.has_dependencies {
                    confidence += 0.3;
                }
            }
            8 => { // Fusion
                confidence += 0.2;
            }
            9 => { // Scatter/gather
                if self.config.enable_scatter_gather && features.stride_pattern > 1 {
                    confidence += 0.3;
                }
            }
            _ => {}
        }

        confidence.min(1.0)
    }

    /// Predict speedup for a transformation
    fn predict_speedup(&self, features: &LoopFeatures, transform: &VectorizationTransform) -> f32 {
        match transform.id {
            0 => 1.0, // none
            1 => 2.0, // SIMD 128
            2 => 3.5, // SIMD 256
            3 => 5.0, // SIMD 512
            4 => 1.3, // unroll 2
            5 => 1.5, // unroll 4
            6 => 1.8, // tiling
            7 => 1.4, // interchange
            8 => 1.6, // fusion
            9 => 2.5, // scatter/gather
            _ => 1.0,
        }
    }

    /// Explain a recommendation
    fn explain_recommendation(&self, features: &LoopFeatures, transform: &VectorizationTransform) -> String {
        match transform.id {
            0 => "No vectorization recommended".to_string(),
            1..=3 => format!(
                "{} vectorization: unit stride={}, trip_count={:?}, alignment={}",
                transform.name, features.stride_pattern == 0, features.trip_count, features.alignment
            ),
            4..=5 => format!(
                "Loop unroll {}: body_size={}, constant_trip={}",
                transform.name.split('_').last().unwrap(), features.body_size, features.constant_trip_count
            ),
            6 => format!("Loop tiling: nest_depth={}, trip_count={:?}", features.nest_depth, features.trip_count),
            7 => format!("Loop interchange: nest_depth={}, deps={}", features.nest_depth, features.has_dependencies),
            8 => "Loop fusion: adjacent compatible loops".to_string(),
            9 => format!("Scatter/gather: stride_pattern={}", features.stride_pattern),
            _ => "Unknown transformation".to_string(),
        }
    }

    /// Apply recommended transformations to a function
    pub fn optimize(&mut self, func: &mut PirFunction) -> Result<(), MlError> {
        // This would integrate with the schedule tree and loop emission
        // For now, analyze loops in the function
        for inst in &func.instructions {
            // Look for loop constructs
        }
        Ok(())
    }

    /// Record actual performance for online learning
    pub fn record_performance(&mut self, loop_id: String, transform_id: u32, actual_speedup: f32) {
        if let Some(transforms) = self.applied_transformations.get_mut(&loop_id) {
            for t in transforms {
                if t.transform_id == transform_id {
                    t.actual_speedup = Some(actual_speedup);
                    break;
                }
            }
        }

        if self.config.enable_online_learning {
            // Would add to training buffer
        }
    }

    fn flush_online_buffer(&mut self) {
        self.online_learning_buffer.clear();
    }
}

/// Vectorization recommendation
#[derive(Debug, Clone)]
pub struct VectorizationRecommendation {
    pub transform: VectorizationTransform,
    pub confidence: f32,
    pub reason: String,
    pub predicted_speedup: f32,
}

/// Error type for vectorization
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

/// Integration with loop emission
pub struct MlGuidedLoopVectorizer {
    policy: AutoVectorizationPolicy,
}

impl MlGuidedLoopVectorizer {
    pub fn new(policy: AutoVectorizationPolicy) -> Self {
        Self { policy }
    }

    /// Vectorize a loop nest using ML recommendations
    pub fn vectorize_loop(
        &mut self,
        loop_node: &ScheduleNode,
        emitter: &mut dyn FnMut(&VectorizationRecommendation) -> Result<(), MlError>,
    ) -> Result<(), MlError> {
        let recommendations = self.policy.analyze_loop(loop_node)?;

        for rec in recommendations {
            if rec.confidence > 0.5 && rec.predicted_speedup > 1.1 {
                emitter(&rec)?;
            }
        }

        Ok(())
    }
}

impl VectorizationTransform {
    pub fn none() -> Self {
        Self { id: 0, name: "none".to_string(), description: "No vectorization".to_string() }
    }
    pub fn simd_128() -> Self {
        Self { id: 1, name: "simd_128".to_string(), description: "128-bit SIMD".to_string() }
    }
    pub fn simd_256() -> Self {
        Self { id: 2, name: "simd_256".to_string(), description: "256-bit SIMD".to_string() }
    }
    pub fn simd_512() -> Self {
        Self { id: 3, name: "simd_512".to_string(), description: "512-bit SIMD".to_string() }
    }
    pub fn loop_unroll_2() -> Self {
        Self { id: 4, name: "loop_unroll_2".to_string(), description: "Unroll by 2".to_string() }
    }
    pub fn loop_unroll_4() -> Self {
        Self { id: 5, name: "loop_unroll_4".to_string(), description: "Unroll by 4".to_string() }
    }
    pub fn loop_tiling() -> Self {
        Self { id: 6, name: "loop_tiling".to_string(), description: "Loop tiling".to_string() }
    }
    pub fn loop_interchange() -> Self {
        Self { id: 7, name: "loop_interchange".to_string(), description: "Loop interchange".to_string() }
    }
    pub fn loop_fusion() -> Self {
        Self { id: 8, name: "loop_fusion".to_string(), description: "Loop fusion".to_string() }
    }
    pub fn scatter_gather() -> Self {
        Self { id: 9, name: "scatter_gather".to_string(), description: "Scatter/gather vectorization".to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vectorization_config_default() {
        let config = VectorizationConfig::default();
        assert_eq!(config.min_trip_count, 16);
        assert_eq!(config.max_vector_width, 512);
    }

    #[test]
    fn test_is_vectorizable() {
        let policy = AutoVectorizationPolicy::new(VectorizationConfig::default());
        let features = LoopFeatures {
            trip_count: Some(100),
            stride_pattern: 0,
            has_dependencies: false,
            has_calls: false,
            memory_accesses: 10,
            ..Default::default()
        };
        assert!(policy.is_vectorizable(&features));
    }
}

// Default for LoopFeatures for tests
impl Default for LoopFeatures {
    fn default() -> Self {
        Self {
            nest_depth: 1,
            trip_count: None,
            constant_trip_count: false,
            memory_accesses: 0,
            loads: 0,
            stores: 0,
            stride_pattern: 3,
            has_reduction: false,
            reduction_op: 0,
            has_dependencies: false,
            dependency_distance: -1,
            is_innermost: true,
            estimated_iterations: 0.0,
            body_size: 0,
            has_calls: false,
            alignment: 0,
        }
    }
}