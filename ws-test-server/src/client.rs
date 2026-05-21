use std::env;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use url::Url;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: {} <websocket_url> [消息]", args[0]);
        std::process::exit(1);
    }

    let ws_url = &args[1];
    let test_msg = if args.len() > 2 {
        args[2].clone()
    } else {
        "Hello, WebSocket!".to_string()
    };

    println!("连接: {}", ws_url);
    let url = Url::parse(ws_url).expect("URL 格式错误");

    match connect_async(url).await {
        Ok((ws_stream, resp)) => {
            println!("WebSocket 握手成功!");
            println!("响应状态: {}", resp.status());
            println!("响应头:");
            for (name, value) in resp.headers() {
                println!("  {}: {:?}", name, value);
            }

            let (mut write, mut read) = ws_stream.split();

            // 发送消息
            println!("\n发送: {}", test_msg);
            write.send(tokio_tungstenite::tungstenite::Message::Text(test_msg.clone())).await.expect("发送失败");

            // 接收回显
            if let Some(msg) = read.next().await {
                match msg {
                    Ok(received) => {
                        let received_text = received.to_text().unwrap_or("").to_string();
                        println!("收到: {}", received_text);

                        if received_text == test_msg {
                            println!("\n✅ 测试通过! 回显消息匹配");
                        } else {
                            println!("\n❌ 测试失败! 期望: '{}', 收到: '{}'", test_msg, received_text);
                        }
                    }
                    Err(e) => {
                        eprintln!("\n❌ 接收错误: {}", e);
                    }
                }
            }

            // 关闭连接
            write.close().await.ok();
        }
        Err(e) => {
            eprintln!("❌ WebSocket 连接失败: {}", e);
            std::process::exit(1);
        }
    }
}
