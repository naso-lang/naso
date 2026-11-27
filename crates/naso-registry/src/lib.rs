//! Naso Package Registry Server
//!
//! High-performance package registry supporting 10k+ packages with:
//! - Trust and supply chain security (sigstore, SBOM)
//! - Semantic versioning resolution
//! - Dependency resolution with SAT solver
//! - Mirror and CDN support
//! - Audit logging and compliance

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State, Multipart},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post, put, delete},
    Router,
};
use blake3;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use lru::LruCache;
use parking_lot::RwLock;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{info, warn, error, instrument};
use uuid::Uuid;

pub mod models;
pub mod storage;
pub mod resolver;
pub mod auth;
pub mod audit;
pub mod mirror;
pub mod metrics_registry;

use models::*;
use storage::*;
use resolver::*;
use auth::*;
use audit::*;
use mirror::*;

#[derive(Error, Debug)]
pub enum RegistryError {
    #[error("Package not found: {0}")]
    NotFound(String),
    #[error("Version conflict: {0}")]
    VersionConflict(String),
    #[error("Invalid package: {0}")]
    InvalidPackage(String),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Authorization denied: {0}")]
    AuthDenied(String),
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Resolver error: {0}")]
    Resolver(#[from] ResolverError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Validation error: {0}")]
    Validation(String),
}

impl IntoResponse for RegistryError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            RegistryError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            RegistryError::VersionConflict(_) => (StatusCode::CONFLICT, self.to_string()),
            RegistryError::InvalidPackage(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            RegistryError::AuthFailed(_) => (StatusCode::UNAUTHORIZED, self.to_string()),
            RegistryError::AuthDenied(_) => (StatusCode::FORBIDDEN, self.to_string()),
            RegistryError::Validation(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string()),
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Clone)]
pub struct RegistryState {
    pub db: PgPool,
    pub storage: Arc<dyn PackageStorage>,
    pub resolver: Arc<DependencyResolver>,
    pub auth: Arc<AuthManager>,
    pub audit: Arc<AuditLogger>,
    pub mirror_manager: Arc<MirrorManager>,
    pub cache: Arc<RwLock<LruCache<String, CachedPackage>>>,
    pub config: RegistryConfig,
    pub shutdown_tx: broadcast::Sender<()>,
}

