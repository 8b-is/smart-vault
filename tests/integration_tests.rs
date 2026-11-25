use std::fs;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

use smart_vault::bm25::{benchmark_queries, build_index, query_index};
use smart_vault::capture::{
    plan_inbox_capture, queue_claim, queue_enqueue, queue_list, queue_recover, queue_transition,
    validate_safe_https_url,
};
use smart_vault::contracts::{validate_contracts, validate_package};
use smart_vault::error::MemVaultError;
use smart_vault::hooks::{pre_compact, session_end, session_start, stop};
use smart_vault::lint::lint_vault;
use smart_vault::memnet::parse_json_from_stdout;
use smart_vault::multi_agent::{setup_global_m1, setup_hosts};
use smart_vault::paths::{resolve_vault_root, safe_vault_join};
use smart_vault::signing::{sign_binary, verify_binary};
use smart_vault::smart_tree::run_tree;
use smart_vault::transaction::{
    apply_bundle, atomic_write_bytes, inspect_bundle, read_bounded_bytes, recover_incomplete,
    FileOp, TransactionBundle, TransactionJournal,
};
use smart_vault::vault::{
    doctor, init_or_adopt, peek_next_address, read_mode, scan_vault, set_mode,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir dst");
    for entry in fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("entry");
        let ty = entry.file_type().expect("file_type");
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else if ty.is_file() {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

#[test]
fn test_init_dry_run_apply_doctor_scan_mode_address() {
    let dir = TempDir::new().expect("tempdir");
    let vault_path = dir.path().join("my-vault");

    // Dry run on non-existent directory leaves it uncreated
    let dry = init_or_adopt(&vault_path, "zettel", false, false, None).expect("dry init");
    assert_eq!(dry["dry_run"], true);
    assert!(!vault_path.exists());

    // Apply creates full vault structure
    let applied = init_or_adopt(&vault_path, "zettel", false, true, None).expect("apply init");
    assert_eq!(applied["dry_run"], false);
    assert!(vault_path.join(".smart-vault.json").is_file());
    assert!(vault_path.join("wiki/index.md").is_file());
    assert!(vault_path.join(".raw/.manifest.json").is_file());

    // Duplicate init fails closed
    let err = init_or_adopt(&vault_path, "zettel", false, true, None).unwrap_err();
    assert!(matches!(err, MemVaultError::AlreadyInitialized(_)));

    // Resolve + Doctor + Scan + Mode + Address
    let sel = resolve_vault_root(Some(&vault_path), false).expect("resolve");
    let doc = doctor(&sel);
    assert!(doc.ok);
    assert_eq!(doc.mode, "zettel");
    assert_eq!(doc.incomplete_journals, 0);

    let scan = scan_vault(&vault_path).expect("scan");
    assert_eq!(scan.markdown_pages, 4);

    let mode_preview = set_mode(&vault_path, "para", false).expect("preview mode");
    assert!(!mode_preview.applied);
    assert_eq!(read_mode(&vault_path), "zettel");

    let mode_set = set_mode(&vault_path, "para", true).expect("apply mode");
    assert!(mode_set.applied);
    assert_eq!(read_mode(&vault_path), "para");

    let next_addr = peek_next_address(&vault_path).expect("peek addr");
    assert_eq!(next_addr, "c-000001");
}

#[test]
fn test_adopt_preserves_existing_notes() {
    let dir = TempDir::new().expect("tempdir");
    let existing = dir.path().join("wiki/concepts");
    fs::create_dir_all(&existing).unwrap();
    fs::write(existing.join("Existing.md"), "# Existing\n\nKeep me.\n").unwrap();
    fs::write(dir.path().join("wiki/index.md"), "# Custom Index\n").unwrap();

    let applied = init_or_adopt(dir.path(), "lyt", true, true, None).expect("adopt");
    assert_eq!(applied["dry_run"], false);
    let idx = fs::read_to_string(dir.path().join("wiki/index.md")).unwrap();
    assert_eq!(idx, "# Custom Index\n");
    assert!(dir.path().join(".smart-vault.json").is_file());
}

#[test]
fn test_lint_against_real_fixture_vault() {
    let fixture = repo_root().join("tests/fixtures/lint/vault");
    let report = lint_vault(&fixture).expect("lint fixture");

    assert!(report.page_count >= 8);
    // Expected kinds from fixture:
    // - Missing Frontmatter.md -> missing_frontmatter + orphan_page (Wait: linked from index.md! Not orphan)
    // - Orphan.md -> orphan_page
    // - Empty Section Page.md -> empty_section
    // - Shared.md (concepts + entities) -> duplicate_title + ambiguous_wikilink from index.md
    // - [[Gone]] -> broken_wikilink (while [[Allowed Future Page]] is allowlisted!)
    // - Inline/Fence ghosts ignored!
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "missing_frontmatter" && f.path.ends_with("Missing Frontmatter.md")));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "orphan_page" && f.path.ends_with("Orphan.md")));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "empty_section" && f.target.as_deref() == Some("Empty")));
    // "Code Content" and "Parent" in Empty Section Page.md must NOT be marked empty
    assert!(!report
        .findings
        .iter()
        .any(|f| f.kind == "empty_section" && f.target.as_deref() == Some("Parent")));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "duplicate_title" && f.target.as_deref() == Some("shared")));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "ambiguous_wikilink" && f.target.as_deref() == Some("Shared")));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "broken_wikilink" && f.target.as_deref() == Some("Gone")));
    assert!(!report.findings.iter().any(|f| f.target.as_deref() == Some("Allowed Future Page")));
    assert!(!report
        .findings
        .iter()
        .any(|f| f.target.as_deref().is_some_and(|t| t.contains("Ghost"))));
}

