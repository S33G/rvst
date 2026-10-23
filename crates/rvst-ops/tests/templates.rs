use rvst_ops::{Generator, NewRequest, Template, TemplateSource};
use std::{collections::BTreeMap, fs};

#[test]
fn builtin_generation_is_independent_and_records_provenance() {
    let parent = tempfile::tempdir().unwrap();
    fs::write(
        parent.path().join("Cargo.toml"),
        "[workspace]\nmembers=[]\n",
    )
    .unwrap();
    let dest = parent.path().join("bright-gain");
    let report = Generator::new()
        .generate(NewRequest {
            template: TemplateSource::Builtin("gain".into()),
            destination: dest.clone(),
            inputs: BTreeMap::new(),
            sdk_path: None,
            vendor_sdk: false,
            git: true,
        })
        .unwrap();
    assert!(report.files > 8);
    assert!(dest.join(".git").is_dir());
    assert!(
        fs::read_to_string(dest.join("Cargo.toml"))
            .unwrap()
            .contains("[workspace]")
    );
    assert!(
        fs::read_to_string(dest.join("crates/plugin-core/Cargo.toml"))
            .unwrap()
            .contains("rvst-core = \"0.1.0\"")
    );
    assert!(dest.join("rvst-project.json").is_file());
}
#[test]
fn rejects_existing_destination_without_changing_it() {
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("existing");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("keep"), "untouched").unwrap();
    let result = Generator::new().generate(NewRequest {
        template: TemplateSource::Builtin("gain".into()),
        destination: dest.clone(),
        inputs: BTreeMap::new(),
        sdk_path: None,
        vendor_sdk: false,
        git: false,
    });
    assert_eq!(result.unwrap_err().code, "destination_exists");
    assert_eq!(fs::read_to_string(dest.join("keep")).unwrap(), "untouched");
}
#[test]
fn rejects_unknown_and_invalid_inputs() {
    let t = Template::builtin("gain").unwrap();
    assert_eq!(
        t.resolve_inputs(BTreeMap::from([("surprise".into(), "x".into())]), "demo")
            .unwrap_err()
            .code,
        "unknown_input"
    );
    assert_eq!(
        t.resolve_inputs(
            BTreeMap::from([("crate_name".into(), "../escape".into())]),
            "demo"
        )
        .unwrap_err()
        .code,
        "invalid_input"
    );
}
#[test]
fn template_validation_rejects_symlinks_and_parent_components() {
    let p = tempfile::tempdir().unwrap();
    fs::create_dir(p.path().join("files")).unwrap();
    fs::write(p.path().join("README.md"), "example").unwrap();
    fs::write(
        p.path().join("rvst-template.toml"),
        "schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", p.path().join("files/leak")).unwrap();
        assert_eq!(
            Template::load(p.path()).unwrap_err().code,
            "unsafe_template"
        );
    }
}
#[test]
fn failed_liquid_render_leaves_no_destination() {
    let p = tempfile::tempdir().unwrap();
    let t = p.path().join("template");
    fs::create_dir_all(t.join("files")).unwrap();
    fs::write(t.join("README.md"), "example").unwrap();
    fs::write(
        t.join("rvst-template.toml"),
        "schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n",
    )
    .unwrap();
    fs::write(t.join("files/broken.txt.liquid"), "{{ missing_variable }}").unwrap();
    let dest = p.path().join("project");
    let result = Generator::new().generate(NewRequest {
        template: TemplateSource::Local(t),
        destination: dest.clone(),
        inputs: BTreeMap::new(),
        sdk_path: None,
        vendor_sdk: false,
        git: false,
    });
    assert!(result.is_err());
    assert!(!dest.exists());
    assert_eq!(fs::read_dir(p.path()).unwrap().count(), 1);
}

#[test]
fn all_embedded_templates_have_renderable_files() {
    for name in ["gain", "instrument", "midi", "debug"] {
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join(format!("example-{name}"));
        let report = Generator::new()
            .generate(NewRequest {
                template: TemplateSource::Builtin(name.into()),
                destination: dest.clone(),
                inputs: BTreeMap::new(),
                sdk_path: None,
                vendor_sdk: false,
                git: false,
            })
            .unwrap();
        assert!(report.files > 15);
        assert!(
            fs::read_to_string(dest.join("crates/plugin-web/src/lib.rs"))
                .unwrap()
                .contains("JsValue>")
        );
        assert!(
            !fs::read_to_string(dest.join("crates/plugin-core/src/lib.rs"))
                .unwrap()
                .contains("{{")
        );
    }
}

