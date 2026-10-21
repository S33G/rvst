//! Build native plugins using Cargo's machine-readable artifact paths.
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Vst3,
    Clap,
}
/// Build a cdylib package and create a local bundle under target/bundled.
/// Never installs or publishes the result into a system plugin directory.
pub fn bundle(workspace: &Path, package: &str, format: Format, release: bool) -> Result<PathBuf> {
    bundle_with_features(workspace, package, format, release, &[])
}
#[derive(Debug, Default, Clone)]
pub struct BundleOptions {
    pub release: bool,
    pub target: Option<String>,
    pub profile: Option<String>,
    pub features: Vec<String>,
}
pub fn bundle_with_features(
    workspace: &Path,
    package: &str,
    format: Format,
    release: bool,
    features: &[&str],
) -> Result<PathBuf> {
    bundle_with_options(
        workspace,
        package,
        format,
        &BundleOptions {
            release,
            features: features.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        },
    )
}
pub fn bundle_with_options(
    workspace: &Path,
    package: &str,
    format: Format,
    options: &BundleOptions,
) -> Result<PathBuf> {
    if options.release && options.profile.is_some() {
        return Err("use either --release or --profile".into());
    }
    let metadata = Command::new("cargo")
        .current_dir(workspace)
        .args(["metadata", "--no-deps", "--format-version=1", "--locked"])
        .output()?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr)
        )
        .into());
    }
    let meta: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let packages = meta["packages"]
        .as_array()
        .ok_or("Cargo returned no packages")?;
    let selected: Vec<_> = packages
        .iter()
        .filter(|p| {
            p["name"] == package
                || p["id"] == package
                || p["name"]
                    .as_str()
                    .zip(p["version"].as_str())
                    .is_some_and(|(name, version)| package == format!("{name}@{version}"))
        })
        .collect();
    if selected.len() != 1 {
        return Err(format!(
            "expected one workspace package matching {package}, found {}",
            selected.len()
        )
        .into());
    }
    let package_id = selected[0]["id"]
        .as_str()
        .ok_or("Cargo returned no package identity")?;
    let mut cmd = Command::new("cargo");
    cmd.current_dir(workspace).args([
        "build",
        "--locked",
        "--package",
        package,
        "--message-format=json-render-diagnostics",
    ]);
    if options.release {
        cmd.arg("--release");
    }
    if let Some(profile) = &options.profile {
        cmd.args(["--profile", profile]);
    }
    if let Some(target) = &options.target {
        cmd.args(["--target", target]);
    }
    if !options.features.is_empty() {
        cmd.args(["--features", &options.features.join(",")]);
    }
    let output = cmd.output()?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(s) = v.pointer("/message/rendered").and_then(|s| s.as_str())
        {
            eprint!("{s}")
        }
    }
    if !output.status.success() {
        return Err(format!(
            "cargo build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let artifact = find_artifact(&output.stdout, package_id)?;
    let target = PathBuf::from(
        meta["target_directory"]
            .as_str()
            .ok_or("missing cargo target directory")?,
    );
    let name = meta
        .pointer("/metadata/rvst/bundle_name")
        .and_then(|n| n.as_str())
        .unwrap_or(package);
    // The binary is authoritative: Cargo may select its target from a config
    // file, CARGO_BUILD_TARGET, or a custom target JSON instead of our flags.
    let (os, arch) = artifact_platform(&artifact)?;
    package_artifact(&artifact, &target.join("bundled"), name, format, os, arch)
}
/// Select by Cargo's exact package identity. Target names may differ from the
/// package name, and dependencies may have a target with the same name.
pub fn find_artifact(json_lines: &[u8], package_id: &str) -> Result<PathBuf> {
    let mut found = None;
    for line in String::from_utf8_lossy(json_lines).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-artifact"
            || v["package_id"] != package_id
            || !v["target"]["crate_types"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "cdylib"))
        {
            continue;
        }
        if let Some(files) = v["filenames"].as_array() {
            for f in files {
                if let Some(s) = f.as_str() {
                    let p = PathBuf::from(s);
                    if matches!(
                        p.extension().and_then(|x| x.to_str()),
                        Some("dylib" | "so" | "dll")
                    ) {
                        if found.as_ref().is_some_and(|existing| existing != &p) {
                            return Err(format!(
                                "Cargo reported multiple cdylib artifacts for {package_id}"
                            )
                            .into());
                        }
                        found = Some(p);
                    }
                }
            }
        }
    }
    found.ok_or_else(|| format!("Cargo reported no cdylib artifact for {package_id}").into())
}
fn artifact_platform(path: &Path) -> Result<(&'static str, &'static str)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)?;
    let (os, machine) = if &header[..4] == b"\x7fELF" {
        let machine = match header[5] {
            1 => u16::from_le_bytes([header[18], header[19]]),
            2 => u16::from_be_bytes([header[18], header[19]]),
            _ => return Err("unsupported ELF byte order".into()),
        };
        ("linux", u32::from(machine))
    } else if &header[..2] == b"MZ" {
        let offset = u32::from_le_bytes(header[60..64].try_into()?);
        file.seek(SeekFrom::Start(u64::from(offset)))?;
        let mut pe = [0u8; 6];
        file.read_exact(&mut pe)?;
        if &pe[..4] != b"PE\0\0" {
            return Err("invalid PE signature".into());
        }
        ("windows", u32::from(u16::from_le_bytes([pe[4], pe[5]])))
    } else {
        let magic = u32::from_le_bytes(header[..4].try_into()?);
        let machine = match magic {
            0xfeedface | 0xfeedfacf => u32::from_le_bytes(header[4..8].try_into()?),
            0xcefaedfe | 0xcffaedfe => u32::from_be_bytes(header[4..8].try_into()?),
            _ => {
                return Err(
                    "unsupported plugin binary format (expected ELF, PE, or thin Mach-O)".into(),
                );
            }
        };
        ("macos", machine)
    };
    let arch = match (os, machine) {
        ("linux", 62) | ("windows", 0x8664) | ("macos", 0x0100_0007) => "x86_64",
        ("linux", 183) | ("windows", 0xaa64) | ("macos", 0x0100_000c) => "aarch64",
        ("linux", 3) | ("windows", 0x014c) | ("macos", 7) => "x86",
        _ => return Err(format!("unsupported {os} plugin architecture {machine:#x}").into()),
    };
    Ok((os, arch))
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn package_artifact(
    artifact: &Path,
    out: &Path,
    name: &str,
    format: Format,
    os: &str,
    arch: &str,
) -> Result<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err("invalid bundle name".into());
    }
    fs::create_dir_all(out)?;
    let path = out.join(format!(
        "{name}.{}",
        if format == Format::Vst3 {
            "vst3"
        } else {
            "clap"
        }
    ));
    if format == Format::Clap && os != "macos" {
        fs::copy(artifact, &path)?;
        return Ok(path);
    }
    let binary = match os {
        "macos" => path.join("Contents/MacOS").join(name),
        "windows" => path
            .join(format!(
                "Contents/{}-win",
                match arch {
                    "x86_64" => "x86_64",
                    "aarch64" => "arm64",
                    "x86" => "x86",
                    _ => return Err("unsupported Windows architecture".into()),
                }
            ))
            .join(format!("{name}.vst3")),
        "linux" => path
            .join(format!(
                "Contents/{}-linux",
                match arch {
                    "x86_64" => "x86_64",
                    "aarch64" => "aarch64",
                    _ => return Err("unsupported Linux architecture".into()),
                }
            ))
            .join(format!("{name}.so")),
        _ => return Err("unsupported target OS".into()),
    };
    fs::create_dir_all(binary.parent().ok_or("invalid bundle path")?)?;
    fs::copy(artifact, &binary)?;
    if os == "macos" {
        let n = xml(name);
        fs::write(
            path.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>{n}</string><key>CFBundleIdentifier</key><string>org.rvst.{n}</string><key>CFBundleName</key><string>{n}</string><key>CFBundlePackageType</key><string>BNDL</string><key>CFBundleVersion</key><string>1.0.0</string><key>CFBundleShortVersionString</key><string>1.0.0</string></dict></plist>"#
            ),
        )?;
        fs::write(path.join("Contents/PkgInfo"), b"BNDL????")?;
    }
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifact_comes_from_cargo_not_guessed_target_path() {
        let data=br#"{"reason":"compiler-artifact","package_id":"my-gain","target":{"name":"my_gain","crate_types":["cdylib"]},"filenames":["/custom/target/libmy_gain.dylib","/custom/target/my_gain.d"]}"#;
        assert_eq!(
            find_artifact(data, "my-gain").unwrap(),
            PathBuf::from("/custom/target/libmy_gain.dylib")
        );
        assert!(find_artifact(data, "other").is_err());
    }
    #[test]
    fn rejects_traversal() {
        assert!(
            package_artifact(
                Path::new("missing"),
                Path::new("/tmp"),
                "../evil",
                Format::Clap,
                "linux",
                "x86_64"
            )
            .is_err()
        );
    }
}

