use std::process::Stdio;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

async fn read_response(stream: &mut tokio::net::TcpStream) -> (u16, Vec<(String, String)>, String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(pos) = text.find("\r\n\r\n") {
            if buf.len() >= pos + 4 + 5 {
                break;
            }
        }
    }
    let mut text = String::from_utf8_lossy(&buf).to_string();
    while text.ends_with('\0') {
        text.pop();
    }
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|l| l.split_once(": ").unwrap())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    (code, headers, body.to_string())
}

async fn spawn_cgi_server(port: u16, script_content: &str) -> tokio::process::Child {
    let dir = std::env::temp_dir().join(format!("cgi-rs-test-{}", port));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script_path = dir.join("test-cgi.sh");
    std::fs::write(&script_path, script_content).unwrap();
    make_executable(&script_path);

    let cgi_server_bin = env!("CARGO_BIN_EXE_cgi");
    Command::new(cgi_server_bin)
        .arg("--address")
        .arg(format!("127.0.0.1:{}", port))
        .arg("--max-processes")
        .arg("4")
        .arg(script_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[tokio::test]
async fn test_simple_cgi_request() {
    let port = 18081;
    let script = "#!/bin/sh\necho 'Content-Type: text/plain'\necho ''\necho 'hello world'\n";
    let mut child = spawn_cgi_server(port, script).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();

    let (code, _headers, body) = read_response(&mut stream).await;
    assert_eq!(code, 200);
    assert_eq!(body.trim(), "hello world");

    child.kill().await.unwrap();
}

#[tokio::test]
async fn test_no_headers_returns_500() {
    let port = 18082;
    let script = "#!/bin/sh\nexit 0\n";
    let mut child = spawn_cgi_server(port, script).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();

    let (code, _headers, _body) = read_response(&mut stream).await;
    assert_eq!(code, 500);

    child.kill().await.unwrap();
}

#[tokio::test]
async fn test_location_without_status_returns_302() {
    let port = 18083;
    let script = "#!/bin/sh\necho 'Location: /elsewhere'\n";
    let mut child = spawn_cgi_server(port, script).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();

    let (code, headers, _body) = read_response(&mut stream).await;
    assert_eq!(code, 302);
    assert!(headers
        .iter()
        .any(|(k, v)| k == "location" && v == "/elsewhere"));

    child.kill().await.unwrap();
}
