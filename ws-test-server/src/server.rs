use std::env;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let port = if args.len() > 1 { &args[1] } else { "9001" };
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await.expect("绑定端口失败");
    println!("WebSocket echo server listening on ws://{}", addr);

    while let Ok((stream, peer)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, peer));
    }
}

async fn handle_connection(stream: tokio::net::TcpStream, peer: std::net::SocketAddr) {
    println!("新连接: {}", peer);
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("WebSocket 握手失败 ({}): {}", peer, e);
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();
    while let Some(msg) = read.next().await {
        match msg {
            Ok(msg) => {
                if msg.is_text() || msg.is_binary() {
                    println!("回显消息 ({}): {:?}", peer, msg);
                    if let Err(e) = write.send(msg).await {
                        eprintln!("发送失败 ({}): {}", peer, e);
                        break;
                    }
                } else if msg.is_close() {
                    println!("连接关闭: {}", peer);
                    break;
                }
            }
            Err(e) => {
                eprintln!("接收错误 ({}): {}", peer, e);
                break;
            }
        }
    }
}
