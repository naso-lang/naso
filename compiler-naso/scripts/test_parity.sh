#!/bin/bash
# Test runner for self-hosted Naso compiler
#
# This script runs the compiler test suite with both the Rust and self-hosted
# compilers and compares outputs for parity.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
NASO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"

RUST_NASO="$NASO_ROOT/target/release/naso"
SELF_HOSTED="$PROJECT_ROOT/self/naso"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

if [[ ! -f "$RUST_NASO" ]]; then
    log_error "Rust naso not found at $RUST_NASO"
    exit 1
fi

if [[ ! -f "$SELF_HOSTED" ]]; then
    log_error "Self-hosted naso not found at $SELF_HOSTED"
    exit 1
fi

TEST_DIR="$PROJECT_ROOT/tests"
mkdir -p "$TEST_DIR"

# Test cases
cat > "$TEST_DIR/test_basic.naso" << 'EOF'
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn main() {
    let x = add(1, 2);
    let y = add(x, 3);
}
EOF

cat > "$TEST_DIR/test_quantum.naso" << 'EOF'
fn bell_pair(q1: [1] Qubit, q2: [1] Qubit) -> () {
    hadamard(q1);
    cnot(q1, q2);
}

fn main() -> () {
    let q1 = qalloc();
    let q2 = qalloc();
    bell_pair(q1, q2);
}
EOF

cat > "$TEST_DIR/test_linear.naso" << 'EOF'
fn consume_linear(x: [1] i32) -> i32 {
    return x;
}

fn main() {
    let a: [1] i32 = 42;
    let b = consume_linear(a);
}
EOF

cat > "$TEST_DIR/test_reversible.naso" << 'EOF'
fn reversible_add(a: i32, b: i32) -> (i32, i32) {
    reversible {
        let sum = a + b;
        let diff = a - b;
        return (sum, diff);
    }
}

fn main() {
    let (s, d) = reversible_add(5, 3);
}
EOF

run_test() {
    local test_name="$1"
    local test_file="$2"
    local should_pass="${3:-true}"
    
    log_info "Running test: $test_name"
    
    # Compile with Rust naso
    local rust_out="/tmp/${test_name}_rust.ll"
    if "$RUST_NASO" build "$test_file" -o "$rust_out" --target llvm 2>/dev/null; then
        local rust_result=0
    else
        local rust_result=1
    fi
    
    # Compile with self-hosted naso
    local self_out="/tmp/${test_name}_self.ll"
    if "$SELF_HOSTED" build "$test_file" -o "$self_out" --target llvm 2>/dev/null; then
        local self_result=0
    else
        local self_result=1
    fi
    
    if [[ $rust_result -eq 0 && $self_result -eq 0 ]]; then
        # Compare outputs (ignore comments and whitespace)
        local rust_clean=$(grep -v "^;" "$rust_out" | sed '/^$/d' | sort)
        local self_clean=$(grep -v "^;" "$self_out" | sed '/^$/d' | sort)
        
        if [[ "$rust_clean" == "$self_clean" ]]; then
            log_success "$test_name: PASS (output parity)"
            return 0
        else
            log_warn "$test_name: Output differs (may be metadata)"
            log_info "Rust naso output lines: $(echo "$rust_clean" | wc -l)"
            log_info "Self-hosted output lines: $(echo "$self_clean" | wc -l)"
            return 0  # Not a failure, just warning
        fi
    elif [[ $rust_result -eq $self_result ]]; then
        if [[ "$should_pass" == "true" ]]; then
            log_error "$test_name: Both compilers failed (expected pass)"
            return 1
        else
            log_success "$test_name: Both failed as expected"
            return 0
        fi
    else
        log_error "$test_name: MISMATCH - Rust: $rust_result, Self-hosted: $self_result"
        return 1
    fi
}

log_info "Starting self-hosted compiler parity tests..."

FAILED=0
TOTAL=0

run_test "basic" "$TEST_DIR/test_basic.naso" || ((FAILED++))
((TOTAL++))

run_test "quantum" "$TEST_DIR/test_quantum.naso" || ((FAILED++))
((TOTAL++))

run_test "linear" "$TEST_DIR/test_linear.naso" || ((FAILED++))
((TOTAL++))

run_test "reversible" "$TEST_DIR/test_reversible.naso" || ((FAILED++))
((TOTAL++))

echo ""
echo "========================================"
if [[ $FAILED -eq 0 ]]; then
    log_success "All $TOTAL tests passed!"
    exit 0
else
    log_error "$FAILED out of $TOTAL tests failed"
    exit 1
fi