#[test]
fn test_bm25_index_unicode_cjk_and_stale_rejection() {
    let dir = TempDir::new().expect("tempdir");
    let fixture_root = repo_root().join("tests/fixtures/retrieval-quality");
    init_or_adopt(dir.path(), "standard", false, true, None).unwrap();
    copy_dir_all(&fixture_root.join("pages"), &dir.path().join("wiki"));

    let stats = build_index(dir.path()).expect("build index");
    assert_eq!(stats.doc_count, 14); // 10 fixture pages + 4 init pages

    // Exact address
    let exact = query_index(dir.path(), "c-100001", 5, false).unwrap();
    assert_eq!(exact.hits[0].path, "wiki/concepts/Vault backup.md");

    // French NFD folding
    let fr = query_index(dir.path(), "cafe memoire", 5, false).unwrap();
    assert_eq!(fr.hits[0].path, "wiki/concepts/Café mémoire.md");

    // Romanian NFD folding
    let ro = query_index(dir.path(), "provenienta dovada citabila", 5, false).unwrap();
    assert_eq!(ro.hits[0].path, "wiki/concepts/Cercetare română.md");

    // Japanese CJK n-gram matching
    let ja = query_index(dir.path(), "全文検索", 5, false).unwrap();
    assert_eq!(ja.hits[0].path, "wiki/concepts/Recherche japonaise.md");

    // Evaluate queries.json fixture before mutation
    let bench = benchmark_queries(dir.path(), &fixture_root.join("queries.json"), 5).unwrap();
    assert_eq!(bench.total_queries, 14);
    assert!(bench.hit_count >= 12);

    // Apply post-index mutation (retiring Old mirror.md) WITHOUT re-indexing -> stale hit must be rejected!
    copy_dir_all(&fixture_root.join("post-index"), &dir.path().join("wiki"));
    let stale_query = query_index(dir.path(), "copperbridge", 5, false).unwrap();
    assert!(stale_query.hits.is_empty());
    assert!(stale_query.stale_skipped >= 1);
}

