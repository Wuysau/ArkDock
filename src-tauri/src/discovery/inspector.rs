use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ipc::model::{Candidate, Environment, Evidence, ProjectRef};

use super::environment_name;

const MAX_ASCENT: usize = 12;
const MAX_CONFIG_BYTES: u64 = 1_048_576;
const MAX_MODULES: usize = 64;
const MAX_VARIANTS: usize = 16;
const MAX_METADATA_BYTES: usize = 1024;
const MAX_CANDIDATES: usize = 256;
const MAX_CANDIDATE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct CandidateBudget {
    count: usize,
    bytes: usize,
}
impl CandidateBudget {
    fn reserve(&mut self, fields: &[&str]) -> Result<(), String> {
        // Reserve before any candidate strings/evidence are cloned. Allow fixed IDs/reasons too.
        let bytes = fields
            .iter()
            .try_fold(1024usize, |total, value| total.checked_add(value.len()))
            .ok_or("Candidate byte budget overflow")?;
        if self.count >= MAX_CANDIDATES || bytes > MAX_CANDIDATE_BYTES.saturating_sub(self.bytes) {
            return Err("Candidate count/byte budget exceeded (256 candidates, 8 MiB); select an application module".into());
        }
        self.count += 1;
        self.bytes += bytes;
        Ok(())
    }
}
fn metadata_field(value: &str) -> Result<(), String> {
    if value.len() > MAX_METADATA_BYTES {
        Err("Declared metadata field exceeds 1024 bytes".into())
    } else {
        Ok(())
    }
}

/// Classifies explicit path identities without opening or converting the path. Native folder
/// pickers return ordinary UNC, while canonicalization may return extended UNC; both retain WSL.
/// Unqualified paths default to WindowsLocal; inspection separately rejects Windows POSIX paths.
pub fn path_environment(path: &Path) -> Environment {
    explicit_environment(path).unwrap_or(Environment::WindowsLocal)
}

fn explicit_environment(path: &Path) -> Option<Environment> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.starts_with("wsl://") {
        return Some(Environment::Wsl);
    }
    if lower.starts_with("ssh://") || lower.starts_with("vscode-remote://") {
        return Some(Environment::Remote);
    }
    let normalized = lower.replace('/', "\\");
    let unc = normalized
        .strip_prefix("\\\\?\\unc\\")
        .or_else(|| normalized.strip_prefix("\\??\\unc\\"))
        .or_else(|| normalized.strip_prefix("\\\\"));
    if let Some(unc) = unc {
        let host = unc.split('\\').next().unwrap_or_default();
        if host == "wsl$" || host == "wsl.localhost" {
            return Some(Environment::Wsl);
        }
        return Some(Environment::WindowsLocal);
    }
    let drive = normalized
        .strip_prefix("\\\\?\\")
        .unwrap_or(&normalized)
        .as_bytes();
    if drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\' {
        return Some(Environment::WindowsLocal);
    }
    None
}

fn validate_path_environment(
    path: &Path,
    environment: &Environment,
    stage: &str,
    windows_paths: bool,
) -> Result<(), String> {
    let supplied = path.to_string_lossy();
    if let Some(detected) = explicit_environment(path) {
        if detected != *environment {
            return Err(format!("{stage} path environment {} disagrees with declared {}; the original path is not converted",
                environment_name(&detected), environment_name(environment)));
        }
    } else if matches!(environment, Environment::Wsl) && !supplied.starts_with('/') {
        return Err(format!("{stage} WSL path requires explicit WSL or POSIX identity; no Windows-local mapping is allowed"));
    }
    // On Windows a leading '/' can be rooted on the current Windows drive. Refuse that implicit
    // conversion for both a mislabeled local path and an explicitly identified WSL POSIX path.
    if windows_paths && supplied.starts_with('/') && !supplied.starts_with("//") {
        return Err(format!("{stage} POSIX path cannot be interpreted as a Windows local path; its environment is unsupported"));
    }
    Ok(())
}

fn validate_environment_pair(
    raw: &Path,
    canonical: &Path,
    environment: &Environment,
    windows_paths: bool,
) -> Result<(), String> {
    validate_path_environment(raw, environment, "Supplied", windows_paths)?;
    validate_path_environment(canonical, environment, "Canonical", windows_paths)
}

