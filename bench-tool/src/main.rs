use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = Arc::new(if args.len() > 1 { args[1].clone() } else { "http://127.0.0.1:8080/".to_string() });
    let concurrency = 50;
    let total = 1000;

    println!("  预热中...");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let _ = client.get(target.as_str()).send();

    let completed = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let latencies = Arc::new(std::sync::Mutex::new(Vec::new()));

    println!("  开始压测... (并发: {}, 总请求: {})", concurrency, total);
    let start = Instant::now();

    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let c = completed.clone();
        let e = errors.clone();
        let l = latencies.clone();
        let t = target.clone();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        handles.push(thread::spawn(move || {
            loop {
                let idx = c.fetch_add(1, Ordering::SeqCst);
                if idx >= total { break; }

                let t0 = Instant::now();
                match client.get(t.as_str()).send() {
                    Ok(resp) => {
                        let _ = resp.bytes();
                        let dur = t0.elapsed().as_millis() as u64;
                        l.lock().unwrap().push(dur);
                    }
                    Err(_) => {
                        e.fetch_add(1, Ordering::SeqCst);
                    }
                }

                if (idx + 1) % 100 == 0 {
                    print!("\r  进度: {}/{}", idx + 1, total);
                    use std::io::{Write, stdout};
                    stdout().flush().unwrap();
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let elapsed = start.elapsed();
    let completed_count = completed.load(Ordering::SeqCst);
    let error_count = errors.load(Ordering::SeqCst);

    let mut lats = latencies.lock().unwrap();
    lats.sort();
    let len = lats.len();
    let avg = if len > 0 { lats.iter().sum::<u64>() as f64 / len as f64 } else { 0.0 };
    let min = lats.first().copied().unwrap_or(0);
    let max = lats.last().copied().unwrap_or(0);
    let p50 = lats.get(len / 2).copied().unwrap_or(0);
    let p90 = lats.get((len as f64 * 0.9) as usize).copied().unwrap_or(0);
    let p99 = lats.get((len as f64 * 0.99) as usize).copied().unwrap_or(0);
    let rps = total as f64 / elapsed.as_secs_f64();

    println!("\n");
    println!("{}", "═".repeat(50));
    println!("  并发压测报告");
    println!("{}", "═".repeat(50));
    println!("  目标地址:     {}", target);
    println!("  并发数:       {}", concurrency);
    println!("  总请求数:     {}", total);
    println!("  总耗时:       {:.2}s", elapsed.as_secs_f64());
    println!("  请求/秒:      {:.2} req/s", rps);
    println!("  成功:         {}", completed_count - error_count);
    println!("  失败:         {}", error_count);
    println!("{}", "─".repeat(50));
    println!("  最小延迟:     {}ms", min);
    println!("  平均延迟:     {:.2}ms", avg);
    println!("  最大延迟:     {}ms", max);
    println!("  P50 (中位):   {}ms", p50);
    println!("  P90:          {}ms", p90);
    println!("  P99:          {}ms", p99);
    println!("{}", "═".repeat(50));
}
