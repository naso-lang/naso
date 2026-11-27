//! Data models for the Naso package registry

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use semver::{Version, VersionReq};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub documentation: Option<String>,
    pub license: String,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub downloads: u64,
    pub latest_version: String,
    pub versions: Vec<String>,
    pub owners: Vec<UserRef>,
    pub metadata: PackageMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub naso_version: String,
    pub edition: String,
    pub targets: Vec<String>,
    pub features: HashMap<String, Vec<String>>,
    pub links: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersion {
    pub id: Uuid,
    pub package_name: String,
    pub version: Version,
    pub manifest: PackageManifest,
    pub checksum: String,
    pub size: u64,
    pub yanked: bool,
    pub yanked_reason: Option<String>,
    pub published_at: DateTime<Utc>,
    pub published_by: Uuid,
    pub downloads: u64,
    pub dependencies: Vec<Dependency>,
    pub dev_dependencies: Vec<Dependency>,
    pub build_dependencies: Vec<Dependency>,
    pub features: HashMap<String, Vec<String>>,
    pub target_dependencies: HashMap<String, Vec<Dependency>>,
    pub sbom_id: Option<Uuid>,
    pub attestation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: String,
    pub edition: String,
    pub naso_version: String,
    pub dependencies: HashMap<String, DependencySpec>,
    pub dev_dependencies: HashMap<String, DependencySpec>,
    pub build_dependencies: HashMap<String, DependencySpec>,
    pub features: HashMap<String, Vec<String>>,
    pub targets: Vec<String>,
    pub checksum: String,
    pub signature: Option<String>,
    pub sbom: Option<SBOMRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencySpec {
    pub version: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub target: Option<String>,
    pub registry: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version_req: VersionReq,
    pub features: Vec<String>,
    pub optional: bool,
    pub target: Option<String>,
    pub kind: DependencyKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencyKind {
    Normal,
    Dev,
    Build,
    Target(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRef {
    pub id: Uuid,
    pub username: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SBOMRef {
    pub id: Uuid,
    pub format: SBOMFormat,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SBOMFormat {
    SPDX,
    CycloneDX,
    Syft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SBOM {
    pub id: Uuid,
    pub package_name: String,
    pub version: String,
    pub format: SBOMFormat,
    pub data: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    pub id: Uuid,
    pub package_name: String,
    pub version: String,
    pub attestation_type: AttestationType,
    pub payload: serde_json::Value,
    pub signature: String,
    pub public_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttestationType {
    Provenance,
    Build,
    Test,
    SecurityScan,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub name: String,
    pub version_req: VersionReq,
    pub features: Vec<String>,
    pub optional: bool,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: Version,
    pub dependencies: Vec<Dependency>,
    pub checksum: String,
    pub source: PackageSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PackageSource {
    Registry(String),
    Git { url: String, rev: String },
    Path(String),
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub package: String,
    pub required: Vec<VersionReq>,
    pub available: Vec<Version>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub version: u32,
    pub packages: Vec<LockfilePackage>,
    pub metadata: LockfileMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfilePackage {
    pub name: String,
    pub version: Version,
    pub source: PackageSource,
    pub checksum: String,
    pub dependencies: Vec<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileMetadata {
    pub generated_at: DateTime<Utc>,
    pub resolver_version: String,
    pub naso_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependentPackage {
    pub name: String,
    pub version: Version,
    pub dependency_type: DependencyKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub package: String,
    pub version: String,
    pub verified: bool,
    pub checks: Vec<VerificationCheck>,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub name: String,
    pub passed: bool,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mirror {
    pub id: Uuid,
    pub url: String,
    pub region: String,
    pub priority: u32,
    pub status: MirrorStatus,
    pub last_sync: Option<DateTime<Utc>>,
    pub bandwidth_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MirrorStatus {
    Active,
    Syncing,
    Error(String),
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    pub package: String,
    pub version: String,
    pub publisher: Uuid,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteEvent {
    pub package: String,
    pub admin: Uuid,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVersionEvent {
    pub package: String,
    pub version: String,
    pub admin: Uuid,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YankEvent {
    pub package: String,
    pub version: String,
    pub publisher: Uuid,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnyankEvent {
    pub package: String,
    pub version: String,
    pub publisher: Uuid,
    pub timestamp: DateTime<Utc>,
}