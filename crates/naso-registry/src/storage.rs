//! Storage layer for the Naso package registry

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

use crate::models::*;
use crate::RegistryError;

#[async_trait]
pub trait PackageStorage: Send + Sync {
    async fn list_packages(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Package>, RegistryError>;
    async fn count_packages(&self) -> Result<u64, RegistryError>;
    async fn get_package(&self, name: &str) -> Result<Option<Package>, RegistryError>;
    async fn store_package(
        &self,
        manifest: &PackageManifest,
        data: &[u8],
        signature: Option<String>,
        sbom: Option<SBOM>,
        publisher: &UserRef,
    ) -> Result<PackageVersion, RegistryError>;
    async fn delete_package(&self, name: &str) -> Result<(), RegistryError>;
    async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>, RegistryError>;
    async fn list_versions_paginated(&self, name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<PackageVersion>, RegistryError>;
    async fn get_version(&self, name: &str, version: &str) -> Result<Option<PackageVersion>, RegistryError>;
    async fn delete_version(&self, name: &str, version: &str) -> Result<(), RegistryError>;
    async fn download_package(&self, name: &str, version: &str) -> Result<Option<(Vec<u8>, String)>, RegistryError>;
    async fn yank_version(&self, name: &str, version: &str) -> Result<(), RegistryError>;
    async fn unyank_version(&self, name: &str, version: &str) -> Result<(), RegistryError>;
    async fn search(&self, query: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<SearchResult>, RegistryError>;
    async fn get_dependents(&self, name: &str) -> Result<Vec<DependentPackage>, RegistryError>;
    async fn get_dependencies(&self, name: &str, version: &str) -> Result<Vec<Dependency>, RegistryError>;
    async fn verify_package(&self, name: &str, version: &str) -> Result<VerificationResult, RegistryError>;
    async fn get_sbom(&self, name: &str, version: &str) -> Result<Option<SBOM>, RegistryError>;
    async fn get_attestation(&self, name: &str, version: &str) -> Result<Option<Attestation>, RegistryError>;
    async fn get_stats(&self) -> Result<AdminStats, RegistryError>;
    async fn record_download(&self, name: &str, version: &str) -> Result<(), RegistryError>;
}

pub struct PostgresStorage {
    pool: PgPool,
}

impl PostgresStorage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PackageStorage for PostgresStorage {
    async fn list_packages(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Package>, RegistryError> {
        let limit = limit.unwrap_or(100) as i64;
        let offset = offset.unwrap_or(0) as i64;
        
        let rows = sqlx::query(
            r#"
            SELECT p.*, 
                   array_agg(DISTINCT v.version::text) as versions,
                   array_agg(DISTINCT u.username) as owner_names,
                   array_agg(DISTINCT u.id) as owner_ids
            FROM packages p
            LEFT JOIN package_versions v ON p.name = v.package_name AND NOT v.yanked
            LEFT JOIN package_owners po ON p.name = po.package_name
            LEFT JOIN users u ON po.user_id = u.id
            GROUP BY p.name
            ORDER BY p.downloads DESC, p.name
            LIMIT $1 OFFSET $2
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|row| Package {
            name: row.get("name"),
            description: row.get("description"),
            homepage: row.get("homepage"),
            repository: row.get("repository"),
            documentation: row.get("documentation"),
            license: row.get("license"),
            keywords: row.get("keywords"),
            categories: row.get("categories"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            downloads: row.get("downloads"),
            latest_version: row.get("latest_version"),
            versions: row.get::<Option<Vec<String>>, _>("versions").unwrap_or_default(),
            owners: row.get::<Option<Vec<String>>, _>("owner_names").unwrap_or_default()
                .into_iter()
                .zip(row.get::<Option<Vec<Uuid>>, _>("owner_ids").unwrap_or_default())
                .map(|(username, id)| UserRef { id, username, avatar_url: None })
                .collect(),
            metadata: serde_json::from_value(row.get("metadata")).unwrap_or_default(),
        }).collect())
    }
    
    async fn count_packages(&self) -> Result<u64, RegistryError> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM packages")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("count"))
    }
    
    async fn get_package(&self, name: &str) -> Result<Option<Package>, RegistryError> {
        let row = sqlx::query(
            r#"
            SELECT p.*, 
                   array_agg(DISTINCT v.version::text) as versions,
                   array_agg(DISTINCT u.username) as owner_names,
                   array_agg(DISTINCT u.id) as owner_ids
            FROM packages p
            LEFT JOIN package_versions v ON p.name = v.package_name AND NOT v.yanked
            LEFT JOIN package_owners po ON p.name = po.package_name
            LEFT JOIN users u ON po.user_id = u.id
            WHERE p.name = $1
            GROUP BY p.name
            "#
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|row| Package {
            name: row.get("name"),
            description: row.get("description"),
            homepage: row.get("homepage"),
            repository: row.get("repository"),
            documentation: row.get("documentation"),
            license: row.get("license"),
            keywords: row.get("keywords"),
            categories: row.get("categories"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            downloads: row.get("downloads"),
            latest_version: row.get("latest_version"),
            versions: row.get::<Option<Vec<String>>, _>("versions").unwrap_or_default(),
            owners: row.get::<Option<Vec<String>>, _>("owner_names").unwrap_or_default()
                .into_iter()
                .zip(row.get::<Option<Vec<Uuid>>, _>("owner_ids").unwrap_or_default())
                .map(|(username, id)| UserRef { id, username, avatar_url: None })
                .collect(),
            metadata: serde_json::from_value(row.get("metadata")).unwrap_or_default(),
        }))
    }
    
    async fn store_package(
        &self,
        manifest: &PackageManifest,
        data: &[u8],
        signature: Option<String>,
        sbom: Option<SBOM>,
        publisher: &UserRef,
    ) -> Result<PackageVersion, RegistryError> {
        let mut tx = self.pool.begin().await?;
        
        // Insert or update package
        sqlx::query(
            r#"
            INSERT INTO packages (name, description, homepage, repository, documentation, license, keywords, categories, latest_version, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (name) DO UPDATE SET
                description = COALESCE($2, packages.description),
                homepage = COALESCE($3, packages.homepage),
                repository = COALESCE($4, packages.repository),
                documentation = COALESCE($5, packages.documentation),
                license = COALESCE($6, packages.license),
                keywords = COALESCE($7, packages.keywords),
                categories = COALESCE($8, packages.categories),
                latest_version = $9,
                metadata = COALESCE($10, packages.metadata),
                updated_at = NOW()
            "#
        )
        .bind(&manifest.name)
        .bind(&manifest.description)
        .bind(&manifest.homepage)
        .bind(&manifest.repository)
        .bind(&manifest.documentation)
        .bind(&manifest.license)
        .bind(&manifest.authors) // using authors as keywords temporarily
        .bind(&Vec::<String>::new()) // categories
        .bind(&manifest.version)
        .bind(serde_json::to_value(&manifest).unwrap())
        .execute(&mut *tx)
        .await?;
        
        // Store package version
        let version_id = Uuid::new_v4();
        let version = Version::parse(&manifest.version).unwrap();
        
        sqlx::query(
            r#"
            INSERT INTO package_versions (
                id, package_name, version, manifest, checksum, size, yanked, 
                published_at, published_by, dependencies, dev_dependencies, 
                build_dependencies, features, target_dependencies, sbom_id, attestation_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            "#
        )
        .bind(version_id)
        .bind(&manifest.name)
        .bind(&version.to_string())
        .bind(serde_json::to_value(manifest).unwrap())
        .bind(&blake3::hash(data).to_hex().to_string())
        .bind(data.len() as i64)
        .bind(false)
        .bind(Utc::now())
        .bind(publisher.id)
        .bind(serde_json::to_value(&manifest.dependencies).unwrap())
        .bind(serde_json::to_value(&manifest.dev_dependencies).unwrap())
        .bind(serde_json::to_value(&manifest.build_dependencies).unwrap())
        .bind(serde_json::to_value(&manifest.features).unwrap())
        .bind(serde_json::to_value(&HashMap::<String, Vec<Dependency>>::new()).unwrap())
        .bind(sbom.as_ref().map(|s| s.id))
        .bind(None::<Uuid>) // attestation_id
        .execute(&mut *tx)
        .await?;
        
        // Store package data
        sqlx::query(
            "INSERT INTO package_data (version_id, data) VALUES ($1, $2)"
        )
        .bind(version_id)
        .bind(data)
        .execute(&mut *tx)
        .await?;
        
        // Store SBOM if provided
        if let Some(sbom) = sbom {
            sqlx::query(
                "INSERT INTO sboms (id, package_name, version, format, data, verified) VALUES ($1, $2, $3, $4, $5, $6)"
            )
            .bind(sbom.id)
            .bind(&manifest.name)
            .bind(&manifest.version)
            .bind(serde_json::to_string(&sbom.format).unwrap())
            .bind(serde_json::to_value(&sbom.data).unwrap())
            .bind(false)
            .execute(&mut *tx)
            .await?;
        }
        
        // Record ownership
        sqlx::query(
            "INSERT INTO package_owners (package_name, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        )
        .bind(&manifest.name)
        .bind(publisher.id)
        .execute(&mut *tx)
        .await?;
        
        tx.commit().await?;
        
        Ok(PackageVersion {
            id: version_id,
            package_name: manifest.name.clone(),
            version,
            manifest: manifest.clone(),
            checksum: blake3::hash(data).to_hex().to_string(),
            size: data.len() as u64,
            yanked: false,
            yanked_reason: None,
            published_at: Utc::now(),
            published_by: publisher.id,
            downloads: 0,
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            build_dependencies: Vec::new(),
            features: manifest.features.clone(),
            target_dependencies: HashMap::new(),
            sbom_id: sbom.map(|s| s.id),
            attestation_id: None,
        })
    }
    
    async fn delete_package(&self, name: &str) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM packages WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    
    async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>, RegistryError> {
        let rows = sqlx::query(
            "SELECT * FROM package_versions WHERE package_name = $1 ORDER BY version DESC"
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|row| PackageVersion {
            id: row.get("id"),
            package_name: row.get("package_name"),
            version: Version::parse(&row.get::<String, _>("version")).unwrap(),
            manifest: serde_json::from_value(row.get("manifest")).unwrap(),
            checksum: row.get("checksum"),
            size: row.get("size"),
            yanked: row.get("yanked"),
            yanked_reason: row.get("yanked_reason"),
            published_at: row.get("published_at"),
            published_by: row.get("published_by"),
            downloads: row.get("downloads"),
            dependencies: serde_json::from_value(row.get("dependencies")).unwrap(),
            dev_dependencies: serde_json::from_value(row.get("dev_dependencies")).unwrap(),
            build_dependencies: serde_json::from_value(row.get("build_dependencies")).unwrap(),
            features: serde_json::from_value(row.get("features")).unwrap(),
            target_dependencies: serde_json::from_value(row.get("target_dependencies")).unwrap(),
            sbom_id: row.get("sbom_id"),
            attestation_id: row.get("attestation_id"),
        }).collect())
    }
    
    async fn list_versions_paginated(&self, name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<PackageVersion>, RegistryError> {
        let limit = limit.unwrap_or(50) as i64;
        let offset = offset.unwrap_or(0) as i64;
        
        let rows = sqlx::query(
            "SELECT * FROM package_versions WHERE package_name = $1 ORDER BY version DESC LIMIT $2 OFFSET $3"
        )
        .bind(name)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|row| PackageVersion {
            id: row.get("id"),
            package_name: row.get("package_name"),
            version: Version::parse(&row.get::<String, _>("version")).unwrap(),
            manifest: serde_json::from_value(row.get("manifest")).unwrap(),
            checksum: row.get("checksum"),
            size: row.get("size"),
            yanked: row.get("yanked"),
            yanked_reason: row.get("yanked_reason"),
            published_at: row.get("published_at"),
            published_by: row.get("published_by"),
            downloads: row.get("downloads"),
            dependencies: serde_json::from_value(row.get("dependencies")).unwrap(),
            dev_dependencies: serde_json::from_value(row.get("dev_dependencies")).unwrap(),
            build_dependencies: serde_json::from_value(row.get("build_dependencies")).unwrap(),
            features: serde_json::from_value(row.get("features")).unwrap(),
            target_dependencies: serde_json::from_value(row.get("target_dependencies")).unwrap(),
            sbom_id: row.get("sbom_id"),
            attestation_id: row.get("attestation_id"),
        }).collect())
    }
    
    async fn get_version(&self, name: &str, version: &str) -> Result<Option<PackageVersion>, RegistryError> {
        let row = sqlx::query(
            "SELECT * FROM package_versions WHERE package_name = $1 AND version = $2"
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|row| PackageVersion {
            id: row.get("id"),
            package_name: row.get("package_name"),
            version: Version::parse(&row.get::<String, _>("version")).unwrap(),
            manifest: serde_json::from_value(row.get("manifest")).unwrap(),
            checksum: row.get("checksum"),
            size: row.get("size"),
            yanked: row.get("yanked"),
            yanked_reason: row.get("yanked_reason"),
            published_at: row.get("published_at"),
            published_by: row.get("published_by"),
            downloads: row.get("downloads"),
            dependencies: serde_json::from_value(row.get("dependencies")).unwrap(),
            dev_dependencies: serde_json::from_value(row.get("dev_dependencies")).unwrap(),
            build_dependencies: serde_json::from_value(row.get("build_dependencies")).unwrap(),
            features: serde_json::from_value(row.get("features")).unwrap(),
            target_dependencies: serde_json::from_value(row.get("target_dependencies")).unwrap(),
            sbom_id: row.get("sbom_id"),
            attestation_id: row.get("attestation_id"),
        }))
    }
    
    async fn delete_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM package_versions WHERE package_name = $1 AND version = $2")
            .bind(name)
            .bind(version)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    
    async fn download_package(&self, name: &str, version: &str) -> Result<Option<(Vec<u8>, String)>, RegistryError> {
        let row = sqlx::query(
            r#"
            SELECT pd.data, pv.checksum
            FROM package_data pd
            JOIN package_versions pv ON pd.version_id = pv.id
            WHERE pv.package_name = $1 AND pv.version = $2 AND NOT pv.yanked
            "#
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|row| (row.get("data"), row.get("checksum"))))
    }
    
    async fn yank_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        sqlx::query("UPDATE package_versions SET yanked = true WHERE package_name = $1 AND version = $2")
            .bind(name)
            .bind(version)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    
    async fn unyank_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        sqlx::query("UPDATE package_versions SET yanked = false, yanked_reason = NULL WHERE package_name = $1 AND version = $2")
            .bind(name)
            .bind(version)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    
    async fn search(&self, query: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<SearchResult>, RegistryError> {
        let limit = limit.unwrap_or(20) as i64;
        let offset = offset.unwrap_or(0) as i64;
        
        let rows = sqlx::query(
            r#"
            SELECT p.*, 
                   ts_rank_cd(to_tsvector('english', p.name || ' ' || COALESCE(p.description, '')), plainto_tsquery('english', $1)) as rank
            FROM packages p
            WHERE to_tsvector('english', p.name || ' ' || COALESCE(p.description, '')) @@ plainto_tsquery('english', $1)
            ORDER BY rank DESC, p.downloads DESC
            LIMIT $2 OFFSET $3
            "#
        )
        .bind(query)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|row| SearchResult {
            package: Package {
                name: row.get("name"),
                description: row.get("description"),
                homepage: row.get("homepage"),
                repository: row.get("repository"),
                documentation: row.get("documentation"),
                license: row.get("license"),
                keywords: row.get("keywords"),
                categories: row.get("categories"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                downloads: row.get("downloads"),
                latest_version: row.get("latest_version"),
                versions: Vec::new(),
                owners: Vec::new(),
                metadata: serde_json::from_value(row.get("metadata")).unwrap_or_default(),
            },
            score: row.get::<Option<f64>, _>("rank").unwrap_or(0.0) as f32,
        }).collect())
    }
    
    async fn get_dependents(&self, name: &str) -> Result<Vec<DependentPackage>, RegistryError> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT pv.package_name, pv.version, 'normal' as dep_type
            FROM package_versions pv
            JOIN jsonb_each_text(pv.dependencies) as dep(name, spec) ON true
            WHERE dep.name = $1
            UNION
            SELECT DISTINCT pv.package_name, pv.version, 'dev' as dep_type
            FROM package_versions pv
            JOIN jsonb_each_text(pv.dev_dependencies) as dep(name, spec) ON true
            WHERE dep.name = $1
            UNION
            SELECT DISTINCT pv.package_name, pv.version, 'build' as dep_type
            FROM package_versions pv
            JOIN jsonb_each_text(pv.build_dependencies) as dep(name, spec) ON true
            WHERE dep.name = $1
            "#
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|row| DependentPackage {
            name: row.get("package_name"),
            version: Version::parse(&row.get::<String, _>("version")).unwrap(),
            dependency_type: match row.get::<String, _>("dep_type").as_str() {
                "dev" => DependencyKind::Dev,
                "build" => DependencyKind::Build,
                _ => DependencyKind::Normal,
            },
        }).collect())
    }
    
    async fn get_dependencies(&self, name: &str, version: &str) -> Result<Vec<Dependency>, RegistryError> {
        let row = sqlx::query(
            "SELECT dependencies, dev_dependencies, build_dependencies FROM package_versions WHERE package_name = $1 AND version = $2"
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        
        let row = row.ok_or(RegistryError::NotFound(format!("{}@{}", name, version)))?;
        
        let deps: HashMap<String, DependencySpec> = serde_json::from_value(row.get("dependencies")).unwrap_or_default();
        let dev_deps: HashMap<String, DependencySpec> = serde_json::from_value(row.get("dev_dependencies")).unwrap_or_default();
        let build_deps: HashMap<String, DependencySpec> = serde_json::from_value(row.get("build_dependencies")).unwrap_or_default();
        
        let mut result = Vec::new();
        
        for (name, spec) in deps {
            result.push(Dependency {
                name,
                version_req: VersionReq::parse(&spec.version).unwrap_or_else(|_| VersionReq::STAR),
                features: spec.features,
                optional: spec.optional,
                target: spec.target,
                kind: DependencyKind::Normal,
            });
        }
        
        for (name, spec) in dev_deps {
            result.push(Dependency {
                name,
                version_req: VersionReq::parse(&spec.version).unwrap_or_else(|_| VersionReq::STAR),
                features: spec.features,
                optional: spec.optional,
                target: spec.target,
                kind: DependencyKind::Dev,
            });
        }
        
        for (name, spec) in build_deps {
            result.push(Dependency {
                name,
                version_req: VersionReq::parse(&spec.version).unwrap_or_else(|_| VersionReq::STAR),
                features: spec.features,
                optional: spec.optional,
                target: spec.target,
                kind: DependencyKind::Build,
            });
        }
        
        Ok(result)
    }
    
    async fn verify_package(&self, name: &str, version: &str) -> Result<VerificationResult, RegistryError> {
        let version_info = self.get_version(name, version).await?
            .ok_or(RegistryError::NotFound(format!("{}@{}", name, version)))?;
        
        let mut checks = Vec::new();
        
        // Checksum verification
        let download = self.download_package(name, version).await?;
        let checksum_ok = download.as_ref().map(|(data, checksum)| {
            blake3::hash(data).to_hex().to_string() == *checksum
        }).unwrap_or(false);
        
        checks.push(VerificationCheck {
            name: "checksum".to_string(),
            passed: checksum_ok,
            details: if checksum_ok { "Checksum matches".into() } else { "Checksum mismatch".into() },
        });
        
        // Signature verification (placeholder)
        checks.push(VerificationCheck {
            name: "signature".to_string(),
            passed: version_info.manifest.signature.is_some(),
            details: if version_info.manifest.signature.is_some() { "Signed".into() } else { "Not signed".into() },
        });
        
        // SBOM verification
        let sbom = self.get_sbom(name, version).await?;
        checks.push(VerificationCheck {
            name: "sbom".to_string(),
            passed: sbom.is_some(),
            details: if sbom.is_some() { "SBOM present".into() } else { "No SBOM".into() },
        });
        
        Ok(VerificationResult {
            package: name.to_string(),
            version: version.to_string(),
            verified: checks.iter().all(|c| c.passed),
            checks,
            verified_at: Utc::now(),
        })
    }
    
    async fn get_sbom(&self, name: &str, version: &str) -> Result<Option<SBOM>, RegistryError> {
        let row = sqlx::query(
            "SELECT * FROM sboms WHERE package_name = $1 AND version = $2"
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|row| SBOM {
            id: row.get("id"),
            package_name: row.get("package_name"),
            version: row.get("version"),
            format: serde_json::from_str(&row.get::<String, _>("format")).unwrap_or(SBOMFormat::SPDX),
            data: row.get("data"),
            created_at: row.get("created_at"),
            verified: row.get("verified"),
        }))
    }
    
    async fn get_attestation(&self, name: &str, version: &str) -> Result<Option<Attestation>, RegistryError> {
        let row = sqlx::query(
            "SELECT * FROM attestations WHERE package_name = $1 AND version = $2"
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.map(|row| Attestation {
            id: row.get("id"),
            package_name: row.get("package_name"),
            version: row.get("version"),
            attestation_type: serde_json::from_str(&row.get::<String, _>("attestation_type")).unwrap_or(AttestationType::Custom("unknown".into())),
            payload: row.get("payload"),
            signature: row.get("signature"),
            public_key: row.get("public_key"),
            created_at: row.get("created_at"),
        }))
    }
    
    async fn get_stats(&self) -> Result<AdminStats, RegistryError> {
        let row = sqlx::query(
            r#"
            SELECT 
                (SELECT COUNT(*) FROM packages) as total_packages,
                (SELECT COUNT(*) FROM package_versions) as total_versions,
                (SELECT COALESCE(SUM(downloads), 0) FROM package_versions) as total_downloads,
                (SELECT COALESCE(SUM(size), 0) FROM package_versions) as storage_bytes,
                (SELECT COUNT(DISTINCT user_id) FROM package_owners) as active_users
            "#
        )
        .fetch_one(&self.pool)
        .await?;
        
        Ok(AdminStats {
            total_packages: row.get("total_packages"),
            total_versions: row.get("total_versions"),
            total_downloads: row.get("total_downloads"),
            storage_bytes: row.get("storage_bytes"),
            active_users: row.get("active_users"),
        })
    }
    
    async fn record_download(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        sqlx::query(
            "UPDATE package_versions SET downloads = downloads + 1 WHERE package_name = $1 AND version = $2"
        )
        .bind(name)
        .bind(version)
        .execute(&self.pool)
        .await?;
        
        sqlx::query(
            "UPDATE packages SET downloads = downloads + 1 WHERE name = $1"
        )
        .bind(name)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
}

