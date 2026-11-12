use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello World\n";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8098").await?;
    println!("Tokio server listening on 127.0.0.1:8098");
    
    loop {
        let (mut socket, _) = listener.accept().await?;
        
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(RESPONSE).await;
            let _ = socket.shutdown().await;
        });
    }
}