//! Real HAP build, signature verification and artifact-bound deployment receipt.
use super::process::{check_cancel, ToolCommandRunner, ToolOutput};
use crate::ipc::model::ProjectRef;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

pub struct Artifact {
    pub path: PathBuf,
    pub sha256: String,
    pub bundle: String,
    pub module: String,
    pub ability: String,
    pub fingerprint: String,
    pub permitted_devices: Vec<String>,
    pub device_id_type: Option<String>,
    pub confirmation: String,
    pub prepared_at: u64,
    verified_profile: Value,
}

/// Bind application identity as well as executable build configuration.
pub fn fingerprint(project: &ProjectRef) -> Result<String, String> {
    let root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let app = root
        .join("AppScope/app.json5")
        .canonicalize()
        .map_err(|_| "DEVICE_APP_MANIFEST_MISSING")?;
    if !app.starts_with(&root) {
        return Err("DEVICE_APP_MANIFEST_OUTSIDE_PROJECT".into());
    }
    let mut hash = Sha256::new();
    hash.update(crate::workspace::fingerprint(project)?.as_bytes());
    hash.update(crate::workspace::read_bounded(&app, 1024 * 1024)?);
    // A device preparation also binds actual sources/resources. Previewer may
    // be stopped, so its watcher epoch cannot protect a pending device install.
    let mut module_roots = std::collections::BTreeSet::from([root.clone()]);
    let build_profile = root.join("build-profile.json5");
    if build_profile.is_file() {
        let profile = read_json(&build_profile)?;
        for module in profile
            .get("modules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let source = module
                .get("srcPath")
                .and_then(Value::as_str)
                .ok_or("MODULE_PATH_MISSING")?;
            let directory = root
                .join(source)
                .canonicalize()
                .map_err(|_| "MODULE_PATH_MISSING")?;
            if !directory.starts_with(&root) {
                return Err("MODULE_OUTSIDE_PROJECT".into());
            }
            module_roots.insert(directory);
        }
    }
    hash_source_tree(
        &root,
        &root,
        &module_roots,
        &mut hash,
        &mut SourceBudget::default(),
        0,
    )?;
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Default)]
struct SourceBudget {
    entries: usize,
    bytes: u64,
}