// In-memory storage for testing
pub struct MemoryStorage {
    packages: DashMap<String, Package>,
    versions: DashMap<String, Vec<PackageVersion>>,
    data: DashMap<Uuid, Vec<u8>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            packages: DashMap::new(),
            versions: DashMap::new(),
            data: DashMap::new(),
        }
    }
}

#[async_trait]
impl PackageStorage for MemoryStorage {
    async fn list_packages(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Package>, RegistryError> {
        let mut packages: Vec<_> = self.packages.iter().map(|p| p.value().clone()).collect();
        packages.sort_by(|a, b| b.downloads.cmp(&a.downloads));
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(100) as usize;
        Ok(packages.into_iter().skip(offset).take(limit).collect())
    }
    
    async fn count_packages(&self) -> Result<u64, RegistryError> {
        Ok(self.packages.len() as u64)
    }
    
    async fn get_package(&self, name: &str) -> Result<Option<Package>, RegistryError> {
        Ok(self.packages.get(name).map(|p| p.value().clone()))
    }
    
    async fn store_package(
        &self,
        manifest: &PackageManifest,
        data: &[u8],
        _signature: Option<String>,
        _sbom: Option<SBOM>,
        publisher: &UserRef,
    ) -> Result<PackageVersion, RegistryError> {
        let version_id = Uuid::new_v4();
        let version = Version::parse(&manifest.version).unwrap();
        
        let pkg_version = PackageVersion {
            id: version_id,
            package_name: manifest.name.clone(),
            version: version.clone(),
            manifest: manifest.clone(),
            checksum: blake3::hash(data).to_hex().to_string(),
            size: data.len() as u64,
            yanked: false,
            yanked_reason: None,
            published_at: Utc::now(),
            published_by: publisher.id,
            downloads: 0,
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            build_dependencies: Vec::new(),
            features: manifest.features.clone(),
            target_dependencies: HashMap::new(),
            sbom_id: None,
            attestation_id: None,
        };
        
        self.data.insert(version_id, data.to_vec());
        self.versions.entry(manifest.name.clone()).or_default().push(pkg_version.clone());
        
        let package = Package {
            name: manifest.name.clone(),
            description: manifest.description.clone(),
            homepage: manifest.homepage.clone(),
            repository: manifest.repository.clone(),
            documentation: manifest.documentation.clone(),
            license: manifest.license.clone(),
            keywords: Vec::new(),
            categories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            downloads: 0,
            latest_version: manifest.version.clone(),
            versions: vec![manifest.version.clone()],
            owners: vec![publisher.clone()],
            metadata: PackageMetadata::default(),
        };
        
        self.packages.insert(manifest.name.clone(), package);
        
        Ok(pkg_version)
    }
    
    async fn delete_package(&self, name: &str) -> Result<(), RegistryError> {
        self.packages.remove(name);
        self.versions.remove(name);
        Ok(())
    }
    
    async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>, RegistryError> {
        Ok(self.versions.get(name).map(|v| v.value().clone()).unwrap_or_default())
    }
    
    async fn list_versions_paginated(&self, name: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<PackageVersion>, RegistryError> {
        let versions = self.list_versions(name).await?;
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(50) as usize;
        Ok(versions.into_iter().skip(offset).take(limit).collect())
    }
    
    async fn get_version(&self, name: &str, version: &str) -> Result<Option<PackageVersion>, RegistryError> {
        Ok(self.versions.get(name)
            .and_then(|v| v.value().iter().find(|v| v.version.to_string() == version).cloned()))
    }
    
    async fn delete_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        if let Some(mut versions) = self.versions.get_mut(name) {
            versions.retain(|v| v.version.to_string() != version);
        }
        Ok(())
    }
    
    async fn download_package(&self, name: &str, version: &str) -> Result<Option<(Vec<u8>, String)>, RegistryError> {
        let version_info = self.get_version(name, version).await?;
        version_info.map(|v| {
            let data = self.data.get(&v.id).map(|d| d.value().clone()).unwrap_or_default();
            (data, v.checksum)
        }).into()
    }
    
    async fn yank_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        if let Some(mut versions) = self.versions.get_mut(name) {
            for v in versions.iter_mut() {
                if v.version.to_string() == version {
                    v.yanked = true;
                }
            }
        }
        Ok(())
    }
    