#[test]
fn test_transaction_approval_apply_and_recovery() {
    let dir = TempDir::new().expect("tempdir");
    init_or_adopt(dir.path(), "standard", false, true, None).unwrap();

    let bundle = TransactionBundle {
        schema: smart_vault::transaction::TRANSACTION_SCHEMA.to_string(),
        operation_id: "op-test-01".to_string(),
        operation_type: "save".to_string(),
        generated_at: "2026-11-24T00:00:00Z".to_string(),
        description: "Create test note".to_string(),
        operations: vec![FileOp::Create {
            path: "wiki/concepts/Rust Engine.md".to_string(),
            content: "---\ntitle: Rust Engine\n---\n\n# Rust Engine\n\nFast.\n".to_string(),
        }],
    };

    let plan = inspect_bundle(dir.path(), &bundle).expect("inspect");

    // Missing hash rejected
    let missing_err = apply_bundle(dir.path(), &bundle, None).unwrap_err();
    assert!(matches!(missing_err, MemVaultError::PlanApprovalRequired));

    // Wrong hash rejected
    let bad_err = apply_bundle(dir.path(), &bundle, Some("deadbeef")).unwrap_err();
    assert!(matches!(bad_err, MemVaultError::PlanChanged));

    // Valid hash applies atomically
    let journal = apply_bundle(dir.path(), &bundle, Some(&plan.approval_sha256)).expect("apply");
    assert_eq!(journal.status, "committed");
    assert!(dir.path().join("wiki/concepts/Rust Engine.md").is_file());

    // Simulate an interrupted `in_progress` journal that created a leftover file
    let stray = dir.path().join("wiki/concepts/Stray.md");
    fs::write(&stray, "incomplete").unwrap();
    let jpath = dir.path().join(".vault-meta/journal/crash-op.json");
    let crashed = TransactionJournal {
        operation_id: "crash-op".to_string(),
        operation_type: "save".to_string(),
        approval_sha256: "abc".to_string(),
        status: "in_progress".to_string(),
        started_at: "2026-11-24T00:00:00Z".to_string(),
        completed_at: None,
        pre_images: vec![smart_vault::transaction::PreImage {
            path: "wiki/concepts/Stray.md".to_string(),
            existed: false,
            content_base64_or_utf8: None,
            sha256: None,
        }],
    };
    fs::write(&jpath, serde_json::to_string_pretty(&crashed).unwrap()).unwrap();

    // Dry-run recovery detects without deleting
    let dry_rec = recover_incomplete(dir.path(), false).unwrap();
    assert_eq!(dry_rec.incomplete_count, 1);
    assert!(stray.exists());

    // Apply recovery deletes the uncommitted file and marks journal recovered
    let app_rec = recover_incomplete(dir.path(), true).unwrap();
    assert_eq!(app_rec.incomplete_count, 1);
    assert!(!stray.exists());
}

#[test]
fn test_path_traversal_and_capture_ssrf_security() {
    let dir = TempDir::new().expect("tempdir");
    init_or_adopt(dir.path(), "standard", false, true, None).unwrap();

    assert!(matches!(
        safe_vault_join(dir.path(), Path::new("../etc/passwd")),
        Err(MemVaultError::SymlinkEscape(_))
    ));

    assert!(validate_safe_https_url("https://example.com/article").is_ok());
    assert!(validate_safe_https_url("http://example.com/insecure").is_err());
    assert!(validate_safe_https_url("https://localhost/admin").is_err());
    assert!(validate_safe_https_url("https://127.0.0.1/secret").is_err());
    assert!(validate_safe_https_url("https://169.254.169.254/latest/meta-data").is_err());
    assert!(validate_safe_https_url("https://10.0.0.1/internal").is_err());

    // Queue lifecycle + inbox capture planning
    fs::write(dir.path().join("inbox/note.md"), "# Inbox Note\nHello\n").unwrap();
    let planned = plan_inbox_capture(dir.path()).unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].source_path, "inbox/note.md");

    let item = queue_enqueue(dir.path(), "https://example.com/paper", "url").unwrap();
    let claimed = queue_claim(dir.path(), "worker-1").unwrap().unwrap();
    assert_eq!(claimed.id, item.id);
    let failed = queue_transition(dir.path(), &item.id, "failed", Some("timeout")).unwrap();
    assert_eq!(failed.status, "failed");
    let resumed = queue_transition(dir.path(), &item.id, "pending", None).unwrap();
    assert_eq!(resumed.status, "pending");
    let _claimed_again = queue_claim(dir.path(), "worker-2").unwrap().unwrap();
    let recovered = queue_recover(dir.path()).unwrap();
    assert_eq!(recovered, vec![item.id.clone()]);
    let completed = queue_transition(dir.path(), &item.id, "completed", None).unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(queue_list(dir.path()).unwrap().items.len(), 1);
}

