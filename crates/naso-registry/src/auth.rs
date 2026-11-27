//! Authentication and authorization for the Naso package registry

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::UserRef;
use crate::RegistryError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,        // User ID
    pub username: String,
    pub scopes: HashSet<String>,
    pub exp: i64,           // Expiration timestamp
    pub iat: i64,           // Issued at
    pub typ: TokenType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenType {
    Access,
    Refresh,
    Publish,
    Admin,
}

pub struct AuthManager {
    pool: PgPool,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    access_token_ttl: Duration,
    refresh_token_ttl: Duration,
}

impl AuthManager {
    pub fn new(pool: PgPool) -> Self {
        let secret = std::env::var("NASO_JWT_SECRET")
            .unwrap_or_else(|_| "dev-secret-change-in-production".to_string());
        
        Self {
            pool,
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            access_token_ttl: Duration::hours(1),
            refresh_token_ttl: Duration::days(30),
        }
    }
    
    pub async fn authenticate_publish(&self, headers: &http::HeaderMap) -> Result<UserRef, RegistryError> {
        let token = self.extract_token(headers)?;
        let claims = self.validate_token(&token).await?;
        
        if !claims.scopes.contains("publish") && !claims.scopes.contains("admin") {
            return Err(RegistryError::AuthDenied("Publish scope required".into()));
        }
        
        Ok(UserRef {
            id: Uuid::parse_str(&claims.sub).unwrap(),
            username: claims.username,
            avatar_url: None,
        })
    }
    
    pub async fn authenticate_admin(&self, headers: &http::HeaderMap) -> Result<UserRef, RegistryError> {
        let token = self.extract_token(headers)?;
        let claims = self.validate_token(&token).await?;
        
        if !claims.scopes.contains("admin") {
            return Err(RegistryError::AuthDenied("Admin scope required".into()));
        }
        
        Ok(UserRef {
            id: Uuid::parse_str(&claims.sub).unwrap(),
            username: claims.username,
            avatar_url: None,
        })
    }
    
    pub async fn create_token(&self, request: &TokenRequest) -> Result<TokenResponse, RegistryError> {
        // Verify user credentials
        let user = self.verify_credentials(&request.username, &request.password).await?;
        
        let scopes: HashSet<String> = request.scopes.iter().cloned().collect();
        
        let access_token = self.create_jwt(&user.id, &user.username, scopes.clone(), TokenType::Access)?;
        let refresh_token = self.create_jwt(&user.id, &user.username, scopes, TokenType::Refresh)?;
        
        // Store refresh token
        self.store_refresh_token(&user.id, &refresh_token).await?;
        
        Ok(TokenResponse {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.access_token_ttl.num_seconds(),
        })
    }
    
    pub async fn verify_token(&self, token: &str) -> Result<bool, RegistryError> {
        let _ = self.validate_token(token).await?;
        Ok(true)
    }
    
    pub async fn refresh_access_token(&self, refresh_token: &str) -> Result<TokenResponse, RegistryError> {
        let claims = self.validate_token(refresh_token).await?;
        
        if claims.typ != TokenType::Refresh {
            return Err(RegistryError::AuthFailed("Invalid token type".into()));
        }
        
        // Check if refresh token is valid and not revoked
        if !self.is_refresh_token_valid(&claims.sub, refresh_token).await? {
            return Err(RegistryError::AuthFailed("Refresh token revoked".into()));
        }
        
        let user = self.get_user(&claims.sub).await?;
        let scopes = claims.scopes;
        
        let access_token = self.create_jwt(&user.id, &user.username, scopes.clone(), TokenType::Access)?;
        let new_refresh_token = self.create_jwt(&user.id, &user.username, scopes, TokenType::Refresh)?;
        
        // Rotate refresh token
        self.revoke_refresh_token(&claims.sub, refresh_token).await?;
        self.store_refresh_token(&user.id, &new_refresh_token).await?;
        
        Ok(TokenResponse {
            access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.access_token_ttl.num_seconds(),
        })
    }
    
    pub async fn revoke_token(&self, token: &str) -> Result<(), RegistryError> {
        let claims = self.validate_token(token).await?;
        self.revoke_refresh_token(&claims.sub, token).await?;
        Ok(())
    }
    
