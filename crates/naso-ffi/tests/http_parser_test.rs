#[test]
fn test_http_parser_integration() {
    use naso_ffi::http_parser::{naso_parse_http_request, naso_build_response};
    use naso_ffi::HttpResponse;
    use std::ptr;
    
    // Test parsing GET request
    let request = b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let mut method_buf = [0u8; 32];
    let mut path_buf = [0u8; 256];
    let mut headers_buf = [0u8; 256];
    
    let parsed = unsafe {
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
    };
    
    println!("Parsed: {}", parsed);
    println!("Method: '{}'", std::str::from_utf8(&method_buf).unwrap_or("invalid"));
    println!("Path: '{}'", std::str::from_utf8(&path_buf).unwrap_or("invalid"));
    println!("Headers: '{}'", std::str::from_utf8(&headers_buf).unwrap_or("invalid"));
    
    assert!(parsed > 0);
    assert_eq!(std::str::from_utf8(&method_buf).unwrap().trim_end_matches('\0'), "GET");
    assert_eq!(std::str::from_utf8(&path_buf).unwrap().trim_end_matches('\0'), "/health");
    
    // Test building response
    let method = b"GET\0";
    let path = b"/health\0";
    let resp: HttpResponse = unsafe {
        naso_build_response(
            method.as_ptr(),
            method.len(),
            path.as_ptr(),
            path.len(),
            ptr::null(),
            0,
        )
    };
    
    println!("Response ptr: {:p}, len: {}", resp.ptr, resp.len);
    assert!(resp.len > 0);
    assert!(!resp.ptr.is_null());
}