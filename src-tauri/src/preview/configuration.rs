//! Trusted page artifacts and conservative native launch profiles. No SDK execution.
use crate::ipc::model::{NativeControlAvailability, PreviewDeviceProfile, ProjectRef};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const CONFIG_LIMIT: u64 = 128 * 1024;
pub const DEFAULT_PROFILE: &str = "phone-360x640";

pub fn profiles(sdk_available: bool) -> Vec<PreviewDeviceProfile> {
    [
        (DEFAULT_PROFILE, "Phone", "phone", 360, 640, 160, "rect", true),
        ("tablet-800x1280", "Tablet", "tablet", 800, 1280, 240, "rect", false),
        ("watch-454x454", "Watch", "wearable", 454, 454, 320, "circle", false),
    ].into_iter().map(|(id, label, device, width, height, dpi, shape, baseline)| PreviewDeviceProfile {
        id: id.into(), label: label.into(), device_type: device.into(), width, height, dpi,
        shape: shape.into(), project_model: "Stage".into(), requires_restart: true,
        status: if !sdk_available { NativeControlAvailability::Unavailable }
            else if baseline { NativeControlAvailability::Available } else { NativeControlAvailability::Blocked },
        reason: if baseline { "既有已核验 phone 启动配置；不证明扩展命令已通过验收".into() }
            else { "PROFILE_RUNTIME_DEPENDENCIES_UNVERIFIED: 官方启动参数存在，但当前 SDK 的设备资源与 Stage 兼容性未核验，禁止开放切换".into() },
    }).collect()
}

/// Typed startup arguments, never the empty DeviceType command. Caller must admit
/// the profile and own the stop/restart/rollback lifecycle before using these.
pub fn profile_arguments(profile: &PreviewDeviceProfile) -> Result<Vec<String>, String> {
    if profile.status != NativeControlAvailability::Available || profile.project_model != "Stage"
        || !matches!(profile.device_type.as_str(), "phone" | "tablet" | "wearable")
        || !matches!(profile.shape.as_str(), "rect" | "circle")
        || !(50..=3000).contains(&profile.width) || !(50..=3000).contains(&profile.height)
        || !(120..=640).contains(&profile.dpi) {
        return Err("PROFILE_NOT_ADMITTED".into());
    }
    Ok(vec!["-device".into(), profile.device_type.clone(), "-or".into(), profile.width.to_string(),
        profile.height.to_string(), "-cr".into(), profile.width.to_string(), profile.height.to_string(),
        "-sd".into(), profile.dpi.to_string(), "-shape".into(), profile.shape.clone()])
}

fn read_json(root: &Path, path: &Path) -> Result<Value, String> {
    let canonical = path.canonicalize().map_err(|_| "PAGE_CONFIG_MISSING")?;
    if !canonical.starts_with(root) { return Err("PAGE_CONFIG_OUTSIDE_PROJECT".into()); }
    let mut file = File::open(canonical).map_err(|_| "PAGE_CONFIG_UNREADABLE")?;
    let metadata = file.metadata().map_err(|_| "PAGE_CONFIG_UNREADABLE")?;
    if !metadata.is_file() || metadata.len() > CONFIG_LIMIT { return Err("PAGE_CONFIG_LIMIT".into()); }
    let mut text = String::new();
    Read::by_ref(&mut file).take(CONFIG_LIMIT + 1).read_to_string(&mut text).map_err(|_| "PAGE_CONFIG_ENCODING")?;
    if text.len() as u64 > CONFIG_LIMIT { return Err("PAGE_CONFIG_LIMIT".into()); }
    json5::from_str(&text).map_err(|_| "PAGE_CONFIG_JSON".into())
}

