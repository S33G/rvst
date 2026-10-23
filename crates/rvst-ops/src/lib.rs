//! Shared, headless operations for the RVST command line and studio console.
use serde::{Deserialize, Serialize};
#[cfg(windows)]
mod windows_job;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, OpError>;
#[derive(Debug, Clone, Serialize)]
pub struct OpError {
    pub code: String,
    pub message: String,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl OpError {
    pub fn new(code: &str, message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            exit_code,
            details: None,
        }
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}
impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for OpError {}
impl From<std::io::Error> for OpError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io_error", e.to_string(), 1)
    }
}
fn invalid(message: impl Into<String>) -> OpError {
    OpError::new("invalid_template", message, 2)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateManifest {
    pub schema_version: u32,
    #[serde(default = "default_template_version")]
    pub version: String,
    #[serde(default = "default_sdk_requirement")]
    pub sdk_version: String,
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, InputSpec>,
}
#[derive(Debug, Clone)]
pub struct Template {
    pub manifest: TemplateManifest,
    pub root: PathBuf,
    pub provenance: String,
    embedded: Option<BTreeMap<PathBuf, Vec<u8>>>,
}
#[derive(Debug, Clone)]
pub enum TemplateSource {
    Builtin(String),
    Local(PathBuf),
    Registered(String),
}
#[derive(Debug, Clone)]
pub struct NewRequest {
    pub template: TemplateSource,
    pub destination: PathBuf,
    pub inputs: BTreeMap<String, String>,
    pub sdk_path: Option<PathBuf>,
    pub vendor_sdk: bool,
    pub git: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct GenerationReport {
    pub path: PathBuf,
    pub template: String,
    pub files: usize,
    pub sdk: String,
    pub git: bool,
    pub next_commands: Vec<String>,
}

pub fn builtins_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates")
}
pub fn registry_root() -> PathBuf {
    std::env::var_os("RVST_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config/rvst")
        })
}
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
// Validate portable destination names even when generating on a case-sensitive
// filesystem: projects and templates can later move to Windows or default APFS.
fn safe_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.contains(['\\', ':', '~', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(|c| {
            c.is_control()
                || matches!(c as u32,
            0x00ad | 0x034f | 0x061c | 0x115f..=0x1160 | 0x17b4..=0x17b5 |
            0x180b..=0x180f | 0x200b..=0x200f | 0x202a..=0x202e |
            0x2060..=0x206f | 0x3164 | 0xfe00..=0xfe0f | 0xfeff |
            0xffa0 | 0xfff0..=0xfff8 | 0x1bca0..=0x1bca3 |
            0x1d173..=0x1d17a | 0xe0000..=0xe0fff)
        })
    {
        return false;
    }
    let lower = name.to_lowercase();
    if lower == ".git" {
        return false;
    }
    let stem = lower.split('.').next().unwrap_or("").trim_end_matches(' ');
    if matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$") {
        return false;
    }
    !["com", "lpt"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}
fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|c| match c {
            Component::Normal(name) => name.to_str().is_some_and(safe_component),
            _ => false,
        })
}
fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(at)? {
            let entry = entry?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                return Err(OpError::new(
                    "unsafe_template",
                    format!("symlink forbidden: {}", path.display()),
                    2,
                ));
            }
            if meta.is_dir() {
                walk(base, &path, out)?;
            } else if meta.is_file() {
                out.push(path.strip_prefix(base).unwrap().to_path_buf());
            } else {
                return Err(invalid("Only regular files/directories are supported"));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}
