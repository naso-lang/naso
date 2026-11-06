extern "C" {
    fn naso_write(fd: u64, buf: *u8, len: usize) -> usize;
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
    let status_str = if resp.status == 200 { "200 OK\r\n" } else { "404 Not Found\r\n" };
    let mut out = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let status_line = [72, 84, 84, 80, 47, 49, 46, 49, 32];
    let clen = resp.body_len;
    
    let mut pos = 0;
    for b in status_line { 
        out[pos] = b; 
        pos = pos + 1; 
    }
    for b in status_str.as_bytes() { 
        out[pos] = b; 
        pos = pos + 1; 
    }
    let cl_header = [67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32];
    for b in cl_header { 
        out[pos] = b; 
        pos = pos + 1; 
    }
    
    out
}

fn main() -> () {
    let r = ok(0 as *u8, 0);
    ()
}