/// Identity includes the full application path and environment, so worktrees never merge by name.
pub fn project_id(
    environment: &Environment,
    app_root: &str,
    module: &str,
    variant: &str,
) -> String {
    let mut hash = Sha256::new();
    for value in [environment_name(environment), app_root, module, variant] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    format!("project-{:x}", hash.finalize())
}

/// Reads bounded JSON5 files on supplied ancestors/modules or a marked, bounded monorepo.
/// Never evaluates hvigor, installs packages or launches processes.
pub fn inspect_directory(
    path: &Path,
    environment: Environment,
    evidence: Evidence,
) -> Result<Vec<Candidate>, String> {
    if environment_name(&evidence.environment) != environment_name(&environment) {
        return Err("Evidence and directory environment identities disagree".into());
    }
    if evidence.raw_path.len() > 32_768 {
        return Err("Evidence path exceeds 32768 bytes".into());
    }
    let mut budget = CandidateBudget::default();
    validate_path_environment(path, &environment, "Supplied", cfg!(windows))?;
    if matches!(environment, Environment::Remote) {
        return Err("Remote project inspection is unsupported; the original path is retained without local conversion".into());
    }
    if matches!(environment, Environment::Wsl)
        && matches!(explicit_environment(path), Some(Environment::Wsl))
    {
        return Err("WSL UNC/URI inspection is unsupported; the original path and WSL environment are retained without local conversion".into());
    }
    let input = fs::canonicalize(path)
        .map_err(|error| format!("Cannot access supplied path {}: {error}", path.display()))?;
    validate_environment_pair(path, &input, &environment, cfg!(windows))?;
    let metadata = fs::metadata(&input).map_err(|error| error.to_string())?;
    let start = if metadata.is_dir() {
        input.clone()
    } else if metadata.is_file() {
        input
            .parent()
            .ok_or("Source file has no parent directory")?
            .to_path_buf()
    } else {
        return Err("Supplied path is not a regular file or directory".into());
    };
    if let Some(app_root) = ancestors(&start).find(|root| is_application(root)) {
        return inspect_application(&input, app_root, environment, evidence, &mut budget);
    }
    // Only descend a supplied workspace directory with an explicit repository/workspace marker.
    // Never descend ancestors, drives, arbitrary folders, symlinks or dependency trees.
    if !metadata.is_dir() || !has_workspace_marker(&start) {
        return Err("No Harmony application root on the bounded ancestor chain; select an actual app or a marked monorepo workspace".into());
    }
    let roots = monorepo_roots(&start)?;
    let mut candidates = Vec::new();
    for root in roots {
        match inspect_application(
            &root,
            root.clone(),
            environment.clone(),
            evidence.clone(),
            &mut budget,
        ) {
            Ok(items) => candidates.extend(items),
            Err(error) => candidates.push(invalid_module(
                "Invalid Harmony application",
                (&path_string(&root)?, &path_string(&start)?),
                &environment,
                &evidence,
                "<application>",
                &error,
                &mut budget,
            )?),
        }
    }
    if candidates.is_empty() {
        return Err("No Harmony application inside bounded monorepo depth (2); select an application subdirectory".into());
    }
    Ok(candidates)
}

fn is_application(root: &Path) -> bool {
    root.join("build-profile.json5").is_file() && root.join("AppScope/app.json5").is_file()
}

fn has_workspace_marker(root: &Path) -> bool {
    root.join(".git").exists()
        || root.join("pnpm-workspace.yaml").is_file()
        || root.join("lerna.json").is_file()
}