fn default_template_version() -> String {
    "0.1.0".into()
}
fn default_sdk_requirement() -> String {
    "^0.1".into()
}
fn validate_manifest(manifest: &TemplateManifest) -> Result<()> {
    semver::Version::parse(&manifest.version)
        .map_err(|e| invalid(format!("Invalid template version: {e}")))?;
    let requirement = semver::VersionReq::parse(&manifest.sdk_version)
        .map_err(|e| invalid(format!("Invalid sdk_version: {e}")))?;
    if !requirement.matches(&semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()) {
        return Err(OpError::new(
            "incompatible_template",
            format!(
                "Template requires RVST {}, current version is {}",
                manifest.sdk_version,
                env!("CARGO_PKG_VERSION")
            ),
            3,
        ));
    }
    Ok(())
}
impl Template {
    pub fn load(root: &Path) -> Result<Self> {
        if fs::symlink_metadata(root)?.file_type().is_symlink() {
            return Err(OpError::new(
                "unsafe_template",
                "Template root cannot be a symlink",
                2,
            ));
        }
        let root = fs::canonicalize(root)?;
        let manifest: TemplateManifest =
            toml::from_str(&fs::read_to_string(root.join("rvst-template.toml"))?)
                .map_err(|e| invalid(e.to_string()))?;
        validate_manifest(&manifest)?;
        if manifest.schema_version != 1 || !valid_identifier(&manifest.id) {
            return Err(invalid(
                "schema_version must be 1 and id must be a lowercase identifier",
            ));
        }
        if !root.join("README.md").is_file() || !root.join("files").is_dir() {
            return Err(invalid("Template requires README.md and files/"));
        }
        collect_files(&root)?;
        for (name, spec) in &manifest.inputs {
            if !valid_identifier(name)
                || !matches!(
                    spec.kind.as_str(),
                    "string" | "identifier" | "bool" | "integer" | "number" | "choice"
                )
            {
                return Err(invalid(format!("Invalid input declaration: {name}")));
            }
            if spec.kind == "choice" && spec.choices.is_empty() {
                return Err(invalid(format!("Choice input {name} requires choices")));
            }
        }
        Ok(Self {
            manifest,
            provenance: format!("local:{}", root.display()),
            root,
            embedded: None,
        })
    }
    pub fn builtin(name: &str) -> Result<Self> {
        static BUILTINS: include_dir::Dir<'_> =
            include_dir::include_dir!("$CARGO_MANIFEST_DIR/templates");
        if !matches!(name, "gain" | "instrument" | "midi" | "debug") {
            return Err(OpError::new(
                "template_not_found",
                format!("Unknown builtin template: {name}"),
                2,
            ));
        }
        let dir = BUILTINS
            .get_dir(name)
            .ok_or_else(|| invalid("Missing bundled template"))?;
        let manifest: TemplateManifest = toml::from_str(
            dir.get_file(format!("{name}/rvst-template.toml"))
                .and_then(|f| f.contents_utf8())
                .ok_or_else(|| invalid("Missing template manifest"))?,
        )
        .map_err(|e| invalid(format!("{e}")))?;
        validate_manifest(&manifest)?;
        fn gather(dir: &include_dir::Dir<'_>, base: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for f in dir.files() {
                files.insert(
                    f.path().strip_prefix(base).unwrap().to_path_buf(),
                    f.contents().to_vec(),
                );
            }
            for child in dir.dirs() {
                gather(child, base, files);
            }
        }
        let mut files = BTreeMap::new();
        let files_dir = dir
            .get_dir(format!("{name}/files"))
            .ok_or_else(|| invalid("Missing bundled files"))?;
        gather(files_dir, files_dir.path(), &mut files);
        Ok(Self {
            manifest,
            root: PathBuf::from(format!("builtin:{name}")),
            provenance: format!("builtin:{name}@{}", env!("CARGO_PKG_VERSION")),
            embedded: Some(files),
        })
    }
    pub fn check(&self) -> Result<()> {
        let parser = liquid::ParserBuilder::with_stdlib()
            .build()
            .map_err(|e| invalid(e.to_string()))?;
        let files = if let Some(files) = &self.embedded {
            files.clone()
        } else {
            collect_files(&self.root.join("files"))?
                .into_iter()
                .map(|rel| {
                    let bytes = fs::read(self.root.join("files").join(&rel))?;
                    Ok((rel, bytes))
                })
                .collect::<Result<BTreeMap<_, _>>>()?
        };
        for (path, bytes) in files {
            if path.components().any(|p| match p {
                Component::Normal(name) => name.to_str().is_none_or(|name| {
                    // Liquid expressions are validated after rendering, when
                    // their actual destination spelling is known.
                    !name.contains("{{") && !name.contains("{%") && !safe_component(name)
                }),
                _ => true,
            }) {
                return Err(OpError::new(
                    "unsafe_template",
                    "Template contains a reserved or nonportable output path",
                    2,
                ));
            }
            parser
                .parse(&path.to_string_lossy())
                .map_err(|e| invalid(e.to_string()))?;
            if path.extension().is_some_and(|ext| ext == "liquid") {
                parser
                    .parse(std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?)
                    .map_err(|e| invalid(e.to_string()))?;
            }
        }
        Ok(())
    }
    pub fn resolve_inputs(
        &self,
        supplied: BTreeMap<String, String>,
        project_name: &str,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        for name in supplied.keys() {
            if !self.manifest.inputs.contains_key(name) {
                return Err(OpError::new(
                    "unknown_input",
                    format!("Unknown input: {name}"),
                    2,
                ));
            }
        }
        let mut values = BTreeMap::new();
        for (name, spec) in &self.manifest.inputs {
            let raw = supplied
                .get(name)
                .cloned()
                .or_else(|| {
                    if name == "crate_name" {
                        Some(project_name.into())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    spec.default.as_ref().map(|v| match v {
                        toml::Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    })
                });
            let Some(raw) = raw else {
                if spec.required {
                    return Err(OpError::new(
                        "missing_input",
                        format!("Required input: {name}"),
                        2,
                    ));
                }
                continue;
            };
            let fail = || {
                OpError::new(
                    "invalid_input",
                    format!("Invalid {} for {name}: {raw:?}", spec.kind),
                    2,
                )
            };
            let value = match spec.kind.as_str() {
                "identifier" => {
                    if !valid_identifier(&raw) {
                        return Err(fail());
                    }
                    serde_json::Value::String(raw)
                }
                "string" => serde_json::Value::String(raw),
                "choice" => {
                    if !spec.choices.contains(&raw) {
                        return Err(fail());
                    }
                    serde_json::Value::String(raw)
                }
                "bool" => serde_json::Value::Bool(raw.parse().map_err(|_| fail())?),
                "integer" => serde_json::json!(raw.parse::<i64>().map_err(|_| fail())?),
                "number" => {
                    let number = raw.parse::<f64>().map_err(|_| fail())?;
                    if !number.is_finite() {
                        return Err(fail());
                    }
                    serde_json::json!(number)
                }
                _ => return Err(fail()),
            };
            values.insert(name.clone(), value);
        }
        Ok(values)
    }
}

