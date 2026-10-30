use cargo_rvst::{Cli, Commands, Output};
use clap::Parser;
#[test]
fn cargo_subcommand_invocation_and_globals() {
    let args = [
        "cargo-rvst",
        "rvst",
        "--output",
        "json",
        "--non-interactive",
        "new",
        "my-gain",
        "--template",
        "gain",
    ]
    .map(Into::into);
    let cli = Cli::try_parse_from(cargo_rvst::normalize_args(args)).unwrap();
    assert_eq!(cli.output, Output::Json);
    assert!(cli.non_interactive);
    assert!(matches!(cli.command, Some(Commands::New { .. })));
}
#[test]
fn no_ambiguous_duplicate_inputs() {
    assert_eq!(
        cargo_rvst::input_pairs(vec!["name=a".into(), "name=b".into()])
            .unwrap_err()
            .code,
        "duplicate_input"
    );
}
#[test]
fn parser_enforces_vendor_source() {
    assert!(Cli::try_parse_from(["cargo-rvst", "new", "demo", "--vendor-sdk"]).is_err());
}
#[test]
fn every_documented_command_parses() {
    for args in [
        vec!["doctor"],
        vec!["dev"],
        vec!["test"],
        vec!["validate"],
        vec!["debug"],
        vec!["inspect", "--open", "--instance", "abc"],
        vec!["replay", "recording.json"],
        vec!["package", "--plugin-format", "clap"],
        vec!["schema"],
        vec!["studio"],
        vec!["templates", "list"],
        vec!["templates", "inspect", "gain"],
        vec!["templates", "check", "gain"],
        vec!["templates", "add", "local", "/tmp/template"],
    ] {
        let mut all = vec!["cargo-rvst"];
        all.extend(args);
        assert!(Cli::try_parse_from(all.clone()).is_ok(), "{all:?}");
    }
}
#[test]
fn json_error_is_single_machine_record() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .args(["--output", "json", "new", "demo", "--bad"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "usage");
    assert!(out.stderr.is_empty());
}
#[test]
fn showcase_dry_run_does_not_create_files() {
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("rvst-debug");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "showcase",
            "--install",
            "--run",
            "--open",
            "--path",
        ])
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["event"], "showcase_plan");
    assert!(!dest.exists());
}
#[test]
fn failed_task_keeps_json_as_one_record() {
    let parent = tempfile::tempdir().unwrap();
    std::fs::write(
        parent.path().join("Cargo.toml"),
        "deliberately invalid manifest",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .args(["--output", "json", "--project"])
        .arg(parent.path())
        .arg("test")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["event"], "error");
    assert!(
        value["error"]["details"]["stderr"]
            .as_str()
            .unwrap()
            .contains("error")
    );
}

#[test]
fn accepted_contract_flags_and_aliases_parse() {
    for args in [
        vec![
            "--format",
            "json",
            "new",
            "demo",
            "--vendor",
            "Example",
            "--id",
            "com.example.demo",
        ],
        vec![
            "templates",
            "add",
            "team",
            "--git",
            "https://example.com/template.git",
            "--rev",
            "0123456789012345678901234567890123456789",
        ],
        vec!["templates", "check", "--path", "/tmp/example"],
        vec![
            "dev",
            "--web",
            "--open",
            "--manifest-path",
            "/tmp/example/Cargo.toml",
            "--features",
            "one,two",
        ],
        vec![
            "package",
            "--plugin-format",
            "clap",
            "--target",
            "aarch64-apple-darwin",
            "--profile",
            "release",
        ],
    ] {
        let mut all = vec!["cargo-rvst"];
        all.extend(args);
        assert!(Cli::try_parse_from(all.clone()).is_ok(), "{all:?}");
    }
}
#[test]
fn git_registration_records_exact_pin_without_mutating_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("template");
    std::fs::create_dir_all(source.join("files")).unwrap();
    std::fs::write(source.join("README.md"), "Template fixture").unwrap();
    std::fs::write(source.join("rvst-template.toml"),"schema_version=1\nversion='1.0.0'\nsdk_version='^0.1'\nid='test-template'\nname='Test'\ndescription='Fixture'\n").unwrap();
    std::fs::write(source.join("files/example.txt"), "Fixture").unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=RVST tests",
            "-c",
            "user.email=rvst@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Test template\n\nCo-Author: Charlie's Agent",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&source)
                .status()
                .unwrap()
                .success()
        );
    }
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&source)
        .output()
        .unwrap();
    let sha = String::from_utf8(head.stdout).unwrap();
    let sha = sha.trim();
    let registry = temp.path().join("registry");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_HOME", &registry)
        .args(["--format", "json", "templates", "add", "team", "--git"])
        .arg(&source)
        .args(["--rev", sha])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["revision"], sha);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_HOME", &registry)
        .args(["--format", "json", "new"])
        .arg(temp.path().join("project"))
        .args(["--template", "team", "--no-git"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let provenance: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("project/rvst-project.json")).unwrap(),
    )
    .unwrap();
    assert!(provenance["template"].as_str().unwrap().ends_with(sha));
    assert_eq!(
        std::fs::read(source.join("files/example.txt")).unwrap(),
        b"Fixture"
    );
}

