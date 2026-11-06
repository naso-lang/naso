#!/bin/bash
# Naso Technical Verification and Benchmark Checks
# Creates timestamped logs in /home/node/naso/logs/

# Get current timestamp for log file
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
LOG_FILE="/home/node/naso/logs/naso_technical_verification_${TIMESTAMP}.log"

echo "=== Starting Naso Technical Verification and Benchmark Checks ==="
echo "Time: $(date)"
echo "Log file: $LOG_FILE"

# Redirect all output to log file and terminal simultaneously
exec 1> >(tee -a "$LOG_FILE")
exec 2>&1

cd /home/node/naso

# Check 1: Verify Naso compiler exists and is working
if [ ! -f "/tmp/cargo-target/debug/naso" ]; then
    echo "❌ ERROR: Naso compiler not found at /tmp/cargo-target/debug/naso"
    exit 1
fi

if [ ! -x "/tmp/cargo-target/debug/naso" ]; then
    echo "❌ ERROR: Naso compiler is not executable"
    exit 1
fi

echo "✅ Naso compiler verified"

# Check 2: Verify FFI library exists
if [ ! -f "/tmp/cargo-target/release/libnaso_ffi.a" ]; then
    echo "❌ ERROR: FFI library not found at /tmp/cargo-target/release/libnaso_ffi.a"
    exit 1
fi

echo "✅ FFI library verified"

# Check 3: Verify entry wrapper exists
if [ ! -f "/tmp/entry.c" ]; then
    echo "❌ ERROR: Entry wrapper not found at /tmp/entry.c"
    exit 1
fi

echo "✅ Entry wrapper verified"

# Check 4: Verify async example compiles and works
if /tmp/cargo-target/debug/naso check examples/techempower_async.nasoc > /tmp/typecheck_async.log 2>&1; then
    echo "✅ techempower_async.nasoc type-check: OK"
else
    echo "❌ techempower_async.nasoc type-check: FAILED"
    cat /tmp/typecheck_async.log
    exit 1
fi

# Build the async server
/tmp/cargo-target/debug/naso build --target llvm examples/techempower_async.nasoc -o techempower_async.ll 2>&1 | head -5

if [ $? -eq 0 ]; then
    echo "✅ techempower_async.nasoc compiled successfully"
    
    # Build native binary
    llc -opaque-pointers -filetype=obj techempower_async.ll -o techempower_async.o 2>&1
    clang -o techempower_async techempower_async.o /tmp/entry.o /tmp/cargo-target/release/libnaso_ffi.a -lpthread -ldl 2>&1
    
    if [ $? -eq 0 ]; then
        echo "✅ techempower_async native binary built"
        
        # Test server functionality
        echo "Starting server on port 8099..."
        nohup ./techempower_async 8099 > /tmp/techempower_async.log 2>&1 &
        SERVER_PID=$!
        
        sleep 3
        
        echo "Testing server..."
        RESPONSE=$(curl -s -D - http://localhost:8099/api/hello 2>/tmp/curl_error.log | head -n 1)
        CURL_EXIT_CODE=$?
        
        if [ $CURL_EXIT_CODE -eq 0 ]; then
            echo "✅ Server test completed successfully - Response: $RESPONSE"
            echo "Curl exit code: $CURL_EXIT_CODE"
            echo "Response length: ${#RESPONSE}"
        else
            echo "❌ Server test failed - Curl exit code: $CURL_EXIT_CODE"
            echo "Curl error output:"
            cat /tmp/curl_error.log
            
            # Check if server is running
            if ps -p $SERVER_PID > /dev/null; then
                echo "Server is still running, testing with simple curl..."
                SIMPLE_RESPONSE=$(curl -s http://localhost:8099/api/hello 2>&1)
                echo "Simple response: $SIMPLE_RESPONSE"
            fi
            
            # Kill the server
            kill $SERVER_PID 2>/dev/null
            wait $SERVER_PID 2>/dev/null
            exit 1
        fi
        
        # Kill the server
        kill $SERVER_PID 2>/dev/null
        wait $SERVER_PID 2>/dev/null
        
    else
        echo "❌ Failed to build native binary"
        cat /tmp/typecheck_async.log
        exit 1
    fi
else
    echo "❌ techempower_async.nasoc build failed"
    cat /tmp/typecheck_async.log
    exit 1
fi

# Check 5: Verify simple example works (baseline)
if /tmp/cargo-target/debug/naso check examples/techempower_simple.nasoc > /tmp/typecheck_simple.log 2>&1; then
    echo "✅ techempower_simple.nasoc type-check: OK"
else
    echo "❌ techempower_simple.nasoc type-check: FAILED"
    cat /tmp/typecheck_simple.log
    exit 1
fi

# Build the simple server
/tmp/cargo-target/debug/naso build --target llvm examples/techempower_simple.nasoc -o techempower_simple.ll 2>&1 | head -5

if [ $? -eq 0 ]; then
    echo "✅ techempower_simple.nasoc compiled successfully"
    
    # Build native binary
    llc -opaque-pointers -filetype=obj techempower_simple.ll -o techempower_simple.o 2>&1
    clang -o techempower_simple techempower_simple.o /tmp/entry.o /tmp/cargo-target/release/libnaso_ffi.a -lpthread -ldl 2>&1
    
    if [ $? -eq 0 ]; then
        echo "✅ techempower_simple native binary built"
        
        # Test simple server functionality
        echo "Starting simple server on port 8080..."
        nohup ./techempower_simple 8080 > /tmp/techempower_simple.log 2>&1 &
        SIMPLE_SERVER_PID=$!
        
        sleep 3
        
        echo "Testing simple server..."
        SIMPLE_RESPONSE=$(curl -s http://localhost:8080/plaintext 2>/tmp/simple_curl_error.log)
        CURL_SIMPLE_EXIT_CODE=$?
        
        if [ $CURL_SIMPLE_EXIT_CODE -eq 0 ]; then
            echo "✅ Simple server test completed successfully - Response: $SIMPLE_RESPONSE"
            echo "Simple curl exit code: $CURL_SIMPLE_EXIT_CODE"
        else
            echo "❌ Simple server test failed - Curl exit code: $CURL_SIMPLE_EXIT_CODE"
            echo "Simple curl error output:"
            cat /tmp/simple_curl_error.log
            
            # Kill the server
            kill $SIMPLE_SERVER_PID 2>/dev/null
            wait $SIMPLE_SERVER_PID 2>/dev/null
            exit 1
        fi
        
        # Kill the simple server
        kill $SIMPLE_SERVER_PID 2>/dev/null
        wait $SIMPLE_SERVER_PID 2>/dev/null
        
    else
        echo "❌ Failed to build techempower_simple native binary"
        exit 1
    fi
else
    echo "❌ techempower_simple.nasoc build failed"
    exit 1
fi

# Cleanup temporary files
rm -f /tmp/typecheck_async.log /tmp/typecheck_simple.log
rm -f techempower_async.ll techempower_simple.ll
rm -f techempower_async.o techempower_simple.o
rm -f techempower_async techempower_simple
rm -f /tmp/techempower_async.log /tmp/techempower_simple.log
rm -f /tmp/curl_error.log /tmp/simple_curl_error.log

echo "=== All Technical Verifications Passed ==="
echo "Time: $(date)"
echo "=== Benchmark checks completed successfully ==="
exit 0