#[derive(Default)]
pub struct Generator;
impl Generator {
    pub fn new() -> Self {
        Self
    }
    pub fn generate(&self, req: NewRequest) -> Result<GenerationReport> {
        let absolute = if req.destination.is_absolute() {
            req.destination.clone()
        } else {
            std::env::current_dir()?.join(&req.destination)
        };
        if absolute.exists() || fs::symlink_metadata(&absolute).is_ok() {
            return Err(OpError::new(
                "destination_exists",
                format!("Destination exists: {}", absolute.display()),
                4,
            ));
        }
        let name = absolute
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                OpError::new(
                    "invalid_destination",
                    "Destination needs a UTF-8 directory name",
                    2,
                )
            })?;
        let template = match &req.template {
            TemplateSource::Builtin(n) => Template::builtin(n)?,
            TemplateSource::Local(p) => Template::load(p)?,
            TemplateSource::Registered(n) => load_registered(n)?,
        };
        template.check()?;
        let values = template.resolve_inputs(req.inputs, name)?;
        let parent = absolute
            .parent()
            .ok_or_else(|| OpError::new("invalid_destination", "Destination has no parent", 2))?;
        if !parent.is_dir() {
            return Err(OpError::new(
                "invalid_destination",
                "Destination parent must exist",
                2,
            ));
        }
        let temp = parent.join(format!(".rvst-{name}-{}-{}", std::process::id(), nonce()));
        fs::create_dir(&temp)?;
        let outcome = (|| {
            let mut globals = liquid::Object::new();
            for (key, value) in &values {
                globals.insert(
                    key.clone().into(),
                    liquid::model::to_value(value).map_err(|e| invalid(e.to_string()))?,
                );
            }
            globals.insert(
                "crate_ident".into(),
                liquid::model::Value::scalar(
                    values
                        .get("crate_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(name)
                        .replace('-', "_"),
                ),
            );
            for key in ["plugin_name", "vendor", "email", "url"] {
                let value = values.get(key).and_then(|v| v.as_str()).unwrap_or("");
                globals.insert(
                    format!("{key}_rust").into(),
                    liquid::model::Value::scalar(format!("{value:?}")),
                );
            }
            let plugin_id = values
                .get("plugin_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "org.rvst.{}",
                        values
                            .get("crate_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or(name)
                    )
                });
            globals.insert(
                "plugin_id".into(),
                liquid::model::Value::scalar(plugin_id.clone()),
            );
            globals.insert(
                "plugin_id_rust".into(),
                liquid::model::Value::scalar(format!("{plugin_id:?}")),
            );
            // Stable project-specific class ID; persisted in generated source, independent of install path.
            let mut hash = 0xcbf29ce484222325u64;
            for b in plugin_id.bytes().chain(template.manifest.id.bytes()) {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
            }
            let bytes = [hash.to_be_bytes(), hash.rotate_left(29).to_be_bytes()].concat();
            globals.insert(
                "vst3_id".into(),
                liquid::model::Value::scalar(
                    bytes
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            );
            let sdk = match &req.sdk_path {
                Some(path) => {
                    let source = fs::canonicalize(path)?;
                    if !source.join("crates/rvst-core/Cargo.toml").is_file() {
                        return Err(OpError::new(
                            "invalid_sdk",
                            "SDK path must contain crates/rvst-core/Cargo.toml",
                            2,
                        ));
                    }
                    if req.vendor_sdk {
                        copy_sdk(&source, &temp.join("vendor/rvst"))?;
                        "vendor/rvst".to_string()
                    } else {
                        source.to_string_lossy().to_string()
                    }
                }
                None => {
                    if req.vendor_sdk {
                        return Err(OpError::new(
                            "invalid_sdk",
                            "--vendor-sdk requires --sdk-path",
                            2,
                        ));
                    }
                    "registry".into()
                }
            };
            globals.insert(
                "sdk_source".into(),
                liquid::model::Value::scalar(sdk.clone()),
            );
            for krate in [
                "rvst-core",
                "rvst-runtime",
                "rvst-test",
                "rvst-ui",
                "rvst-platform",
                "rvst-vst3",
                "rvst-clap",
                "rvst-build",
                "rvst-standalone",
                "rvst-web",
            ] {
                let dep = if sdk == "registry" {
                    format!("\"{}\"", env!("CARGO_PKG_VERSION"))
                } else {
                    let p = if req.vendor_sdk {
                        format!("../../vendor/rvst/crates/{krate}")
                    } else {
                        format!("{sdk}/crates/{krate}")
                    };
                    format!("{{ path = {} }}", serde_json::to_string(&p).unwrap())
                };
                globals.insert(
                    krate.replace('-', "_").into(),
                    liquid::model::Value::scalar(dep),
                );
            }
            let parser = liquid::ParserBuilder::with_stdlib()
                .build()
                .map_err(|e| invalid(e.to_string()))?;
            let file_contents: BTreeMap<PathBuf, Vec<u8>> = if let Some(files) = &template.embedded
            {
                files.clone()
            } else {
                collect_files(&template.root.join("files"))?
                    .into_iter()
                    .map(|rel| {
                        let content = fs::read(template.root.join("files").join(&rel))?;
                        Ok((rel, content))
                    })
                    .collect::<Result<_>>()?
            };
            let files: Vec<_> = file_contents.keys().cloned().collect();
            for rel in &files {
                let rendered_path = parser
                    .parse(&rel.to_string_lossy())
                    .map_err(|e| invalid(e.to_string()))?
                    .render(&globals)
                    .map_err(|e| invalid(e.to_string()))?;
                let rendered_path = rendered_path
                    .strip_suffix(".liquid")
                    .unwrap_or(&rendered_path);
                let target_rel = Path::new(rendered_path);
                if !safe_relative(target_rel) {
                    return Err(OpError::new(
                        "unsafe_template",
                        format!("Unsafe rendered path: {rendered_path}"),
                        2,
                    ));
                }
                let target = temp.join(target_rel);
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p)?;
                }
                let source = &file_contents[rel];
                let contents = if rel.extension().is_some_and(|s| s == "liquid") {
                    parser
                        .parse(std::str::from_utf8(source).map_err(|e| invalid(e.to_string()))?)
                        .map_err(|e| invalid(e.to_string()))?
                        .render(&globals)
                        .map_err(|e| invalid(e.to_string()))?
                        .into_bytes()
                } else {
                    source.clone()
                };
                let contents = if target_rel
                    .extension()
                    .is_some_and(|extension| extension == "rs")
                {
                    format_rust(&contents)?
                } else {
                    contents
                };
                fs::write(target, contents)?;
            }
            let provenance = serde_json::json!({"schema_version":1,"template":template.provenance,"template_version":template.manifest.version,"sdk_version_requirement":template.manifest.sdk_version,"inputs":values,"sdk":sdk,"sdk_development_override":req.sdk_path.is_some(),"sdk_vendored":req.vendor_sdk});
            fs::write(
                temp.join("rvst-project.json"),
                serde_json::to_vec_pretty(&provenance).unwrap(),
            )?;
            if req.git {
                let result = Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(&temp)
                    .output()?;
                if !result.status.success() {
                    return Err(OpError::new(
                        "git_error",
                        String::from_utf8_lossy(&result.stderr),
                        1,
                    ));
                }
            }
            // Sibling rename is atomic. Refuse races; never merge generated contents into a destination.
            if absolute.exists() {
                return Err(OpError::new(
                    "destination_exists",
                    "Destination appeared during generation",
                    4,
                ));
            }
            atomic_rename(&temp, &absolute)?;
            Ok(GenerationReport {
                path: absolute.clone(),
                template: template.manifest.id.clone(),
                files: files.len() + 1,
                sdk,
                git: req.git,
                next_commands: vec![
                    format!("cd {}", shell_quote(&absolute.to_string_lossy())),
                    "cargo rvst doctor".into(),
                    "cargo rvst test".into(),
                    "cargo rvst dev".into(),
                ],
            })
        })();
        if outcome.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        outcome
    }
}
// Parse only the supplied text: no source paths, child-module traversal, user
// rustfmt configuration, or template hooks. Vendored SDK files are never passed.
fn format_rust(source: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new("rustfmt")
        .args([
            "--edition",
            "2024",
            "--emit",
            "stdout",
            "--config",
            "skip_children=true",
            "--config-path",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            OpError::new(
                "rustfmt_unavailable",
                format!(
                    "Rust formatting requires rustfmt; run `rustup component add rustfmt`: {error}"
                ),
                3,
            )
        })?;
    let write_result = child
        .stdin
        .take()
        .expect("piped rustfmt input")
        .write_all(source);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let missing = error.contains("not installed")
            || error.contains("does not have")
            || error.contains("no default toolchain");
        return Err(OpError::new(
            if missing {
                "rustfmt_unavailable"
            } else {
                "invalid_rust_template"
            },
            if missing {
                format!("Install rustfmt with `rustup component add rustfmt`: {error}")
            } else {
                format!("Generated Rust could not be formatted: {error}")
            },
            if missing { 3 } else { 2 },
        ));
    }
    write_result?;
    Ok(output.stdout)
}
fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
fn copy_sdk(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "THIRD_PARTY_NOTICES.md",
        "THIRD_PARTY_LICENSES.txt",
    ] {
        if source.join(file).is_file() {
            fs::copy(source.join(file), dest.join(file))?;
        }
    }
    for rel in collect_files(&source.join("crates"))? {
        if rel
            .components()
            .any(|c| matches!(c,Component::Normal(v) if v=="target"||v==".git"))
        {
            continue;
        }
        let target = dest.join("crates").join(&rel);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(source.join("crates").join(rel), target)?;
    }
    if source.join("templates").is_dir() {
        for rel in collect_files(&source.join("templates"))? {
            let target = dest.join("templates").join(&rel);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(source.join("templates").join(rel), target)?;
        }
    }
    if source.join("docs").is_dir() {
        for rel in collect_files(&source.join("docs"))? {
            let target = dest.join("docs").join(&rel);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(source.join("docs").join(rel), target)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    pub name: String,
    pub source: String,
    pub revision: Option<String>,
    pub path: PathBuf,
}
pub fn register_template(name: &str, source: &str, revision: Option<&str>) -> Result<Registration> {
    if source.starts_with("https://") || source.starts_with("http://") {
        let authority = source
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("");
        if authority.contains('@') {
            return Err(OpError::new(
                "credential_url",
                "Use a Git credential helper; credentials cannot be stored in template provenance",
                2,
            ));
        }
    }
    if !valid_identifier(name) {
        return Err(OpError::new(
            "invalid_name",
            "Template name must be a lowercase identifier",
            2,
        ));
    }
    let root = registry_root();
    fs::create_dir_all(root.join("templates"))?;
    let record_path = root.join("templates").join(format!("{name}.json"));
    if record_path.exists() {
        return Err(OpError::new(
            "template_exists",
            format!("Template {name} is already registered"),
            4,
        ));
    }
    let registration = if let Some(revision) = revision {
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(OpError::new(
                "unpinned_git",
                "Git templates require a full 40-character commit SHA",
                2,
            ));
        }
        if source.starts_with('-') {
            return Err(OpError::new(
                "invalid_source",
                "Git URL must not begin with a dash",
                2,
            ));
        }
        let cache = root.join("cache").join(format!("{name}-{revision}"));
        fs::create_dir_all(cache.parent().unwrap())?;
        if cache.exists() {
            return Err(OpError::new(
                "cache_exists",
                "Template cache already exists; choose a new registered name",
                4,
            ));
        }
        let cloned = Command::new("git")
            .args(["clone", "--no-checkout", "--", source])
            .arg(&cache)
            .output()?;
        if !cloned.status.success() {
            let _ = fs::remove_dir_all(&cache);
            return Err(OpError::new(
                "git_error",
                String::from_utf8_lossy(&cloned.stderr),
                1,
            ));
        }
        let result = (|| {
            let checkout = Command::new("git")
                .arg("-C")
                .arg(&cache)
                .args(["checkout", "--detach", revision])
                .output()?;
            if !checkout.status.success() {
                return Err(OpError::new(
                    "git_error",
                    String::from_utf8_lossy(&checkout.stderr),
                    1,
                ));
            }
            let head = Command::new("git")
                .arg("-C")
                .arg(&cache)
                .args(["rev-parse", "HEAD"])
                .output()?;
            let actual = String::from_utf8_lossy(&head.stdout).trim().to_string();
            if !head.status.success() || actual != revision.to_ascii_lowercase() {
                return Err(OpError::new(
                    "git_revision_mismatch",
                    "Checked-out Git revision differs from the pinned SHA",
                    4,
                ));
            }
            Template::load(&cache)?;
            Ok(Registration {
                name: name.into(),
                source: source.into(),
                revision: Some(revision.into()),
                path: cache.clone(),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&cache);
        }
        result?
    } else {
        let path = fs::canonicalize(source)?;
        Template::load(&path)?;
        Registration {
            name: name.into(),
            source: source.into(),
            revision: None,
            path,
        }
    };
    fs::write(
        record_path,
        serde_json::to_vec_pretty(&registration).unwrap(),
    )?;
    Ok(registration)
}
pub fn registrations() -> Result<Vec<Registration>> {
    let dir = registry_root().join("templates");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut records = vec![];
    for e in fs::read_dir(dir)? {
        let path = e?.path();
        if path.extension().is_some_and(|x| x == "json") {
            records.push(
                serde_json::from_slice(&fs::read(path)?)
                    .map_err(|e| OpError::new("registry_error", e.to_string(), 1))?,
            );
        }
    }
    Ok(records)
}
pub fn load_registered(name: &str) -> Result<Template> {
    let r = registrations()?
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| {
            OpError::new(
                "template_not_found",
                format!("No registered template: {name}"),
                2,
            )
        })?;
    let mut t = Template::load(&r.path)?;
    t.provenance = if let Some(rev) = r.revision {
        format!("git:{}@{rev}", r.source)
    } else {
        format!("local:{}", r.path.display())
    };
    Ok(t)
}
pub fn list_templates() -> Result<Vec<TemplateManifest>> {
    let mut result = vec![];
    for name in ["debug", "gain", "instrument", "midi"] {
        result.push(Template::builtin(name)?.manifest);
    }
    for r in registrations()? {
        let mut t = load_registered(&r.name)?.manifest;
        t.id = r.name;
        result.push(t);
    }
    Ok(result)
}
pub fn resolve_template(value: &str) -> Result<TemplateSource> {
    if matches!(value, "gain" | "instrument" | "midi" | "debug") {
        Ok(TemplateSource::Builtin(value.into()))
    } else if Path::new(value).exists() {
        Ok(TemplateSource::Local(value.into()))
    } else {
        Ok(TemplateSource::Registered(value.into()))
    }
}
pub fn inspect_template(value: &str) -> Result<Template> {
    match resolve_template(value)? {
        TemplateSource::Builtin(n) => Template::builtin(&n),
        TemplateSource::Local(p) => Template::load(&p),
        TemplateSource::Registered(n) => load_registered(&n),
    }
}
pub fn shell_quote(s: &str) -> String {
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_=/.".contains(&b))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskReport {
    pub command: Vec<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    pub stream: String,
    pub line: String,
}
#[derive(Clone, Default)]
pub struct Cancellation(pub Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
pub fn run_task(
    directory: &Path,
    command: &[String],
    cancel: &Cancellation,
    events: Option<mpsc::Sender<LogEvent>>,
) -> Result<TaskReport> {
    let Some(program) = command.first() else {
        return Err(OpError::new("invalid_command", "Command is empty", 2));
    };
    if cancel.cancelled() {
        return Ok(TaskReport {
            command: command.to_vec(),
            exit_code: 130,
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    let mut builder = Command::new(program);
    builder
        .args(&command[1..])
        .current_dir(directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        builder.process_group(0);
    }
    #[cfg(windows)]
    let job = windows_job::Job::new()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Suspend before assigning the job, so no child can escape ownership.
        builder.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    let mut child = builder
        .spawn()
        .map_err(|e| OpError::new("command_unavailable", format!("{program}: {e}"), 3))?;
    #[cfg(windows)]
    if let Err(error) = job.attach_and_resume(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let spawn_reader = |reader: Box<dyn std::io::Read + Send>,
                        stream: &'static str,
                        events: Option<mpsc::Sender<LogEvent>>| {
        thread::spawn(move || {
            let mut output = String::new();
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(line) => {
                        if let Some(tx) = &events {
                            let _ = tx.send(LogEvent {
                                stream: stream.into(),
                                line: line.clone(),
                            });
                        }
                        output.push_str(&line);
                        output.push('\n');
                    }
                    Err(e) => {
                        output.push_str(&e.to_string());
                        break;
                    }
                }
            }
            output
        })
    };
    let out = spawn_reader(Box::new(stdout), "stdout", events.clone());
    let err = spawn_reader(Box::new(stderr), "stderr", events);
    let mut status = None;
    let exit = loop {
        if cancel.cancelled() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            job.terminate()?;
            #[cfg(not(any(unix, windows)))]
            {
                let _ = child.kill();
            }
            let _ = child.wait();
            break 130;
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        // A launcher may exit while descendants still own inherited pipes.
        // Continue polling cancellation until both readers have reached EOF.
        if let Some(status) = status.as_ref()
            && out.is_finished()
            && err.is_finished()
        {
            break status.code().unwrap_or(1);
        }
        thread::sleep(Duration::from_millis(40));
    };
    Ok(TaskReport {
        command: command.to_vec(),
        exit_code: exit,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}
pub fn task_command(
    task: &str,
    release: bool,
    format: &str,
    input: Option<&Path>,
) -> Result<Vec<String>> {
    let mut cmd = vec!["cargo".into()];
    match task {
        "test" => cmd.extend(["test".into(), "--workspace".into()]),
        "validate" => cmd.extend(["test".into(), "--workspace".into(), "--all-targets".into()]),
        "web" => cmd.extend(
            ["run", "-p", "xtask", "--", "web", "--serve"]
                .into_iter()
                .map(String::from),
        ),
        "dev" => cmd.extend(["run".into(), "-p".into(), "plugin-standalone".into()]),
        "debug" => cmd.extend([
            "run".into(),
            "-p".into(),
            "xtask".into(),
            "--".into(),
            "debug".into(),
        ]),
        "replay" => {
            cmd.extend([
                "run".into(),
                "-p".into(),
                "xtask".into(),
                "--".into(),
                "replay".into(),
            ]);
            cmd.push(
                input
                    .ok_or_else(|| {
                        OpError::new("missing_input", "Replay requires a recording path", 2)
                    })?
                    .to_string_lossy()
                    .into(),
            );
        }
        "package" => {
            if !matches!(format, "vst3" | "clap" | "all") {
                return Err(OpError::new(
                    "invalid_format",
                    "Format must be vst3, clap, or all",
                    2,
                ));
            }
            cmd.extend([
                "run".into(),
                "-p".into(),
                "xtask".into(),
                "--".into(),
                "package".into(),
                format.into(),
            ]);
            if release {
                cmd.push("--release".into());
            }
        }
        _ => {
            return Err(OpError::new(
                "invalid_task",
                format!("Unknown task: {task}"),
                2,
            ));
        }
    }
    Ok(cmd)
}
pub fn doctor(directory: &Path) -> serde_json::Value {
    let mut checks = vec![];
    for (program, args, setup, required) in [
        (
            "rustc",
            vec!["--version"],
            "Install Rust from https://rustup.rs",
            true,
        ),
        (
            "cargo",
            vec!["--version"],
            "Install Rust from https://rustup.rs",
            true,
        ),
        (
            "rustfmt",
            vec!["--version"],
            "rustup component add rustfmt (required to format generated Rust)",
            true,
        ),
        (
            "git",
            vec!["--version"],
            "Install Git (macOS: xcode-select --install; Debian/Ubuntu: sudo apt install git)",
            true,
        ),
        (
            "rustup",
            vec!["--version"],
            "Install rustup from https://rustup.rs to manage WASM targets",
            false,
        ),
        (
            "wasm-bindgen",
            vec!["--version"],
            "cargo install wasm-bindgen-cli --version 0.2.130 --locked (match Cargo.lock)",
            false,
        ),
    ] {
        let result = Command::new(program)
            .args(args)
            .current_dir(directory)
            .output();
        let (ok, detail) = match result {
            Ok(output) => (
                output.status.success(),
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ),
            Err(error) => (false, error.to_string()),
        };
        checks.push(serde_json::json!({"name":program,"ok":ok,"required":required,"detail":detail,"setup":setup}));
    }
    let targets = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let wasm = targets.is_ok_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "wasm32-unknown-unknown")
    });
    checks.push(serde_json::json!({"name":"wasm32-unknown-unknown","ok":wasm,"required":false,"setup":"rustup target add wasm32-unknown-unknown"}));
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("xcode-select").arg("-p").output();
        checks.push(serde_json::json!({"name":"Apple command-line tools","required":false,"ok":status.is_ok_and(|o|o.status.success()),"setup":"xcode-select --install"}));
    }
    #[cfg(target_os = "linux")]
    {
        let status = Command::new("pkg-config")
            .args(["--exists", "alsa", "x11", "xkbcommon"])
            .status();
        checks.push(serde_json::json!({"name":"Linux audio/window libraries","required":false,"ok":status.is_ok_and(|s|s.success()),"setup":"sudo apt install build-essential pkg-config libasound2-dev libx11-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev"}));
    }
    #[cfg(target_os = "windows")]
    {
        checks.push(serde_json::json!({"name":"MSVC build tools","required":false,"ok":Command::new("where").arg("link.exe").output().is_ok_and(|o|o.status.success()),"setup":"Install Visual Studio Build Tools with Desktop development with C++ and Windows SDK; use an x64 Native Tools command prompt"}));
    }
    serde_json::json!({"checks":checks,"project":directory.join("rvst-project.json").is_file(),"cargo_manifest":directory.join("Cargo.toml").is_file(),"host":std::env::consts::OS,"qualification":"Host prerequisites only. Native format validators and DAW acceptance remain separate; optional checks apply to web/native targets."})
}

fn atomic_rename(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| OpError::new("invalid_destination", "Path contains NUL", 2))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| OpError::new("invalid_destination", "Path contains NUL", 2))?;
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let result = unsafe { libc::rename(source.as_ptr(), destination.as_ptr()) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(OpError::new(
                    "destination_exists",
                    "Destination appeared during generation",
                    4,
                ));
            }
            return Err(error.into());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::rename(source, destination)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ShowcaseRequest {
    pub path: PathBuf,
    pub sdk_path: Option<PathBuf>,
    pub vendor_sdk: bool,
    pub install: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ShowcasePlan {
    pub path: PathBuf,
    pub create: bool,
    pub template: String,
    pub command: Vec<String>,
    pub install: bool,
}
pub fn showcase_plan(request: &ShowcaseRequest) -> Result<ShowcasePlan> {
    let create = !request.path.exists();
    if !create {
        let file = request.path.join("rvst-project.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&file).map_err(|_| {
            OpError::new(
                "unowned_project",
                "Showcase path exists without RVST provenance; choose an empty path",
                4,
            )
        })?)
        .map_err(|e| OpError::new("invalid_project", e.to_string(), 2))?;
        if !value["template"]
            .as_str()
            .is_some_and(|s| s.starts_with("builtin:debug@"))
        {
            return Err(OpError::new(
                "unowned_project",
                "Showcase only reuses an RVST Debug generated project",
                4,
            ));
        }
    }
    if request.vendor_sdk && request.sdk_path.is_none() {
        return Err(OpError::new(
            "invalid_sdk",
            "--vendor-sdk requires --sdk-path",
            2,
        ));
    }
    if let Some(sdk) = &request.sdk_path
        && !sdk.join("crates/rvst-core/Cargo.toml").is_file()
    {
        return Err(OpError::new(
            "invalid_sdk",
            "SDK path must contain crates/rvst-core/Cargo.toml",
            2,
        ));
    }
    let command = vec![
        "cargo",
        "run",
        "-p",
        "xtask",
        "--",
        if request.install {
            "install"
        } else {
            "package"
        },
        "all",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    Ok(ShowcasePlan {
        path: request.path.clone(),
        create,
        template: "debug".into(),
        command,
        install: request.install,
    })
}
pub fn showcase(
    request: ShowcaseRequest,
    cancel: &Cancellation,
    events: Option<mpsc::Sender<LogEvent>>,
) -> Result<TaskReport> {
    let plan = showcase_plan(&request)?;
    if plan.create {
        Generator::new().generate(NewRequest {
            template: TemplateSource::Builtin("debug".into()),
            destination: request.path.clone(),
            inputs: Default::default(),
            sdk_path: request.sdk_path,
            vendor_sdk: request.vendor_sdk,
            git: true,
        })?;
    }
    if cancel.cancelled() {
        return Err(OpError::new("cancelled", "Showcase cancelled", 130));
    }
    let report = run_task(&request.path, &plan.command, cancel, events)?;
    if report.exit_code != 0 {
        return Err(OpError::new(
            if report.exit_code == 130 {
                "cancelled"
            } else {
                "task_failed"
            },
            "Showcase build/install failed",
            if report.exit_code == 130 { 130 } else { 1 },
        )
        .with_details(serde_json::to_value(report).unwrap()));
    }
    Ok(report)
}