fn hash_source_tree(
    root: &Path,
    directory: &Path,
    module_roots: &std::collections::BTreeSet<PathBuf>,
    hash: &mut Sha256,
    budget: &mut SourceBudget,
    depth: usize,
) -> Result<(), String> {
    if depth > 24 {
        return Err("DEVICE_SOURCE_TREE_LIMIT".into());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| "DEVICE_SOURCE_READ")? {
        if entries.len().saturating_add(budget.entries) >= 25_000 {
            return Err("DEVICE_SOURCE_TREE_LIMIT".into());
        }
        entries.push(entry.map_err(|_| "DEVICE_SOURCE_READ")?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        budget.entries += 1;
        if budget.entries > 25_000 {
            return Err("DEVICE_SOURCE_TREE_LIMIT".into());
        }
        let kind = entry.file_type().map_err(|_| "DEVICE_SOURCE_READ")?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "node_modules" | "oh_modules"))
            || module_roots.contains(directory)
                && matches!(
                    name.to_str(),
                    Some("build" | ".preview" | ".hvigor" | ".cxx")
                )
        {
            continue;
        }
        if kind.is_symlink() {
            return Err("DEVICE_SOURCE_LINK_UNSUPPORTED".into());
        }
        let path = entry
            .path()
            .canonicalize()
            .map_err(|_| "DEVICE_SOURCE_READ")?;
        if !path.starts_with(root) {
            return Err("DEVICE_SOURCE_OUTSIDE_PROJECT".into());
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "DEVICE_SOURCE_OUTSIDE_PROJECT")?
            .to_string_lossy();
        hash.update((relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        if kind.is_dir() {
            hash.update(b"directory");
            hash_source_tree(root, &path, module_roots, hash, budget, depth + 1)?;
        } else if kind.is_file() {
            let bytes = crate::workspace::read_bounded(&path, 64 * 1024 * 1024)?;
            budget.bytes = budget.bytes.saturating_add(bytes.len() as u64);
            if budget.bytes > 256 * 1024 * 1024 {
                return Err("DEVICE_SOURCE_BYTES_LIMIT".into());
            }
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
        } else {
            return Err("DEVICE_SOURCE_TYPE_UNSUPPORTED".into());
        }
    }
    Ok(())
}

pub fn revalidate(project: &ProjectRef, artifact: &Artifact) -> Result<(), String> {
    if fingerprint(project)? != artifact.fingerprint {
        return Err("TRUST_CHANGED_DURING_DEVICE_BUILD".into());
    }
    if file_hash(&artifact.path)? != artifact.sha256 {
        return Err("DEVICE_ARTIFACT_CHANGED".into());
    }
    let restrictions = verify_profile(
        &artifact.verified_profile,
        &artifact.bundle,
        crate::preview::now_ms() / 1000,
    )?;
    if restrictions.devices != artifact.permitted_devices
        || restrictions.device_id_type != artifact.device_id_type
    {
        return Err("DEVICE_PROFILE_CHANGED".into());
    }
    Ok(())
}

pub fn build(
    project: &ProjectRef,
    sdk: &Path,
    run: &Path,
    cancel: &AtomicBool,
) -> Result<Artifact, String> {
    check_cancel(cancel)?;
    let fingerprint = fingerprint(project)?;
    let root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let module = crate::preview::configuration::module_directory(project)?;
    crate::preview::dependencies::preflight(&root, &module)?;
    let profile = read_json(&root.join("build-profile.json5"))?;
    let (product, target) = build_selection(&profile, project)?;
    let expected = source_identity(&root, &module, &profile, &product, project)?;
    // Every preparation owns fresh verification files, even when run is reused.
    fs::create_dir_all(run).map_err(|_| "DEVICE_BUILD_DIRECTORY")?;
    let preparation = run.join(format!("prepare-{}", super::fresh_id()));
    fs::create_dir(&preparation).map_err(|_| "DEVICE_BUILD_DIRECTORY")?;
    let run = preparation.as_path();
    fs::create_dir_all(run.join("hvigor-cache")).map_err(|_| "DEVICE_BUILD_DIRECTORY")?;
    let node = sdk.join("tool/node/node.exe");
    let wrapper = sdk.join("hvigor/bin/hvigorw.js");
    if !node.is_file() || !wrapper.is_file() {
        return Err("DEVICE_BUILD_TOOLCHAIN_MISSING".into());
    }
    let args = vec![
        wrapper.to_string_lossy().into_owned(),
        "--mode".into(),
        "module".into(),
        "-p".into(),
        format!("module={}@{target}", project.module),
        "-p".into(),
        format!("product={product}"),
        "-p".into(),
        "buildMode=debug".into(),
        "assembleHap".into(),
        "--no-daemon".into(),
        "--config".into(),
        format!("build-cache-dir={}", run.join("hvigor-cache").display()),
    ];
    let env = vec![
        (
            "DEVECO_SDK_HOME".into(),
            sdk.join("sdk").to_string_lossy().into_owned(),
        ),
        (
            "DEVECO_NODE_HOME".into(),
            sdk.join("tool/node").to_string_lossy().into_owned(),
        ),
        (
            "HVIGOR_USER_HOME".into(),
            root.join(".hvigor/arkdock-home")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    if self::fingerprint(project)? != fingerprint {
        return Err("TRUST_CHANGED_DURING_DEVICE_BUILD".into());
    }
    let output = ToolCommandRunner::run_limited(
        &node,
        &args,
        &root,
        &env,
        Duration::from_secs(300),
        cancel,
        16 * 1024 * 1024,
        1024 * 1024,
    )?;
    if !build_succeeded(&output, &project.module, &target) {
        return Err(format!("DEVICE_BUILD_FAILED: {}", output.diagnostic()));
    }
    if self::fingerprint(project)? != fingerprint {
        return Err("TRUST_CHANGED_DURING_DEVICE_BUILD".into());
    }
    let output_root = module
        .join("build")
        .join(&product)
        .join("outputs")
        .join(&target);
    let mut haps = Vec::new();
    collect_haps(&output_root, &module, 0, &mut haps)?;
    // A successful UP-TO-DATE graph legitimately retains old file mtimes.
    haps.sort();
    if haps.is_empty() {
        return Err("SIGNED_HAP_REQUIRED: 构建没有 HAP 产物，请检查模块打包及签名配置。".into());
    }
    // A filename is not signature evidence. Verify every bounded candidate and
    // require one uniquely valid package for the selected module.
    let java = find_java(sdk)?;
    let signer = sdk.join("sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar");
    let unpacker = sdk.join("sdk/default/openharmony/toolchains/lib/app_unpacking_tool.jar");
    if !signer.is_file() || !unpacker.is_file() {
        return Err("DEVICE_SIGNATURE_TOOLS_MISSING".into());
    }
    let mut valid = Vec::new();
    for (index, hap) in haps.iter().enumerate() {
        check_cancel(cancel)?;
        let attempt = run.join(format!("artifact-{index}"));
        fs::create_dir(&attempt).map_err(|_| "DEVICE_ARTIFACT_DIRECTORY")?;
        let owned = attempt.join("application.hap");
        let sha256 = copy_artifact(hap, &owned)?;
        let cert = attempt.join("cert-chain.pem");
        let provision = attempt.join("profile.p7b");
        let args = vec![
            "-jar".into(),
            signer.to_string_lossy().into_owned(),
            "verify-app".into(),
            "-inFile".into(),
            owned.to_string_lossy().into_owned(),
            "-outCertChain".into(),
            cert.to_string_lossy().into_owned(),
            "-outProfile".into(),
            provision.to_string_lossy().into_owned(),
        ];
        let verified =
            ToolCommandRunner::run(&java, &args, &attempt, &[], Duration::from_secs(30), cancel)?;
        if !signer_succeeded(&verified, "verify-app")
            || !nonempty_bounded(&cert)
            || !nonempty_bounded(&provision)
        {
            continue;
        }
        let verified_profile = attempt.join("verified-profile.json");
        let args = vec![
            "-jar".into(),
            signer.to_string_lossy().into_owned(),
            "verify-profile".into(),
            "-inFile".into(),
            provision.to_string_lossy().into_owned(),
            "-outFile".into(),
            verified_profile.to_string_lossy().into_owned(),
        ];
        let result =
            ToolCommandRunner::run(&java, &args, &attempt, &[], Duration::from_secs(30), cancel)?;
        if !signer_succeeded(&result, "verify-profile") || !nonempty_bounded(&verified_profile) {
            continue;
        }
        let unpacked = attempt.join("unpacked");
        let args = vec![
            "-jar".into(),
            unpacker.to_string_lossy().into_owned(),
            "--mode".into(),
            "hap".into(),
            "--hap-path".into(),
            owned.to_string_lossy().into_owned(),
            "--out-path".into(),
            unpacked.to_string_lossy().into_owned(),
            "--force".into(),
            "false".into(),
        ];
        let result =
            ToolCommandRunner::run(&java, &args, &attempt, &[], Duration::from_secs(30), cancel)?;
        if result.status != 0 {
            return Err("DEVICE_PACKAGE_METADATA_FAILED".into());
        }
        let manifest_path = unpacked
            .join("module.json")
            .canonicalize()
            .map_err(|_| "DEVICE_PACKAGE_METADATA_FAILED")?;
        if !manifest_path.starts_with(
            attempt
                .canonicalize()
                .map_err(|_| "DEVICE_ARTIFACT_DIRECTORY")?,
        ) {
            return Err("DEVICE_PACKAGE_METADATA_OUTSIDE_ARTIFACT".into());
        }
        let manifest = read_json(&manifest_path)?;
        let (bundle, module_name, ability) = package_identity(&manifest)?;
        if (bundle.clone(), module_name.clone(), ability.clone()) != expected {
            continue;
        }
        let profile = read_json(&verified_profile)?;
        let restrictions = verify_profile(&profile, &bundle, crate::preview::now_ms() / 1000)?;
        if file_hash(&owned)? != sha256 {
            return Err("DEVICE_ARTIFACT_CHANGED".into());
        }
        valid.push(Artifact {
            sha256,
            path: owned,
            bundle,
            module: module_name,
            ability,
            fingerprint: fingerprint.clone(),
            permitted_devices: restrictions.devices,
            device_id_type: restrictions.device_id_type,
            confirmation: super::fresh_id(),
            prepared_at: crate::preview::now_ms(),
            verified_profile: profile,
        });
    }
    check_cancel(cancel)?;
    if self::fingerprint(project)? != fingerprint {
        return Err("TRUST_CHANGED_DURING_DEVICE_BUILD".into());
    }
    if valid.len() != 1 {
        return Err(
            "SIGNED_HAP_REQUIRED: 需要唯一、可验证签名且与当前模块匹配的 HAP；请先配置工程签名。"
                .into(),
        );
    }
    Ok(valid.remove(0))
}

pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|_| "DEVICE_ARTIFACT_MISSING")?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    let mut total = 0;
    loop {
        let count = file.read(&mut buf).map_err(|_| "DEVICE_ARTIFACT_READ")?;
        if count == 0 {
            break;
        }
        total += count;
        if total > 256 * 1024 * 1024 {
            return Err("DEVICE_ARTIFACT_LIMIT".into());
        }
        hash.update(&buf[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = crate::workspace::read_bounded(path, 1024 * 1024)?;
    json5::from_str(std::str::from_utf8(&bytes).map_err(|_| "DEVICE_METADATA_ENCODING")?)
        .map_err(|_| "DEVICE_METADATA_INVALID".into())
}
fn token(value: &str) -> Result<String, String> {
    super::plans::validate_identifier(value)?;
    Ok(value.into())
}
fn package_identity(value: &Value) -> Result<(String, String, String), String> {
    let bundle = token(
        value
            .pointer("/app/bundleName")
            .and_then(Value::as_str)
            .ok_or("DEVICE_BUNDLE_MISSING")?,
    )?;
    let module = value.get("module").ok_or("DEVICE_MODULE_MISSING")?;
    let name = token(
        module
            .get("name")
            .and_then(Value::as_str)
            .ok_or("DEVICE_MODULE_MISSING")?,
    )?;
    let ability = token(
        module
            .get("mainElement")
            .and_then(Value::as_str)
            .ok_or("DEVICE_ABILITY_MISSING")?,
    )?;
    if !module
        .get("abilities")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|item| item.get("name").and_then(Value::as_str) == Some(&ability))
        })
    {
        return Err("DEVICE_ABILITY_NOT_DECLARED".into());
    }
    Ok((bundle, name, ability))
}
fn build_selection(value: &Value, project: &ProjectRef) -> Result<(String, String), String> {
    for name in [&project.module, &project.variant] {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("DEVICE_BUILD_TARGET_INVALID".into());
        }
    }
    let products = value
        .pointer("/app/products")
        .and_then(Value::as_array)
        .ok_or("DEVICE_PRODUCT_MISSING")?;
    let product = products
        .iter()
        .find_map(|p| {
            p.get("name")
                .and_then(Value::as_str)
                .filter(|name| *name == project.variant)
        })
        .ok_or("DEVICE_PRODUCT_NOT_DECLARED")?;
    let module = value
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|m| {
            m.iter()
                .find(|m| m.get("name").and_then(Value::as_str) == Some(&project.module))
        })
        .ok_or("DEVICE_MODULE_MISSING")?;
    let targets = module
        .get("targets")
        .and_then(Value::as_array)
        .ok_or("DEVICE_TARGET_NOT_DECLARED")?;
    let matching: Vec<_> = targets
        .iter()
        .filter(|target| {
            target
                .get("applyToProducts")
                .and_then(Value::as_array)
                .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(product)))
        })
        .collect();
    let selected = if matching.len() == 1 {
        matching[0]
    } else {
        return Err("DEVICE_BUILD_TARGET_AMBIGUOUS".into());
    };
    let target = selected
        .get("name")
        .and_then(Value::as_str)
        .ok_or("DEVICE_TARGET_NOT_DECLARED")?;
    if target.is_empty()
        || target.len() > 128
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("DEVICE_BUILD_TARGET_INVALID".into());
    }
    Ok((
        token(product)?,
        token(
            selected
                .get("name")
                .and_then(Value::as_str)
                .ok_or("DEVICE_TARGET_NOT_DECLARED")?,
        )?,
    ))
}
fn collect_haps(
    path: &Path,
    module: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > 4 {
        return Err("DEVICE_ARTIFACT_LIMIT".into());
    }
    for entry in fs::read_dir(path).map_err(|_| "DEVICE_BUILD_OUTPUT_MISSING")? {
        let entry = entry.map_err(|_| "DEVICE_ARTIFACT_READ")?;
        let metadata = entry.file_type().map_err(|_| "DEVICE_ARTIFACT_READ")?;
        if metadata.is_symlink() {
            return Err("DEVICE_ARTIFACT_LINK_REJECTED".into());
        }
        let path = entry
            .path()
            .canonicalize()
            .map_err(|_| "DEVICE_ARTIFACT_MISSING")?;
        if !path.starts_with(module) {
            return Err("DEVICE_ARTIFACT_OUTSIDE_PROJECT".into());
        }
        if metadata.is_dir() {
            collect_haps(&path, module, depth + 1, out)?;
        } else if path.extension().is_some_and(|e| e == "hap") {
            if out.len() >= 16 {
                return Err("DEVICE_ARTIFACT_LIMIT".into());
            }
            out.push(path);
        }
    }
    Ok(())
}
fn find_java(sdk: &Path) -> Result<PathBuf, String> {
    let mut candidates = vec![
        sdk.join("jbr/bin/java.exe"),
        sdk.join("tool/jbr/bin/java.exe"),
        sdk.join("tool/jdk/bin/java.exe"),
    ];
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin/java.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|path| path.join("java.exe")));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("JAVA_REQUIRED: HAP 验签需要本机 JDK，请配置 JAVA_HOME。".into())
}
struct ProfileRestrictions {
    devices: Vec<String>,
    device_id_type: Option<String>,
}