fn monorepo_roots(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0)]);
    let mut directories = 1;
    let mut entries = 0;
    let mut apps = Vec::new();
    while let Some((directory, depth)) = queue.pop_front() {
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("Cannot read bounded monorepo: {error}"))?
        {
            entries += 1;
            if entries > 512 {
                return Err("Entry count exceeds bounded monorepo limit (512); select an application subdirectory".into());
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if matches!(
                name.as_str(),
                ".git"
                    | ".local"
                    | ".cache"
                    | ".hvigor"
                    | ".arkdock"
                    | "node_modules"
                    | "oh_modules"
                    | "build"
                    | "dist"
                    | "target"
                    | "temp"
                    | "tmp"
            ) {
                continue;
            }
            directories += 1;
            if directories > 64 {
                return Err("Directory count exceeds bounded monorepo limit (64); select an application subdirectory".into());
            }
            let child = entry
                .path()
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if !child.starts_with(root) {
                return Err("Directory escapes bounded monorepo workspace".into());
            }
            if is_application(&child) {
                apps.push(child);
                if apps.len() > 16 {
                    return Err("Application count exceeds bounded monorepo limit (16); select an application subdirectory".into());
                }
            } else if depth + 1 < 2 {
                queue.push_back((child, depth + 1));
            }
        }
    }
    apps.sort();
    Ok(apps)
}