#[test]
fn paths_with_spaces_and_dry_run_create_no_cache_or_project() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent with spaces");
    std::fs::create_dir(&parent).unwrap();
    let dest = parent.join("spaced-plugin");
    let registry = temp.path().join("must-not-exist");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_HOME", &registry)
        .args(["--format", "json", "--dry-run", "new"])
        .arg(&dest)
        .args(["--template", "gain"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!dest.exists());
    assert!(!registry.exists());
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_HOME", &registry)
        .args(["--format", "json", "new"])
        .arg(&dest)
        .args(["--template", "gain", "--no-git"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(dest.join("Cargo.toml").exists());
    assert!(!registry.exists());
}

#[cfg(unix)]
#[test]
fn jsonl_ctrl_c_cancels_child_and_emits_parseable_records() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let cargo = bin.join("cargo");
    std::fs::write(&cargo, "#!/bin/sh\nsleep 30 &\necho grandchild=$!\nwait\n").unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let oldpath = std::env::var("PATH").unwrap_or_default();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("PATH", format!("{}:{oldpath}", bin.display()))
        .args(["--format", "jsonl", "--project"])
        .arg(temp.path())
        .arg("test")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut values = vec![];
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            tx.send(value.clone()).ok();
            values.push(value);
        }
        values
    });
    let mut saw_child = false;
    for _ in 0..5 {
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if event["event"] == "log"
            && event["data"]["line"]
                .as_str()
                .unwrap_or("")
                .starts_with("grandchild=")
        {
            saw_child = true;
            break;
        }
    }
    assert!(saw_child);
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    let values = reader.join().unwrap();
    assert_eq!(values.first().unwrap()["event"], "task_started");
    assert_eq!(values.last().unwrap()["error"]["code"], "cancelled");
}

