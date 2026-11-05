extern "C" {
    fn naso_write(fd: u64, buf: *u8, len: usize) -> usize;
    fn naso_read(fd: u64, buf: *u8, max_len: usize) -> usize;
    fn naso_tcp_bind(addr: *u8, len: usize) -> u64;
    fn naso_tcp_accept(listener_fd: u64, out_ptr: *u64) -> u64;
    fn naso_accept_get_client(out_ptr: *u64) -> u64;
    fn naso_close(fd: u64) -> u64;
    fn naso_runtime_new(workers: u64) -> *u8;
    fn naso_runtime_run(runtime: *u8) -> u64;
    fn naso_yield() -> u64;
}

struct Request {
    method: Int,
    path_offset: Int,
    path_len: Int,
    headers_offset: Int,
    headers_len: Int,
    body_offset: Int,
    body_len: Int,
}

struct Response {
    status: Int,
    body: *u8,
    body_len: usize,
}

fn ok(body: *u8, len: usize) -> Response {
    Response { status: 200, body: body, body_len: len }
}

fn not_found() -> Response {
    Response { status: 404, body: 0 as *u8, body_len: 0 }
}

fn split_http(buf: [u8]) -> Int {
    let len = buf.len();
    let mut i = 0;
    let mut body_start = -1;
    while i + 3 < len {
        if buf[i] == 13 && buf[i+1] == 10 && buf[i+2] == 13 && buf[i+3] == 10 {
            body_start = i + 4;
            break;
        }
        i = i + 1;
    }
    if body_start == -1 { return -1; }
    
    let mut method_end = 0;
    while method_end < len && buf[method_end] != 32 { method_end = method_end + 1; }
    let mut path_start = method_end + 1;
    let mut path_end = path_start;
    while path_end < len && buf[path_end] != 32 { path_end = path_end + 1; }
    
    let method_len = method_end;
    let path_len = path_end - path_start;
    
    0
}

fn format_http(resp: Response) -> [u8] {
    let mut out = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let status_line = [72, 84, 84, 80, 47, 49, 46, 49, 32];
    
    let mut pos = 0;
    let mut idx = 0;
    while idx < 9 {
        out[pos] = status_line[idx];
        pos = pos + 1;
        idx = idx + 1;
    }
    
    out
}

fn handler_root(req: Request) -> Response {
    let body = [72, 101, 108, 108, 111, 32, 87, 111, 114, 108, 100, 10];
    ok(body as *u8, 12)
}

fn handler_health(req: Request) -> Response {
    let body = [79, 75];
    ok(body as *u8, 2)
}

fn handler_not_found(req: Request) -> Response {
    not_found()
}