fn verify_profile(value: &Value, bundle: &str, now: u64) -> Result<ProfileRestrictions, String> {
    // Installed SDK26 VerificationResult + VerifyHelper: GSON emits this exact
    // success envelope. Its `content` is the verified CMS payload, not `profile`.
    if value.get("verifiedPassed").and_then(Value::as_bool) != Some(true)
        || value.get("message").and_then(Value::as_str) != Some("OK")
    {
        return Err("DEVICE_PROFILE_SIGNATURE_UNVERIFIED".into());
    }
    let profile = value
        .get("content")
        .filter(|value| value.is_object())
        .ok_or("DEVICE_PROFILE_INVALID")?;
    if profile
        .pointer("/bundle-info/bundle-name")
        .and_then(Value::as_str)
        != Some(bundle)
    {
        return Err("DEVICE_PROFILE_BUNDLE_MISMATCH".into());
    }
    let valid = profile
        .get("validity")
        .ok_or("DEVICE_PROFILE_VALIDITY_MISSING")?;
    let start = valid
        .get("not-before")
        .and_then(Value::as_u64)
        .ok_or("DEVICE_PROFILE_VALIDITY_MISSING")?;
    let end = valid
        .get("not-after")
        .and_then(Value::as_u64)
        .ok_or("DEVICE_PROFILE_VALIDITY_MISSING")?;
    if start >= end || now < start || now >= end {
        return Err("DEVICE_PROFILE_EXPIRED".into());
    }
    let kind = profile
        .get("type")
        .and_then(Value::as_str)
        .ok_or("DEVICE_PROFILE_TYPE_MISSING")?;
    if !matches!(kind, "debug" | "release") {
        return Err("DEVICE_PROFILE_TYPE_UNSUPPORTED".into());
    }
    let Some(debug) = profile.get("debug-info") else {
        return if kind == "release" {
            Ok(ProfileRestrictions {
                devices: Vec::new(),
                device_id_type: None,
            })
        } else {
            Err("DEVICE_PROFILE_DEBUG_DEVICES_REQUIRED".into())
        };
    };
    let id_type = debug
        .get("device-id-type")
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "sn" | "udid"))
        .ok_or("DEVICE_PROFILE_DEVICE_ID_TYPE")?;
    let ids = debug
        .get("device-ids")
        .and_then(Value::as_array)
        .ok_or("DEVICE_PROFILE_DEBUG_DEVICES_REQUIRED")?;
    if ids.is_empty() || ids.len() > 100 {
        return Err("DEVICE_PROFILE_DEBUG_DEVICES_REQUIRED".into());
    }
    let mut devices = std::collections::BTreeSet::new();
    for id in ids {
        let id = id.as_str().ok_or("DEVICE_PROFILE_DEVICE_ID_INVALID")?;
        if id.is_empty()
            || id.len() > 256
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err("DEVICE_PROFILE_DEVICE_ID_INVALID".into());
        }
        devices.insert(id.to_owned());
    }
    Ok(ProfileRestrictions {
        devices: devices.into_iter().collect(),
        device_id_type: Some(id_type.into()),
    })
}