fn inspect_application(
    input: &Path,
    app_root: PathBuf,
    environment: Environment,
    evidence: Evidence,
    budget: &mut CandidateBudget,
) -> Result<Vec<Candidate>, String> {
    let profile = read_json5_within(&app_root.join("build-profile.json5"), &app_root)?;
    let app = read_json5_within(&app_root.join("AppScope/app.json5"), &app_root)?;
    if !app.get("app").is_some_and(Value::is_object) {
        return Err("AppScope/app.json5 must contain an app object".into());
    }
    let modules = profile
        .get("modules")
        .and_then(Value::as_array)
        .ok_or("build-profile.json5 must declare a modules array")?;
    if modules.is_empty() || modules.len() > MAX_MODULES {
        return Err(format!("Module count must be in 1..={MAX_MODULES}"));
    }
    let mut declared_names = std::collections::BTreeSet::new();
    for module in modules {
        if let Some(name) = module.get("name").and_then(Value::as_str) {
            metadata_field(name)?;
            if !declared_names.insert(name) {
                return Err("Duplicate declared module names are ambiguous".into());
            }
        }
    }
    let workspace_root = ancestors(&app_root)
        .find(|root| has_workspace_marker(root))
        .unwrap_or_else(|| app_root.clone());
    let app_root_string = path_string(&app_root)?;
    let workspace_root_string = path_string(&workspace_root)?;
    let app_name = app
        .get("app")
        .and_then(|value| value.get("bundleName"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|name| {
            metadata_field(name)?;
            Ok::<_, String>(name.to_owned())
        })
        .transpose()?
        .unwrap_or_else(|| {
            app_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    metadata_field(&app_name)?;
    let sdk_id = sdk_identity(&profile)?;
    // A library source belongs to the containing application; it is not itself a HAP.
    // Preserve narrowing for an actual application module, but let a declared library
    // hint identify the application's HAP candidates without guessing a module by name.
    let narrowed = input != app_root
        && !app_root.join("AppScope").starts_with(input)
        && !declared_library_contains_input(modules, input, &app_root);
    let mut candidates = Vec::new();
    for module in modules {
        let name = module
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let source = module.get("srcPath").and_then(Value::as_str);
        let (Some(name), Some(source)) = (name, source) else {
            candidates.push(invalid_module(
                &app_name,
                (&app_root_string, &workspace_root_string),
                &environment,
                &evidence,
                "<invalid-module>",
                "Declared module requires nonempty name and srcPath",
                budget,
            )?);
            continue;
        };
        metadata_field(source)?;
        let module_root = match safe_module_root(&app_root, source) {
            Ok(root) => root,
            Err(reason) => {
                candidates.push(invalid_module(
                    &app_name,
                    (&app_root_string, &workspace_root_string),
                    &environment,
                    &evidence,
                    name,
                    &reason,
                    budget,
                )?);
                continue;
            }
        };
        // A source file or module subdirectory points only at its containing declared module.
        if narrowed && !input.starts_with(&module_root) && !module_root.starts_with(input) {
            continue;
        }
        let manifest =
            match read_json5_within(&module_root.join("src/main/module.json5"), &module_root) {
                Ok(manifest) => manifest,
                Err(reason) => {
                    candidates.push(invalid_module(
                        &app_name,
                        (&app_root_string, &workspace_root_string),
                        &environment,
                        &evidence,
                        name,
                        &reason,
                        budget,
                    )?);
                    continue;
                }
            };
        let Some(module_config) = manifest.get("module").filter(|value| value.is_object()) else {
            candidates.push(invalid_module(
                &app_name,
                (&app_root_string, &workspace_root_string),
                &environment,
                &evidence,
                name,
                "module.json5 must contain a module object",
                budget,
            )?);
            continue;
        };
        if module_config.get("name").and_then(Value::as_str) != Some(name) {
            candidates.push(invalid_module(
                &app_name,
                (&app_root_string, &workspace_root_string),
                &environment,
                &evidence,
                name,
                "Module name does not match its declared build-profile name",
                budget,
            )?);
            continue;
        }
        match module_config.get("type").and_then(Value::as_str) {
            Some("har" | "shared") => continue,
            Some("entry" | "feature") => {}
            _ => {
                candidates.push(invalid_module(
                    &app_name,
                    (&app_root_string, &workspace_root_string),
                    &environment,
                    &evidence,
                    name,
                    "Module type must explicitly declare entry or feature for an application candidate; HAR/HSP libraries are not application choices",
                    budget,
                )?);
                continue;
            }
        }
        let variants = variants(module)?;
        for variant in variants {
            budget.reserve(&[
                &app_name,
                &app_root_string,
                &workspace_root_string,
                name,
                &variant,
                sdk_id.as_deref().unwrap_or_default(),
                &evidence.raw_path,
            ])?;
            let id = project_id(&environment, &app_root_string, name, &variant);
            let mut reasons = vec![
                "Read-only JSON5 inspection found a declared Harmony HAP application module; library modules are excluded".into(),
            ];
            if !matches!(environment, Environment::WindowsLocal) {
                reasons.push("Environment identity is preserved; native execution for this environment is unsupported".into());
            }
            candidates.push(Candidate {
                id: id.clone(),
                project: ProjectRef {
                    id,
                    name: app_name.clone(),
                    environment: environment.clone(),
                    workspace_root: workspace_root_string.clone(),
                    app_root: app_root_string.clone(),
                    module: name.to_owned(),
                    variant,
                    sdk_id: sdk_id.clone(),
                },
                evidence: vec![evidence.clone()],
                valid: true,
                auto_eligible: false,
                reasons,
            });
        }
    }
    if candidates.is_empty() {
        return Err("No declared entry/feature application module matches this path; HAR/HSP libraries are not standalone application candidates".into());
    }
    Ok(candidates)
}

fn declared_library_contains_input(modules: &[Value], input: &Path, app_root: &Path) -> bool {
    modules.iter().any(|module| {
        let Some(name) = module.get("name").and_then(Value::as_str) else { return false; };
        let Some(source) = module.get("srcPath").and_then(Value::as_str) else { return false; };
        if name.is_empty() || metadata_field(source).is_err() { return false; }
        let Ok(root) = safe_module_root(app_root, source) else { return false; };
        if !input.starts_with(&root) && !root.starts_with(input) { return false; }
        let Ok(manifest) = read_json5_within(&root.join("src/main/module.json5"), &root) else { return false; };
        let Some(config) = manifest.get("module").filter(|value| value.is_object()) else { return false; };
        config.get("name").and_then(Value::as_str) == Some(name)
            && matches!(config.get("type").and_then(Value::as_str), Some("har" | "shared"))
    })
}

fn ancestors(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    path.ancestors().take(MAX_ASCENT).map(Path::to_path_buf)
}

fn path_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or("Path is not valid Unicode".into())
}

fn read_json5_within(path: &Path, root: &Path) -> Result<Value, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("Cannot read required config {}: {error}", path.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "Config escapes its application/module root: {}",
            path.display()
        ));
    }
    let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "Config must be a regular file of at most {MAX_CONFIG_BYTES} bytes"
        ));
    }
    let mut text = String::new();
    fs::File::open(&canonical)
        .map_err(|error| error.to_string())?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err("Config exceeded the byte limit while reading".into());
    }
    json5::from_str(&text).map_err(|error| format!("Invalid JSON5 {}: {error}", path.display()))
}

