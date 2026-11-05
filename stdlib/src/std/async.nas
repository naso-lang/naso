# Naso standard library for async operations

use std::time::Duration

# Async runtime functionality
export {
  async fn sleep(ms: u64) -> ()
  async fn spawn(f: fn() -> ())
  async fn join(runtime: *Runtime) -> ()
  async fn channel() -> (*Sender, *Receiver)
  async fn send(chan: *Sender, value: u64) -> ()
  async fn receive(chan: *Receiver) -> u64
}

# HTTP client functionality
export {
  async fn get(url: &str) -> u32
  async fn post(url: &str, body: &str) -> u32
  async fn put(url: &str, body: &str) -> u32
  async fn delete(url: &str) -> u32
}

# Error handling
export {
  enum Error {
    None,
    Generic,
    Network,
    Parse,
    Timeout,
    Auth,
  }
}