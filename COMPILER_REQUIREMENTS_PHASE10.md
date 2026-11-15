# Naso Compiler Requirements for Phase 10 Stdlib

## Executive Summary
The Phase 10 HTTP Middleware & Routing stdlib is **architecturally complete** but **blocked by parser limitations**. The compiler rejects valid Naso code that follows the language's own design principles (zero-cost abstractions, QTT-linear ownership, pointer-based memory management).

These are not edge cases — they are fundamental patterns required for any systems programming standard library.

---

## Required Compiler Fixes (Priority Order)

### 1. Struct Array Fields > 16 Elements
**Current**: `type mismatch: expected Nat, found Nat` when struct contains `[u8; 32]` or larger
**Required**: Support arrays of arbitrary size in struct definitions
**Impact**: Response bodies, request buffers, route patterns all need >16 bytes

```naso
// MUST WORK
struct Response {
    status: u16,
    body: [u8; 256],  // HTTP response bodies
    body_len: usize,
}

struct Request {
    method: u8,
    path: [u8; 256],  // URL paths
    path_len: usize,
    headers: [u8; 512],  // HTTP headers
    headers_len: usize,
}
```

### 2. Array Repeat Initialization `[value; N]`
**Current**: `parse error: expected ',', found ';'`
**Required**: Support `[0; 256]`, `[u8; 256]` syntax
**Impact**: Zero-initializing buffers is standard systems programming

```naso
// MUST WORK
let buffer: [u8; 256] = [0; 256];
let routes: [Route; 64] = [default_route(); 64];
```

### 3. `loop` / `break` Control Flow
**Current**: `parse error: expected '}', found '{'`
**Required**: Support infinite loops with break conditions
**Impact**: Server loops, parser loops, middleware chains

```naso
// MUST WORK
fn main() -> () {
    loop {
        if should_exit() { break; }
        process_request();
    }
}
```

### 4. Pointer-to-Struct Field Access
**Current**: `field 'count' not found in type '*MiddlewareChain'`
**Required**: Dereference and field access on `*T` where `T` is struct
**Impact**: All middleware/chain/state mutation patterns

```naso
// MUST WORK
fn middleware_chain_add(chain: *MiddlewareChain, middleware: Middleware) -> () {
    chain.middlewares[chain.count] = middleware;  // Currently fails
    chain.count = chain.count + 1;
}
```

### 5. Struct Initializers in Function Returns
**Current**: `parse error` or `type mismatch` on complex returns
**Required**: Full struct literal support in return position
**Impact**: Handler returns, middleware chain results, router construction

```naso
// MUST WORK
fn handler_plaintext(req: Request) -> Response {
    Response { 
        status: 200, 
        body: build_body(),  // Helper function returning array
        body_len: 13 
    }
}
```

### 6. Mut Parameters with Struct Field Mutation
**Current**: Works for value params (`mut chain: T`), fails for pointers
**Required**: Consistent mutation semantics for both `mut T` and `*T`
**Impact**: API consistency, zero-copy patterns

### 7. Large Array Literals in Expressions
**Current**: Parser fails on inline arrays > ~16 elements
**Required**: Arbitrary array literals anywhere expressions allowed
**Impact**: HTTP status lines, default responses, test data

```naso
// MUST WORK
let status_line = [72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10];
// 17 elements - currently fails if in certain contexts
```

---

## Code That Must Compile (Test Suite)

### Test 1: Response Struct with 256-byte Body
```naso
struct Response {
    status: u16,
    body: [u8; 256],
    body_len: usize,
}

fn make_response() -> Response {
    let mut body: [u8; 256] = [0; 256];
    body[0] = 72;  // 'H'
    body[1] = 101; // 'e'
    // ... fill rest
    Response { status: 200, body: body, body_len: 13 }
}
```

### Test 2: Router with 64 Routes
```naso
struct Route {
    method: u8,
    path: [u8; 128],
    path_len: usize,
    handler: *u8,
}

struct Router {
    routes: [Route; 64],
    route_count: usize,
}

fn router_new() -> Router {
    let default = Route { method: 0, path: [0; 128], path_len: 0, handler: 0 as *u8 };
    Router { routes: [default; 64], route_count: 0 }
}
```

### Test 3: Middleware Chain Mutation
```naso
struct Middleware { handler: *u8 }
struct MiddlewareChain { middlewares: [Middleware; 16], count: usize }

fn add_middleware(chain: *MiddlewareChain, m: Middleware) -> () {
    if chain.count < 16 {
        chain.middlewares[chain.count] = m;
        chain.count = chain.count + 1;
    }
}
```

### Test 4: Server Loop
```naso
fn main() -> () {
    let listener = bind_socket();
    loop {
        let client = accept(listener);
        if client == 0 { continue; }
        handle(client);
        close(client);
    }
}
```

### Test 5: Handler with Array Return Helper
```naso
fn build_body() -> [u8; 13] {
    [72, 101, 108, 108, 111, 44, 32, 87, 111, 114, 108, 100, 33]
}

fn handler(req: Request) -> Response {
    Response { status: 200, body: build_body(), body_len: 13 }
}
```

---

## Architecture Impact

### What Phase 10 Enables (Once Compiler Fixed)
| Feature | Status | Blocked By |
|---------|--------|------------|
| Full middleware chain (5 built-in) | Designed | #1, #2, #4 |
| Wildcard route matching (`/users/:id`) | Designed | #1, #2, #7 |
| QTT-linear `OwnedRequest/Response` | Designed | #4, #5 |
| Method-specific routing | Designed | #1, #2 |
| Path/Query/JSON/Header extractors | Designed | #1, #5 |
| Response formatting with headers | Designed | #1, #2, #7 |
| TechEmpower benchmark parity | Designed | All |

### Files Blocked
- `stdlib/src/std/http.naso` — **Primary deliverable** (~500 lines, fully designed)
- `examples/techempower_middleware.naso` — Benchmark target
- Any stdlib module needing buffers > 16 bytes

---

## Timeline Estimate

| Fix | Effort | Unlocks |
|-----|--------|---------|
| #1 Struct arrays >16 | Medium | All structs |
| #2 Array repeat init | Low | Buffer init |
| #3 Loop/break | Low | Server loops |
| #4 Pointer field access | Medium | All mutation |
| #5 Return struct literals | Medium | All handlers |
| #6 Mut consistency | Low | API coherence |
| #7 Large array literals | Low | Constants |

**Total**: ~2-3 weeks compiler work → **Phase 10 stdlib compiles in 1 day**

---

## Recommendation

**Fix compiler first.** The stdlib is ready — it's been designed, documented, and validated in `phase10_working.naso` within current constraints. The compiler gaps are exactly the gaps between "toy language" and "systems language better than Rust."

Every item above is table stakes for:
- HTTP servers
- Database drivers  
- Protocol implementations
- Any zero-copy parsing
- High-performance networking

**Deliverable**: This document + test cases above → Naso compiler team → fixes → Phase 10 merges immediately.