pub fn module_directory(project: &ProjectRef) -> Result<PathBuf, String> {
    let root = Path::new(&project.app_root).canonicalize().map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let profile = read_json(&root, &root.join("build-profile.json5"))?;
    let source = profile.get("modules").and_then(Value::as_array)
        .and_then(|modules| modules.iter().find(|module| module.get("name").and_then(Value::as_str) == Some(project.module.as_str())))
        .and_then(|module| module.get("srcPath")).and_then(Value::as_str).ok_or("MODULE_PATH_MISSING")?;
    let module = root.join(source).canonicalize().map_err(|_| "MODULE_PATH_MISSING")?;
    if !module.starts_with(root) { return Err("MODULE_OUTSIDE_PROJECT".into()); }
    Ok(module)
}

fn page_list(value: &Value) -> Result<Vec<String>, String> {
    let items = value.get("src").and_then(Value::as_array).ok_or("PAGES_SCHEMA_UNSUPPORTED")?;
    if items.is_empty() || items.len() > 128 { return Err("PAGES_LIMIT".into()); }
    let mut pages = Vec::new();
    for item in items {
        let page = item.as_str().ok_or("PAGE_PATH_INVALID")?;
        if page.len() > 256 || page.starts_with('/') || page.contains('\\')
            || page.split('/').any(|part| part.is_empty() || part == "." || part == ".."
                || !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
            || pages.iter().any(|existing| existing == page) { return Err("PAGE_PATH_INVALID".into()); }
        pages.push(page.to_owned());
    }
    Ok(pages)
}

pub struct CompiledPageProfile {
    /// Previewer's -pages argument is the profile basename, without .json.
    pub name: String,
    pub pages: Vec<String>,
}

pub fn page_profile_name(project: &ProjectRef) -> Result<String, String> {
    let module = module_directory(project)?;
    let manifest = read_json(&module, &module.join("src/main/module.json5"))?;
    let name = manifest.get("module").and_then(|value| value.get("pages")).and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("$profile:")).ok_or("STAGE_PAGE_PROFILE_REQUIRED")?;
    if name.is_empty() || name.len() > 128 || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return Err("PAGE_PROFILE_INVALID".into());
    }
    Ok(name.to_owned())
}

/// Source pages alone are not compiled-route evidence. Require the actual
/// PreviewBuild resource copy to agree and a nonempty compiled ABC artifact.
/// The same checked profile name must be used by the Previewer launch.
pub fn compiled_page_profile(project: &ProjectRef) -> Result<CompiledPageProfile, String> {
    let module = module_directory(project)?;
    let reference = page_profile_name(project)?;
    let filename = format!("{reference}.json");
    let source = page_list(&read_json(&module, &module.join("src/main/resources/base/profile").join(&filename))?)?;
    let intermediates = module.join(".preview/default/intermediates");
    let abc = intermediates.join("loader_out/default/ets/modules.abc").canonicalize().map_err(|_| "COMPILED_PAGES_ARTIFACT_REQUIRED")?;
    if !abc.starts_with(&module) || fs::metadata(&abc).map_err(|_| "COMPILED_PAGES_ARTIFACT_REQUIRED")?.len() == 0 {
        return Err("COMPILED_PAGES_ARTIFACT_REQUIRED".into());
    }
    let compiled = ["res/default/resources/base/profile", "res/default/base/profile", "res/default/profile"]
        .into_iter().find_map(|directory| read_json(&module, &intermediates.join(directory).join(&filename)).ok())
        .ok_or("COMPILED_PAGE_PROFILE_REQUIRED: 未找到当前 PreviewBuild 的可核验页面资源，源码列表不能单独开放导航")?;
    if page_list(&compiled)? != source { return Err("COMPILED_PAGE_PROFILE_CHANGED".into()); }
    for page in &source {
        let file = module.join("src/main/ets").join(format!("{page}.ets")).canonicalize().map_err(|_| "PAGE_SOURCE_MISSING")?;
        if !file.starts_with(&module) || !file.is_file() { return Err("PAGE_SOURCE_OUTSIDE_PROJECT".into()); }
    }
    Ok(CompiledPageProfile { name: reference, pages: source })
}

pub fn compiled_pages(project: &ProjectRef) -> Result<Vec<String>, String> {
    compiled_page_profile(project).map(|profile| profile.pages)
}