fn source_identity(
    root: &Path,
    module: &Path,
    profile: &Value,
    product: &str,
    project: &ProjectRef,
) -> Result<(String, String, String), String> {
    let mut app = read_json(&root.join("AppScope/app.json5"))?;
    if !app.get("app").is_some_and(Value::is_object) {
        return Err("DEVICE_APP_MANIFEST_INVALID".into());
    }
    // ProductBuildOpt.bundleName is an SDK-supported AppScope override.
    if let Some(bundle) = profile
        .pointer("/app/products")
        .and_then(Value::as_array)
        .and_then(|products| {
            products
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(product))
        })
        .and_then(|item| item.get("bundleName"))
    {
        app["app"]["bundleName"] = bundle.clone();
    }
    let source = read_json(&module.join("src/main/module.json5"))?;
    app["module"] = source.get("module").ok_or("DEVICE_MODULE_MISSING")?.clone();
    let identity = package_identity(&app)?;
    if identity.1 != project.module {
        return Err("DEVICE_MODULE_SOURCE_MISMATCH".into());
    }
    Ok(identity)
}

fn copy_artifact(source: &Path, owned: &Path) -> Result<String, String> {
    let before = file_hash(source)?;
    let input = fs::File::open(source).map_err(|_| "DEVICE_ARTIFACT_MISSING")?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(owned)
        .map_err(|_| "DEVICE_ARTIFACT_COPY")?;
    let copied = std::io::copy(&mut input.take(256 * 1024 * 1024 + 1), &mut output)
        .map_err(|_| "DEVICE_ARTIFACT_COPY")?;
    drop(output);
    if copied > 256 * 1024 * 1024 {
        return Err("DEVICE_ARTIFACT_LIMIT".into());
    }
    if file_hash(source)? != before || file_hash(owned)? != before {
        return Err("DEVICE_ARTIFACT_CHANGED_DURING_COPY".into());
    }
    Ok(before)
}