#[test]
fn test_smart_tree_memnet_parser_hooks_and_contracts() {
    let sample = repo_root().join("examples/sample-vault");
    let tree = run_tree(&sample, "ai", Some(2)).expect("tree");
    assert!(!tree.output.trim().is_empty());

    let parsed = parse_json_from_stdout("Upgrade notice\n{\"local_address\":42}").unwrap();
    assert_eq!(parsed["local_address"], 42);

    let ss = session_start(Some(&sample), false);
    assert!(ss.vault_detected);
    let stp = stop(Some(&sample), false);
    assert!(stp.vault_detected);
    let pc = pre_compact(Some(&sample), false);
    assert_eq!(pc.event, "pre-compact");
    let se = session_end(Some(&sample), false);
    assert_eq!(se.event, "session-end");

    let contracts_rep = validate_contracts(&repo_root()).unwrap();
    assert!(contracts_rep.ok, "contracts findings: {:?}", contracts_rep.findings);

    let pkg_rep = validate_package(&repo_root()).unwrap();
    assert!(pkg_rep.ok, "package findings: {:?}", pkg_rep.findings);

    let setup_preview =
        setup_hosts(&repo_root(), &["gemini".to_string(), "codex".to_string()], None, false)
            .unwrap();
    assert!(setup_preview.dry_run);
    assert_eq!(setup_preview.actions.len(), 2);
}

#[test]
fn test_mcp_stdio_roundtrip() {
    let bin = env!("CARGO_BIN_EXE_smart-vault");
    let sample = repo_root().join("examples/sample-vault");

    let mut child = Command::new(bin)
        .arg("mcp")
        .arg("--vault")
        .arg(&sample)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn smart-vault mcp");

    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#)
            .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#)
            .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"vault_lint","arguments":{{}}}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"vault_scan","arguments":{{}}}}}}"#
        )
        .unwrap();
    }

    let reader = BufReader::new(child.stdout.take().unwrap());
    let lines: Vec<Value> =
        reader.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "smart-vault");
    let tools = lines[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 15);
    assert!(tools.iter().any(|t| t["name"] == "vault_query"));
    assert!(tools.iter().any(|t| t["name"] == "mesh_ask"));
    assert!(tools.iter().any(|t| t["name"] == "palace_status"));
    assert!(tools.iter().any(|t| t["name"] == "palace_wake_up"));
    assert!(tools.iter().any(|t| t["name"] == "palace_search"));
    assert!(tools.iter().any(|t| t["name"] == "palace_mine"));
    assert_eq!(lines[2]["result"]["isError"], false);
    assert_eq!(lines[3]["result"]["isError"], false);
}

