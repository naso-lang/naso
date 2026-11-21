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

// Phase 19 - Stdlib Completeness modules:
pub mod async_io;      // Phase 19.1: Async I/O traits (AsyncRead, AsyncWrite, AsyncSeek, AsyncBufRead)
pub mod http_client;   // Phase 19.2: HTTP/1.1 + HTTP/2 client with connection pooling
pub mod tls;           // Phase 19.3: TLS 1.2/1.3 with verified cryptographic primitives
pub mod serde;         // Phase 19.4: Serialization with QTT types (JSON, MessagePack, CBOR, Bincode)
pub mod sql;           // Phase 19.5: SQL driver (PostgreSQL, MySQL, SQLite)
pub mod redis;         // Phase 19.5: Redis client with clustering, pub/sub, pipelines
pub mod metrics;       // Phase 19.6: Metrics collection (Prometheus, OpenTelemetry, custom exporters)
pub mod tracing;       // Phase 19.6: Distributed tracing (OpenTelemetry, Jaeger, Zipkin)
pub mod cli;           // Phase 19.7: CLI framework (typed args, subcommands, completions)
pub mod hal;           // Phase 19.8: Hardware Abstraction Layer (GPIO, I2C, SPI, UART, PWM, ADC, DMA, timers)