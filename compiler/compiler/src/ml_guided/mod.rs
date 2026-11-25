//! ML-Guided Optimization (Phase 24)
//!
//! Compiler learns optimal optimizations from codebase — beyond static heuristics.
//!
//! This module implements:
//! - 24.1 Learned cost model: ML-based instruction selection
//! - 24.2 Predictive inlining: ML-guided inline decisions
//! - 24.3 Auto-vectorization policy: Learned loop transformation
//! - 24.4 Profile-driven recompilation: JIT online ML model updates
//! - 24.5 Quantum circuit optimization: ML gate scheduling

#[cfg(feature = "ml")]
pub mod cost_model;
#[cfg(feature = "ml")]
pub mod predictive_inlining;
#[cfg(feature = "ml")]
pub mod auto_vectorization;
#[cfg(feature = "ml")]
pub mod profile_driven_recompilation;
#[cfg(feature = "ml")]
pub mod quantum_circuit_optimization;

#[cfg(feature = "ml")]
pub mod features;
#[cfg(feature = "ml")]
pub mod training;
#[cfg(feature = "ml")]
pub mod inference;

#[cfg(feature = "ml")]
pub use cost_model::{LearnedCostModel, CostModelConfig, InstructionFeatures};
#[cfg(feature = "ml")]
pub use predictive_inlining::{PredictiveInliner, InliningConfig, InliningFeatures};
#[cfg(feature = "ml")]
pub use auto_vectorization::{AutoVectorizationPolicy, VectorizationConfig, LoopFeatures};
#[cfg(feature = "ml")]
pub use profile_driven_recompilation::{ProfileDrivenRecompiler, RecompilationConfig, ProfileFeatures};
#[cfg(feature = "ml")]
pub use quantum_circuit_optimization::{QuantumCircuitOptimizer, QuantumOptimizationConfig, CircuitFeatures};

/// ML-guided optimization configuration
#[derive(Debug, Clone)]
pub struct MlGuidedConfig {
    pub cost_model: CostModelConfig,
    pub inlining: InliningConfig,
    pub vectorization: VectorizationConfig,
    pub recompilation: RecompilationConfig,
    pub quantum: QuantumOptimizationConfig,
}

impl Default for MlGuidedConfig {
    fn default() -> Self {
        Self {
            cost_model: CostModelConfig::default(),
            inlining: InliningConfig::default(),
            vectorization: VectorizationConfig::default(),
            recompilation: RecompilationConfig::default(),
            quantum: QuantumOptimizationConfig::default(),
        }
    }
}

/// Main ML-guided optimization pipeline
#[cfg(feature = "ml")]
pub struct MlGuidedPipeline {
    config: MlGuidedConfig,
    cost_model: LearnedCostModel,
    inliner: PredictiveInliner,
    vectorizer: AutoVectorizationPolicy,
    recompiler: ProfileDrivenRecompiler,
    quantum_optimizer: QuantumCircuitOptimizer,
}

#[cfg(feature = "ml")]
impl MlGuidedPipeline {
    pub fn new(config: MlGuidedConfig) -> Self {
        let cost_model = LearnedCostModel::new(config.cost_model.clone());
        let inliner = PredictiveInliner::new(config.inlining.clone());
        let vectorizer = AutoVectorizationPolicy::new(config.vectorization.clone());
        let recompiler = ProfileDrivenRecompiler::new(config.recompilation.clone());
        let quantum_optimizer = QuantumCircuitOptimizer::new(config.quantum.clone());

        Self {
            config,
            cost_model,
            inliner,
            vectorizer,
            recompiler,
            quantum_optimizer,
        }
    }

    /// Optimize a function using learned cost model for instruction selection
    pub fn optimize_instruction_selection(&mut self, func: &mut Function) -> Result<(), MlError> {
        self.cost_model.optimize(func)
    }

    /// Make inlining decisions using ML predictions
    pub fn optimize_inlining(&mut self, module: &mut Module) -> Result<(), MlError> {
        self.inliner.optimize(module)
    }

    /// Apply learned auto-vectorization policies
    pub fn optimize_vectorization(&mut self, func: &mut Function) -> Result<(), MlError> {
        self.vectorizer.optimize(func)
    }

    /// Trigger profile-driven recompilation for hot functions
    pub fn maybe_recompile(&mut self, profile: &ExecutionProfile) -> Result<Vec<RecompilationAction>, MlError> {
        self.recompiler.analyze(profile)
    }

    /// Optimize quantum circuits using ML gate scheduling
    pub fn optimize_quantum_circuit(&mut self, circuit: &mut QuantumCircuit) -> Result<(), MlError> {
        self.quantum_optimizer.optimize(circuit)
    }
}

/// Error type for ML-guided optimization
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("Model not trained: {0}")]
    NotTrained(String),
    #[error("Inference failed: {0}")]
    InferenceFailed(String),
    #[error("Training failed: {0}")]
    TrainingFailed(String),
    #[error("Feature extraction failed: {0}")]
    FeatureExtractionFailed(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Backend error: {0}")]
    BackendError(String),
}

/// Forward declarations for types used in the pipeline
pub struct Function;
pub struct Module;
pub struct ExecutionProfile;
pub struct RecompilationAction;
pub struct QuantumCircuit;