#[test]
fn test_setup_global_hermetic_idempotent() {
    let fake_home = TempDir::new().expect("temp home");
    let gemini_cfg = fake_home.path().join(".gemini/config");
    fs::create_dir_all(&gemini_cfg).unwrap();

    // Seed pre-existing MCP server and caveman hook
    let mcp_path = gemini_cfg.join("mcp_config.json");
    fs::write(
        &mcp_path,
        r#"{"mcpServers":{"smart-tree":{"command":"/Users/peterlodri/.local/bin/st","args":["--mcp"]}}}"#,
    )
    .unwrap();

    let settings_path = fake_home.path().join(".gemini/settings.json");
    fs::write(
        &settings_path,
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"caveman-hook"}]}]}}"#,
    )
    .unwrap();

    let real_bin = Path::new(env!("CARGO_BIN_EXE_smart-vault"));

    // Dry-run does not modify files or create backups
    let dry = setup_global_m1(&repo_root(), Some(real_bin), Some(fake_home.path()), false, false)
        .expect("dry setup_global");
    assert!(dry.dry_run);
    assert!(!fake_home.path().join(".local/bin/smart-vault").exists());
    assert!(!mcp_path.with_extension("json.bak").exists());

    // First apply installs, signs, creates `sv` symlink, merges without clobbering, and writes .bak
    let applied =
        setup_global_m1(&repo_root(), Some(real_bin), Some(fake_home.path()), false, true)
            .expect("apply setup_global");
    assert!(!applied.dry_run);
    let installed_bin = fake_home.path().join(".local/bin/smart-vault");
    assert!(installed_bin.is_file());
    assert!(installed_bin.with_file_name("smart-vault.sha256").is_file());
    assert!(fake_home.path().join(".local/bin/sv").exists());
    assert!(mcp_path.with_extension("json.bak").is_file());
    assert!(settings_path.with_extension("json.bak").is_file());
    assert!(fake_home.path().join(".gemini/config/skills/smart-vault").exists());

    let mcp_json: Value = serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
    assert!(mcp_json["mcpServers"]["smart-tree"].is_object());
    assert!(mcp_json["mcpServers"]["smart-vault"].is_object());

    // Second apply is idempotent (SessionStart keeps exactly 2 entries: caveman + smart-vault)
    setup_global_m1(&repo_root(), Some(real_bin), Some(fake_home.path()), false, true)
        .expect("second apply");
    let settings_json: Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    let ss_hooks = settings_json["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(ss_hooks.len(), 2);
}

