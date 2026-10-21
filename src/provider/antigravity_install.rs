//! Instalação gerenciada do `agy_acp_server` (Antigravity).
//!
//! Espelha `AntigravityInstallation.ts` / `antigravityRelease.ts` do T3:
//! - binário oficial `agy-acp-server-1.3.0` de `dl.google.com`,
//! - harness `localharness_external` (.par / .exe),
//! - registro `.install-complete.json` com `releaseId` (sha256), versão e bytes.

use std::path::{Path, PathBuf};
use thiserror::Error;
use serde::{Deserialize, Serialize};

pub const VERSION: &str = "1.3.0";

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("plataforma não suportada: {0}")]
    UnsupportedPlatform(String),
    #[error("zip inválido: {0}")]
    Zip(String),
    #[error("falta componente no zip: {0}")]
    MissingComponent(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallComplete {
    pub release_id: String,
    pub version: String,
    pub bytes: u64,
}

/// Mapeia `std::env::consts::{OS,ARCH}` para o par usado na URL do Google.
pub fn platform() -> Result<(String, String), InstallError> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_for(os: &str, arch: &str) -> Result<(String, String), InstallError> {
    let os_mapped = match os {
        "linux" => "linux",
        "macos" | "darwin" => "macos",
        "windows" => "windows",
        other => {
            return Err(InstallError::UnsupportedPlatform(format!(
                "OS {other} não suportado (linux/macos/windows)"
            )))
        }
    };
    let arch_mapped = match arch {
        "x86_64" | "x64" => "x86_64",
        "aarch64" | "arm64" => "arm64",
        other => {
            return Err(InstallError::UnsupportedPlatform(format!(
                "arch {other} não suportado (x86_64/arm64)"
            )))
        }
    };
    Ok((os_mapped.to_string(), arch_mapped.to_string()))
}

/// URL oficial do zip para a plataforma dada.
pub fn download_url(os: &str, arch: &str) -> String {
    format!(
        "https://dl.google.com/agy-extensions/releases/{os}/agy-acp-server-{VERSION}-{os}-{arch}.zip"
    )
}

/// URL para a plataforma atual.
pub fn download_url_for_current() -> Result<String, InstallError> {
    let (os, arch) = platform()?;
    Ok(download_url(&os, &arch))
}

/// Diretório de instalação gerenciada: `~/.local/share/rusi/installs/antigravity`.
pub fn install_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("rusi").join("installs").join("antigravity"))
}

/// Caminho do binário dentro do diretório de instalação.
pub fn binary_path(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("agy_acp_server.exe")
    } else {
        dir.join("agy_acp_server")
    }
}

/// Caminho do harness ao lado do binário.
pub fn harness_path(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("localharness_external.exe")
    } else {
        dir.join("localharness_external.par")
    }
}

/// Nome do harness dentro do zip (o T3 extrai `.par` fora do Windows, `.exe` nele).
pub fn harness_name_in_zip(os: &str) -> &'static str {
    if os == "windows" {
        "localharness_external.exe"
    } else {
        "localharness_external.par"
    }
}

/// Caminho do registro de instalação.
pub fn complete_marker(dir: &Path) -> PathBuf {
    dir.join(".install-complete.json")
}

/// Calcula o sha256 hex de bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Baixa o zip da URL para `dest` com timeout de 45 min (T3).
pub async fn download_to(url: &str, dest: &Path) -> Result<u64, InstallError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45 * 60))
        .build()
        .map_err(|e| InstallError::Http(e.to_string()))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| InstallError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(InstallError::Http(format!(
            "HTTP {} em {url}",
            resp.status()
        )));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| InstallError::Http(e.to_string()))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Extrai o zip para `dir`, validando que contém binário + harness.
///
/// `os` é o OS alvo (para saber o nome do harness).
/// Devolve `(bytes, sha256)` do zip.
pub fn extract_zip(zip_path: &Path, dir: &Path, os: &str) -> Result<(u64, String), InstallError> {
    let data = std::fs::read(zip_path)?;
    let sha = sha256_hex(&data);
    let bytes = data.len() as u64;

    let reader = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| InstallError::Zip(e.to_string()))?;

    let mut found_bin = false;
    let mut found_harness = false;
    let expected_harness = harness_name_in_zip(os);

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| InstallError::Zip(e.to_string()))?;
        let name = file.name().to_string();
        // normaliza: remove prefixos de diretório
        let file_name = Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&name)
            .to_string();

        let out_path = if file_name == "agy_acp_server"
            || file_name == "agy_acp_server.exe"
            || file_name == "agy-acp-server"
        {
            found_bin = true;
            binary_path(dir)
        } else if file_name == expected_harness || file_name == "localharness_external" {
            found_harness = true;
            harness_path(dir)
        } else {
            // ignora outros arquivos (README, licença etc.)
            continue;
        };

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut file, &mut out)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
        }
    }

    if !found_bin {
        return Err(InstallError::MissingComponent(
            "agy_acp_server não encontrado no zip".into(),
        ));
    }
    if !found_harness {
        return Err(InstallError::MissingComponent(format!(
            "{expected_harness} não encontrado no zip"
        )));
    }

    // grava registro
    let marker = InstallComplete {
        release_id: sha.clone(),
        version: VERSION.to_string(),
        bytes,
    };
    let json = serde_json::to_string_pretty(&marker).unwrap();
    std::fs::write(complete_marker(dir), json)?;

    Ok((bytes, sha))
}

