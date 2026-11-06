#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations
extern uint64_t naso_tcp_bind(const uint8_t* addr, size_t len);
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

void* worker_thread(void* arg) {
    int worker_id = *(int*)arg;
    uint64_t out[2];
    uint8_t buf[16];
    
    while (g_running) {
        if (naso_tcp_accept(g_listener_fd, out) != 0) {
            uint64_t client_fd = naso_accept_get_client(out);
            
            // Handle client
            naso_read(client_fd, buf, sizeof(buf));
            naso_write(client_fd, response, response_len);
            naso_close(client_fd);
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
    
    printf("Server listening on %s with %d workers\n", addr_str, workers);
    
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
    
    // Wait for threads (they run until killed)
    for (int i = 0; i < workers; i++) {
        pthread_join(threads[i], NULL);
    }
    
    free(threads);
    free(worker_ids);
    return 0;
}