/// Explicitly install an already built plugin into this user's plugin directory.
/// The destination is staged first; only the matching plugin name is replaced.
pub fn install(bundle: &Path) -> Result<PathBuf> {
    let extension = bundle
        .extension()
        .and_then(|s| s.to_str())
        .ok_or("missing plugin extension")?;
    if !matches!(extension, "vst3" | "clap") {
        return Err("only .vst3 and .clap plugins can be installed".into());
    }
    let name = bundle.file_name().ok_or("missing plugin name")?;
    #[cfg(target_os = "macos")]
    let root = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?)
        .join("Library/Audio/Plug-Ins")
        .join(if extension == "vst3" { "VST3" } else { "CLAP" });
    #[cfg(target_os = "linux")]
    let root = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(
        if extension == "vst3" {
            ".vst3"
        } else {
            ".clap"
        },
    );
    #[cfg(target_os = "windows")]
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?)
        .join("Programs/Common")
        .join(if extension == "vst3" { "VST3" } else { "CLAP" });
    install_into(bundle, &root.join(name))
}
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Err("plugin bundle may not contain symbolic links".into());
    }
    if meta.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        fs::copy(source, destination)?;
    } else {
        return Err("unsupported plugin bundle entry".into());
    }
    Ok(())
}
fn remove_entry(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
pub fn install_into(bundle: &Path, destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or("missing plugin destination parent")?;
    fs::create_dir_all(parent)?;
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let stage = parent.join(format!(".rvst-stage-{}-{tag}", std::process::id()));
    let backup = parent.join(format!(".rvst-backup-{}-{tag}", std::process::id()));
    if let Err(e) = copy_tree(bundle, &stage) {
        if stage.exists() {
            let _ = remove_entry(&stage);
        }
        return Err(e);
    }
    let existed = fs::symlink_metadata(destination).is_ok();
    if existed && let Err(e) = fs::rename(destination, &backup) {
        let _ = remove_entry(&stage);
        return Err(e.into());
    }
    if let Err(e) = fs::rename(&stage, destination) {
        if existed {
            let _ = fs::rename(&backup, destination);
        }
        let _ = remove_entry(&stage);
        return Err(e.into());
    }
    if existed {
        remove_entry(&backup)?;
    }
    Ok(destination.to_path_buf())
}

pub mod web;

#[cfg(test)]
mod review_regressions {
    use super::*;
    #[test]
    fn artifact_selection_uses_package_identity_and_custom_library_name() {
        let data = br#"{"reason":"compiler-artifact","package_id":"path+file:///project#review-plugin@0.1.0","target":{"name":"custom_library","crate_types":["cdylib"]},"filenames":["/target/libcustom_library.dylib"]}
{"reason":"compiler-artifact","package_id":"path+file:///other#other@0.1.0","target":{"name":"review_plugin","crate_types":["cdylib"]},"filenames":["/target/libwrong.dylib"]}"#;
        assert_eq!(
            find_artifact(data, "path+file:///project#review-plugin@0.1.0").unwrap(),
            PathBuf::from("/target/libcustom_library.dylib")
        );
    }
    #[cfg(unix)]
    #[test]
    fn configured_target_is_packaged_using_binary_platform() {
        use std::os::unix::fs::PermissionsExt;
        if let Some(root) = std::env::var_os("RVST_REVIEW_BUILD_FIXTURE") {
            let root = PathBuf::from(root);
            let bundle = bundle(&root, "review-plugin", Format::Vst3, false).unwrap();
            assert!(
                bundle
                    .join("Contents/x86_64-linux/review-plugin.so")
                    .is_file()
            );
            assert!(!bundle.join("Contents/MacOS").exists());
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "rvst-build-review-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        let artifact = root.join("libreview_plugin.so");
        let mut elf = [0u8; 64];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        fs::write(&artifact, elf).unwrap();
        let metadata = serde_json::json!({"target_directory":root.join("target"), "packages":[{"name":"review-plugin","id":"path+file:///fixture#review-plugin@0.1.0"}]});
        let built = serde_json::json!({"reason":"compiler-artifact","package_id":"path+file:///fixture#review-plugin@0.1.0","target":{"name":"review_plugin","crate_types":["cdylib"]},"filenames":[artifact]});
        fs::write(root.join("metadata.json"), metadata.to_string()).unwrap();
        fs::write(root.join("built.json"), built.to_string()).unwrap();
        fs::write(root.join("bin/cargo"), "#!/bin/sh\nif [ \"$1\" = metadata ]; then cat metadata.json; else cat built.json; fi\n").unwrap();
        fs::set_permissions(root.join("bin/cargo"), fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "review_regressions::configured_target_is_packaged_using_binary_platform",
                "--nocapture",
            ])
            .env("RVST_REVIEW_BUILD_FIXTURE", &root)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("CARGO_BUILD_TARGET", "x86_64-unknown-linux-gnu")
            .status()
            .unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(status.success());
    }
}
