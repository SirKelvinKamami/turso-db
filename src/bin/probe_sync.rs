use std::time::{Duration, Instant};

use turso::sync::Builder as SyncBuilder;

const HUB: &str = "http://127.0.0.1:4333";
const TOKEN: &str = "eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9.eyJleHAiOjE5NDY3MjU1Mzh9.gnT8N9j5rcCjpft_9_1pW79vBo6FEUXGtvselszDKkNeH8oQeD1nKZv5pPaBygFuJaZqoNDMiDtuvF5VN1LdCg";

async fn attempt(label: &str, path: &str) {
    let start = Instant::now();
    let res = SyncBuilder::new_remote(path)
        .with_remote_url(HUB)
        .with_auth_token(TOKEN.to_string())
        .with_long_poll_timeout(Duration::from_millis(1500))
        .build()
        .await;
    match res {
        Ok(_) => println!("[{label}] OK ({:?}) file={}", start.elapsed(), path),
        Err(e) => println!("[{label}] ERR after {:?}: {e}", start.elapsed()),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let dir = std::env::temp_dir().join("probe_sync");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let missing = dir.join("missing.db");
    let p_missing = missing.to_str().unwrap().to_string();
    println!("== case 1: missing file, hub up ==");
    attempt("missing", &p_missing).await;

    let empty = dir.join("empty.db");
    std::fs::write(&empty, []).unwrap();
    let p_empty = empty.to_str().unwrap().to_string();
    println!("== case 2: zero-byte file ==");
    attempt("zero", &p_empty).await;

    let valid = dir.join("valid.db");
    std::fs::write(&valid, &[0u8; 0]).unwrap();
    let _ = valid;
    println!("== done; hub log should show requests ==");
    std::thread::sleep(Duration::from_secs(2));
}
