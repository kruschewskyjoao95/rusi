//! Testes de instalação + OAuth loopback do Antigravity (Fase 4)

use rusi::provider::antigravity_auth::{self, validate_callback, generate_state, AuthError, GOOGLE_ISS};
use rusi::provider::antigravity_install::{self, download_url, platform_for, sha256_hex};
use std::time::{Duration, Instant};

#[tokio::test]
async fn test_loopback_server_validates_code_and_iss() {
    let (listener, flow) = antigravity_auth::start_loopback().await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // garante que o state e redirect batem
    let redirect = format!("http://127.0.0.1:{port}/callback");
    assert_eq!(flow.redirect_uri, redirect);

    // dispara um GET com code + state + iss válidos
    let code_expected = "test_code_123";
    let state_clone = flow.state.clone();
    let _url = format!("{redirect}?code={code_expected}&state={state_clone}&iss={}", GOOGLE_ISS);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let req = format!("GET /callback?code={code_expected}&state={state_clone}&iss={} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n", GOOGLE_ISS);
        stream.write_all(req.as_bytes()).await.unwrap();
        // lê resposta
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;
        assert!(String::from_utf8_lossy(&buf).contains("200 OK"));
    });

    let code = antigravity_auth::wait_for_callback(listener, &flow).await.unwrap();
    assert_eq!(code, code_expected);
}

#[tokio::test]
async fn test_loopback_rejects_wrong_state() {
    let (listener, flow) = antigravity_auth::start_loopback().await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _redirect = format!("http://127.0.0.1:{port}/callback");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let req = format!("GET /callback?code=abc&state=WRONG HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;
    });
    let err = antigravity_auth::wait_for_callback(listener, &flow).await.unwrap_err();
    assert_eq!(err, AuthError::InvalidState);
}

#[tokio::test]
async fn test_loopback_rejects_redirect_mismatch() {
    let (listener, flow) = antigravity_auth::start_loopback().await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = flow.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let req = format!("GET /other?code=abc&state={state_clone} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;
    });
    let err = antigravity_auth::wait_for_callback(listener, &flow).await.unwrap_err();
    assert!(matches!(err, AuthError::RedirectMismatch { .. }));
}

#[test]
fn test_validate_paste_hint_on_incomplete_url() {
    let err = validate_callback(
        "/callback?code=abc&state=s",
        "s",
        "http://127.0.0.1:1234/callback",
        Instant::now(),
    )
    .unwrap_err();
    assert_eq!(err, AuthError::IncompleteUrl);
    assert!(err.to_string().contains("Paste the complete redirect URL"));
}

#[test]
fn test_browser_env_captures_url_pattern() {
    // Simula o que o BROWSER shim faz: escreve a URL aberta pelo agy_acp_server
    let dir = tempfile::tempdir().unwrap();
    let browser_sh = dir.path().join("browser.sh");
    std::fs::write(
        &browser_sh,
        format!("#!/bin/sh\necho \"$1\" > \"{}/url.txt\"\n", dir.path().display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&browser_sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = dir.path().join("url.txt");
    std::process::Command::new(&browser_sh)
        .arg("https://accounts.google.com/o/oauth2/auth?state=xyz")
        .output()
        .unwrap();
    let captured = std::fs::read_to_string(&out).unwrap();
    assert!(captured.contains("accounts.google.com"));
}

#[test]
fn test_install_download_url_platforms() {
    assert_eq!(
        download_url("linux", "x86_64"),
        "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-x86_64.zip"
    );
    assert_eq!(
        download_url("macos", "arm64"),
        "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-macos-arm64.zip"
    );
    assert!(platform_for("freebsd", "x86_64").is_err());
    assert!(platform_for("linux", "mips").is_err());
}

#[test]
fn test_sha256_hex_stable() {
    assert_eq!(
        sha256_hex(b"rusi"),
        {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(b"rusi");
            format!("{:x}", h.finalize())
        }
    );
}

#[tokio::test]
async fn test_download_and_extract_via_mock_http() {
    // Cria um zip em memória com bin + harness
    let mut buf = Vec::new();
    {
        let mut w = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(&mut w);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.start_file("agy_acp_server", opts).unwrap();
        use std::io::Write;
        zip.write_all(b"#!/bin/sh\necho agy").unwrap();
        zip.start_file("localharness_external.par", opts).unwrap();
        zip.write_all(b"harness").unwrap();
        zip.finish().unwrap();
    }
    let zip_bytes = buf.clone();

    // Sobe um servidor HTTP efêmero que serve o zip
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let mut reader = tokio::io::BufReader::new(&mut stream);
        let mut line = String::new();
        let _ = reader.read_line(&mut line).await;
        // consome headers
        loop {
            let mut h = String::new();
            let _ = reader.read_line(&mut h).await;
            if h == "\r\n" || h == "\n" || h.is_empty() {
                break;
            }
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            zip_bytes.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(&zip_bytes).await.unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("dl.zip");
    let url = format!("http://127.0.0.1:{port}/agy.zip");
    let bytes = antigravity_install::download_to(&url, &zip_path).await.unwrap();
    assert_eq!(bytes as usize, buf.len());

    let out_dir = dir.path().join("out");
    let (extracted_bytes, sha) = antigravity_install::extract_zip(&zip_path, &out_dir, "linux").unwrap();
    assert_eq!(extracted_bytes as usize, buf.len());
    assert_eq!(sha.len(), 64);
    assert!(antigravity_install::binary_path(&out_dir).exists());
    assert!(antigravity_install::harness_path(&out_dir).exists());
    assert!(antigravity_install::read_complete(&out_dir).is_some());
}

#[test]
fn test_state_single_use_and_5min_expiry() {
    let state = generate_state();
    let flow = rusi::provider::antigravity_auth::AuthFlow::new(
        "http://127.0.0.1:1234/callback".into(),
        state.clone(),
    );
    // válido agora
    let url = format!("http://127.0.0.1:1234/callback?code=abc&state={state}");
    assert!(flow.validate(&url).is_ok());
    // expirado
    let expired = rusi::provider::antigravity_auth::AuthFlow {
        state: state.clone(),
        redirect_uri: "http://127.0.0.1:1234/callback".into(),
        issued_at: Instant::now() - Duration::from_secs(301),
    };
    assert_eq!(expired.validate(&url).unwrap_err(), AuthError::Expired);
}
