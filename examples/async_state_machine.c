#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
#include <string.h>
#include <stdint.h>

// FFI declarations
extern uint64_t naso_tcp_bind_reuseport(const uint8_t* addr, size_t len);
extern uint64_t naso_tcp_accept_async(uint64_t listener_fd, uint64_t* out_ptr, size_t out_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_read_async(uint64_t fd, uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_write_async(uint64_t fd, const uint8_t* buf_ptr, size_t buf_len, const uint8_t* callback, size_t callback_len);
extern uint64_t naso_close(uint64_t fd);
extern void* naso_runtime_new(uint64_t worker_count);
extern uint64_t naso_runtime_run(void* runtime);
extern void* naso_future_new();
extern uint64_t naso_future_complete(void* future, const uint8_t* data, size_t len);
extern uint64_t naso_future_get_result(void* future, uint8_t* out, size_t len);
extern uint64_t naso_spawn_future(void* future);

typedef struct {
    uint64_t client_fd;
    uint64_t state;  // 0=start, 1=reading, 2=writing, 3=done
    uint8_t read_buf[16];
    const uint8_t* response;
    size_t response_len;
    size_t response_offset;
} client_state_t;

static const uint8_t HTTP_RESPONSE[] = {
    72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13, 10,
    67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32, 49, 50, 13, 10,
    67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 58, 32, 99, 108, 111, 115, 101, 13, 10,
    13, 10,
    72, 101, 108, 108, 111, 32, 87, 111, 114, 108, 100, 10
};

#define RESPONSE_SIZE (sizeof(HTTP_RESPONSE))

// Forward declaration
void client_read_complete(void* arg);
void client_write_complete(void* arg);

// State machine for client handling
void client_state_machine(client_state_t* state) {
    switch (state->state) {
        case 0: { // Start - initiate read
            state->state = 1;
            state->response = HTTP_RESPONSE;
            state->response_len = RESPONSE_SIZE;
            state->response_offset = 0;
            naso_read_async(state->client_fd, state->read_buf, sizeof(state->read_buf), 
                           (const uint8_t*)client_read_complete, sizeof(void*));
            break;
        }
        case 1: { // Read complete - initiate write
            state->state = 2;
            naso_write_async(state->client_fd, 
                           state->response + state->response_offset, 
                           state->response_len - state->response_offset,
                           (const uint8_t*)client_write_complete, sizeof(void*));
            break;
        }
        case 2: { // Write complete - close
            state->state = 3;
            naso_close(state->client_fd);
            free(state);
            break;
        }
        default:
            break;
    }
}

void client_read_complete(void* arg) {
    client_state_t* state = (client_state_t*)arg;
    printf("Read complete for fd=%lu\n", state->client_fd);
    client_state_machine(state);
}

void client_write_complete(void* arg) {
    client_state_t* state = (client_state_t*)arg;
    printf("Write complete for fd=%lu\n", state->client_fd);
    client_state_machine(state);
}

// Accept state machine
void accept_complete(void* arg) {
    uint64_t* out_ptr = (uint64_t*)arg;
    uint64_t listener_fd = out_ptr[0];
    uint64_t client_fd = out_ptr[1];
    printf("Accepted connection: listener=%lu, client=%lu\n", listener_fd, client_fd);
    
    // Create client state
    client_state_t* state = malloc(sizeof(client_state_t));
    state->client_fd = client_fd;
    state->state = 0;
    
    // Start client state machine
    client_state_machine(state);
    
    // Re-arm accept
    naso_tcp_accept_async(listener_fd, out_ptr, 16, (const uint8_t*)accept_complete, sizeof(void*));
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
    printf("Runtime created with 4 workers\n");
    
    // Start async accept
    uint64_t out[2] = {fd, 0};
    naso_tcp_accept_async(fd, out, 16, (const uint8_t*)accept_complete, sizeof(void*));
    printf("Accept armed\n");
    
    // Run runtime (event loop)
    printf("Starting runtime...\n");
    naso_runtime_run(runtime);
    
    return 0;
}