    async fn validate_token(&self, token: &str) -> Result<Claims, RegistryError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        
        let token_data = decode::<Claims>(token, &self.decoding_key, &validation)
            .map_err(|e| RegistryError::AuthFailed(format!("Invalid token: {}", e)))?;
        
        Ok(token_data.claims)
    }
    
    fn create_jwt(&self, user_id: &Uuid, username: &str, scopes: HashSet<String>, typ: TokenType) -> Result<String, RegistryError> {
        let now = Utc::now();
        let exp = match typ {
            TokenType::Access => now + self.access_token_ttl,
            TokenType::Refresh => now + self.refresh_token_ttl,
            TokenType::Publish => now + Duration::hours(24),
            TokenType::Admin => now + Duration::hours(8),
        };
        
        let claims = Claims {
            sub: user_id.to_string(),
            username: username.to_string(),
            scopes,
            exp: exp.timestamp(),
            iat: now.timestamp(),
            typ,
        };
        
        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| RegistryError::AuthFailed(format!("Failed to create token: {}", e)))
    }
    
    fn extract_token(&self, headers: &http::HeaderMap) -> Result<String, RegistryError> {
        let auth_header = headers.get("Authorization")
            .ok_or(RegistryError::AuthFailed("Missing Authorization header".into()))?;
        
        let auth_str = auth_header.to_str()
            .map_err(|_| RegistryError::AuthFailed("Invalid Authorization header".into()))?;
        
        if !auth_str.starts_with("Bearer ") {
            return Err(RegistryError::AuthFailed("Invalid token format".into()));
        }
        
        Ok(auth_str[7..].to_string())
    }
    
    async fn verify_credentials(&self, username: &str, password: &str) -> Result<UserRef, RegistryError> {
        let row = sqlx::query(
            "SELECT id, username, password_hash FROM users WHERE username = $1"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        
        let row = row.ok_or(RegistryError::AuthFailed("Invalid credentials".into()))?;
        
        let password_hash: String = row.get("password_hash");
        
        // Verify password (using argon2)
        argon2::verify_encoded(&password_hash, password.as_bytes())
            .map_err(|_| RegistryError::AuthFailed("Invalid credentials".into()))?;
        
        Ok(UserRef {
            id: row.get("id"),
            username: row.get("username"),
            avatar_url: None,
        })
    }
    
    async fn store_refresh_token(&self, user_id: &Uuid, token: &str) -> Result<(), RegistryError> {
        let claims = self.validate_token(token).await?;
        let expires_at = DateTime::from_timestamp(claims.exp, 0).unwrap();
        
        sqlx::query(
            "INSERT INTO refresh_tokens (user_id, token_hash, expires_at) VALUES ($1, $2, $3)"
        )
        .bind(user_id)
        .bind(blake3::hash(token.as_bytes()).to_hex().to_string())
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    async fn is_refresh_token_valid(&self, user_id: &str, token: &str) -> Result<bool, RegistryError> {
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        
        let row = sqlx::query(
            "SELECT 1 FROM refresh_tokens WHERE user_id = $1 AND token_hash = $2 AND expires_at > NOW() AND revoked = false"
        )
        .bind(Uuid::parse_str(user_id).unwrap())
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(row.is_some())
    }
    
    async fn revoke_refresh_token(&self, user_id: &str, token: &str) -> Result<(), RegistryError> {
        let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        
        sqlx::query(
            "UPDATE refresh_tokens SET revoked = true WHERE user_id = $1 AND token_hash = $2"
        )
        .bind(Uuid::parse_str(user_id).unwrap())
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    async fn get_user(&self, user_id: &str) -> Result<UserRef, RegistryError> {
        let row = sqlx::query(
            "SELECT id, username FROM users WHERE id = $1"
        )
        .bind(Uuid::parse_str(user_id).unwrap())
        .fetch_one(&self.pool)
        .await?;
        
        Ok(UserRef {
            id: row.get("id"),
            username: row.get("username"),
            avatar_url: None,
        })
    }
}

#[derive(Deserialize)]
pub struct TokenRequest {
    pub username: String,
    pub password: String,
    pub scopes: Vec<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

#[derive(Deserialize)]
pub struct VerifyTokenRequest {
    pub token: String,
}

#[derive(Serialize)]
pub struct VerifyTokenResponse {
    pub valid: bool,
}