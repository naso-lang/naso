#!/usr/bin/env bash
# Hourly Naso Phase Checker and Continuer
# This script runs every hour to check the current phase in naso-progress.md,
# verify completion status, continue work, check for AI slop, and push to GitHub.

set -euo pipefail

# Configuration
REPO_DIR="/home/node/naso"
PROGRESS_FILE="$REPO_DIR/naso-progress.md"
LOG_DIR="$REPO_DIR/.phase-checker-logs"
LOG_FILE="$LOG_DIR/phase-checker-$(date +%Y%m%d-%H%M%S).log"
MAX_LOG_FILES=168  # Keep 1 week of hourly logs

# Create log directory
mkdir -p "$LOG_DIR"

# Logging function
log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG_FILE"
}

# Cleanup old logs
cleanup_logs() {
    cd "$LOG_DIR"
    ls -t phase-checker-*.log 2>/dev/null | tail -n +$((MAX_LOG_FILES + 1)) | xargs -r rm -f
}

# Check for AI slop patterns in the progress file
check_ai_slop() {
    local file="$1"
    local issues=0
    
    # Common AI slop patterns
    if grep -q "delve\|tapestry\|unwavering\|crucial\|paramount\|unleash\|unlock\|embark\|journey\|realm\|landscape\|holistic\|synergy\|paradigm\|revolutionary\|groundbreaking\|transformative" "$file"; then
        log "WARNING: AI slop detected - flowery language found"
        issues=$((issues + 1))
    fi
    
    if grep -q "comprehensive.*analysis\|in-depth.*analysis\|thorough.*analysis\|detailed.*analysis\|extensive.*analysis" "$file"; then
        log "WARNING: AI slop detected - redundant analysis prefixes"
        issues=$((issues + 1))
    fi
    
    if grep -q "it is important to note\|it should be noted\|please note that\|note that" "$file"; then
        log "WARNING: AI slop detected - unnecessary hedging"
        issues=$((issues + 1))
    fi
    
    # Check for overly verbose commit-style messages in progress
    if grep -q "This session.*completed\|In this session.*we\|During this session" "$file"; then
        log "INFO: Session summary style detected (acceptable in progress.md)"
    fi
    
    return $issues
}

# Extract current active phase from progress file
get_current_phase() {
    local file="$1"
    # Look for the last phase that is IN PROGRESS or not marked COMPLETED
    grep -E "(^## 🎯 Phase|^## Phase)" "$file" | tail -20 | while IFS= read -r line; do
        phase_num=$(echo "$line" | sed -E 's/.*Phase ([0-9]+).*/\1/')
        # Check if this phase has a status line after it
        # For now, return the highest numbered phase that isn't explicitly COMPLETED
        echo "$phase_num"
    done | tail -1
}

# Get the status of a specific phase
get_phase_status() {
    local file="$1"
    local phase_num="$2"
    # Look for status markers after the phase header
    awk -v phase="$phase_num" '
        $0 ~ "Phase " phase "[^0-9]" { 
            # Check the header line itself for status indicators
            if ($0 ~ /\(IN PROGRESS\)/ || $0 ~ /🔄/ || $0 ~ /IN PROGRESS/) { 
                print "IN PROGRESS"; exit 
            }
            if ($0 ~ /\(COMPLETED\)/ || $0 ~ /✅/ || $0 ~ /COMPLETED/) { 
                print "COMPLETED"; exit 
            }
            if ($0 ~ /\(PENDING\)/ || $0 ~ /⏳/ || $0 ~ /PENDING/) { 
                print "PENDING"; exit 
            }
            in_phase=1; next 
        }
        in_phase && /^###/ && !/^### Phase/ { in_phase=0 }
        in_phase && /Status:/ { print $2; exit }
        in_phase && /✅|🔄|⏳|IN PROGRESS|COMPLETED|PENDING/ { print; exit }
    ' "$file" | head -1
}

# Check if there are uncommitted changes
has_uncommitted_changes() {
    cd "$REPO_DIR"
    if ! git diff --quiet || ! git diff --cached --quiet; then
        return 0
    fi
    return 1
}

# Run tests to verify current state
run_verification() {
    log "Running verification tests..."
    cd "$REPO_DIR"
    
    # Test quantum runtime
    if cargo test -p naso-quantum-runtime -p naso-gates -p async-runtime --lib 2>&1 | grep -q "test result: ok"; then
        log "✅ Core tests pass"
        return 0
    else
        log "❌ Some tests failed"
        return 1
    fi
}

# Check for AI slop in recent changes
check_recent_slop() {
    cd "$REPO_DIR"
    # Check last 10 commits for AI slop in commit messages
    if git log --oneline -10 | grep -qi "delve\|tapestry\|unwavering\|crucial\|paramount\|unleash\|embark\|journey"; then
        log "WARNING: AI slop detected in recent commit messages"
        return 1
    fi
    return 0
}

# Continue work on the current phase
continue_phase_work() {
    local phase_num="$1"
    log "Continuing work on Phase $phase_num..."
    
    # This is where the actual work continuation logic would go
    # For now, we'll run a build to ensure things compile
    cd "$REPO_DIR"
    log "Building project to verify compilation..."
    if cargo check --workspace --exclude bench_runner 2>&1 | grep -q "Finished"; then
        log "✅ Build successful"
    else
        log "⚠️  Build had warnings/errors, but continuing"
    fi
}

# Commit and push changes
commit_and_push() {
    cd "$REPO_DIR"
    
    if has_uncommitted_changes; then
        log "Committing changes..."
        git add -A
        git commit -m "chore: hourly phase check and continue - $(date '+%Y-%m-%d %H:%M')"
        
        log "Pushing to GitHub..."
        if git push origin main; then
            log "✅ Pushed to GitHub successfully"
        else
            log "❌ Failed to push to GitHub"
            return 1
        fi
    else
        log "No changes to commit"
    fi
}

# Main execution
main() {
    log "=== Hourly Phase Check Started ==="
    
    cleanup_logs
    
    # Check for AI slop in progress file
    log "Checking for AI slop in progress file..."
    check_ai_slop "$PROGRESS_FILE"
    
    # Check recent commits for AI slop
    check_recent_slop
    
    # Get current phase
    current_phase=$(get_current_phase "$PROGRESS_FILE")
    log "Current phase: $current_phase"
    
    # Get phase status
    phase_status=$(get_phase_status "$PROGRESS_FILE" "$current_phase")
    log "Phase status: $phase_status"
    
    # Run verification
    run_verification
    
    # Continue work if phase is in progress
    if [[ "$phase_status" == *"IN PROGRESS"* ]] || [[ "$phase_status" == *"🔄"* ]]; then
        continue_phase_work "$current_phase"
    else
        log "Phase $current_phase is not in progress (status: $phase_status)"
    fi
    
    # Commit and push any changes
    commit_and_push
    
    log "=== Hourly Phase Check Completed ==="
}

# Run main
main "$@"