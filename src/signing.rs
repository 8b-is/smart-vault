use std::fs;
use std::path::Path;
use std::process::Command;

use serde::Serialize;
use smart_vault_core::asm::ct_eq_bytes;

use crate::error::{MemVaultError, Result};
use crate::transaction::{assert_not_symlink, atomic_write_bytes_with_mode, sha256_bytes};

const MAX_BINARY_SIGN_BYTES: u64 = 256 * 1024 * 1024;

/// Summary returned after signing and verifying a compiled `smart-vault` binary.
#[derive(Debug, Clone, Serialize)]
pub struct SignReport {
    pub binary: String,
    pub sha256: String,
    pub codesign_applied: bool,
    pub identity: String,
    pub verified: bool,
    pub sha256_file: Option<String>,
    pub details: String,
}

/// Summary returned when verifying binary digest and OS code signature.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub binary: String,
    pub sha256: String,
    pub sha256_match: Option<bool>,
    pub codesign_valid: bool,
    pub details: String,
}

/// Computes SHA-256, signs `binary_path` with macOS `codesign` (ad-hoc `-` by default, or
/// Developer ID with hardened runtime when `identity` is provided), verifies the signature
/// strictly, and optionally writes `<binary>.sha256`.
pub fn sign_binary(
    binary_path: &Path,
    identity: Option<&str>,
    write_sha256: bool,
) -> Result<SignReport> {
    assert_not_symlink(binary_path)?;
    let meta = fs::metadata(binary_path)?;
    if !meta.is_file() {
        return Err(MemVaultError::InvalidPath(format!(
            "{} is not a regular file",
            binary_path.display()
        )));
    }
    if meta.len() > MAX_BINARY_SIGN_BYTES {
        return Err(MemVaultError::Validation(format!(
            "binary exceeds {} byte signing limit",
            MAX_BINARY_SIGN_BYTES
        )));
    }

    let id = identity.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("-");
    if id.contains('\0') {
        return Err(MemVaultError::Validation("signing identity contains NUL".to_string()));
    }

    let (codesign_applied, verified, details) = platform_codesign(binary_path, id)?;
    let bytes = fs::read(binary_path)?;
    let sha256 = sha256_bytes(&bytes);

    let sha256_file = if write_sha256 {
        let name = binary_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "smart-vault".to_string());
        let sidecar = binary_path.with_file_name(format!("{name}.sha256"));
        assert_not_symlink(&sidecar)?;
        let line = format!("{sha256}  {name}\n");
        atomic_write_bytes_with_mode(&sidecar, line.as_bytes(), Some(0o644))?;
        Some(sidecar.display().to_string())
    } else {
        None
    };

    Ok(SignReport {
        binary: binary_path.display().to_string(),
        sha256,
        codesign_applied,
        identity: if id == "-" { "ad-hoc (-)".to_string() } else { id.to_string() },
        verified,
        sha256_file,
        details,
    })
}

/// Verifies `binary_path` against an optional expected SHA-256 hex digest (constant-time via
/// `smart_vault_core::asm::ct_eq_bytes`) and macOS `codesign --verify --deep --strict`.
pub fn verify_binary(binary_path: &Path, expected_sha256: Option<&str>) -> Result<VerifyReport> {
    assert_not_symlink(binary_path)?;
    let meta = fs::metadata(binary_path)?;
    if !meta.is_file() {
        return Err(MemVaultError::InvalidPath(format!(
            "{} is not a regular file",
            binary_path.display()
        )));
    }
    if meta.len() > MAX_BINARY_SIGN_BYTES {
        return Err(MemVaultError::Validation(format!(
            "binary exceeds {} byte verification limit",
            MAX_BINARY_SIGN_BYTES
        )));
    }

    let bytes = fs::read(binary_path)?;
    let actual_sha = sha256_bytes(&bytes);
    let sha256_match = expected_sha256.map(|exp| {
        let norm = exp.trim().to_ascii_lowercase();
        ct_eq_bytes(actual_sha.as_bytes(), norm.as_bytes())
    });

    if sha256_match == Some(false) {
        return Err(MemVaultError::Validation(format!(
            "SHA-256 mismatch for {}: computed {actual_sha}",
            binary_path.display()
        )));
    }

    let (codesign_valid, details) = platform_verify(binary_path)?;
    if !codesign_valid {
        return Err(MemVaultError::Validation(format!(
            "code signature verification failed for {}: {details}",
            binary_path.display()
        )));
    }

    Ok(VerifyReport {
        binary: binary_path.display().to_string(),
        sha256: actual_sha,
        sha256_match,
        codesign_valid,
        details,
    })
}

#[cfg(target_os = "macos")]
fn platform_codesign(binary_path: &Path, identity: &str) -> Result<(bool, bool, String)> {
    let mut cmd = Command::new("/usr/bin/codesign");
    cmd.args(["--force", "--sign", identity]);
    if identity == "-" {
        cmd.arg("--timestamp=none");
    } else {
        cmd.args(["--options", "runtime", "--timestamp"]);
    }
    cmd.arg("--").arg(binary_path);

    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(MemVaultError::Validation(format!(
            "codesign failed ({}): {}",
            out.status,
            stderr.trim()
        )));
    }

    let (verified, details) = platform_verify(binary_path)?;
    if !verified {
        return Err(MemVaultError::Validation(format!(
            "post-sign codesign verification failed: {details}"
        )));
    }
    Ok((true, verified, details))
}

#[cfg(not(target_os = "macos"))]
fn platform_codesign(_binary_path: &Path, _identity: &str) -> Result<(bool, bool, String)> {
    Ok((
        false,
        true,
        "non-macOS host: SHA-256 manifest computed (Apple codesign skipped)".to_string(),
    ))
}

#[cfg(target_os = "macos")]
fn platform_verify(binary_path: &Path) -> Result<(bool, String)> {
    let out = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2", "--"])
        .arg(binary_path)
        .output()?;
    let combined =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
            .trim()
            .to_string();
    Ok((out.status.success(), combined))
}

#[cfg(not(target_os = "macos"))]
fn platform_verify(_binary_path: &Path) -> Result<(bool, String)> {
    Ok((true, "non-macOS host: SHA-256 verification only".to_string()))
}
