#include <stdio.h>
#include <stdint.h>
#include <string.h>

// FFI declarations matching naso-ffi exports
extern int64_t naso_uring_init();
extern int64_t naso_uring_poll();
extern int64_t naso_yield();
extern int64_t naso_tcp_bind_reuseport(const char* addr, int64_t len);
extern int64_t naso_tcp_accept_async(int64_t listener_fd);
extern int64_t naso_check_read_completion(int64_t fd);
extern int64_t naso_check_write_completion(int64_t fd);
extern int64_t naso_read_async(int64_t fd, char* buf, int64_t len);
extern int64_t naso_write_async(int64_t fd, const char* buf, int64_t len);
extern int64_t naso_timer_sleep_async(int64_t duration_ms, void* callback);

// HTTP parser FFI
extern int64_t naso_parse_http_request(const uint8_t* buf, int64_t buf_len,
    uint8_t* out_method, int64_t out_method_len,
    uint8_t* out_path, int64_t out_path_len,
    uint8_t* out_headers, int64_t out_headers_len);

// HttpResponse struct (matching #[repr(C)] struct in Rust)
struct HttpResponse {
    const uint8_t* ptr;
    int64_t len;
};

extern struct HttpResponse naso_build_response(const uint8_t* method_ptr, int64_t method_len,
    const uint8_t* path_ptr, int64_t path_len,
    const uint8_t* body_ptr, int64_t body_len);

// Timer wheel FFI
extern int64_t naso_timer_wheel_insert(int64_t duration_ms, void* callback);
extern int64_t naso_timer_wheel_tick();
extern int64_t naso_timer_wheel_now_ms();

int main() {
    printf("Testing naso-ffi aarch64 static library via qemu-user...\n");
    
    // Test 1: Initialize uring
    int64_t init_result = naso_uring_init();
    printf("naso_uring_init() = %ld\n", init_result);
    
    // Test 2: Timer wheel functions
    int64_t now = naso_timer_wheel_now_ms();
    printf("naso_timer_wheel_now_ms() = %ld\n", now);
    
    int64_t tick_result = naso_timer_wheel_tick();
    printf("naso_timer_wheel_tick() = %ld\n", tick_result);
    
    // Test 3: HTTP parser
    const char* request = "GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";
    uint8_t method_buf[32] = {0};
    uint8_t path_buf[256] = {0};
    uint8_t headers_buf[256] = {0};
    
    printf("Request: '%s' (len=%ld)\n", request, strlen(request));
    
    int64_t parse_result = naso_parse_http_request(
        (const uint8_t*)request, strlen(request),
        method_buf, sizeof(method_buf),
        path_buf, sizeof(path_buf),
        headers_buf, sizeof(headers_buf)
    );
    printf("naso_parse_http_request() = %ld\n", parse_result);
    printf("  method: '%s'\n", method_buf);
    printf("  path: '%s'\n", path_buf);
    printf("  headers: '%s'\n", headers_buf);
    
    // Print raw buffers
    printf("  raw method bytes: ");
    for (int i = 0; i < 10; i++) printf("%02x ", method_buf[i]);
    printf("\n");
    printf("  raw path bytes: ");
    for (int i = 0; i < 10; i++) printf("%02x ", path_buf[i]);
    printf("\n");
    
    // Test 3b: HTTP parser with body
    const char* request2 = "POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
    uint8_t method_buf2[32] = {0};
    uint8_t path_buf2[256] = {0};
    uint8_t headers_buf2[256] = {0};
    
    int64_t parse_result2 = naso_parse_http_request(
        (const uint8_t*)request2, strlen(request2),
        method_buf2, sizeof(method_buf2),
        path_buf2, sizeof(path_buf2),
        headers_buf2, sizeof(headers_buf2)
    );
    printf("naso_parse_http_request() with body = %ld\n", parse_result2);
    printf("  method: '%s'\n", method_buf2);
    printf("  path: '%s'\n", path_buf2);
    printf("  headers: '%s'\n", headers_buf2);
    
    // Test 4: Build response
    struct HttpResponse resp = naso_build_response(
        (uint8_t*)"GET\0", 4,
        (uint8_t*)"/health\0", 8,
        NULL, 0
    );
    printf("naso_build_response() = ptr=%p, len=%ld\n", resp.ptr, resp.len);
    if (resp.ptr && resp.len > 0) {
        printf("  Response preview: %.100s\n", resp.ptr);
    }
    
    // Test 5: TCP bind (should fail without root but returns error code)
    int64_t bind_result = naso_tcp_bind_reuseport("127.0.0.1:8080", 14);
    printf("naso_tcp_bind_reuseport() = %ld (expected negative for non-root)\n", bind_result);
    
    // Test 6: Poll loop iteration
    for (int i = 0; i < 3; i++) {
        int64_t poll_result = naso_uring_poll();
        printf("naso_uring_poll() iteration %d = %ld\n", i+1, poll_result);
    }
    
    printf("\nAll FFI calls completed successfully!\n");
    return 0;
}