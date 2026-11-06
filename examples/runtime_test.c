#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations
extern uint64_t naso_tcp_bind_reuseport(const uint8_t* addr, size_t addr_len);
extern void* naso_runtime_new(size_t worker_count);
extern void* naso_spawn(void* rt, const uint8_t* task_fn, size_t task_len);
extern uint64_t naso_runtime_run(void* rt);
extern uint64_t naso_runtime_shutdown(void* rt);
extern void naso_runtime_free(void* rt);

// Simple task that runs the accept loop
void accept_task(void* arg) {
    // This would be the async accept loop
    // For now, just print
    printf("Accept task running\n");
}

int main(int argc, char* argv[]) {
    const char* host = argc > 1 ? argv[1] : "127.0.0.1";
    const char* port = argc > 2 ? argv[2] : "8099";
    
    char addr[64];
    snprintf(addr, sizeof(addr), "%s:%s", host, port);
    size_t addr_len = strlen(addr);
    
    printf("Binding to %s\n", addr);
    uint64_t listener_fd = naso_tcp_bind_reuseport((const uint8_t*)addr, addr_len);
    if (listener_fd == 0) {
        fprintf(stderr, "Failed to bind\n");
        return 1;
    }
    printf("Listener fd: %lu\n", listener_fd);
    
    // Create runtime with 4 workers
    void* rt = naso_runtime_new(4);
    if (rt == NULL) {
        fprintf(stderr, "Failed to create runtime\n");
        return 1;
    }
    printf("Runtime created\n");
    
    // Spawn accept task
    // In real usage, this would be a compiled async function
    void* task = naso_spawn(rt, (const uint8_t*)(void*)accept_task, 0);
    printf("Task spawned: %p\n", task);
    
    // Run runtime (this blocks)
    printf("Running runtime...\n");
    naso_runtime_run(rt);
    
    naso_runtime_shutdown(rt);
    naso_runtime_free(rt);
    
    return 0;
}