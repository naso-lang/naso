//! Naso Standard Library
//!
//! Standard library modules for Naso.

pub mod alloc;
pub mod prelude;
pub mod quantum;
pub mod sys;
pub mod tensor;

// Naso source files (compiled by naso compiler, not rustc)
// Core modules:
// async.naso, channel.naso, hashmap.naso, http.naso, mutex.naso, vec.naso

// Phase 19 - Stdlib Completeness modules (Naso source):
// async_io.naso, http_client.naso, tls.naso, serde.naso, sql.naso, redis.naso,
// metrics.naso, tracing.naso, cli.naso, hal.naso

// Phase 20 - Ecosystem modules (Naso source):
// pkg_registry.naso