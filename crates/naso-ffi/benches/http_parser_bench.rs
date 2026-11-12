// Benchmarks for naso-ffi HTTP parser and async I/O

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use naso_ffi::http_parser::{naso_parse_http_request, naso_build_response};
use naso_ffi::HttpResponse;
use std::ptr;

fn bench_http_parser(c: &mut Criterion) {
    let request = b"GET /api/health HTTP/1.1\r\nHost: localhost:8080\r\nUser-Agent: test\r\nAccept: */*\r\n\r\n";
    let mut method_buf = [0u8; 32];
    let mut path_buf = [0u8; 256];
    let mut headers_buf = [0u8; 256];

    c.bench_function("parse_http_request", |b| {
        b.iter(|| {
            naso_parse_http_request(
                request.as_ptr(),
                request.len(),
                method_buf.as_mut_ptr(),
                method_buf.len(),
                path_buf.as_mut_ptr(),
                path_buf.len(),
                headers_buf.as_mut_ptr(),
                headers_buf.len(),
            )
        })
    });
}

fn bench_build_response(c: &mut Criterion) {
    let method = b"GET\0";
    let path = b"/health\0";

    c.bench_function("build_response", |b| {
        b.iter(|| {
            naso_build_response(
                method.as_ptr(),
                method.len(),
                path.as_ptr(),
                path.len(),
                ptr::null(),
                0,
            )
        })
    });
}

fn bench_parse_and_build(c: &mut Criterion) {
    let request = b"GET /echo HTTP/1.1\r\nHost: localhost:8080\r\nContent-Length: 5\r\n\r\nhello";
    let mut method_buf = [0u8; 32];
    let mut path_buf = [0u8; 256];
    let mut headers_buf = [0u8; 256];

    c.bench_function("parse_and_build", |b| {
        b.iter(|| {
            let parsed = naso_parse_http_request(
                request.as_ptr(),
                request.len(),
                method_buf.as_mut_ptr(),
                method_buf.len(),
                path_buf.as_mut_ptr(),
                path_buf.len(),
                headers_buf.as_mut_ptr(),
                headers_buf.len(),
            );
            
            if parsed > 0 {
                naso_build_response(
                    method_buf.as_ptr(),
                    method_buf.len(),
                    path_buf.as_ptr(),
                    path_buf.len(),
                    ptr::null(),
                    0,
                )
            } else {
                HttpResponse { ptr: ptr::null(), len: 0 }
            }
        })
    });
}

criterion_group!(benches, bench_http_parser, bench_build_response, bench_parse_and_build);
criterion_main!(benches);