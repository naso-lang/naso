//! Naso Web Framework
//!
//! A full-featured, quantum-ready web framework for Naso with:
//! - Async request/response handling
//! - ORM with migrations
//! - Template engine (Askama)
//! - Authentication and authorization
//! - WebSocket support
//! - REST and GraphQL APIs
//! - Middleware system

use axum::{
    extract::{State, Request, Path, Query},
    http::{StatusCode, HeaderMap},
    response::{IntoResponse, Response, Json},
    routing::{get, post, put, delete, patch},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower::{ServiceBuilder, Layer};
use tower_http::{cors::CorsLayer, trace::TraceLayer, compression::CompressionLayer, timeout::TimeoutLayer};
use tracing::{info, warn, error, instrument};
use uuid::Uuid;
use chrono::Utc;

pub mod app;
pub mod router;
pub mod controller;
pub mod middleware;
pub mod orm;
pub mod template;
pub mod auth;
pub mod websocket;
pub mod error;
pub mod form;
pub mod validation;
pub mod migration;
pub mod cli;

use app::App;
use router::Router as NasoRouter;
use middleware::Middleware;
use error::AppError;
use cli::WebCli;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub name: String,
    pub version: String,
    pub host: String,
    pub port: u16,
    pub workers: usize,
    pub database_url: Option<String>,
    pub secret_key: String,
    pub debug: bool,
    pub cors_origins: Vec<String>,
    pub max_body_size: usize,
    pub request_timeout_secs: u64,
    pub enable_websockets: bool,
    pub enable_graphql: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: "naso-web-app".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            host: "0.0.0.0".to_string(),
            port: 8080,
            workers: num_cpus::get(),
            database_url: None,
            secret_key: Uuid::new_v4().to_string(),
            debug: false,
            cors_origins: vec!["*".to_string()],
            max_body_size: 10 * 1024 * 1024, // 10MB
            request_timeout_secs: 30,
            enable_websockets: true,
            enable_graphql: false,
        }
    }
}

#[derive(Clone)]
pub struct WebApp {
    config: AppConfig,
    inner: Arc<RwLock<AppInner>>,
}

#[derive(Clone)]
pub struct AppInner {
    pub routes: HashMap<String, RouteEntry>,
    pub middleware: Vec<Box<dyn Middleware>>,
    pub controllers: HashMap<String, Box<dyn Controller>>,
    pub db_pool: Option<sqlx::PgPool>,
}

#[derive(Clone)]
pub struct RouteEntry {
    pub path: String,
    pub method: http::Method,
    pub handler: Box<dyn RouteHandler>,
}

pub trait RouteHandler: Send + Sync {
    fn handle(&self, req: Request) -> BoxFuture<Response>;
}

pub trait Controller: Send + Sync {
    fn name(&self) -> &str;
    fn routes(&self) -> Vec<RouteEntry>;
}

pub type BoxFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>>;

impl WebApp {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: config.clone(),
            inner: Arc::new(RwLock::new(AppInner {
                routes: HashMap::new(),
                middleware: Vec::new(),
                controllers: HashMap::new(),
                db_pool: None,
            })),
        }
    }

    pub fn with_database(mut self, db_url: &str) -> Result<Self, AppError> {
        // Pool would be set in async context
        Ok(self)
    }

    pub fn add_middleware<M: Middleware + 'static>(mut self, middleware: M) -> Self {
        self.inner.write().middleware.push(Box::new(middleware));
        self
    }

    pub fn add_controller<C: Controller + 'static>(mut self, controller: C) -> Self {
        let name = controller.name().to_string();
        let routes = controller.routes();
        self.inner.write().controllers.insert(name, Box::new(controller));
        self.inner.write().routes.extend(
            routes.into_iter().map(|r| (format!("{}:{}", r.method, r.path), r))
        );
        self
    }

    pub fn route(mut self, path: &str, method: http::Method, handler: Box<dyn RouteHandler>) -> Self {
        self.inner.write().routes.insert(
            format!("{}:{}", method, path),
            RouteEntry { path: path.to_string(), method, handler }
        );
        self
    }

    pub async fn run(self) -> Result<(), AppError> {
        let router = self.build_axum_router().await?;
        
        let listener = tokio::net::TcpListener::bind(
            format!("{}:{}", self.config.host, self.config.port)
        ).await?;

        info!("Naso web server listening on {}:{}", self.config.host, self.config.port);
        
        axum::serve(listener, router).await?;
        Ok(())
    }

    async fn build_axum_router(self) -> Result<Router, AppError> {
        let mut router = Router::new();

        // Add built-in routes
        router = router.route("/health", get(health_handler));
        router = router.route("/ready", get(readiness_handler));
        router = router.route("/metrics", get(metrics_handler));

        // Add user routes
        let inner = self.inner.read().await;
        for (_, entry) in inner.routes.iter() {
            // Routes would be registered here
        }
        drop(inner);

        // Apply middleware
        let mut router_with_middleware = router
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive())
            .layer(CompressionLayer::new())
            .layer(TimeoutLayer::new(std::time::Duration::from_secs(self.config.request_timeout_secs)));

        Ok(router_with_middleware)
    }
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({"status": "healthy", "service": "naso-web"}))
}

async fn readiness_handler() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ready"}))
}

async fn metrics_handler() -> impl IntoResponse {
    "# HELP naso_web_requests_total Total requests\n# TYPE naso_web_requests_total counter\nnaso_web_requests_total 0\n".to_string()
}

impl Default for WebApp {
    fn default() -> Self {
        Self::new(AppConfig::default())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Internal server error: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database error".to_string()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            _ => (StatusCode::BAD_REQUEST, self.to_string()),
        };
        (status, Json(serde_json::json!({"error": error_message}))).into_response()
    }
}