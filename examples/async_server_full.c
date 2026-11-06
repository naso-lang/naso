#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations
extern uint64_t naso_tcp_bind(const uint8_t* addr, size_t len);
extern uint64_t naso_tcp_bind_reuseport(const uint8_t* addr, size_t len);
extern uint64_t naso_tcp_accept(uint64_t listener_fd, uint64_t* out_ptr);
extern uint64_t naso_accept_get_client(const uint64_t* out_ptr);
extern uint64_t naso_read(uint64_t fd, uint8_t* buf, size_t max_len);
extern uint64_t naso_write(uint64_t fd, const uint8_t* buf, size_t len);
extern uint64_t naso_close(uint64_t fd);
extern void* naso_runtime_new(uint64_t worker_count);
extern uint64_t naso_runtime_run(void* runtime);
extern uint64_t naso_yield();
extern uint64_t naso_tcp_accept_async(uint64_t listener_fd, uint64_t* out_ptr, size_t out_len);
extern uint64_t naso_read_async(uint64_t fd, uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_write_async(uint64_t fd, const uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern void* naso_future_new();
extern uint64_t naso_future_complete(void* future, const uint8_t* data, size_t len);
extern uint64_t naso_future_get_result(void* future, uint8_t* out, size_t len);
extern void naso_spawn(void* runtime, void* task_fn, size_t task_len);
extern uint64_t naso_await(void* future);
extern uint64_t naso_spawn_future(void* future);

typedef struct {
    uint64_t client_fd;
} client_context_t;

static const uint8_t response[] = {
    72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10,
    67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32, 49, 50, 13, 10,
    67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 58, 32, 99, 108, 111, 115, 101, 13, 10,
    13, 10,
    72, 101, 108, 108, 111, 32, 87, 111, 114, 108, 100, 10
};

static void handle_client_read_complete(void* arg) {
    client_context_t* ctx = (client_context_t*)arg;
    printf("Read complete for fd=%lu\n", ctx->client_fd);
    
    // Write response
    uint64_t written = naso_write(ctx->client_fd, response, sizeof(response));
    printf("Written %lu bytes\n", written);
    
    // Close
    naso_close(ctx->client_fd);
    free(ctx);
}

static void accept_complete(void* arg) {
    uint64_t* out_ptr = (uint64_t*)arg;
    uint64_t listener_fd = out_ptr[0];
    uint64_t client_fd = out_ptr[1];
    printf("Accepted connection: listener=%lu, client=%lu\n", listener_fd, client_fd);
    
    // Register for read
    client_context_t* ctx = malloc(sizeof(client_context_t));
    ctx->client_fd = client_fd;
    uint8_t buf[16];
    naso_read_async(client_fd, buf, sizeof(buf), (const uint8_t*)handle_client_read_complete, sizeof(void*));
    
    // Re-arm accept
    naso_tcp_accept_async(listener_fd, out_ptr, 16);
}

int main(int argc, char* argv[]) {
    const char* host = argc > 1 ? argv[1] : "127.0.0.1";
    const char* port = argc > 2 ? argv[2] : "8099";
    
    char addr[64];
    snprintf(addr, sizeof(addr), "%s:%s", host, port);
    size_t addr_len = strlen(addr);
    
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
    printf("Runtime created\n");
    
    // Start async accept
    uint64_t out[2] = {fd, 0};
    naso_tcp_accept_async(fd, out, 16);
    printf("Accept armed\n");
    
    // Run runtime (event loop)
    naso_runtime_run(runtime);
    
    return 0;
}