#[test]
fn test_security_ssrf_url_hardening() {
    // Public HTTPS URLs and public IPv6 continue to pass
    assert!(validate_safe_https_url("https://example.com:8443/article").is_ok());
    assert!(validate_safe_https_url("https://[2606:4700:4700::1111]/dns-query").is_ok());

    // IPv4-mapped IPv6 and IPv6 private/link-local SSRF vectors
    for bad in [
        "https://[::ffff:127.0.0.1]/admin",
        "https://[::ffff:10.0.0.1]/internal",
        "https://[::ffff:169.254.169.254]/latest/meta-data",
        "https://[::ffff:7f00:1]/admin",
        "https://[::127.0.0.1]/admin",
        "https://[::1]/admin",
        "https://[::]/admin",
        "https://[fe80::1]/metadata",
        "https://[fd00:ec2::254]/latest/meta-data",
        // ASCII control chars & embedded whitespace
        "https://example.com/\x00admin",
        "https://example.com/\nHost: 127.0.0.1",
        "https://example.com/path\r",
        "https://example.com/a b",
        "https://example.com/\tfoo",
        // Backslash, percent-escape, trailing dot, obfuscated IPv4
        "https://127.0.0.1\\admin",
        "https://localhost./admin",
        "https://metadata.google.internal./",
        "https://0177.0.0.1/admin",
        "https://0x7f.0.0.1/admin",
        "https://2130706433/admin",
    ] {
        assert!(
            matches!(validate_safe_https_url(bad), Err(MemVaultError::UnsafeUrl(_))),
            "expected UnsafeUrl rejection for {bad:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn test_security_symlink_write_and_bounded_read_rejection() {
    let dir = TempDir::new().expect("tempdir");
    let vault = dir.path().join("vault");
    init_or_adopt(&vault, "standard", false, true, None).unwrap();

    // 1. Live symlink target rejected by atomic_write_bytes without touching target file
    let outside = dir.path().join("outside.txt");
    fs::write(&outside, "original-secret").unwrap();
    let live_link = vault.join("wiki/concepts/live-link.md");
    symlink(&outside, &live_link).unwrap();

    let err = atomic_write_bytes(&live_link, b"overwritten").unwrap_err();
    assert!(matches!(err, MemVaultError::SymlinkEscape(_)));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original-secret");

    // 2. Dangling symlink target rejected by atomic_write_bytes
    let dangling_link = vault.join("wiki/concepts/dangling-link.md");
    symlink(dir.path().join("does-not-exist.md"), &dangling_link).unwrap();
    let err2 = atomic_write_bytes(&dangling_link, b"payload").unwrap_err();
    assert!(matches!(err2, MemVaultError::SymlinkEscape(_)));

    // 3. Dangling vault symlink rejected by resolve_vault_root even with allow_uninitialized=true
    let dangling_vault = dir.path().join("dangling-vault");
    symlink(dir.path().join("missing-vault-dir"), &dangling_vault).unwrap();
    let err3 = resolve_vault_root(Some(&dangling_vault), true).unwrap_err();
    assert!(matches!(err3, MemVaultError::SymlinkEscape(_)));

    // 4. TOCTOU symlink created between inspect_bundle and apply_bundle is rejected
    let toctou_bundle = TransactionBundle {
        schema: smart_vault::transaction::TRANSACTION_SCHEMA.to_string(),
        operation_id: "op-toctou-symlink".to_string(),
        operation_type: "save".to_string(),
        generated_at: "2026-11-24T00:00:00Z".to_string(),
        description: "Test symlink TOCTOU rejection".to_string(),
        operations: vec![FileOp::Create {
            path: "wiki/concepts/race-link.md".to_string(),
            content: "overwritten-via-bundle".to_string(),
        }],
    };
    let toctou_plan = inspect_bundle(&vault, &toctou_bundle).unwrap();
    symlink(&outside, vault.join("wiki/concepts/race-link.md")).unwrap();
    let err_toctou =
        apply_bundle(&vault, &toctou_bundle, Some(&toctou_plan.approval_sha256)).unwrap_err();
    assert!(matches!(err_toctou, MemVaultError::SymlinkEscape(_)));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original-secret");

    // 5. Bounded read rejects files exceeding max byte limit
    let sized = vault.join("wiki/concepts/sized.md");
    fs::write(&sized, vec![b'a'; 256]).unwrap();
    let err4 = read_bounded_bytes(&sized, 128).unwrap_err();
    assert!(matches!(err4, MemVaultError::Validation(_)));
    assert_eq!(read_bounded_bytes(&sized, 256).unwrap().len(), 256);
}

#[cfg(unix)]
#[test]
fn test_security_setup_global_m1_preserves_0600_permissions() {
    let temp = TempDir::new().expect("temp home");
    let fake_home = temp.path().join("o'malley-home");
    let gemini_cfg = fake_home.join(".gemini/config");
    fs::create_dir_all(&gemini_cfg).unwrap();

    let mcp_path = gemini_cfg.join("mcp_config.json");
    fs::write(&mcp_path, r#"{"mcpServers":{"existing":{"command":"echo","args":[]}}}"#).unwrap();
    fs::set_permissions(&mcp_path, fs::Permissions::from_mode(0o600)).unwrap();

    let settings_path = fake_home.join(".gemini/settings.json");
    fs::write(&settings_path, r#"{"hooks":{}}"#).unwrap();
    fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o600)).unwrap();

    let real_bin = Path::new(env!("CARGO_BIN_EXE_smart-vault"));

    setup_global_m1(&repo_root(), Some(real_bin), Some(&fake_home), false, true)
        .expect("apply setup_global_m1");

    for protected in [
        &mcp_path,
        &mcp_path.with_extension("json.bak"),
        &settings_path,
        &settings_path.with_extension("json.bak"),
    ] {
        let mode = fs::metadata(protected).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600 on {}, got {mode:#o}", protected.display());
    }

    // Verify single quote in HOME is safely POSIX-quoted in settings.json hook command
    let settings_val: Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    let cmd = settings_val["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.contains("o'\\''malley-home"), "expected POSIX escaped quote in command: {cmd}");
}

#[test]
fn test_security_mcp_bounded_line_rejects_oversized_and_continues() {
    let bin = env!("CARGO_BIN_EXE_smart-vault");
    let sample = repo_root().join("examples/sample-vault");

    let mut child = Command::new(bin)
        .arg("mcp")
        .arg("--vault")
        .arg(&sample)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn smart-vault mcp");

    {
        let mut stdin = child.stdin.take().unwrap();
        let pad = "a".repeat(4 * 1024 * 1024 + 64);
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":99,"method":"ping","params":{{"pad":"{pad}"}}}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":100,"method":"ping","params":{{}}}}"#).unwrap();
    }

    let reader = BufReader::new(child.stdout.take().unwrap());
    let lines: Vec<Value> =
        reader.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32600);
    assert_eq!(lines[1]["id"], 100);
    assert_eq!(lines[1]["result"], serde_json::json!({}));
}

#[test]
fn test_csr_graph_metrics_and_binary_signing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("vault");
    init_or_adopt(&root, "zettel", false, true, None).unwrap();

    // Build a 3-node cycle A -> B -> C -> A plus D -> A so Tarjan SCC detects the 3-cycle
    // and PageRank ranks A highest.
    let pages = [
        (
            "wiki/concepts/A.md",
            "---\ntitle: A\ntype: concept\ntags: [graph]\n---\n# A\n\nLinks to [[B]].\n",
        ),
        (
            "wiki/concepts/B.md",
            "---\ntitle: B\ntype: concept\ntags: [graph]\n---\n# B\n\nLinks to [[C]].\n",
        ),
        (
            "wiki/concepts/C.md",
            "---\ntitle: C\ntype: concept\ntags: [graph]\n---\n# C\n\nBack to [[A]].\n",
        ),
        (
            "wiki/concepts/D.md",
            "---\ntitle: D\ntype: concept\ntags: [graph]\n---\n# D\n\nSupports [[A]].\n",
        ),
    ];
    for (rel, content) in pages {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    let report = lint_vault(&root).unwrap();
    assert!(report.graph_metrics.node_count >= 4);
    assert!(report.graph_metrics.edge_count >= 4);
    assert!(report.graph_metrics.cyclic_sccs >= 1);
    assert!(report.graph_metrics.largest_scc >= 3);
    assert!(!report.graph_metrics.top_hubs.is_empty());
    assert_eq!(report.graph_metrics.top_hubs[0].path, "wiki/concepts/A.md");

    // Copy compiled binary into TempDir and exercise sign_binary + verify_binary + tamper detection.
    let src_bin = Path::new(env!("CARGO_BIN_EXE_smart-vault"));
    let copy_bin = dir.path().join("smart-vault-copy");
    fs::copy(src_bin, &copy_bin).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&copy_bin, fs::Permissions::from_mode(0o755)).unwrap();

    let sign_rep = sign_binary(&copy_bin, None, true).unwrap();
    assert!(sign_rep.verified);
    let sidecar = PathBuf::from(sign_rep.sha256_file.unwrap());
    assert!(sidecar.exists());

    let verify_rep = verify_binary(&copy_bin, Some(&sign_rep.sha256)).unwrap();
    assert_eq!(verify_rep.sha256_match, Some(true));
    assert!(verify_rep.codesign_valid);

    let bad_verify = verify_binary(&copy_bin, Some(&"0".repeat(64)));
    assert!(bad_verify.is_err());
}