#[test]
fn malicious_rendered_path_is_rejected_atomically() {
    let parent = tempfile::tempdir().unwrap();
    let template = parent.path().join("template");
    fs::create_dir_all(template.join("files")).unwrap();
    fs::write(template.join("README.md"), "example").unwrap();
    fs::write(template.join("rvst-template.toml"),"schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n[inputs.filename]\ntype='string'\ndescription='name'\nrequired=true\n").unwrap();
    fs::write(template.join("files/{{ filename }}.liquid"), "data").unwrap();
    let dest = parent.path().join("project");
    let result = Generator::new().generate(NewRequest {
        template: TemplateSource::Local(template),
        destination: dest.clone(),
        inputs: BTreeMap::from([("filename".into(), "../escape".into())]),
        sdk_path: None,
        vendor_sdk: false,
        git: false,
    });
    assert_eq!(result.unwrap_err().code, "unsafe_template");
    assert!(!dest.exists());
    assert!(!parent.path().join("escape").exists());
}

#[test]
fn showcase_plan_never_creates_or_adopts_an_unrelated_project() {
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("demo");
    let request = rvst_ops::ShowcaseRequest {
        path: dest.clone(),
        sdk_path: None,
        vendor_sdk: false,
        install: false,
    };
    assert!(rvst_ops::showcase_plan(&request).unwrap().create);
    assert!(!dest.exists());
    fs::create_dir(&dest).unwrap();
    assert_eq!(
        rvst_ops::showcase_plan(&request).unwrap_err().code,
        "unowned_project"
    );
}

#[cfg(unix)]
#[test]
fn cancellation_terminates_process_group_and_returns_130() {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let parent = tempfile::tempdir().unwrap();
    let token = rvst_ops::Cancellation::default();
    let worker_token = token.clone();
    let (tx, rx) = mpsc::channel();
    let path = parent.path().to_path_buf();
    let start = Instant::now();
    let worker = std::thread::spawn(move || {
        rvst_ops::run_task(
            &path,
            &[
                "/bin/sh".into(),
                "-c".into(),
                "sleep 30 & echo child=$!; wait".into(),
            ],
            &worker_token,
            Some(tx),
        )
        .unwrap()
    });
    let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let child = event
        .line
        .strip_prefix("child=")
        .unwrap()
        .trim()
        .to_string();
    token.cancel();
    let report = worker.join().unwrap();
    assert_eq!(report.exit_code, 130);
    assert!(start.elapsed() < Duration::from_secs(5));
    let status = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &child])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&status.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "grandchild still running: {state}"
    );
}

#[test]
fn rejects_git_aliases_and_nonportable_rendered_paths() {
    let parent = tempfile::tempdir().unwrap();
    let template = parent.path().join("template");
    fs::create_dir_all(template.join("files")).unwrap();
    fs::write(template.join("README.md"), "fixture").unwrap();
    fs::write(template.join("rvst-template.toml"), "schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n[inputs.filename]\ntype='string'\ndescription='name'\nrequired=true\n").unwrap();
    fs::write(template.join("files/{{ filename }}.liquid"), "fixture").unwrap();
    for name in [
        ".GIT/config",
        ".git./config",
        ".git /config",
        "git~1/config",
        "gi7eba~1/config",
        ".gi\u{200c}t/config",
        ".git::$INDEX_ALLOCATION/config",
        "folder\\.git\\config",
        "CON",
        "aux.txt",
        "COM1",
        "lpt\u{00b9}.txt",
        "safe. ",
        "file:stream",
    ] {
        let result = Generator::new().generate(NewRequest {
            template: TemplateSource::Local(template.clone()),
            destination: parent.path().join("project"),
            inputs: BTreeMap::from([("filename".into(), name.into())]),
            sdk_path: None,
            vendor_sdk: false,
            git: false,
        });
        assert_eq!(
            result.unwrap_err().code,
            "unsafe_template",
            "accepted {name:?}"
        );
        assert!(!parent.path().join("project").exists());
    }
    let result = Generator::new().generate(NewRequest {
        template: TemplateSource::Local(template),
        destination: parent.path().join("project with spaces"),
        inputs: BTreeMap::from([("filename".into(), "source files/read me.txt".into())]),
        sdk_path: None,
        vendor_sdk: false,
        git: false,
    });
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn validation_rejects_literal_casefold_git_directory() {
    let parent = tempfile::tempdir().unwrap();
    fs::create_dir_all(parent.path().join("files/.GIT")).unwrap();
    fs::write(parent.path().join("README.md"), "fixture").unwrap();
    fs::write(
        parent.path().join("rvst-template.toml"),
        "schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n",
    )
    .unwrap();
    fs::write(parent.path().join("files/.GIT/config"), "[core]\n").unwrap();
    assert_eq!(
        Template::load(parent.path())
            .unwrap()
            .check()
            .unwrap_err()
            .code,
        "unsafe_template"
    );
}

#[cfg(unix)]
#[test]
fn cancellation_remains_active_after_launcher_exits() {
    use std::time::{Duration, Instant};
    let parent = tempfile::tempdir().unwrap();
    let token = rvst_ops::Cancellation::default();
    let worker_token = token.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        worker_token.cancel();
    });
    let start = Instant::now();
    let report = rvst_ops::run_task(
        parent.path(),
        &["/bin/sh".into(), "-c".into(), "sleep 3 & echo ready".into()],
        &token,
        None,
    )
    .unwrap();
    canceller.join().unwrap();
    assert_eq!(report.exit_code, 130);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(windows)]