fn safe_module_root(root: &Path, source: &str) -> Result<PathBuf, String> {
    let relative = Path::new(source);
    // Backslash traversal is rejected on Unix as well as Windows; never reinterpret foreign paths.
    if source.is_empty()
        || source.contains('\\')
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Declared module srcPath must be a confined relative path".into());
    }
    let module = fs::canonicalize(root.join(relative))
        .map_err(|error| format!("Declared module is inaccessible: {error}"))?;
    if !module.starts_with(root) || !module.is_dir() {
        return Err(
            "Declared module path escapes the application root or is not a directory".into(),
        );
    }
    Ok(module)
}

fn variants(module: &Value) -> Result<Vec<String>, String> {
    let Some(targets) = module.get("targets") else {
        return Ok(vec!["default".into()]);
    };
    let targets = targets
        .as_array()
        .ok_or("Module targets must be an array")?;
    if targets.len() > MAX_VARIANTS {
        return Err("Too many module target variants".into());
    }
    let mut result = Vec::new();
    for target in targets {
        let name = target
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("Every declared module target must have a nonempty name")?;
        metadata_field(name)?;
        if !result.iter().any(|value| value == name) {
            result.push(name.to_owned());
        }
    }
    if result.is_empty() {
        result.push("default".into());
    }
    Ok(result)
}

fn sdk_identity(profile: &Value) -> Result<Option<String>, String> {
    // This is declared metadata, never a claim that a usable SDK has been detected.
    let Some(products) = profile
        .get("app")
        .and_then(|app| app.get("products"))
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    let Some(product) = products.first() else {
        return Ok(None);
    };
    let value = product
        .get("compileSdkVersion")
        .or_else(|| product.get("compatibleSdkVersion"));
    Ok(match value {
        Some(Value::String(value)) => {
            metadata_field(value)?;
            Some(value.clone())
        }
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    })
}

