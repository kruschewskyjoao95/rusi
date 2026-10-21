//! Fluxo OAuth loopback do Antigravity (Google), espelhando o T3.
//!
//! O Rusi cria um redirect loopback `http://127.0.0.1:<porta>/<caminho>` com um
//! `state` de uso único amarrado ao fluxo. O callback é validado com a mesma
//! rigidez do T3 (`AntigravityAuth.ts`):
//! - exatamente um `code` **ou** um `error`,
//! - no máximo um `iss`, e esse `iss` tem que ser `https://accounts.google.com`,
//! - `state` único, expira em 5 min,
//! - origem/caminho têm que bater com o `redirectUri` anunciado,
//! - senão: "Paste the complete redirect URL from the Google sign-in page."

use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;
use url::Url;

pub const AUTH_TIMEOUT: Duration = Duration::from_millis(300_000);
pub const GOOGLE_ISS: &str = "https://accounts.google.com";
pub const PASTE_HINT: &str =
    "Paste the complete redirect URL from the Google sign-in page.";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("fluxo expirado (5 min)")]
    Expired,
    #[error("state inválido ou ausente")]
    InvalidState,
    #[error("state ausente")]
    MissingState,
    #[error("múltiplos state")]
    MultipleStates,
    #[error("múltiplos code")]
    MultipleCodes,
    #[error("múltiplos iss")]
    MultipleIss,
    #[error("iss inválido: esperado {expected}, recebido {got}")]
    InvalidIss { expected: String, got: String },
    #[error("code e error presentes simultaneamente")]
    BothCodeAndError,
    #[error("nenhum code ou error presente")]
    MissingCodeOrError,
    #[error("erro do provedor: {0}")]
    ProviderError(String),
    #[error("redirect não confere: esperado {expected}, recebido {got}")]
    RedirectMismatch { expected: String, got: String },
    #[error("{PASTE_HINT}")]
    IncompleteUrl,
    #[error("URL inválida: {0}")]
    InvalidUrl(String),
    #[error("I/O: {0}")]
    Io(String),
}