#[test]
fn cancellation_terminates_windows_descendants() {
    use std::{sync::mpsc, time::Duration};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().to_path_buf();
    let token = rvst_ops::Cancellation::default();
    let worker_token = token.clone();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        rvst_ops::run_task(&path, &[
        "powershell.exe".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(),
        "$child = Start-Process powershell.exe -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -NoNewWindow -PassThru; Write-Output ('descendant=' + $child.Id); Start-Sleep -Seconds 30".into(),
    ], &worker_token, Some(tx)).unwrap()
    });
    let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let pid: u32 = event
        .line
        .strip_prefix("descendant=")
        .unwrap()
        .parse()
        .unwrap();
    token.cancel();
    assert_eq!(worker.join().unwrap().exit_code, 130);
    // SAFETY: query the exact child PID reported by our fixture, closing any
    // handle returned. No process modification is performed by this assertion.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !process.is_null() {
            let mut exit = 259u32;
            let queried = GetExitCodeProcess(process, &mut exit);
            CloseHandle(process);
            assert_ne!(queried, 0);
            assert_ne!(exit, 259, "descendant is still active");
        }
    }
}

#[test]
fn generated_long_names_are_already_rustfmt_clean() {
    if std::process::Command::new("rustfmt")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let destination = parent
        .path()
        .join("unusually-long-generated-plugin-workspace-name");
    Generator::new().generate(NewRequest {
        template: TemplateSource::Builtin("debug".into()), destination: destination.clone(),
        inputs: BTreeMap::from([("plugin_name".into(), "A Very Long Plugin Display Name That Requires The Generated Rust Expressions To Wrap According To The Standard Formatter".into())]),
        sdk_path: None, vendor_sdk: false, git: false,
    }).unwrap();
    let output = std::process::Command::new("cargo")
        .current_dir(&destination)
        .args(["fmt", "--all", "--", "--check"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn missing_formatter_is_actionable_and_generation_is_atomic() {
    if std::env::var_os("RVST_TEST_MISSING_FORMATTER").is_none() {
        let empty_path = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "missing_formatter_is_actionable_and_generation_is_atomic",
                "--nocapture",
            ])
            .env("RVST_TEST_MISSING_FORMATTER", "1")
            .env("PATH", empty_path.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("missing-formatter");
    let error = Generator::new()
        .generate(NewRequest {
            template: TemplateSource::Builtin("gain".into()),
            destination: destination.clone(),
            inputs: BTreeMap::new(),
            sdk_path: None,
            vendor_sdk: false,
            git: false,
        })
        .unwrap_err();
    assert_eq!(error.code, "rustfmt_unavailable");
    assert_eq!(error.exit_code, 3);
    assert!(error.message.contains("rustup component add rustfmt"));
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}
#[test]
fn formatter_does_not_follow_modules_or_apply_external_configuration() {
    if std::env::var_os("RVST_TEST_FORMATTER_CONTEXT").is_none() {
        let context = tempfile::tempdir().unwrap();
        fs::write(context.path().join("rustfmt.toml"), "hard_tabs = true\n").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "formatter_does_not_follow_modules_or_apply_external_configuration",
                "--nocapture",
            ])
            .env("RVST_TEST_FORMATTER_CONTEXT", "1")
            .current_dir(context.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    if std::process::Command::new("rustfmt")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let template = parent.path().join("template");
    fs::create_dir_all(template.join("files/src")).unwrap();
    fs::write(template.join("README.md"), "example").unwrap();
    fs::write(
        template.join("rvst-template.toml"),
        "schema_version=1\nid='custom'\nname='Custom'\ndescription='test'\n",
    )
    .unwrap();
    let outside = parent.path().join("outside.rs");
    fs::write(&outside, "this is deliberately invalid Rust").unwrap();
    fs::write(parent.path().join("rustfmt.toml"), "hard_tabs = true\n").unwrap();
    fs::write(
        template.join("files/src/lib.rs"),
        format!(
            "#[path = {:?}] mod outside;\npub fn hello(){{println!(\"hello\");}}\n",
            outside.to_str().unwrap()
        ),
    )
    .unwrap();
    let destination = parent.path().join("project");
    Generator::new()
        .generate(NewRequest {
            template: TemplateSource::Local(template),
            destination: destination.clone(),
            inputs: BTreeMap::new(),
            sdk_path: None,
            vendor_sdk: false,
            git: false,
        })
        .unwrap();
    let output = fs::read_to_string(destination.join("src/lib.rs")).unwrap();
    assert!(output.contains("    println!(\"hello\");"));
    assert!(!output.contains('\t'));
    assert_eq!(
        fs::read_to_string(outside).unwrap(),
        "this is deliberately invalid Rust"
    );
}
