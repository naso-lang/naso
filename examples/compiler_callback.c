#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations - using the updated FFI with callback support
extern uint64_t naso_tcp_bind_reuseport(const uint8_t* addr, size_t len);
extern uint64_t naso_tcp_accept_async(uint64_t listener_fd, uint64_t* out_ptr, size_t out_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_read_async(uint64_t fd, uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_write_async(uint64_t fd, const uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_close(uint64_t fd);
extern void* naso_runtime_new(uint64_t worker_count);
extern uint64_t naso_runtime_run(void* runtime);

// Callback functions for async/await
void accept_callback_trampoline(void* state_ptr) {
    printf("Accept callback trampoline called with state_ptr: %p\n", state_ptr);
    // In real implementation, this would:
    // 1. Extract captured variables from the await point
    // 2. Update the suspended async task's captured variables
    // 3. Resume the async task on the work-stealing queue
    printf("Accept completed - would resume async task here\n");
}

void read_callback_trampoline(void* state_ptr) {
    printf("Read callback trampoline called with state_ptr: %p\n", state_ptr);
    // In real implementation, this would:
    // 1. Extract captured variables from the read await point
    // 2. Update the suspended async task with read completion
    // 3. Resume the async task on the work-stealing queue
    printf("Read completed - would resume async task here\n");
}

void write_callback_trampoline(void* state_ptr) {
    printf("Write callback trampoline called with state_ptr: %p\n", state_ptr);
    // In real implementation, this would:
    // 1. Extract captured variables from the write await point
    // 2. Update the suspended async task with write completion
    // 3. Resume the async task on the work-stealing queue
    printf("Write completed - would resume async task here\n");
}

int main(int argc, char* argv[]) {
    const char* host = argc > 1 ? argv[1] : "127.0.0.1";
    const char* port = argc > 2 ? argv[2] : "8099";
    
    char addr[64];
    snprintf(addr, sizeof(addr), "%s:%s", host, port);
    size_t addr_len = strlen(addr);
    
    printf("Binding to %s\n", addr);
    uint64_t fd = naso_tcp_bind_reuseport((const uint8_t*)addr, addr_len);
    if (fd == 0) {
        printf("Failed to bind\n");
        return 1;
    }
    printf("Bound to %s (fd=%lu)\n", addr, fd);
    
    void* runtime = naso_runtime_new(4);
    if (!runtime) {
        printf("Failed to create runtime\n");
        return 1;
    }
    printf("Runtime created with 4 workers\n");
    
    // Test async accept with callback trampoline
    // This will call accept_callback_trampoline when accept completes
    uint64_t out[2] = {fd, 0};
    printf("Arming async accept with callback trampoline...\n");
    uint64_t result = naso_tcp_accept_async(fd, out, 16, (const uint8_t*)accept_callback_trampoline, sizeof(void*));
    if (result != 1) {
        printf("Failed to arm async accept\n");
        return 1;
    }
    printf("Async accept armed successfully\n");
    
    // Test async read with callback trampoline
    uint64_t client_fd = 0;
    if (out[1] != 0) {
        client_fd = out[1];
        printf("Client connected: fd=%lu\n", client_fd);
        
        uint8_t read_buf[16];
        printf("Arming async read with callback trampoline...\n");
        result = naso_read_async(client_fd, read_buf, sizeof(read_buf), (const uint8_t*)read_callback_trampoline, sizeof(void*));
        if (result != 1) {
            printf("Failed to arm async read\n");
        }
        
        // Send some data back to test async write
        uint8_t write_buf[] = "HTTP/1.1 200 OK\\r\\nContent-Length: 13\\r\\n\\r\\nHello from async";
        printf("Arming async write with callback trampoline...\n");
        result = naso_write_async(client_fd, write_buf, sizeof(write_buf), (const uint8_t*)write_callback_trampoline, sizeof(void*));
        if (result != 1) {
            printf("Failed to arm async write\n");
        }
        
        printf("Async operations armed - callbacks will be invoked when I/O completes\n");
    }
    
    // Run the runtime (event loop)
    printf("Starting runtime...\n");
    printf("NOTE: In a real implementation, this would run indefinitely until runtime shutdown\n");
    printf("The async callbacks would be invoked here to resume suspended tasks\n");
    printf("This example demonstrates the callback trampoline infrastructure\n");
    
    return 0;
}