/// Gera um `state` url-safe de uso único.
pub fn generate_state() -> String {
    // uuid v4 hex sem hífens = 32 chars url-safe
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

/// Verifica se o fluxo expirou.
pub fn is_expired(issued_at: Instant) -> bool {
    issued_at.elapsed() > AUTH_TIMEOUT
}

/// Valida o `iss` segundo a regra do T3: 0 ou 1 valor, se presente deve ser Google.
pub fn validate_iss(values: &[String]) -> Result<(), AuthError> {
    match values.len() {
        0 => Ok(()),
        1 if values[0] == GOOGLE_ISS => Ok(()),
        1 => Err(AuthError::InvalidIss {
            expected: GOOGLE_ISS.to_string(),
            got: values[0].clone(),
        }),
        _ => Err(AuthError::MultipleIss),
    }
}

/// Valida o callback completo e extrai o `code`.
///
/// `callback_url` é a URL completa que o navegador abriu (ex:
/// `http://127.0.0.1:54321/callback?code=abc&state=xyz&iss=https://accounts.google.com`).
/// `expected_state` é o state gerado no início do fluxo.
/// `expected_redirect_uri` é o redirect anunciado (ex: `http://127.0.0.1:54321/callback`).
/// `issued_at` é o instante de criação do fluxo.
pub fn validate_callback(
    callback_url: &str,
    expected_state: &str,
    expected_redirect_uri: &str,
    issued_at: Instant,
) -> Result<String, AuthError> {
    if is_expired(issued_at) {
        return Err(AuthError::Expired);
    }

    let cb = Url::parse(callback_url).map_err(|_| AuthError::IncompleteUrl)?;
    let expected = Url::parse(expected_redirect_uri)
        .map_err(|e| AuthError::InvalidUrl(e.to_string()))?;

    // origem + caminho devem bater
    let got_origin = format!(
        "{}://{}{}",
        cb.scheme(),
        cb.host_str().unwrap_or(""),
        cb.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let exp_origin = format!(
        "{}://{}{}",
        expected.scheme(),
        expected.host_str().unwrap_or(""),
        expected.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    if got_origin != exp_origin || cb.path() != expected.path() {
        let got = format!("{}{}", got_origin, cb.path());
        let exp = format!("{}{}", exp_origin, expected.path());
        return Err(AuthError::RedirectMismatch { expected: exp, got });
    }

    // coleta query params preservando duplicatas
    let mut buckets: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in cb.query_pairs() {
        buckets.entry(k.to_string()).or_default().push(v.to_string());
    }

    // state: exatamente um e deve bater
    let states = buckets.get("state").cloned().unwrap_or_default();
    match states.len() {
        0 => return Err(AuthError::MissingState),
        1 if states[0] == expected_state => {},
        1 => return Err(AuthError::InvalidState),
        _ => return Err(AuthError::MultipleStates),
    }

    // iss: 0 ou 1, se 1 deve ser Google
    let iss_vals = buckets.get("iss").cloned().unwrap_or_default();
    validate_iss(&iss_vals)?;

    let codes = buckets.get("code").cloned().unwrap_or_default();
    let errors = buckets.get("error").cloned().unwrap_or_default();

    match (codes.len(), errors.len()) {
        (0, 0) => Err(AuthError::MissingCodeOrError),
        (1, 0) => Ok(codes.into_iter().next().unwrap()),
        (0, 1) => Err(AuthError::ProviderError(errors.into_iter().next().unwrap())),
        (1, 1) => Err(AuthError::BothCodeAndError),
        (n, 0) if n > 1 => Err(AuthError::MultipleCodes),
        _ => Err(AuthError::BothCodeAndError),
    }
}

/// HTML minimalista para o navegador após o callback.
pub fn callback_html(success: bool, detail: &str) -> String {
    if success {
        format!(
            "<html><body style=\"font-family:sans-serif;padding:40px\">\
             <h2>Login concluído</h2><p>{}</p><p>Volte ao terminal.</p></body></html>",
            html_escape(detail)
        )
    } else {
        format!(
            "<html><body style=\"font-family:sans-serif;padding:40px\">\
             <h2>Falha no login</h2><p style=\"color:#c00\">{}</p><p>{}</p></body></html>",
            html_escape(detail),
            PASTE_HINT
        )
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Um fluxo loopback ativo: guarda `state`, `redirect_uri` e o instante de criação.
#[derive(Debug)]
pub struct AuthFlow {
    pub state: String,
    pub redirect_uri: String,
    pub issued_at: Instant,
}

impl AuthFlow {
    pub fn new(redirect_uri: String, state: String) -> Self {
        Self {
            state,
            redirect_uri,
            issued_at: Instant::now(),
        }
    }

    pub fn validate(&self, callback_url: &str) -> Result<String, AuthError> {
        validate_callback(callback_url, &self.state, &self.redirect_uri, self.issued_at)
    }
}

/// Inicia um listener TCP em `127.0.0.1:0` e devolve `AuthFlow` com a porta real.
///
/// O chamador deve chamar `wait_for_callback` para aguardar o `GET` com o `code`.
pub async fn start_loopback() -> Result<(tokio::net::TcpListener, AuthFlow), AuthError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AuthError::Io(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::Io(e.to_string()))?
        .port();
    let state = generate_state();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let flow = AuthFlow::new(redirect_uri, state);
    Ok((listener, flow))
}

/// Aguarda um único `GET` no listener, valida e responde com HTML.
///
/// Devolve o `code` em caso de sucesso.
pub async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    flow: &AuthFlow,
) -> Result<String, AuthError> {
    let deadline = flow.issued_at + AUTH_TIMEOUT;
    let timeout = deadline.saturating_duration_since(Instant::now());
    let (mut stream, _) =
        tokio::time::timeout(timeout, listener.accept())
            .await
            .map_err(|_| AuthError::Expired)?
            .map_err(|e| AuthError::Io(e.to_string()))?;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let mut reader = tokio::io::BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| AuthError::Io(e.to_string()))?;

    // consome headers até linha vazia
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| AuthError::Io(e.to_string()))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
    }

    // request_line: "GET /callback?code=... HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or(AuthError::IncompleteUrl)?;
    // reconstrói URL completa para validação (prefixo para detectar caminho errado)
    let _callback_url = format!("{}{}", flow.redirect_uri.split('?').next().unwrap_or(&flow.redirect_uri), {
        if path.contains('?') {
            // path já contém query; extrai só a query
            let q = path.split_once('?').map(|x| x.1).unwrap_or("");
            format!("?{q}")
        } else if path == "/callback" {
            String::new()
        } else {
            // se o cliente pediu caminho diferente, usa o path recebido para gerar mismatch
            let base = Url::parse(&flow.redirect_uri).unwrap();
            let got = format!("http://127.0.0.1:{}/{}", base.port().unwrap_or(80), path.trim_start_matches('/'));
            // devolve erro direto via validação de redirect
            let err = validate_callback(&got, &flow.state, &flow.redirect_uri, flow.issued_at).unwrap_err();
            let body = callback_html(false, &err.to_string());
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            return Err(err);
        }
    });

    // quando path bate, a callback_url completa é redirect_uri + query
    let full_url = if path.starts_with("/callback") {
        // preserva query original do request
        let query = path.split_once('?').map(|x| x.1).unwrap_or("");
        if query.is_empty() {
            flow.redirect_uri.clone()
        } else {
            format!("{}?{query}", flow.redirect_uri)
        }
    } else {
        // fallback: usa path recebido como URL (vai falhar em RedirectMismatch)
        format!(
            "http://127.0.0.1:{}{}",
            Url::parse(&flow.redirect_uri).unwrap().port().unwrap_or(80),
            path
        )
    };

    match validate_callback(&full_url, &flow.state, &flow.redirect_uri, flow.issued_at) {
        Ok(code) => {
            let body = callback_html(true, "Autenticação recebida.");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            Ok(code)
        }
        Err(e) => {
            let body = callback_html(false, &e.to_string());
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn flow_for(port: u16, state: &str) -> AuthFlow {
        AuthFlow {
            state: state.to_string(),
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
            issued_at: Instant::now(),
        }
    }

    #[test]
    fn test_generate_state_is_url_safe_and_unique() {
        let a = generate_state();
        let b = generate_state();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn test_validate_callback_success_with_iss() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc123&state=s123&iss=https://accounts.google.com";
        assert_eq!(f.validate(url).unwrap(), "abc123");
    }

    #[test]
    fn test_validate_callback_success_without_iss() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc&state=s123";
        assert_eq!(f.validate(url).unwrap(), "abc");
    }

    #[test]
    fn test_validate_callback_rejects_wrong_iss() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc&state=s123&iss=https://evil.com";
        assert!(matches!(f.validate(url), Err(AuthError::InvalidIss { .. })));
    }

    #[test]
    fn test_validate_callback_rejects_multiple_iss() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc&state=s123&iss=https://accounts.google.com&iss=https://accounts.google.com";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::MultipleIss);
    }

    #[test]
    fn test_validate_callback_rejects_wrong_state() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc&state=wrong";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::InvalidState);
    }

    #[test]
    fn test_validate_callback_rejects_missing_state() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=abc";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::MissingState);
    }

    #[test]
    fn test_validate_callback_rejects_missing_code_and_error() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?state=s123";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::MissingCodeOrError);
    }

    #[test]
    fn test_validate_callback_error_param_maps_to_provider_error() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?error=access_denied&state=s123";
        assert!(matches!(f.validate(url), Err(AuthError::ProviderError(m)) if m == "access_denied"));
    }

    #[test]
    fn test_validate_callback_rejects_both_code_and_error() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=a&error=e&state=s123";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::BothCodeAndError);
    }

    #[test]
    fn test_validate_callback_rejects_multiple_codes() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/callback?code=a&code=b&state=s123";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::MultipleCodes);
    }

    #[test]
    fn test_validate_callback_rejects_redirect_mismatch_path() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:4123/other?code=abc&state=s123";
        assert!(matches!(f.validate(url), Err(AuthError::RedirectMismatch { .. })));
    }

    #[test]
    fn test_validate_callback_rejects_redirect_mismatch_port() {
        let f = flow_for(4123, "s123");
        let url = "http://127.0.0.1:9999/callback?code=abc&state=s123";
        assert!(matches!(f.validate(url), Err(AuthError::RedirectMismatch { .. })));
    }

    #[test]
    fn test_validate_callback_expired() {
        let f = AuthFlow {
            state: "s123".into(),
            redirect_uri: "http://127.0.0.1:4123/callback".into(),
            issued_at: Instant::now() - Duration::from_millis(300_001),
        };
        let url = "http://127.0.0.1:4123/callback?code=abc&state=s123";
        assert_eq!(f.validate(url).unwrap_err(), AuthError::Expired);
    }

    #[test]
    fn test_validate_callback_incomplete_url() {
        let _f = flow_for(4123, "s123");
        // URL sem host
        let err = validate_callback(
            "/callback?code=abc&state=s123",
            "s123",
            "http://127.0.0.1:4123/callback",
            Instant::now(),
        )
        .unwrap_err();
        assert_eq!(err, AuthError::IncompleteUrl);
    }

    #[test]
    fn test_is_expired_boundary() {
        let recent = Instant::now();
        assert!(!is_expired(recent));
        let old = Instant::now() - Duration::from_millis(300_001);
        assert!(is_expired(old));
    }
}
