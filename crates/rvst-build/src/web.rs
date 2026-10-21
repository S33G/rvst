//! Build and serve the actual shared Rust UI with ordinary Cargo/wasm-bindgen.
use crate::Result;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
#[derive(Debug, Clone)]
pub struct WebOptions {
    pub package: String,
    pub release: bool,
    pub target: Option<String>,
    pub profile: Option<String>,
    pub features: Vec<String>,
    pub serve: bool,
    pub open: bool,
    pub port: u16,
}
impl Default for WebOptions {
    fn default() -> Self {
        Self {
            package: "plugin-web".into(),
            release: false,
            target: None,
            profile: None,
            features: Vec::new(),
            serve: true,
            open: false,
            port: 8787,
        }
    }
}
pub fn run(workspace: &Path, options: &WebOptions) -> Result<PathBuf> {
    // Validate before invoking tools or creating output directories.
    let mut cargo = build_command(workspace, options)?;
    let metadata = Command::new("cargo")
        .current_dir(workspace)
        .args(["metadata", "--format-version=1", "--locked"])
        .output()?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr)
        )
        .into());
    }
    let meta: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let package_id = web_package_id(&meta, &options.package)?;
    let version = meta["packages"]
        .as_array()
        .and_then(|p| p.iter().find(|p| p["name"] == "wasm-bindgen"))
        .and_then(|p| p["version"].as_str())
        .ok_or("web package needs wasm-bindgen")?;
    let tool=Command::new("wasm-bindgen").arg("--version").output().map_err(|_|format!("Install the matching web helper: cargo install wasm-bindgen-cli --version {version} --locked"))?;
    if !tool.status.success()
        || String::from_utf8_lossy(&tool.stdout)
            .split_whitespace()
            .nth(1)
            != Some(version)
    {
        return Err(format!("wasm-bindgen version mismatch; run: cargo install wasm-bindgen-cli --version {version} --locked --force").into());
    }
    let output = cargo.output()?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(s) = v.pointer("/message/rendered").and_then(|v| v.as_str())
        {
            eprint!("{s}");
        }
    }
    if !output.status.success() {
        return Err(format!("Web build failed. Install the Rust target with `rustup target add wasm32-unknown-unknown`. {}",String::from_utf8_lossy(&output.stderr)).into());
    }
    let wasm = find_wasm_artifact(&output.stdout, package_id)?;
    let target = PathBuf::from(
        meta["target_directory"]
            .as_str()
            .ok_or("Cargo returned no target directory")?,
    )
    .join("rvst-web");
    fs::create_dir_all(target.join("pkg"))?;
    let status = Command::new("wasm-bindgen")
        .arg(&wasm)
        .args([
            "--target",
            "web",
            "--keep-debug",
            "--out-name",
            "plugin_web",
            "--out-dir",
        ])
        .arg(target.join("pkg"))
        .status()?;
    if !status.success() {
        return Err("wasm-bindgen failed".into());
    }
    let index = workspace.join("web/index.html");
    if index.is_file() {
        fs::copy(index, target.join("index.html"))?;
    } else {
        fs::write(target.join("index.html"), include_str!("web-index.html"))?;
    }
    eprintln!("Built shared Rust UI: {}", target.display());
    if options.serve {
        serve(&target, options.port, options.open)?;
    }
    Ok(target)
}
fn web_package_id<'a>(meta: &'a serde_json::Value, requested: &str) -> Result<&'a str> {
    let packages = meta["packages"]
        .as_array()
        .ok_or("Cargo returned no packages")?;
    let members = meta["workspace_members"]
        .as_array()
        .ok_or("Cargo returned no workspace members")?;
    let selected: Vec<_> = packages
        .iter()
        .filter(|p| {
            members.contains(&p["id"])
                && (p["name"] == requested
                    || p["id"] == requested
                    || p["name"]
                        .as_str()
                        .zip(p["version"].as_str())
                        .is_some_and(|(name, version)| requested == format!("{name}@{version}")))
        })
        .collect();
    if selected.len() != 1 {
        return Err(format!(
            "expected one workspace package matching {requested}, found {}",
            selected.len()
        )
        .into());
    }
    selected[0]["id"]
        .as_str()
        .ok_or_else(|| "Cargo returned no package identity".into())
}
fn find_wasm_artifact(lines: &[u8], package_id: &str) -> Result<PathBuf> {
    let mut found = None;
    for line in String::from_utf8_lossy(lines).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-artifact" || value["package_id"] != package_id {
            continue;
        }
        if let Some(paths) = value["filenames"].as_array() {
            for path in paths
                .iter()
                .filter_map(|p| p.as_str())
                .filter(|p| p.ends_with(".wasm"))
            {
                let path = PathBuf::from(path);
                if found.as_ref().is_some_and(|previous| previous != &path) {
                    return Err(
                        format!("Cargo reported multiple WASM artifacts for {package_id}").into(),
                    );
                }
                found = Some(path);
            }
        }
    }
    found.ok_or_else(|| format!("Cargo reported no WASM artifact for {package_id}").into())
}
fn build_command(workspace: &Path, options: &WebOptions) -> Result<Command> {
    if options
        .target
        .as_deref()
        .is_some_and(|target| target != "wasm32-unknown-unknown")
    {
        return Err("web preview requires --target wasm32-unknown-unknown".into());
    }
    if options.release && options.profile.is_some() {
        return Err("use either --release or --profile".into());
    }
    if options.open && !options.serve {
        return Err("--open requires --serve".into());
    }
    let mut cargo = Command::new("cargo");
    cargo.current_dir(workspace).args([
        "build",
        "--locked",
        "--package",
        &options.package,
        "--target",
        "wasm32-unknown-unknown",
        "--message-format=json-render-diagnostics",
    ]);
    if options.release {
        cargo.arg("--release");
    }
    if let Some(profile) = &options.profile {
        cargo.args(["--profile", profile]);
    }
    if !options.features.is_empty() {
        cargo.args(["--features", &options.features.join(",")]);
    }
    Ok(cargo)
}
/// Serve loopback only. No directory listing or decoded traversal is accepted.
/// Owned child-process cancellation terminates this server through normal exit.
pub fn serve(root: &Path, port: u16, open: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    let url = format!("http://{}/", listener.local_addr()?);
    eprintln!("RVST shared UI: {url} (Ctrl-C to stop)");
    if open {
        open_browser(&url)?;
    }
    for stream in listener.incoming() {
        {
            let stream = stream?;
            if let Err(error) = respond(stream, &root) {
                eprintln!("preview request: {error}");
            }
        }
    }
    Ok(())
}
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    if !command.arg(url).status()?.success() {
        return Err("could not open browser".into());
    }
    Ok(())
}
fn request_path(root: &Path, path: &str) -> Option<PathBuf> {
    let path = path.split('?').next()?;
    if !path.starts_with('/') || path.contains(['%', '\\', '\0']) {
        return None;
    }
    let relative = if path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/')?
    };
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let target = root.join(relative).canonicalize().ok()?;
    if !target.starts_with(root) || !target.is_file() {
        return None;
    }
    Some(target)
}
fn respond(mut stream: TcpStream, root: &Path) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut data = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut data)?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&data[..n]);
        if request.len() > 8192 {
            return Ok(());
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut first = text.lines().next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default();
    let path = first.next().unwrap_or_default();
    let file = if method == "GET" {
        request_path(root, path)
    } else {
        None
    };
    let (status, kind, body) = match file {
        Some(file) => {
            let kind = match file.extension().and_then(|s| s.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "application/javascript",
                Some("wasm") => "application/wasm",
                Some("css") => "text/css",
                Some("png") => "image/png",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            ("200 OK", kind, fs::read(file)?)
        }
        None => ("404 Not Found", "text/plain", b"Not found".to_vec()),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_is_rejected() {
        let root = std::env::temp_dir();
        for p in [
            "/../Cargo.toml",
            "/%2e%2e/etc/passwd",
            "/a\\b",
            "/a/../../b",
        ] {
            assert!(request_path(&root, p).is_none());
        }
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    #[test]
    fn web_artifacts_use_package_identity_and_reject_ambiguity() {
        let data = br#"{"reason":"compiler-artifact","package_id":"path+file:///demo#web-package@0.1.0","target":{"name":"custom_library"},"filenames":["/target/custom_library.wasm"]}
{"reason":"compiler-artifact","package_id":"path+file:///dependency#other@0.1.0","target":{"name":"web_package"},"filenames":["/target/wrong.wasm"]}"#;
        assert_eq!(
            find_wasm_artifact(data, "path+file:///demo#web-package@0.1.0").unwrap(),
            PathBuf::from("/target/custom_library.wasm")
        );
        assert!(find_wasm_artifact(data, "missing").is_err());
        let ambiguous = br#"{"reason":"compiler-artifact","package_id":"web","filenames":["/a.wasm","/b.wasm"]}"#;
        assert!(find_wasm_artifact(ambiguous, "web").is_err());
    }
    #[test]
    fn web_package_must_match_one_workspace_member() {
        let meta = serde_json::json!({"workspace_members":["local"],"packages":[
            {"name":"web", "id":"local", "version":"0.1.0"},
            {"name":"web", "id":"dependency", "version":"0.1.0"}
        ]});
        assert_eq!(web_package_id(&meta, "web").unwrap(), "local");
        assert_eq!(web_package_id(&meta, "web@0.1.0").unwrap(), "local");
        assert!(web_package_id(&meta, "dependency").is_err());
    }
    #[test]
    fn web_build_forwards_package_profile_and_features() {
        let command = build_command(
            Path::new("/project"),
            &WebOptions {
                package: "custom-web".into(),
                profile: Some("profiling".into()),
                features: vec!["first".into(), "second".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--package", "custom-web"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--profile", "profiling"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--features", "first,second"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--target", "wasm32-unknown-unknown"])
        );
        assert!(!args.contains(&"--release"));
    }
    #[test]
    fn incompatible_web_options_fail_before_running_cargo() {
        let nonexistent = Path::new("/rvst-nonexistent-option-test");
        for (options, expected) in [
            (
                WebOptions {
                    target: Some("x86_64-unknown-linux-gnu".into()),
                    ..Default::default()
                },
                "wasm32-unknown-unknown",
            ),
            (
                WebOptions {
                    release: true,
                    profile: Some("release".into()),
                    ..Default::default()
                },
                "either",
            ),
            (
                WebOptions {
                    open: true,
                    serve: false,
                    ..Default::default()
                },
                "requires --serve",
            ),
        ] {
            assert!(
                run(nonexistent, &options)
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }
    }
}