#[test]
fn actual_result_and_error_records_match_published_schema_shape() {
    let contract = cargo_rvst::schema();
    let schema = &contract["json_schema"];
    assert_eq!(schema["$id"], "urn:rvst:schema:result:v1");
    assert!(contract["recording_schema"]["properties"].is_object());
    for args in [
        vec!["--format", "json", "schema"],
        vec!["--format", "json", "--unknown"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
            .args(args)
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for field in schema["required"].as_array().unwrap() {
            assert!(value.get(field.as_str().unwrap()).is_some());
        }
        for key in value.as_object().unwrap().keys() {
            assert!(
                schema["properties"].get(key).is_some(),
                "Unknown result field {key}"
            );
        }
        if value["ok"] == false {
            for key in value["error"].as_object().unwrap().keys() {
                assert!(
                    schema["properties"]["error"]["properties"]
                        .get(key)
                        .is_some()
                );
            }
            assert!(value.get("data").is_none());
        } else {
            assert!(value.get("error").is_none());
        }
    }
}

#[test]
fn inspect_json_contains_live_state_and_metrics_without_capability() {
    let directory = tempfile::tempdir().unwrap();
    let (session, _rt) =
        rvst_devtools::DebugSession::start_in("org.test.cli", "CLI fixture", directory.path())
            .unwrap();
    session
        .register_state_provider("fixture", || serde_json::json!({"value": 42}))
        .unwrap();
    session.metrics().record_callback(2500, 1000);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_SESSION_DIR", directory.path())
        .args([
            "inspect",
            "--format",
            "json",
            "--instance",
            &session.info().id,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["data"][0]["snapshot"]["live"]["fixture"]["value"],
        42
    );
    assert_eq!(result["data"][0]["metrics"]["callbacks"], 1);
    assert_eq!(result["data"][0]["metrics"]["deadline_misses"], 1);
    assert!(
        !String::from_utf8_lossy(&output.stdout)
            .contains(session.info().url.split_once('#').unwrap().1)
    );
    assert!(output.stderr.is_empty());
    let missing = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("RVST_SESSION_DIR", directory.path())
        .args(["inspect", "--format", "json", "--instance", "missing"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}

#[cfg(unix)]
fn doctor_with_failed_rustc(format: &str) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let rustc = temp.path().join("rustc");
    std::fs::write(
        &rustc,
        "#!/bin/sh\nprintf 'toolchain unavailable for this project\\n' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("PATH", temp.path())
        .args(["doctor", "--format", format, "--project"])
        .arg(temp.path())
        .output()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn doctor_human_failure_identifies_tools_and_remedies() {
    let output = doctor_with_failed_rustc("human");
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("rustc"), "{stderr}");
    assert!(
        stderr.contains("toolchain unavailable for this project"),
        "{stderr}"
    );
    assert!(stderr.contains("rustup component add rustfmt"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn doctor_machine_failure_preserves_tool_stderr_and_envelope() {
    for format in ["json", "jsonl"] {
        let output = doctor_with_failed_rustc(format);
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["event"], "error");
        assert_eq!(value["error"]["code"], "doctor_failed");
        let rustc = value["error"]["details"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == "rustc")
            .unwrap();
        assert_eq!(rustc["ok"], false);
        assert_eq!(rustc["detail"], "toolchain unavailable for this project");
    }
}

#[cfg(unix)]
#[test]
fn doctor_checks_wasm_target_in_selected_project() {
    use std::os::unix::fs::PermissionsExt;

    for (caller_has_wasm, project_has_wasm) in [(true, false), (false, true)] {
        let temp = tempfile::tempdir().unwrap();
        let caller = temp.path().join("caller");
        let project = temp.path().join("project");
        let bin = temp.path().join("bin");
        for path in [&caller, &project, &bin] {
            std::fs::create_dir(path).unwrap();
        }
        for (path, has_wasm) in [(&caller, caller_has_wasm), (&project, project_has_wasm)] {
            if has_wasm {
                std::fs::write(path.join("wasm-installed"), "").unwrap();
            }
        }
        for name in ["rustc", "cargo", "rustfmt", "git", "rustup", "wasm-bindgen"] {
            let program = bin.join(name);
            let script = if name == "rustup" {
                "#!/bin/sh\nif [ \"$1\" = target ]; then\n  if [ -f wasm-installed ]; then printf 'wasm32-unknown-unknown\\n'; fi\nelse\n  printf 'test-tool 1.0\\n'\nfi\n"
            } else {
                "#!/bin/sh\nprintf 'test-tool 1.0\\n'\n"
            };
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
            .env("PATH", &bin)
            .current_dir(&caller)
            .args(["doctor", "--format", "json", "--project"])
            .arg(&project)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let wasm = value["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == "wasm32-unknown-unknown")
            .unwrap();
        assert_eq!(wasm["ok"], project_has_wasm);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn doctor_machine_output_captures_optional_library_probe_diagnostics() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    for name in [
        "rustc",
        "cargo",
        "rustfmt",
        "git",
        "rustup",
        "wasm-bindgen",
        "pkg-config",
    ] {
        let program = temp.path().join(name);
        let script = if name == "pkg-config" {
            "#!/bin/sh\nprintf 'library probe stdout\\n'\nprintf 'library probe stderr\\n' >&2\nexit 1\n"
        } else {
            "#!/bin/sh\nprintf 'test-tool 1.0\\n'\n"
        };
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rvst"))
        .env("PATH", temp.path())
        .args(["doctor", "--format", "json", "--project"])
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let libraries = value["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "Linux audio/window libraries")
        .unwrap();
    assert_eq!(libraries["ok"], false);
}
