#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations - use async poll API
extern uint64_t naso_tcp_bind(const uint8_t* addr, size_t len);
extern uint64_t naso_poll_add_listener(uint64_t listener_fd, uint64_t* out_ptr);
extern int32_t naso_poll_wait(uint64_t* token_ptr, size_t max_events, int64_t timeout_ms);
extern uint64_t naso_tcp_accept(uint64_t listener_fd, uint64_t* out);
extern uint64_t naso_accept_get_client(const uint64_t* out);
extern uint64_t naso_read(uint64_t fd, const uint8_t* buf, size_t max_len);
extern uint64_t naso_write(uint64_t fd, const uint8_t* buf, size_t len);
extern uint64_t naso_close(uint64_t fd);
extern uint64_t naso_yield(void);

// HTTP response
static const uint8_t response[] = {
    72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10,
    67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32, 49, 50, 13, 10,
    67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 58, 32, 99, 108, 111, 115, 101, 13, 10,
    13, 10,
    72, 101, 108, 108, 111, 32, 87, 111, 114, 108, 100, 10
};
static const size_t response_len = 70;

static uint64_t g_listener_fd = 0;
static int g_running = 1;

// Each worker gets its own out array
typedef struct {
    uint64_t listener_fd;
    uint64_t stream_fd;
} accept_out_t;

void* worker_thread(void* arg) {
    int worker_id = *(int*)arg;
    uint64_t token = 0;
    accept_out_t out = {0};
    uint8_t buf[16];
    
    // Register listener for async accept (one-time per worker)
    // Actually, let's use the global listener with the poll API
    while (g_running) {
        // Poll for events (10ms timeout)
        uint64_t ready_tokens[8];
        int32_t count = naso_poll_wait(ready_tokens, 8, 10);
        
        if (count > 0) {
            for (int i = 0; i < count; i++) {
                // Token is ready - for listener, this means accept
                // We need to call accept manually after poll
                uint64_t accept_out[2];
                if (naso_tcp_accept(g_listener_fd, accept_out) != 0) {
                    uint64_t client_fd = naso_accept_get_client(accept_out);
                    
                    // Handle client
                    naso_read(client_fd, buf, sizeof(buf));
                    naso_write(client_fd, response, response_len);
                    naso_close(client_fd);
                }
            }
        }
        
        naso_yield();
    }
    return NULL;
}

int main(int argc, char** argv) {
    if (argc < 3) {
        fprintf(stderr, "Usage: %s <port> <workers>\n", argv[0]);
        return 1;
    }
    
    int port = atoi(argv[1]);
    int workers = atoi(argv[2]);
    
    if (workers < 1) workers = 1;
    if (workers > 16) workers = 16;
    
    // Bind address
    char addr_str[32];
    snprintf(addr_str, sizeof(addr_str), "127.0.0.1:%d", port);
    
    g_listener_fd = naso_tcp_bind((const uint8_t*)addr_str, strlen(addr_str));
    if (g_listener_fd == 0) {
        fprintf(stderr, "Failed to bind to %s\n", addr_str);
        return 1;
    }
    
    // Register listener with poll
    uint64_t out_ptr[2] = {0};
    naso_poll_add_listener(g_listener_fd, out_ptr);
    
    printf("Server listening on %s with %d workers (poll-based)\n", addr_str, workers);
    
    // Create worker threads
    pthread_t* threads = malloc(workers * sizeof(pthread_t));
    int* worker_ids = malloc(workers * sizeof(int));
    for (int i = 0; i < workers; i++) {
        worker_ids[i] = i;
        if (pthread_create(&threads[i], NULL, worker_thread, &worker_ids[i]) != 0) {
            fprintf(stderr, "Failed to create worker thread %d\n", i);
            workers = i;
            break;
        }
    }
    
    // Wait for threads
    for (int i = 0; i < workers; i++) {
        pthread_join(threads[i], NULL);
    }
    
    free(threads);
    free(worker_ids);
    return 0;
}