    async fn unyank_version(&self, name: &str, version: &str) -> Result<(), RegistryError> {
        if let Some(mut versions) = self.versions.get_mut(name) {
            for v in versions.iter_mut() {
                if v.version.to_string() == version {
                    v.yanked = false;
                    v.yanked_reason = None;
                }
            }
        }
        Ok(())
    }
    
    async fn search(&self, query: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<SearchResult>, RegistryError> {
        let mut results = Vec::new();
        for pkg in self.packages.iter() {
            if pkg.name().contains(query) || pkg.description().as_ref().map_or(false, |d| d.contains(query)) {
                results.push(SearchResult {
                    package: pkg.value().clone(),
                    score: 1.0,
                });
            }
        }
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;
        Ok(results.into_iter().skip(offset).take(limit).collect())
    }
    
    async fn get_dependents(&self, _name: &str) -> Result<Vec<DependentPackage>, RegistryError> {
        Ok(Vec::new())
    }
    
    async fn get_dependencies(&self, name: &str, version: &str) -> Result<Vec<Dependency>, RegistryError> {
        Ok(self.get_version(name, version).await?
            .map(|v| v.dependencies)
            .unwrap_or_default())
    }
    
    async fn verify_package(&self, name: &str, version: &str) -> Result<VerificationResult, RegistryError> {
        let version_info = self.get_version(name, version).await?
            .ok_or(RegistryError::NotFound(format!("{}@{}", name, version)))?;
        
        Ok(VerificationResult {
            package: name.to_string(),
            version: version.to_string(),
            verified: true,
            checks: vec![VerificationCheck {
                name: "checksum".to_string(),
                passed: true,
                details: "Checksum matches".to_string(),
            }],
            verified_at: Utc::now(),
        })
    }
    
    async fn get_sbom(&self, _name: &str, _version: &str) -> Result<Option<SBOM>, RegistryError> {
        Ok(None)
    }
    
    async fn get_attestation(&self, _name: &str, _version: &str) -> Result<Option<Attestation>, RegistryError> {
        Ok(None)
    }
    
    async fn get_stats(&self) -> Result<AdminStats, RegistryError> {
        Ok(AdminStats {
            total_packages: self.packages.len() as u64,
            total_versions: self.versions.iter().map(|v| v.len()).sum::<usize>() as u64,
            total_downloads: 0,
            storage_bytes: 0,
            active_users: 0,
        })
    }
    
    async fn record_download(&self, _name: &str, _version: &str) -> Result<(), RegistryError> {
        Ok(())
    }
}