/// Instalação completa: baixa, extrai e registra.
///
/// `url_override` permite teste com servidor local; se `None`, usa a URL oficial.
pub async fn install(url_override: Option<&str>) -> Result<PathBuf, InstallError> {
    let dir = install_dir().ok_or_else(|| InstallError::Io(std::io::Error::other("sem data_dir")))?;
    std::fs::create_dir_all(&dir)?;

    let (os, arch) = platform()?;
    let url = url_override
        .map(|u| u.to_string())
        .unwrap_or_else(|| download_url(&os, &arch));

    let tmp_zip = dir.join(format!("agy-{VERSION}.zip.tmp"));
    let _bytes = download_to(&url, &tmp_zip).await?;

    // verificação de 90s: o zip deve ser legível
    let extract_res =
        tokio::time::timeout(std::time::Duration::from_secs(90), async { extract_zip(&tmp_zip, &dir, &os) })
            .await
            .map_err(|_| InstallError::Zip("timeout ao verificar zip (90s)".into()))??;

    let _ = std::fs::remove_file(&tmp_zip);
    let _ = extract_res;
    Ok(dir)
}

/// Lê o registro de instalação, se existir.
pub fn read_complete(dir: &Path) -> Option<InstallComplete> {
    let raw = std::fs::read_to_string(complete_marker(dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_platform_mapping() {
        assert_eq!(platform_for("linux", "x86_64").unwrap(), ("linux".into(), "x86_64".into()));
        assert_eq!(platform_for("linux", "aarch64").unwrap(), ("linux".into(), "arm64".into()));
        assert_eq!(platform_for("darwin", "arm64").unwrap(), ("macos".into(), "arm64".into()));
        assert_eq!(platform_for("macos", "x86_64").unwrap(), ("macos".into(), "x86_64".into()));
        assert!(platform_for("freebsd", "x86_64").is_err());
        assert!(platform_for("linux", "mips").is_err());
    }

    #[test]
    fn test_download_url_format() {
        assert_eq!(
            download_url("linux", "x86_64"),
            "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.3.0-linux-x86_64.zip"
        );
        assert_eq!(
            download_url("macos", "arm64"),
            "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.3.0-macos-arm64.zip"
        );
    }

    #[test]
    fn test_sha256_hex_is_stable() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_extract_zip_creates_bin_and_harness() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("test.zip");
        let out_dir = dir.path().join("out");

        // cria zip com bin + harness
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts: zip::write::SimpleFileOptions = Default::default();
            zip.start_file("agy_acp_server", opts).unwrap();
            zip.write_all(b"#!/bin/sh\necho agy").unwrap();
            zip.start_file("localharness_external.par", opts).unwrap();
            zip.write_all(b"harness").unwrap();
            zip.finish().unwrap();
        }

        let (bytes, sha) = extract_zip(&zip_path, &out_dir, "linux").unwrap();
        assert!(bytes > 0);
        assert_eq!(sha.len(), 64);
        assert!(binary_path(&out_dir).exists());
        assert!(harness_path(&out_dir).exists());
        let marker = read_complete(&out_dir).unwrap();
        assert_eq!(marker.version, VERSION);
        assert_eq!(marker.release_id, sha);
    }

    #[test]
    fn test_extract_zip_missing_harness_fails() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts: zip::write::SimpleFileOptions = Default::default();
            zip.start_file("agy_acp_server", opts).unwrap();
            zip.write_all(b"bin").unwrap();
            zip.finish().unwrap();
        }
        let out = dir.path().join("out");
        let err = extract_zip(&zip_path, &out, "linux").unwrap_err();
        assert!(matches!(err, InstallError::MissingComponent(_)));
    }

    #[test]
    fn test_harness_name() {
        assert_eq!(harness_name_in_zip("linux"), "localharness_external.par");
        assert_eq!(harness_name_in_zip("windows"), "localharness_external.exe");
    }
}
