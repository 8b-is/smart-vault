use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::Result;
use crate::markdown::parse_frontmatter;

#[derive(Debug, Clone, Serialize)]
pub struct ValidationFinding {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub ok: bool,
    pub checked_files: usize,
    pub findings: Vec<ValidationFinding>,
}

/// Validates repository product & capability contracts and checks referenced paths exist.
pub fn validate_contracts(repo_root: &Path) -> Result<ValidationReport> {
    let mut findings = Vec::new();
    let mut checked = 0usize;

    for rel in ["config/product-contract.json", "config/capabilities.json"] {
        checked += 1;
        let path = repo_root.join(rel);
        let Ok(raw) = fs::read_to_string(&path) else {
            findings.push(ValidationFinding {
                code: "missing_contract".to_string(),
                path: rel.to_string(),
                message: "required contract JSON is missing".to_string(),
            });
            continue;
        };

        let val: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(err) => {
                findings.push(ValidationFinding {
                    code: "invalid_json".to_string(),
                    path: rel.to_string(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        if rel == "config/capabilities.json" {
            if let Some(caps) = val.get("capabilities").and_then(Value::as_array) {
                let mut seen = BTreeSet::new();
                for cap in caps {
                    let id = cap.get("id").and_then(Value::as_str).unwrap_or("");
                    if id.is_empty() || !seen.insert(id.to_string()) {
                        findings.push(ValidationFinding {
                            code: "duplicate_or_empty_capability".to_string(),
                            path: rel.to_string(),
                            message: format!("invalid capability id '{id}'"),
                        });
                    }
                    if let Some(impls) = cap.get("implementation_paths").and_then(Value::as_array) {
                        for p in impls.iter().filter_map(Value::as_str) {
                            if !repo_root.join(p).exists() {
                                findings.push(ValidationFinding {
                                    code: "missing_implementation_path".to_string(),
                                    path: rel.to_string(),
                                    message: format!(
                                        "capability '{id}' references missing path '{p}'"
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(ValidationReport { ok: findings.is_empty(), checked_files: checked, findings })
}

/// Validates portable plugin manifests, hooks, and all `skills/*/SKILL.md` frontmatter.
pub fn validate_package(repo_root: &Path) -> Result<ValidationReport> {
    let mut findings = Vec::new();
    let mut checked = 0usize;

    for rel in [
        ".claude-plugin/plugin.json",
        ".claude-plugin/marketplace.json",
        "hooks/hooks.json",
        "config/public-marketplace.json",
    ] {
        checked += 1;
        let path = repo_root.join(rel);
        match fs::read_to_string(&path) {
            Ok(raw) => {
                if let Err(err) = serde_json::from_str::<Value>(&raw) {
                    findings.push(ValidationFinding {
                        code: "invalid_json".to_string(),
                        path: rel.to_string(),
                        message: err.to_string(),
                    });
                }
            }
            Err(_) => {
                findings.push(ValidationFinding {
                    code: "missing_file".to_string(),
                    path: rel.to_string(),
                    message: "required package file is missing".to_string(),
                });
            }
        }
    }

    let skills_dir = repo_root.join("skills");
    if skills_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&skills_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            let skill_md = entry.path().join("SKILL.md");
            let rel = format!("skills/{dir_name}/SKILL.md");
            checked += 1;
            let Ok(raw) = fs::read_to_string(&skill_md) else {
                findings.push(ValidationFinding {
                    code: "missing_skill_md".to_string(),
                    path: rel,
                    message: "skill directory is missing SKILL.md".to_string(),
                });
                continue;
            };
            let fm = parse_frontmatter(&raw);
            if !fm.present {
                findings.push(ValidationFinding {
                    code: "missing_frontmatter".to_string(),
                    path: rel.clone(),
                    message: "SKILL.md missing YAML frontmatter".to_string(),
                });
                continue;
            }
            if !fm.fields.contains_key("name") {
                findings.push(ValidationFinding {
                    code: "missing_skill_name".to_string(),
                    path: rel.clone(),
                    message: "SKILL.md frontmatter missing 'name'".to_string(),
                });
            }
            if !fm.fields.contains_key("description") {
                findings.push(ValidationFinding {
                    code: "missing_skill_description".to_string(),
                    path: rel,
                    message: "SKILL.md frontmatter missing 'description'".to_string(),
                });
            }
        }
    }

    Ok(ValidationReport { ok: findings.is_empty(), checked_files: checked, findings })
}
