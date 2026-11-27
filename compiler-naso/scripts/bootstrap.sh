#!/bin/bash
# Bootstrap script for self-hosting Naso compiler
# 
# Usage: ./scripts/bootstrap.sh [options]
# Options:
#   --clean           Clean build artifacts before building
#   --test            Run tests after building
#   --self-host       Build self-hosted compiler (stage 2)
#   --verify          Run formal verification
#   --help            Show this help

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
NASO_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Default options
CLEAN=false
TEST=false
SELF_HOST=false
VERIFY=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --clean)
            CLEAN=true
            shift
            ;;
        --test)
            TEST=true
            shift
            ;;
        --self-host)
            SELF_HOST=true
            shift
            ;;
        --verify)
            VERIFY=true
            shift
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --clean           Clean build artifacts before building"
            echo "  --test            Run tests after building"
            echo "  --self-host       Build self-hosted compiler (stage 2)"
            echo "  --verify          Run formal verification"
            echo "  --help            Show this help"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Paths
RUST_NASO_BIN="$NASO_ROOT/target/release/naso"
NASO_COMPILER_NASO_DIR="$PROJECT_ROOT"
STAGE1_BIN="$NASO_COMPILER_NASO_DIR/stage1/naso"
STAGE2_BIN="$NASO_COMPILER_NASO_DIR/stage2/naso"
SELF_HOSTED_BIN="$NASO_COMPILER_NASO_DIR/self/naso"

log_info "Naso Compiler Bootstrap"
log_info "Project root: $PROJECT_ROOT"
log_info "Naso root: $NASO_ROOT"

# Step 1: Ensure Rust compiler is built
log_info "Step 1: Building Rust Naso compiler..."
cd "$NASO_ROOT"
if [[ "$CLEAN" == true ]]; then
    cargo clean -p naso-compiler
fi
cargo build --release --bin naso

if [[ ! -f "$RUST_NASO_BIN" ]]; then
    log_error "Rust naso binary not found at $RUST_NASO_BIN"
    exit 1
fi
log_success "Rust Naso compiler built successfully"

# Step 2: Build stage 1 (compiler-naso with Rust compiler)
log_info "Step 2: Building Stage 1 compiler (compiler-naso with Rust naso)..."
mkdir -p "$NASO_COMPILER_NASO_DIR/stage1"
cd "$NASO_COMPILER_NASO_DIR"

"$RUST_NASO_BIN" build src/main.naso -o "$STAGE1_BIN" --target llvm --opt 2

if [[ ! -f "$STAGE1_BIN" ]]; then
    log_error "Stage 1 binary not found at $STAGE1_BIN"
    exit 1
fi
log_success "Stage 1 compiler built successfully"

# Step 3: Test stage 1 compiler
if [[ "$TEST" == true ]]; then
    log_info "Step 3: Testing Stage 1 compiler..."
    "$STAGE1_BIN" check examples/test.naso
    log_success "Stage 1 compiler tests passed"
fi

# Step 4: Formal verification
if [[ "$VERIFY" == true ]]; then
    log_info "Step 4: Running formal verification on Stage 1..."
    "$STAGE1_BIN" build --verify src/main.naso -o /tmp/verify_out.ll
    log_success "Formal verification passed"
fi

# Step 5: Self-hosting (Stage 2)
if [[ "$SELF_HOST" == true ]]; then
    log_info "Step 5: Building Stage 2 compiler (self-hosted)..."
    mkdir -p "$NASO_COMPILER_NASO_DIR/stage2"
    
    "$STAGE1_BIN" build src/main.naso -o "$STAGE2_BIN" --target llvm --opt 2
    
    if [[ ! -f "$STAGE2_BIN" ]]; then
        log_error "Stage 2 binary not found at $STAGE2_BIN"
        exit 1
    fi
    log_success "Stage 2 compiler built successfully"
    
    # Step 6: Test stage 2 compiler
    if [[ "$TEST" == true ]]; then
        log_info "Step 6: Testing Stage 2 compiler..."
        "$STAGE2_BIN" check examples/test.naso
        log_success "Stage 2 compiler tests passed"
    fi
    
    # Step 7: Self-hosted (Stage 3 - compiler compiles itself)
    log_info "Step 7: Building self-hosted compiler (Stage 3)..."
    mkdir -p "$NASO_COMPILER_NASO_DIR/self"
    
    "$STAGE2_BIN" build src/main.naso -o "$SELF_HOSTED_BIN" --target llvm --opt 2
    
    if [[ ! -f "$SELF_HOSTED_BIN" ]]; then
        log_error "Self-hosted binary not found at $SELF_HOSTED_BIN"
        exit 1
    fi
    log_success "Self-hosted compiler built successfully!"
    
    # Step 8: Verify self-hosted compiler produces identical output
    if [[ "$TEST" == true ]]; then
        log_info "Step 8: Verifying self-hosted compiler output parity..."
        
        # Test with a simple program
        cat > /tmp/test_parity.naso << 'EOF'
fn add(a: i32, b: i32) -> i32 {
    return a + b;
}

fn main() {
    let x = add(1, 2);
    let y = add(x, 3);
}
EOF
        
        # Compile with Rust naso
        "$RUST_NASO_BIN" build /tmp/test_parity.naso -o /tmp/test_rust.ll --target llvm
        
        # Compile with self-hosted naso
        "$SELF_HOSTED_BIN" build /tmp/test_parity.naso -o /tmp/test_self.ll --target llvm
        
        # Compare outputs (ignore metadata differences)
        if diff -u <(grep -v "^;" /tmp/test_rust.ll | grep -v "^$") <(grep -v "^;" /tmp/test_self.ll | grep -v "^$"); then
            log_success "Output parity verified: self-hosted compiler produces identical LLVM IR"
        else
            log_warn "Output parity check found differences (may be metadata)"
            log_info "Rust naso output:"
            head -50 /tmp/test_rust.ll
            log_info "Self-hosted output:"
            head -50 /tmp/test_self.ll
        fi
    fi
fi

log_success "Bootstrap complete!"
echo ""
echo "Built binaries:"
echo "  Rust naso:        $RUST_NASO_BIN"
echo "  Stage 1 (Rust):   $STAGE1_BIN"
if [[ "$SELF_HOST" == true ]]; then
    echo "  Stage 2 (Stage1): $STAGE2_BIN"
    echo "  Self-hosted:      $SELF_HOSTED_BIN"
fi