fn nonempty_bounded(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= 1024 * 1024
    })
}

fn plain_log(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

fn build_succeeded(output: &ToolOutput, module: &str, target: &str) -> bool {
    let log = plain_log(&format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    let tasks = [
        format!(":{module}:assembleHap..."),
        format!(":{module}:{target}@assembleHap..."),
    ];
    output.status == 0
        && !log
            .lines()
            .any(|line| line.starts_with("> hvigor ERROR:") || line.contains("BUILD FAILED"))
        && log
            .lines()
            .any(|line| line.starts_with("> hvigor BUILD SUCCESSFUL in "))
        && log.lines().any(|line| {
            (line.starts_with("> hvigor Finished ") || line.starts_with("> hvigor UP-TO-DATE "))
                && tasks.iter().any(|task| line.contains(task))
        })
}

fn signer_succeeded(output: &ToolOutput, operation: &str) -> bool {
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.status == 0
        && log
            .lines()
            .any(|line| line.trim_end().ends_with(&format!("{operation} success")))
        && !log
            .lines()
            .any(|line| line.trim_end().ends_with(&format!("{operation} failed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_identity_requires_declared_ability() {
        let mut v = serde_json::json!({"app":{"bundleName":"com.test.app"},"module":{"name":"entry","mainElement":"EntryAbility","abilities":[{"name":"EntryAbility"}]}});
        assert!(package_identity(&v).is_ok());
        v["module"]["abilities"] = serde_json::json!([]);
        assert!(package_identity(&v).is_err());
    }
    #[test]
    fn profile_must_match_bundle_and_validity() {
        let p = verified_profile();
        assert_eq!(
            verify_profile(&p, "com.test.app", 15).unwrap().devices,
            vec!["exact-target"]
        );
        assert!(verify_profile(&p, "other", 15).is_err());
        assert!(verify_profile(&p, "com.test.app", 21).is_err());
    }

    fn verified_profile() -> Value {
        serde_json::json!({"verifiedPassed":true,"message":"OK","content":{"type":"debug","bundle-info":{"bundle-name":"com.test.app"},"validity":{"not-before":10,"not-after":20},"debug-info":{"device-id-type":"udid","device-ids":["exact-target"]}}})
    }

    fn project(root: &Path) -> ProjectRef {
        ProjectRef {
            id: "w".into(),
            name: "fixture".into(),
            environment: crate::ipc::model::Environment::WindowsLocal,
            workspace_root: root.to_string_lossy().into_owned(),
            app_root: root.to_string_lossy().into_owned(),
            module: "entry".into(),
            variant: "phone".into(),
            sdk_id: None,
        }
    }

    #[test]
    fn explicit_verification_failure_or_malformed_debug_devices_never_become_unrestricted() {
        let original = verified_profile();
        assert!(verify_profile(&original["content"], "com.test.app", 15).is_err());
        let mut p = original.clone();
        p["verifiedPassed"] = serde_json::json!(false);
        assert!(verify_profile(&p, "com.test.app", 15).is_err());
        let mut p = original.clone();
        p["message"] = serde_json::json!("signature invalid");
        assert!(verify_profile(&p, "com.test.app", 15).is_err());
        assert!(verify_profile(&original, "com.test.app", 20).is_err());
        for ids in [
            serde_json::json!([]),
            serde_json::json!([null]),
            serde_json::json!(["*"]),
        ] {
            let mut p = original.clone();
            p["content"]["debug-info"]["device-ids"] = ids;
            assert!(verify_profile(&p, "com.test.app", 15).is_err());
        }
        let mut p = original;
        p["content"].as_object_mut().unwrap().remove("debug-info");
        assert!(verify_profile(&p, "com.test.app", 15).is_err());
        p["content"]["type"] = serde_json::json!("release");
        assert!(verify_profile(&p, "com.test.app", 15)
            .unwrap()
            .devices
            .is_empty());
    }

    #[test]
    fn target_mapping_never_selects_a_target_declared_for_another_product() {
        let mut value = serde_json::json!({"app":{"products":[{"name":"phone"}]},"modules":[{"name":"entry","targets":[{"name":"handset","applyToProducts":["phone"]}]}]});
        assert_eq!(
            build_selection(&value, &project(Path::new("."))).unwrap(),
            ("phone".into(), "handset".into())
        );
        value["modules"][0]["targets"][0]["applyToProducts"] = serde_json::json!(["tablet"]);
        assert!(build_selection(&value, &project(Path::new("."))).is_err());
    }

    #[test]
    fn cached_build_requires_selected_graph_and_stable_copy_not_new_mtime() {
        let output = ToolOutput {
            status: 0,
            stdout: b"> hvigor UP-TO-DATE :entry:assembleHap...\n> hvigor BUILD SUCCESSFUL in 1 s"
                .to_vec(),
            stderr: vec![],
        };
        assert!(build_succeeded(&output, "entry", "handset"));
        assert!(!build_succeeded(&output, "other", "handset"));
        let mut failed = output.clone();
        failed.stderr = b"> hvigor ERROR: BUILD FAILED".to_vec();
        assert!(!build_succeeded(&failed, "entry", "handset"));
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("old.hap");
        fs::write(&source, b"fixture package").unwrap();
        let owned = root.path().join("owned.hap");
        assert_eq!(
            copy_artifact(&source, &owned).unwrap(),
            file_hash(&source).unwrap()
        );
        assert!(
            copy_artifact(&source, &owned).is_err(),
            "never reuse earlier signature evidence"
        );
    }

    #[test]
    fn application_identity_is_fingerprinted_and_signer_needs_positive_receipt() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("AppScope")).unwrap();
        let manifest = root.path().join("AppScope/app.json5");
        fs::write(&manifest, b"{app:{bundleName:'com.test.one'}}").unwrap();
        let project = project(root.path());
        let before = fingerprint(&project).unwrap();
        fs::write(&manifest, b"{app:{bundleName:'com.test.two'}}").unwrap();
        assert_ne!(before, fingerprint(&project).unwrap());
        let mut output = ToolOutput {
            status: 0,
            stdout: b"verify-app failed".to_vec(),
            stderr: vec![],
        };
        assert!(!signer_succeeded(&output, "verify-app"));
        output.stdout = b"INFO - verify-app success\n".to_vec();
        assert!(signer_succeeded(&output, "verify-app"));
    }

    #[test]
    fn source_change_invalidates_install_even_without_a_running_preview_watcher() {
        let root = tempfile::tempdir().unwrap();
        let run = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("AppScope")).unwrap();
        fs::write(
            root.path().join("AppScope/app.json5"),
            b"{app:{bundleName:'com.test.app'}}",
        )
        .unwrap();
        let source = root.path().join("Index.ets");
        fs::write(&source, b"original source").unwrap();
        let project = project(root.path());
        let path = run.path().join("application.hap");
        fs::write(&path, b"fixture artifact").unwrap();
        let mut profile = verified_profile();
        profile["content"]["validity"] = serde_json::json!({"not-before":1,"not-after":u64::MAX});
        let artifact = Artifact {
            sha256: file_hash(&path).unwrap(),
            path,
            bundle: "com.test.app".into(),
            module: "entry".into(),
            ability: "EntryAbility".into(),
            fingerprint: fingerprint(&project).unwrap(),
            permitted_devices: vec!["exact-target".into()],
            device_id_type: Some("udid".into()),
            confirmation: "fixture".into(),
            prepared_at: 1,
            verified_profile: profile,
        };
        assert!(revalidate(&project, &artifact).is_ok());
        for directory in ["build", ".hvigor", "node_modules"] {
            fs::create_dir(root.path().join(directory)).unwrap();
            fs::write(
                root.path().join(directory).join("generated"),
                b"ignored output",
            )
            .unwrap();
        }
        assert!(
            revalidate(&project, &artifact).is_ok(),
            "generated output does not invalidate its source graph"
        );
        fs::write(&source, b"edited source").unwrap();
        assert!(revalidate(&project, &artifact).is_err());
        fs::write(&source, b"original source").unwrap();
        fs::write(&artifact.path, b"tampered artifact").unwrap();
        assert!(revalidate(&project, &artifact).is_err());
    }
}