#[derive(Clone, Debug)]
pub struct RegistryConfig {
    pub max_package_size: usize,
    pub max_versions_per_package: usize,
    pub retention_days: i64,
    pub enable_mirrors: bool,
    pub cdn_base_url: Option<String>,
    pub require_signed: bool,
    pub sbom_required: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            max_package_size: 100 * 1024 * 1024, // 100MB
            max_versions_per_package: 1000,
            retention_days: 365 * 10, // 10 years
            enable_mirrors: true,
            cdn_base_url: None,
            require_signed: false,
            sbom_required: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedPackage {
    package: Package,
    versions: Vec<PackageVersion>,
    fetched_at: DateTime<Utc>,
}

/// Create the registry router
pub fn create_router(state: RegistryState) -> Router {
    Router::new()
        // Package CRUD
        .route("/v1/packages", get(list_packages))
        .route("/v1/packages", post(publish_package))
        .route("/v1/packages/:name", get(get_package))
        .route("/v1/packages/:name", delete(delete_package))
        .route("/v1/packages/:name/versions", get(list_versions))
        .route("/v1/packages/:name/versions/:version", get(get_version))
        .route("/v1/packages/:name/versions/:version", delete(delete_version))
        .route("/v1/packages/:name/versions/:version/download", get(download_package))
        .route("/v1/packages/:name/versions/:version/yank", put(yank_version))
        .route("/v1/packages/:name/versions/:version/unyank", put(unyank_version))
        // Dependency resolution
        .route("/v1/resolve", post(resolve_dependencies))
        .route("/v1/resolve/lock", post(generate_lockfile))
        // Search and discovery
        .route("/v1/search", get(search_packages))
        .route("/v1/packages/:name/dependents", get(get_dependents))
        .route("/v1/packages/:name/dependencies", get(get_dependencies))
        // Security and trust
        .route("/v1/packages/:name/versions/:version/verify", get(verify_package))
        .route("/v1/packages/:name/versions/:version/sbom", get(get_sbom))
        .route("/v1/packages/:name/versions/:version/attestation", get(get_attestation))
        // Publishing auth
        .route("/v1/auth/token", post(create_token))
        .route("/v1/auth/verify", post(verify_token))
        // Admin
        .route("/v1/admin/stats", get(admin_stats))
        .route("/v1/admin/mirrors", get(list_mirrors))
        .route("/v1/admin/mirrors", post(add_mirror))
        .route("/v1/admin/audit", get(audit_log))
        // Metrics
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

// ============================================================================
// Package CRUD Handlers
// ============================================================================

#[instrument(skip(state))]
async fn list_packages(
    State(state): State<RegistryState>,
    Query(params): Query<ListParams>,
) -> Result<Json<PackageListResponse>, RegistryError> {
    let packages = state.storage.list_packages(params.limit, params.offset).await?;
    let total = state.storage.count_packages().await?;
    Ok(Json(PackageListResponse { packages, total }))
}

#[instrument(skip(state))]
async fn publish_package(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<PublishResponse>, RegistryError> {
    // Authenticate publisher
    let publisher = state.auth.authenticate_publish(&headers).await?;
    
    // Parse multipart form
    let (manifest, package_data, signature, sbom) = parse_publish_multipart(multipart).await?;
    
    // Validate package
    validate_package(&manifest, &package_data, &state.config).await?;
    
    // Verify signature if required
    if state.config.require_signed {
        state.auth.verify_signature(&manifest, &signature, &publisher).await?;
    }
    
    // Check SBOM requirement
    if state.config.sbom_required && sbom.is_none() {
        return Err(RegistryError::Validation("SBOM required".into()));
    }
    
    // Store package
    let version = state.storage.store_package(
        &manifest,
        &package_data,
        signature,
        sbom,
        &publisher,
    ).await?;
    
    // Audit log
    state.audit.log(PublishEvent {
        package: manifest.name.clone(),
        version: manifest.version.clone(),
        publisher: publisher.id,
        timestamp: Utc::now(),
    }).await;
    
    // Invalidate cache
    state.cache.write().pop(&manifest.name, None);
    
    // Trigger mirror sync
    if state.config.enable_mirrors {
        state.mirror_manager.trigger_sync(&manifest.name).await;
    }
    
    Ok(Json(PublishResponse { version }))
}

#[instrument(skip(state))]
async fn get_package(
    State(state): State<RegistryState>,
    Path(name): Path<String>,
) -> Result<Json<Package>, RegistryError> {
    // Check cache first
    if let Some(cached) = state.cache.read().get(&name) {
        if cached.fetched_at > Utc::now() - chrono::Duration::hours(1) {
            return Ok(Json(cached.package.clone()));
        }
    }
    
    let package = state.storage.get_package(&name).await?
        .ok_or_else(|| RegistryError::NotFound(name))?;
    
    // Update cache
    let versions = state.storage.list_versions(&name).await?;
    state.cache.write().put(name.clone(), CachedPackage {
        package: package.clone(),
        versions,
        fetched_at: Utc::now(),
    });
    
    Ok(Json(package))
}

#[instrument(skip(state))]
async fn delete_package(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<StatusCode, RegistryError> {
    let publisher = state.auth.authenticate_admin(&headers).await?;
    state.storage.delete_package(&name).await?;
    state.audit.log(DeleteEvent { package: name, admin: publisher.id, timestamp: Utc::now() }).await;
    state.cache.write().pop(&name, None);
    Ok(StatusCode::NO_CONTENT)
}

#[instrument(skip(state))]
async fn list_versions(
    State(state): State<RegistryState>,
    Path(name): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<VersionListResponse>, RegistryError> {
    let versions = state.storage.list_versions_paginated(&name, params.limit, params.offset).await?;
    Ok(Json(VersionListResponse { versions }))
}

#[instrument(skip(state))]
async fn get_version(
    State(state): State<RegistryState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<PackageVersion>, RegistryError> {
    let version = state.storage.get_version(&name, &version).await?
        .ok_or_else(|| RegistryError::NotFound(format!("{}@{}", name, version)))?;
    Ok(Json(version))
}

#[instrument(skip(state))]
async fn delete_version(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path((name, version)): Path<(String, String)>,
) -> Result<StatusCode, RegistryError> {
    let publisher = state.auth.authenticate_admin(&headers).await?;
    state.storage.delete_version(&name, &version).await?;
    state.audit.log(DeleteVersionEvent { package: name, version, admin: publisher.id, timestamp: Utc::now() }).await;
    state.cache.write().pop(&name, None);
    Ok(StatusCode::NO_CONTENT)
}

#[instrument(skip(state))]
async fn download_package(
    State(state): State<RegistryState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<impl IntoResponse, RegistryError> {
    let (data, checksum) = state.storage.download_package(&name, &version).await?
        .ok_or_else(|| RegistryError::NotFound(format!("{}@{}", name, version)))?;
    
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/octet-stream".parse().unwrap());
    headers.insert("X-Checksum-Blake3", checksum.parse().unwrap());
    headers.insert("Content-Disposition", format!("attachment; filename=\"{}-{}.tar.gz\"", name, version).parse().unwrap());
    
    Ok((headers, data))
}

#[instrument(skip(state))]
async fn yank_version(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path((name, version)): Path<(String, String)>,
) -> Result<StatusCode, RegistryError> {
    let publisher = state.auth.authenticate_publish(&headers).await?;
    state.storage.yank_version(&name, &version).await?;
    state.audit.log(YankEvent { package: name, version, publisher: publisher.id, timestamp: Utc::now() }).await;
    state.cache.write().pop(&name, None);
    Ok(StatusCode::OK)
}

#[instrument(skip(state))]
async fn unyank_version(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path((name, version)): Path<(String, String)>,
) -> Result<StatusCode, RegistryError> {
    let publisher = state.auth.authenticate_publish(&headers).await?;
    state.storage.unyank_version(&name, &version).await?;
    state.audit.log(UnyankEvent { package: name, version, publisher: publisher.id, timestamp: Utc::now() }).await;
    state.cache.write().pop(&name, None);
    Ok(StatusCode::OK)
}

// ============================================================================
// Dependency Resolution Handlers
// ============================================================================

#[instrument(skip(state))]
async fn resolve_dependencies(
    State(state): State<RegistryState>,
    Json(request): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, RegistryError> {
    let resolution = state.resolver.resolve(&request.requirements, request.constraints).await?;
    Ok(Json(resolution))
}

#[instrument(skip(state))]
async fn generate_lockfile(
    State(state): State<RegistryState>,
    Json(request): Json<LockfileRequest>,
) -> Result<Json<LockfileResponse>, RegistryError> {
    let lockfile = state.resolver.generate_lockfile(&request.requirements).await?;
    Ok(Json(LockfileResponse { lockfile }))
}

// ============================================================================
// Search and Discovery Handlers
// ============================================================================

#[instrument(skip(state))]
async fn search_packages(
    State(state): State<RegistryState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, RegistryError> {
    let results = state.storage.search(&params.query, params.limit, params.offset).await?;
    Ok(Json(SearchResponse { results }))
}

#[instrument(skip(state))]
async fn get_dependents(
    State(state): State<RegistryState>,
    Path(name): Path<String>,
) -> Result<Json<DependentsResponse>, RegistryError> {
    let dependents = state.storage.get_dependents(&name).await?;
    Ok(Json(DependentsResponse { dependents }))
}

#[instrument(skip(state))]
async fn get_dependencies(
    State(state): State<RegistryState>,
    Path(name): Path<String>,
    Query(params): Query<VersionParam>,
) -> Result<Json<DependenciesResponse>, RegistryError> {
    let version = params.version.unwrap_or_else(|| "latest".to_string());
    let deps = state.storage.get_dependencies(&name, &version).await?;
    Ok(Json(DependenciesResponse { dependencies: deps }))
}

// ============================================================================
// Security and Trust Handlers
// ============================================================================

#[instrument(skip(state))]
async fn verify_package(
    State(state): State<RegistryState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<VerificationResult>, RegistryError> {
    let result = state.storage.verify_package(&name, &version).await?;
    Ok(Json(result))
}

#[instrument(skip(state))]
async fn get_sbom(
    State(state): State<RegistryState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<SBOM>, RegistryError> {
    let sbom = state.storage.get_sbom(&name, &version).await?
        .ok_or_else(|| RegistryError::NotFound("SBOM not found".into()))?;
    Ok(Json(sbom))
}

#[instrument(skip(state))]
async fn get_attestation(
    State(state): State<RegistryState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<Attestation>, RegistryError> {
    let attestation = state.storage.get_attestation(&name, &version).await?
        .ok_or_else(|| RegistryError::NotFound("Attestation not found".into()))?;
    Ok(Json(attestation))
}

// ============================================================================
// Auth Handlers
// ============================================================================

#[instrument(skip(state))]
async fn create_token(
    State(state): State<RegistryState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<TokenResponse>, RegistryError> {
    let token = state.auth.create_token(&request).await?;
    Ok(Json(token))
}

#[instrument(skip(state))]
async fn verify_token(
    State(state): State<RegistryState>,
    Json(request): Json<VerifyTokenRequest>,
) -> Result<Json<VerifyTokenResponse>, RegistryError> {
    let valid = state.auth.verify_token(&request.token).await?;
    Ok(Json(VerifyTokenResponse { valid }))
}

// ============================================================================
// Admin Handlers
// ============================================================================

#[instrument(skip(state))]
async fn admin_stats(
    State(state): State<RegistryState>,
) -> Result<Json<AdminStats>, RegistryError> {
    let stats = state.storage.get_stats().await?;
    Ok(Json(stats))
}

#[instrument(skip(state))]
async fn list_mirrors(
    State(state): State<RegistryState>,
) -> Result<Json<Vec<Mirror>>, RegistryError> {
    let mirrors = state.mirror_manager.list_mirrors().await;
    Ok(Json(mirrors))
}

#[instrument(skip(state))]
async fn add_mirror(
    State(state): State<RegistryState>,
    Json(request): Json<AddMirrorRequest>,
) -> Result<Json<Mirror>, RegistryError> {
    let mirror = state.mirror_manager.add_mirror(request).await?;
    Ok(Json(mirror))
}

#[instrument(skip(state))]
async fn audit_log(
    State(state): State<RegistryState>,
    Query(params): Query<AuditParams>,
) -> Result<Json<AuditResponse>, RegistryError> {
    let logs = state.audit.query(params).await?;
    Ok(Json(logs))
}

// ============================================================================
// Metrics Handler
// ============================================================================

async fn metrics_handler(State(state): State<RegistryState>) -> impl IntoResponse {
    state.metrics_registry.render()
}

// ============================================================================
// Helper Functions
// ============================================================================

async fn parse_publish_multipart(
    mut multipart: Multipart,
) -> Result<(PackageManifest, Vec<u8>, Option<String>, Option<SBOM>), RegistryError> {
    let mut manifest = None;
    let mut package_data = Vec::new();
    let mut signature = None;
    let mut sbom = None;
    
    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "manifest" => {
                let data = field.bytes().await?;
                manifest = Some(serde_json::from_slice(&data)?);
            }
            "package" => {
                package_data = field.bytes().await?.to_vec();
            }
            "signature" => {
                signature = Some(field.text().await?);
            }
            "sbom" => {
                let data = field.bytes().await?;
                sbom = Some(serde_json::from_slice(&data)?);
            }
            _ => {}
        }
    }
    
    Ok((manifest.ok_or(RegistryError::InvalidPackage("Missing manifest".into()))?, package_data, signature, sbom))
}

async fn validate_package(
    manifest: &PackageManifest,
    data: &[u8],
    config: &RegistryConfig,
) -> Result<(), RegistryError> {
    // Size check
    if data.len() > config.max_package_size {
        return Err(RegistryError::Validation(format!("Package too large: {} bytes", data.len())));
    }
    
    // Name validation
    if !manifest.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(RegistryError::Validation("Invalid package name".into()));
    }
    
    // Version validation
    Version::parse(&manifest.version)
        .map_err(|e| RegistryError::Validation(format!("Invalid version: {}", e)))?;
    
    // Checksum verification
    let computed = blake3::hash(data);
    if manifest.checksum != computed.to_hex().to_string() {
        return Err(RegistryError::Validation("Checksum mismatch".into()));
    }
    
    Ok(())
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Deserialize)]
struct ListParams {
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
struct VersionParam {
    version: Option<String>,
}

#[derive(Deserialize)]
struct AuditParams {
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    event_type: Option<String>,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct PackageListResponse {
    packages: Vec<Package>,
    total: u64,
}

#[derive(Serialize)]
struct VersionListResponse {
    versions: Vec<PackageVersion>,
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct SearchResult {
    package: Package,
    score: f32,
}

#[derive(Serialize)]
struct DependentsResponse {
    dependents: Vec<DependentPackage>,
}

#[derive(Serialize)]
struct DependenciesResponse {
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct ResolveRequest {
    requirements: Vec<Requirement>,
    constraints: Option<HashMap<String, VersionReq>>,
}

#[derive(Serialize)]
struct ResolveResponse {
    packages: Vec<ResolvedPackage>,
    conflicts: Vec<Conflict>,
}

#[derive(Deserialize)]
struct LockfileRequest {
    requirements: Vec<Requirement>,
}

#[derive(Serialize)]
struct LockfileResponse {
    lockfile: Lockfile,
}

#[derive(Deserialize)]
struct TokenRequest {
    username: String,
    password: String,
    scopes: Vec<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct VerifyTokenRequest {
    token: String,
}

#[derive(Serialize)]
struct VerifyTokenResponse {
    valid: bool,
}

#[derive(Deserialize)]
struct AddMirrorRequest {
    url: String,
    region: String,
    priority: u32,
}

#[derive(Serialize)]
struct AdminStats {
    total_packages: u64,
    total_versions: u64,
    total_downloads: u64,
    storage_bytes: u64,
    active_users: u64,
}

// ============================================================================
// Server Entry Point
// ============================================================================

pub async fn run_server(config: RegistryConfig, db_url: &str) -> Result<(), RegistryError> {
    let db = PgPool::connect(db_url).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    
    let storage = Arc::new(PostgresStorage::new(db.clone()));
    let resolver = Arc::new(DependencyResolver::new(storage.clone()));
    let auth = Arc::new(AuthManager::new(db.clone()));
    let audit = Arc::new(AuditLogger::new(db.clone()));
    let mirror_manager = Arc::new(MirrorManager::new(storage.clone()));
    let cache = Arc::new(RwLock::new(LruCache::new(10000)));
    let metrics_registry = metrics_registry::create();
    let (shutdown_tx, _) = broadcast::channel(1);
    
    let state = RegistryState {
        db,
        storage,
        resolver,
        auth,
        audit,
        mirror_manager,
        cache,
        config,
        shutdown_tx,
    };
    
    let app = create_router(state);
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    info!("Registry server listening on {}", listener.local_addr()?);
    
    axum::serve(listener, app).await?;
    Ok(())
}