fn invalid_module(
    name: &str,
    roots: (&str, &str),
    environment: &Environment,
    evidence: &Evidence,
    module: &str,
    reason: &str,
    budget: &mut CandidateBudget,
) -> Result<Candidate, String> {
    let (app_root, workspace_root) = roots;
    budget.reserve(&[
        name,
        app_root,
        workspace_root,
        &evidence.raw_path,
        module,
        reason,
    ])?;
    let id = project_id(environment, app_root, module, "default");
    Ok(Candidate {
        id: id.clone(),
        project: ProjectRef {
            id,
            name: name.into(),
            environment: environment.clone(),
            workspace_root: workspace_root.into(),
            app_root: app_root.into(),
            module: module.into(),
            variant: "default".into(),
            sdk_id: None,
        },
        evidence: vec![evidence.clone()],
        valid: false,
        auto_eligible: false,
        reasons: vec![reason.into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::EvidenceSource;

    fn evidence(path: &Path, environment: Environment) -> Evidence {
        Evidence {
            source: EvidenceSource::Manual,
            target_id: None,
            process: None,
            session_id: None,
            observed_at: 100,
            expires_at: 200,
            environment,
            raw_path: path.to_string_lossy().into_owned(),
            verifiable: false,
            active_scope: false,
        }
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/harmony-app")
    }

    fn make_monorepo_app(path: &Path) {
        fs::create_dir_all(path.join("AppScope")).unwrap();
        fs::create_dir_all(path.join("entry/src/main")).unwrap();
        fs::write(
            path.join("build-profile.json5"),
            "{modules:[{name:'entry',srcPath:'entry'}]}",
        )
        .unwrap();
        fs::write(
            path.join("AppScope/app.json5"),
            "{app:{bundleName:'same.bundle'}}",
        )
        .unwrap();
        fs::write(
            path.join("entry/src/main/module.json5"),
            "{module:{name:'entry',type:'entry'}}",
        )
        .unwrap();
        fs::write(
            path.join("hvigorfile.ts"),
            "throw new Error('discovery must never execute this')",
        )
        .unwrap();
    }

    #[test]
    fn monorepo_selection_lists_distinct_applications_without_executing_scripts() {
        let repo = tempfile::tempdir().unwrap();
        fs::write(
            repo.path().join(".git"),
            "gitdir: private marker never read",
        )
        .unwrap();
        make_monorepo_app(&repo.path().join("apps/first"));
        make_monorepo_app(&repo.path().join("apps/second"));
        make_monorepo_app(&repo.path().join("node_modules/ignored"));
        let candidates = inspect_directory(
            repo.path(),
            Environment::WindowsLocal,
            evidence(repo.path(), Environment::WindowsLocal),
        )
        .unwrap();
        assert_eq!(candidates.len(), 2);
        assert_ne!(candidates[0].id, candidates[1].id);
        assert!(candidates
            .iter()
            .all(|item| item.valid && !item.auto_eligible));
        assert!(candidates
            .iter()
            .all(|item| Path::new(&item.project.workspace_root)
                == repo.path().canonicalize().unwrap()));
    }

    #[test]
    fn monorepo_descent_refuses_unmarked_roots_and_limits_total_work() {
        let repo = tempfile::tempdir().unwrap();
        make_monorepo_app(&repo.path().join("apps/first"));
        assert!(inspect_directory(
            repo.path(),
            Environment::WindowsLocal,
            evidence(repo.path(), Environment::WindowsLocal)
        )
        .is_err());
        fs::write(repo.path().join(".git"), "marker").unwrap();
        for number in 0..70 {
            fs::create_dir(repo.path().join(format!("folder-{number}"))).unwrap();
        }
        let error = inspect_directory(
            repo.path(),
            Environment::WindowsLocal,
            evidence(repo.path(), Environment::WindowsLocal),
        )
        .unwrap_err();
        assert!(error.contains("bounded monorepo"), "{error}");
    }

    #[test]
    fn source_subdirectory_selects_only_its_declared_module() {
        let path = fixture().join("entry/src/main/ets/pages/Index.ets");
        let candidates = inspect_directory(
            &path,
            Environment::WindowsLocal,
            evidence(&path, Environment::WindowsLocal),
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].project.module, "entry");
        assert!(candidates[0].valid);
        assert!(!candidates[0].auto_eligible);
    }

    #[test]
    fn app_root_lists_hap_modules_without_shared_libraries() {
        let root = fixture();
        let candidates = inspect_directory(
            &root,
            Environment::WindowsLocal,
            evidence(&root, Environment::WindowsLocal),
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].project.module, "entry");
        assert!(candidates.iter().all(|candidate| !candidate.auto_eligible));
    }

    #[test]
    fn declared_library_paths_identify_the_containing_application() {
        let library = fixture().join("feature/src/main/module.json5");
        let candidates = inspect_directory(
            &library,
            Environment::WindowsLocal,
            evidence(&library, Environment::WindowsLocal),
        ).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].project.module, "entry");
        assert!(candidates[0].valid && !candidates[0].auto_eligible);
    }

    #[test]
    fn hap_feature_remains_selectable_while_har_and_hsp_are_not_applications() {
        let repo = tempfile::tempdir().unwrap();
        make_monorepo_app(repo.path());
        let modules = [("entry", "entry"), ("screen", "feature"), ("utils", "har"), ("common", "shared")];
        for (name, kind) in modules {
            let root = repo.path().join(name).join("src/main");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("module.json5"),
                serde_json::json!({"module":{"name":name,"type":kind}}).to_string()).unwrap();
        }
        fs::write(repo.path().join("build-profile.json5"),
            serde_json::json!({"modules":modules.map(|(name,_)|
                serde_json::json!({"name":name,"srcPath":name}))}).to_string()).unwrap();
        let candidates = inspect_directory(repo.path(), Environment::WindowsLocal,
            evidence(repo.path(), Environment::WindowsLocal)).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|item| item.valid && matches!(item.project.module.as_str(), "entry" | "screen")));
        let library = repo.path().join("common/src/main");
        let from_library = inspect_directory(&library, Environment::WindowsLocal,
            evidence(&library, Environment::WindowsLocal)).unwrap();
        assert_eq!(from_library.len(), 2); // Multiple application modules still need a choice.
    }

    #[cfg(unix)]
    #[test]
    fn readable_posix_fixture_preserves_explicit_wsl_identity_without_execution_support() {
        let root = fixture();
        let candidates =
            inspect_directory(&root, Environment::Wsl, evidence(&root, Environment::Wsl)).unwrap();
        assert!(candidates
            .iter()
            .all(|candidate| matches!(candidate.project.environment, Environment::Wsl)));
        assert!(candidates.iter().all(|candidate| !candidate.auto_eligible));
    }

    #[test]
    fn ordinary_and_canonical_wsl_unc_paths_cannot_be_windows_local() {
        for path in [
            r"\\wsl$\Ubuntu\home\user\app",
            r"\\wsl.localhost\Ubuntu\home\user\app",
            r"\\?\UNC\wsl$\Ubuntu\home\user\app",
            r"\\?\UNC\WSL.Localhost\Ubuntu\home\user\app",
            "//wsl.localhost/Ubuntu/home/user/app",
        ] {
            let path = Path::new(path);
            assert_eq!(path_environment(path), Environment::Wsl);
            let error = inspect_directory(
                path,
                Environment::WindowsLocal,
                evidence(path, Environment::WindowsLocal),
            )
            .unwrap_err();
            assert!(error.contains("disagrees with declared windowsLocal"));
            let unavailable =
                inspect_directory(path, Environment::Wsl, evidence(path, Environment::Wsl))
                    .unwrap_err();
            assert!(unavailable.contains("WSL UNC/URI inspection is unsupported"));
        }
        assert_eq!(
            path_environment(Path::new(r"\\wsl.localhost.example\share\app")),
            Environment::WindowsLocal
        );
        assert_eq!(
            path_environment(Path::new(r"\\?\C:\work\app")),
            Environment::WindowsLocal
        );
    }

    #[test]
    fn canonicalization_cannot_cross_environment_identity() {
        let raw = Path::new(r"C:\work\linked-app");
        for canonical in [
            r"\\?\UNC\wsl$\Ubuntu\app",
            r"\\?\UNC\wsl.localhost\Ubuntu\app",
        ] {
            let error = validate_environment_pair(
                raw,
                Path::new(canonical),
                &Environment::WindowsLocal,
                true,
            )
            .unwrap_err();
            assert!(error.contains("Canonical path environment wsl"));
        }
        assert!(validate_environment_pair(
            Path::new("/home/user/app"),
            raw,
            &Environment::Wsl,
            false
        )
        .is_err());
        assert!(validate_environment_pair(
            raw,
            Path::new(r"\\?\C:\work\app"),
            &Environment::WindowsLocal,
            true
        )
        .is_ok());
    }

    #[test]
    fn posix_prefix_never_becomes_a_windows_drive_root() {
        for environment in [Environment::WindowsLocal, Environment::Wsl] {
            let error = validate_path_environment(
                Path::new("/home/user/app"),
                &environment,
                "Supplied",
                true,
            )
            .unwrap_err();
            assert!(error.contains("POSIX path cannot be interpreted as a Windows local path"));
        }
        assert!(validate_environment_pair(
            Path::new(r"C:\app"),
            Path::new("/home/user/app"),
            &Environment::WindowsLocal,
            true
        )
        .is_err());
    }

    #[test]
    fn worktree_paths_and_environments_have_distinct_identities() {
        let a = project_id(
            &Environment::WindowsLocal,
            "C:\\repo\\main",
            "entry",
            "default",
        );
        let b = project_id(
            &Environment::WindowsLocal,
            "C:\\repo\\worktree",
            "entry",
            "default",
        );
        let c = project_id(&Environment::Wsl, "C:\\repo\\main", "entry", "default");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn parent_traversal_and_symlink_escape_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        fs::create_dir(&root).unwrap();
        assert!(safe_module_root(&root, "../outside").is_err());
        assert!(safe_module_root(&root, "..\\outside").is_err());
        #[cfg(unix)]
        {
            let outside = temp.path().join("outside");
            fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(outside, root.join("escape")).unwrap();
            assert!(safe_module_root(&fs::canonicalize(&root).unwrap(), "escape").is_err());
        }
    }

    #[test]
    fn hostile_hvigor_is_never_evaluated() {
        let root = fixture();
        let candidates = inspect_directory(
            &root,
            Environment::WindowsLocal,
            evidence(&root, Environment::WindowsLocal),
        )
        .unwrap();
        assert!(candidates.iter().all(|candidate| candidate.valid));
    }

    fn write_minimal(root: &Path) {
        fs::create_dir_all(root.join("AppScope")).unwrap();
        fs::create_dir_all(root.join("entry/src/main")).unwrap();
        fs::write(
            root.join("build-profile.json5"),
            "{ modules: [{name:'entry',srcPath:'./entry'}] }",
        )
        .unwrap();
        fs::write(
            root.join("AppScope/app.json5"),
            "{ app: {bundleName:'com.example.test'} }",
        )
        .unwrap();
        fs::write(
            root.join("entry/src/main/module.json5"),
            "{ module: {name:'entry',type:'entry'} }",
        )
        .unwrap();
    }

    #[test]
    fn monorepo_root_and_git_worktree_marker_are_metadata_only() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".git"),
            "gitdir: /must/not/be/read/or/executed",
        )
        .unwrap();
        let root = temp.path().join("apps/harmony");
        write_minimal(&root);
        let candidates = inspect_directory(
            &root,
            Environment::WindowsLocal,
            evidence(&root, Environment::WindowsLocal),
        )
        .unwrap();
        assert_eq!(
            candidates[0].project.workspace_root,
            path_string(&fs::canonicalize(temp.path()).unwrap()).unwrap()
        );
        assert_eq!(
            candidates[0].project.app_root,
            path_string(&fs::canonicalize(root).unwrap()).unwrap()
        );
    }

    #[test]
    fn ancestor_search_stops_at_its_bound() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal(temp.path());
        let mut deep = temp.path().join("entry");
        for _ in 0..MAX_ASCENT {
            deep = deep.join("deeper");
        }
        fs::create_dir_all(&deep).unwrap();
        assert!(inspect_directory(
            &deep,
            Environment::WindowsLocal,
            evidence(&deep, Environment::WindowsLocal)
        )
        .is_err());
    }

    #[test]
    fn executable_json5_and_environment_mismatch_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal(temp.path());
        assert!(inspect_directory(
            temp.path(),
            Environment::Wsl,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
        fs::write(
            temp.path().join("build-profile.json5"),
            "{modules: require('fs').readFileSync('/private')}",
        )
        .unwrap();
        assert!(inspect_directory(
            temp.path(),
            Environment::WindowsLocal,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
    }

    #[test]
    fn duplicate_modules_and_oversized_configs_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal(temp.path());
        fs::write(
            temp.path().join("build-profile.json5"),
            "{modules:[{name:'entry',srcPath:'entry'},{name:'entry',srcPath:'entry'}]}",
        )
        .unwrap();
        assert!(inspect_directory(
            temp.path(),
            Environment::WindowsLocal,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
        fs::write(
            temp.path().join("build-profile.json5"),
            vec![b' '; MAX_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        assert!(inspect_directory(
            temp.path(),
            Environment::WindowsLocal,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
    }

    #[test]
    fn metadata_and_candidate_fanout_are_bounded_before_cloning() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal(temp.path());
        fs::write(
            temp.path().join("AppScope/app.json5"),
            serde_json::json!({"app":{"bundleName":"x".repeat(900_000)}}).to_string(),
        )
        .unwrap();
        assert!(inspect_directory(
            temp.path(),
            Environment::WindowsLocal,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
        write_minimal(temp.path());
        let mut modules = Vec::new();
        for number in 0..17 {
            let name = format!("module-{number}");
            fs::create_dir_all(temp.path().join(&name).join("src/main")).unwrap();
            fs::write(
                temp.path().join(&name).join("src/main/module.json5"),
                serde_json::json!({"module":{"name":name,"type":"entry"}}).to_string(),
            )
            .unwrap();
            modules.push(serde_json::json!({"name":name,"srcPath":name,
                "targets": (0..16).map(|n|serde_json::json!({"name":format!("variant-{n}")})).collect::<Vec<_>>() }));
        }
        fs::write(
            temp.path().join("build-profile.json5"),
            serde_json::json!({"modules":modules}).to_string(),
        )
        .unwrap();
        assert!(inspect_directory(
            temp.path(),
            Environment::WindowsLocal,
            evidence(temp.path(), Environment::WindowsLocal)
        )
        .is_err());
    }
}
