//! Graceful shutdown for Naso Cloud Runtime

use super::{CloudError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;

/// Shutdown signal types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    Sigterm,
    Sigint,
    Sigquit,
    Custom(i32),
}

/// Shutdown manager for graceful shutdown
pub struct ShutdownManager {
    handlers: Vec<Box<dyn FnOnce() + Send>>,
    signals_registered: bool,
    deadline: Option<std::time::Instant>,
    notify: Arc<Notify>,
    shutdown_initiated: Arc<std::sync::atomic::AtomicBool>,
}

impl ShutdownManager {
    pub fn new() -> Self {
        ShutdownManager {
            handlers: Vec::new(),
            signals_registered: false,
            deadline: None,
            notify: Arc::new(Notify::new()),
            shutdown_initiated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Register a shutdown handler
    pub fn on_shutdown(&mut self, handler: impl FnOnce() + Send + 'static) {
        self.handlers.push(Box::new(handler));
    }

    /// Register signal handlers
    pub fn register_signals(&mut self) -> Result<()> {
        if self.signals_registered {
            return Ok(());
        }

        let notify = self.notify.clone();
        let shutdown_initiated = self.shutdown_initiated.clone();

        tokio::spawn(async move {
            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM");
            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT");

            tokio::select! {
                _ = sigterm.recv() => {
                    shutdown_initiated.store(true, std::sync::atomic::Ordering::SeqCst);
                    notify.notify_waiters();
                }
                _ = sigint.recv() => {
                    shutdown_initiated.store(true, std::sync::atomic::Ordering::SeqCst);
                    notify.notify_waiters();
                }
            }
        });

        self.signals_registered = true;
        Ok(())
    }

    /// Set shutdown deadline
    pub fn set_deadline(&mut self, duration: Duration) {
        self.deadline = Some(std::time::Instant::now() + duration);
    }

    /// Check if shutdown has been initiated
    pub fn is_initiated(&self) -> bool {
        self.shutdown_initiated.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get remaining time until deadline
    pub fn time_remaining(&self) -> Option<Duration> {
        self.deadline.map(|d| {
            let now = std::time::Instant::now();
            if d > now { d - now } else { Duration::from_secs(0) }
        })
    }

    /// Wait for shutdown signal
    pub async fn wait(&self) {
        self.notify.notified().await;
    }

    /// Execute all shutdown handlers
    pub fn execute(&mut self) {
        for handler in self.handlers.drain(..) {
            handler();
        }
    }

    /// Execute shutdown handlers with timeout
    pub async fn execute_with_timeout(&mut self, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;

        for handler in self.handlers.drain(..) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            // Run handler with timeout
            let handler_future = async { handler() };
            tokio::time::timeout(remaining, handler_future).await.ok();
        }
    }
}

impl Default for ShutdownManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Shutdown coordination for distributed systems
pub struct DistributedShutdown {
    manager: ShutdownManager,
    peers: Vec<String>,
    coordinator: Option<String>,
}

impl DistributedShutdown {
    pub fn new() -> Self {
        DistributedShutdown {
            manager: ShutdownManager::new(),
            peers: Vec::new(),
            coordinator: None,
        }
    }

    pub fn add_peer(mut self, peer: &str) -> Self {
        self.peers.push(peer.into());
        self
    }

    pub fn set_coordinator(mut self, coordinator: &str) -> Self {
        self.coordinator = Some(coordinator.into());
        self
    }

    pub fn manager(&mut self) -> &mut ShutdownManager {
        &mut self.manager
    }

    pub async fn initiate_shutdown(&self) -> Result<()> {
        // Notify peers
        for peer in &self.peers {
            // Would send shutdown signal to peer
        }
        Ok(())
    }
}