//! Serverless runtime for Naso Cloud Runtime

use super::{CloudError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerlessPlatform {
    AwsLambda,
    GcpCloudRun,
    AzureFunctions,
    CloudflareWorkers,
    Knative,
    OpenFaaS,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
    pub context: Option<RequestContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestContext {
    pub request_id: String,
    pub trace_id: Option<String>,
    pub identity: Option<String>,
    pub source_ip: Option<String>,
    pub stage: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub is_base64_encoded: bool,
}

impl Response {
    pub fn ok(body: Vec<u8>) -> Self {
        Response {
            status: 200,
            headers: HashMap::new(),
            body,
            is_base64_encoded: false,
        }
    }

    pub fn json<T: Serialize>(value: &T) -> Result<Self> {
        let body = serde_json::to_vec(value)
            .map_err(|e| CloudError::SerializationError(e.to_string()))?;
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        Ok(Response {
            status: 200,
            headers,
            body,
            is_base64_encoded: false,
        })
    }

    pub fn error(status: u16, message: &str) -> Self {
        Response {
            status,
            headers: HashMap::new(),
            body: message.into(),
            is_base64_encoded: false,
        }
    }
}

/// Serverless runtime
pub struct ServerlessRuntime {
    platform: ServerlessPlatform,
    handler: Option<Arc<dyn Fn(Request) -> Response + Send + Sync>>,
    initialization_handler: Option<Arc<dyn Fn() + Send + Sync>>,
    shutdown_handler: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ServerlessRuntime {
    pub fn new(platform: ServerlessPlatform) -> Self {
        ServerlessRuntime {
            platform,
            handler: None,
            initialization_handler: None,
            shutdown_handler: None,
        }
    }

    pub fn handle<F>(mut self, handler: F) -> Self
    where
        F: Fn(Request) -> Response + Send + Sync + 'static,
    {
        self.handler = Some(Arc::new(handler));
        self
    }

    pub fn on_init<F>(mut self, handler: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.initialization_handler = Some(Arc::new(handler));
        self
    }

    pub fn on_shutdown<F>(mut self, handler: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.shutdown_handler = Some(Arc::new(handler));
        self
    }

    pub fn run(&self) -> Result<()> {
        match self.platform {
            ServerlessPlatform::AwsLambda => self.run_lambda(),
            ServerlessPlatform::GcpCloudRun => self.run_cloud_run(),
            ServerlessPlatform::Knative => self.run_knative(),
            _ => Err(CloudError::NotImplemented(format!("Platform {:?} not implemented", self.platform))),
        }
    }

    fn run_lambda(&self) -> Result<()> {
        // AWS Lambda Runtime API
        // Would implement lambda_runtime crate integration
        Err(CloudError::NotImplemented("Lambda runtime not implemented".into()))
    }

    fn run_cloud_run(&self) -> Result<()> {
        // HTTP server for Cloud Run
        // Cloud Run sends HTTP requests to the container
        Err(CloudError::NotImplemented("Cloud Run runtime not implemented".into()))
    }

    fn run_knative(&self) -> Result<()> {
        // Knative serving - HTTP based
        Err(CloudError::NotImplemented("Knative runtime not implemented".into()))
    }
}

/// AWS Lambda event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaEvent {
    pub version: String,
    pub route_key: String,
    pub raw_path: String,
    pub raw_query_string: String,
    pub headers: HashMap<String, String>,
    pub query_string_parameters: Option<HashMap<String, String>>,
    pub request_context: LambdaRequestContext,
    pub body: Option<String>,
    pub is_base64_encoded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaRequestContext {
    pub account_id: String,
    pub api_id: String,
    pub domain_name: String,
    pub domain_prefix: String,
    pub http: LambdaHttpContext,
    pub request_id: String,
    pub route_key: String,
    pub stage: String,
    pub time: String,
    pub time_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LambdaHttpContext {
    pub method: String,
    pub path: String,
    pub protocol: String,
    pub source_ip: String,
    pub user_agent: String,
}

/// Cloud Run request format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRunRequest {
    pub message: CloudRunMessage,
    pub subscription: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRunMessage {
    pub data: String,
    pub message_id: String,
    pub publish_time: String,
    pub attributes: HashMap<String, String>,
}

/// Knative event format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudEvent {
    pub specversion: String,
    pub id: String,
    pub source: String,
    pub typ: String,
    pub datacontenttype: Option<String>,
    pub dataschema: Option<String>,
    pub subject: Option<String>,
    pub time: Option<String>,
    pub data: Option<serde_json::Value>,
}

/// Convert platform-specific event to generic Request
pub fn lambda_to_request(event: LambdaEvent) -> Request {
    let mut query = HashMap::new();
    if let Some(q) = event.query_string_parameters {
        query = q;
    }

    Request {
        method: event.request_context.http.method,
        path: event.raw_path,
        headers: event.headers,
        query,
        body: event.body.unwrap_or_default().into_bytes(),
        context: Some(RequestContext {
            request_id: event.request_context.request_id,
            trace_id: None,
            identity: None,
            source_ip: Some(event.request_context.http.source_ip),
            stage: Some(event.request_context.stage),
        }),
    }
}

pub fn cloud_run_to_request(event: CloudRunRequest) -> Request {
    let data = base64::decode(&event.message.data).unwrap_or_default();
    Request {
        method: "POST".into(),
        path: "/".into(),
        headers: HashMap::new(),
        query: HashMap::new(),
        body: data,
        context: Some(RequestContext {
            request_id: event.message.message_id,
            trace_id: None,
            identity: None,
            source_ip: None,
            stage: None,
        }),
    }
}

/// Function signature for serverless handlers
pub type ServerlessHandler = dyn Fn(Request) -> Response + Send + Sync;

/// Helper to create a simple HTTP handler
pub fn http_handler<F>(handler: F) -> Box<ServerlessHandler>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    Box::new(handler)
}

/// Middleware for serverless
pub trait ServerlessMiddleware: Send + Sync {
    fn before(&self, request: &mut Request) -> Result<()>;
    fn after(&self, request: &Request, response: &mut Response) -> Result<()>;
}

/// Logging middleware
pub struct LoggingMiddleware;

impl ServerlessMiddleware for LoggingMiddleware {
    fn before(&self, request: &mut Request) -> Result<()> {
        tracing::info!("Request: {} {}", request.method, request.path);
        Ok(())
    }

    fn after(&self, request: &Request, response: &mut Response) -> Result<()> {
        tracing::info!("Response: {} for {} {}", response.status, request.method, request.path);
        Ok(())
    }
}

/// Tracing middleware
pub struct TracingMiddleware;

impl ServerlessMiddleware for TracingMiddleware {
    fn before(&self, request: &mut Request) -> Result<()> {
        if let Some(ctx) = &request.context {
            if let Some(trace_id) = &ctx.trace_id {
                // Set trace context
            }
        }
        Ok(())
    }

    fn after(&self, _request: &Request, _response: &mut Response) -> Result<()> {
        Ok(())
    }
}

/// Middleware chain
pub struct MiddlewareChain {
    middlewares: Vec<Box<dyn ServerlessMiddleware>>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        MiddlewareChain {
            middlewares: Vec::new(),
        }
    }

    pub fn add(mut self, middleware: Box<dyn ServerlessMiddleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    pub fn execute(&self, request: &mut Request, response: &mut Response) -> Result<()> {
        for mw in &self.middlewares {
            mw.before(request)?;
        }
        for mw in &self.middlewares {
            mw.after(request, response)?;
        }
        Ok(())
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}