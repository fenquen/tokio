//!     cargo run --example echo_server
//!
//!     cargo run --example echo_client 127.0.0.1:8080

#![warn(rust_2018_idioms)]
#![allow(non_snake_case)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use std::env;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let tcpListener = TcpListener::bind(&addr).await?;

    loop {
        // Asynchronously wait for an inbound socket.
        let (mut tcpStream, _) = tcpListener.accept().await?;

        tokio::spawn(async move {
            let mut buf = vec![0; 1024];

            loop {
                let n = tcpStream.read(&mut buf).await.expect("failed to read data from socket");
                if n == 0 {
                    return;
                }

                tcpStream.write_all(&buf[0..n]).await.expect("failed to write data to socket");
            }
        });
    }
}
