use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:8093").await.unwrap();
    println!("Rust Tokio server listening on 127.0.0.1:8093");
    
    loop {
        let (mut stream, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await;
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello World\n";
            let _ = stream.write_all(response).await;
        });
    }
}
