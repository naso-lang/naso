//! Dependency resolver for the Naso package registry

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use semver::{Version, VersionReq};
use thiserror::Error;
use tracing::{debug, warn};

use crate::models::*;
use crate::storage::PackageStorage;
use crate::RegistryError;

#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("Package not found: {0}")]
    NotFound(String),
    #[error("Version conflict: {package} - {required:?} vs available: {available:?}")]
    VersionConflict { package: String, required: Vec<VersionReq>, available: Vec<Version> },
    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),
    #[error("Unsatisfiable requirements: {0}")]
    Unsatisfiable(String),
    #[error("Storage error: {0}")]
    Storage(#[from] RegistryError),
    #[error("Invalid requirement: {0}")]
    InvalidRequirement(String),
}

pub struct DependencyResolver {
    storage: Arc<dyn PackageStorage>,
    cache: dashmap::DashMap<String, Vec<Version>>,
}

impl DependencyResolver {
    pub fn new(storage: Arc<dyn PackageStorage>) -> Self {
        Self {
            storage,
            cache: dashmap::DashMap::new(),
        }
    }
    
    pub async fn resolve(
        &self,
        requirements: &[Requirement],
        constraints: Option<HashMap<String, VersionReq>>,
    ) -> Result<ResolveResponse, ResolverError> {
        let start = Instant::now();
        
        // Build dependency graph
        let mut graph = DependencyGraph::new();
        let mut to_process: VecDeque<Requirement> = requirements.iter().cloned().collect();
        let mut processed = HashSet::new();
        
        while let Some(req) = to_process.pop_front() {
            let key = format!("{}@{}", req.name, req.version_req);
            if processed.contains(&key) {
                continue;
            }
            processed.insert(key.clone());
            
            // Get available versions
            let versions = self.get_available_versions(&req.name).await?;
            let compatible: Vec<_> = versions.into_iter()
                .filter(|v| req.version_req.matches(v))
                .collect();
            
            if compatible.is_empty() {
                return Err(ResolverError::Unsatisfiable(format!(
                    "No compatible version for {}@{}",
                    req.name, req.version_req
                )));
            }
            
            // Select best version (highest compatible)
            let selected = compatible.iter().max().unwrap().clone();
            
            // Get dependencies for this version
            let deps = self.storage.get_dependencies(&req.name, &selected.to_string()).await?;
            
            graph.add_node(req.name.clone(), selected.clone(), deps.clone());
            
            // Add dependencies to processing queue
            for dep in deps {
                if !processed.contains(&format!("{}@{}", dep.name, dep.version_req)) {
                    to_process.push_back(Requirement {
                        name: dep.name,
                        version_req: dep.version_req,
                        features: dep.features,
                        optional: dep.optional,
                        target: dep.target,
                    });
                }
            }
        }
        
        // Check for conflicts
        let conflicts = graph.detect_conflicts();
        
        // Build resolved package list
        let packages = graph.topological_sort()?;
        
        debug!("Resolved {} packages in {:?}", packages.len(), start.elapsed());
        
        Ok(ResolveResponse { packages, conflicts })
    }
    
    pub async fn generate_lockfile(&self, requirements: &[Requirement]) -> Result<Lockfile, ResolverError> {
        let resolution = self.resolve(requirements, None).await?;
        
        let packages = resolution.packages.into_iter().map(|p| LockfilePackage {
            name: p.name,
            version: p.version,
            source: p.source,
            checksum: p.checksum,
            dependencies: p.dependencies.iter().map(|d| d.name.clone()).collect(),
            features: p.features.into_keys().collect(),
        }).collect();
        
        Ok(Lockfile {
            version: 1,
            packages,
            metadata: LockfileMetadata {
                generated_at: chrono::Utc::now(),
                resolver_version: env!("CARGO_PKG_VERSION").to_string(),
                naso_version: "0.1.0".to_string(),
            },
        })
    }
    
    async fn get_available_versions(&self, name: &str) -> Result<Vec<Version>, ResolverError> {
        if let Some(cached) = self.cache.get(name) {
            return Ok(cached.value().clone());
        }
        
        let versions = self.storage.list_versions(name).await?
            .into_iter()
            .filter(|v| !v.yanked)
            .map(|v| v.version)
            .collect::<Vec<_>>();
        
        self.cache.insert(name.to_string(), versions.clone());
        Ok(versions)
    }
}

struct DependencyGraph {
    nodes: HashMap<String, GraphNode>,
    edges: HashMap<String, Vec<String>>,
}

struct GraphNode {
    name: String,
    version: Version,
    dependencies: Vec<Dependency>,
    features: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }
    
    fn add_node(&mut self, name: String, version: Version, dependencies: Vec<Dependency>) {
        let key = format!("{}@{}", name, version);
        if self.nodes.contains_key(&key) {
            return;
        }
        
        self.nodes.insert(key.clone(), GraphNode {
            name: name.clone(),
            version,
            dependencies: dependencies.clone(),
            features: HashMap::new(),
        });
        
        for dep in dependencies {
            let dep_key = format!("{}@{}", dep.name, dep.version_req);
            self.edges.entry(key.clone()).or_default().push(dep_key);
        }
    }
    
    fn detect_conflicts(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let mut version_map: HashMap<String, Vec<Version>> = HashMap::new();
        
        for node in self.nodes.values() {
            version_map.entry(node.name.clone()).or_default().push(node.version.clone());
        }
        
        for (name, versions) in version_map {
            if versions.len() > 1 {
                conflicts.push(Conflict {
                    package: name,
                    required: Vec::new(), // Would need to track requirements
                    available: versions,
                });
            }
        }
        
        conflicts
    }
    
    fn topological_sort(&self) -> Result<Vec<ResolvedPackage>, ResolverError> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        
        // Build adjacency list
        for (from, tos) in &self.edges {
            for to in tos {
                adj.entry(from.clone()).or_default().push(to.clone());
                *in_degree.entry(to.clone()).or_insert(0) += 1;
                in_degree.entry(from.clone()).or_insert(0);
            }
        }
        
        // Kahn's algorithm
        let mut queue: VecDeque<String> = in_degree.iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(k, _)| k.clone())
            .collect();
        
        let mut sorted = Vec::new();
        
        while let Some(node) = queue.pop_front() {
            if let Some(graph_node) = self.nodes.get(&node) {
                sorted.push(ResolvedPackage {
                    name: graph_node.name.clone(),
                    version: graph_node.version.clone(),
                    dependencies: graph_node.dependencies.clone(),
                    checksum: String::new(), // Would need to fetch
                    source: PackageSource::Registry("naso".to_string()),
                });
            }
            
            if let Some(neighbors) = adj.get(&node) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }
        }
        
        if sorted.len() != self.nodes.len() {
            return Err(ResolverError::CircularDependency("Cycle detected in dependency graph".into()));
        }
        
        Ok(sorted)
    }
}