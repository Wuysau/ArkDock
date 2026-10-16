//! Trusted page artifacts and admitted native launch profiles. No SDK execution.
use crate::ipc::model::{NativeControlAvailability, PreviewDeviceProfile, ProjectRef};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const CONFIG_LIMIT: u64 = 128 * 1024;
pub const DEFAULT_PROFILE: &str = "phone-360x640";

pub fn profiles(sdk_available: bool) -> Vec<PreviewDeviceProfile> {
    [
        (DEFAULT_PROFILE, "Phone", "phone", 360, 640, 160, "rect"),
        (
            "tablet-800x1280",
            "Tablet",
            "tablet",
            800,
            1280,
            240,
            "rect",
        ),
        (
            "watch-454x454",
            "Watch",
            "wearable",
            454,
            454,
            320,
            "circle",
        ),
    ]
    .into_iter()
    .map(
        |(id, label, device, width, height, dpi, shape)| PreviewDeviceProfile {
            id: id.into(),
            label: label.into(),
            device_type: device.into(),
            width,
            height,
            dpi,
            shape: shape.into(),
            project_model: "Stage".into(),
            requires_restart: true,
            status: if !sdk_available {
                NativeControlAvailability::Unavailable
            } else {
                NativeControlAvailability::Available
            },
            reason: if sdk_available {
                "通过原生启动参数切换设备配置；重启后以实际首帧确认。".into()
            } else {
                "SDK_UNAVAILABLE: 尚未连接受支持的原生预览 SDK。".into()
            },
        },
    )
    .collect()
}

pub fn default_profile() -> PreviewDeviceProfile {
    profiles(true).remove(0)
}

/// All three rich Previewer device types share common resources. liteWearable
/// uses a different engine and is deliberately not admitted as wearable.
pub fn validate_profile_resources(sdk: &Path) -> Result<(), String> {
    let root = sdk.canonicalize().map_err(|_| "PROFILE_SDK_UNAVAILABLE")?;
    for (relative, directory) in [
        (
            "sdk/default/openharmony/previewer/common/resources/resources.index",
            false,
        ),
        ("sdk/default/openharmony/previewer/common/bin/fonts", true),
        ("sdk/default/openharmony/previewer/common/bin/module", true),
    ] {
        let resource = root
            .join(relative)
            .canonicalize()
            .map_err(|_| "PROFILE_RESOURCE_MISSING")?;
        if !resource.starts_with(&root) {
            return Err("PROFILE_RESOURCE_OUTSIDE_SDK".into());
        }
        let metadata = fs::metadata(resource).map_err(|_| "PROFILE_RESOURCE_UNREADABLE")?;
        if directory && !metadata.is_dir()
            || !directory && (!metadata.is_file() || metadata.len() == 0)
        {
            return Err("PROFILE_RESOURCE_INVALID".into());
        }
    }
    Ok(())
}

fn project_device_types(project: &ProjectRef) -> Result<Vec<String>, String> {
    let module = module_directory(project)?;
    let manifest = read_json(&module, &module.join("src/main/module.json5"))?;
    let types = manifest
        .get("module")
        .and_then(|value| value.get("deviceTypes"))
        .and_then(Value::as_array)
        .filter(|types| !types.is_empty() && types.len() <= 32)
        .ok_or("PROFILE_PROJECT_DEVICE_TYPES_REQUIRED")?;
    types
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 32)
                .map(str::to_owned)
                .ok_or_else(|| "PROFILE_PROJECT_DEVICE_TYPES_INVALID".into())
        })
        .collect()
}

pub fn validate_project_profile(
    project: &ProjectRef,
    profile: &PreviewDeviceProfile,
) -> Result<(), String> {
    let types = project_device_types(project)?;
    if types.iter().any(|device| {
        device == &profile.device_type || profile.device_type == "phone" && device == "default"
    }) {
        Ok(())
    } else {
        Err(format!(
            "PROFILE_PROJECT_DEVICE_UNSUPPORTED: module.json5 未声明 {}。",
            profile.device_type
        ))
    }
}

/// Catalog availability is refined by the installed resources and the selected
/// module's deviceTypes; this is eligibility, not evidence of a rendered frame.
pub fn profiles_for_project(
    sdk: Option<&Path>,
    sdk_available: bool,
    project: Option<&ProjectRef>,
) -> Vec<PreviewDeviceProfile> {
    let resources = sdk
        .ok_or_else(|| "PROFILE_SDK_UNAVAILABLE".to_owned())
        .and_then(validate_profile_resources);
    profiles(sdk_available)
        .into_iter()
        .map(|mut profile| {
            if sdk_available {
                let compatibility = resources.clone().and_then(|()| {
                    project
                        .ok_or_else(|| "PROFILE_PROJECT_REQUIRED".to_owned())
                        .and_then(|project| validate_project_profile(project, &profile))
                });
                if let Err(reason) = compatibility {
                    profile.status = NativeControlAvailability::Blocked;
                    profile.reason = reason;
                }
            }
            profile
        })
        .collect()
}

pub fn admit_profile(
    id: &str,
    project: &ProjectRef,
    sdk: &Path,
    sdk_available: bool,
) -> Result<PreviewDeviceProfile, String> {
    let profile = profiles_for_project(Some(sdk), sdk_available, Some(project))
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or("PROFILE_UNKNOWN")?;
    if profile.status != NativeControlAvailability::Available {
        return Err(profile.reason);
    }
    profile_arguments(&profile)?;
    Ok(profile)
}

/// Typed startup arguments, never the empty DeviceType command. Caller must admit
/// the profile and own the stop/restart/rollback lifecycle before using these.
pub fn profile_arguments(profile: &PreviewDeviceProfile) -> Result<Vec<String>, String> {
    if profile.status != NativeControlAvailability::Available
        || profile.project_model != "Stage"
        || !matches!(
            profile.device_type.as_str(),
            "phone" | "tablet" | "wearable"
        )
        || !matches!(profile.shape.as_str(), "rect" | "circle")
        || !(50..=3000).contains(&profile.width)
        || !(50..=3000).contains(&profile.height)
        || !(120..=640).contains(&profile.dpi)
        || u64::from(profile.width) * u64::from(profile.height) > super::frames::MAX_SURFACE_PIXELS
        || !profiles(true).iter().any(|known| {
            known.id == profile.id
                && known.device_type == profile.device_type
                && known.width == profile.width
                && known.height == profile.height
                && known.dpi == profile.dpi
                && known.shape == profile.shape
                && profile.requires_restart
        })
    {
        return Err("PROFILE_NOT_ADMITTED".into());
    }
    Ok(vec![
        "-device".into(),
        profile.device_type.clone(),
        "-or".into(),
        profile.width.to_string(),
        profile.height.to_string(),
        "-cr".into(),
        profile.width.to_string(),
        profile.height.to_string(),
        "-sd".into(),
        profile.dpi.to_string(),
        "-shape".into(),
        profile.shape.clone(),
    ])
}

fn read_json(root: &Path, path: &Path) -> Result<Value, String> {
    let canonical = path.canonicalize().map_err(|_| "PAGE_CONFIG_MISSING")?;
    if !canonical.starts_with(root) {
        return Err("PAGE_CONFIG_OUTSIDE_PROJECT".into());
    }
    let mut file = File::open(canonical).map_err(|_| "PAGE_CONFIG_UNREADABLE")?;
    let metadata = file.metadata().map_err(|_| "PAGE_CONFIG_UNREADABLE")?;
    if !metadata.is_file() || metadata.len() > CONFIG_LIMIT {
        return Err("PAGE_CONFIG_LIMIT".into());
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(CONFIG_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|_| "PAGE_CONFIG_ENCODING")?;
    if text.len() as u64 > CONFIG_LIMIT {
        return Err("PAGE_CONFIG_LIMIT".into());
    }
    json5::from_str(&text).map_err(|_| "PAGE_CONFIG_JSON".into())
}

pub fn module_directory(project: &ProjectRef) -> Result<PathBuf, String> {
    let root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let profile = read_json(&root, &root.join("build-profile.json5"))?;
    let source = profile
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules.iter().find(|module| {
                module.get("name").and_then(Value::as_str) == Some(project.module.as_str())
            })
        })
        .and_then(|module| module.get("srcPath"))
        .and_then(Value::as_str)
        .ok_or("MODULE_PATH_MISSING")?;
    let module = root
        .join(source)
        .canonicalize()
        .map_err(|_| "MODULE_PATH_MISSING")?;
    if !module.starts_with(root) {
        return Err("MODULE_OUTSIDE_PROJECT".into());
    }
    Ok(module)
}

fn page_list(value: &Value) -> Result<Vec<String>, String> {
    let items = value
        .get("src")
        .and_then(Value::as_array)
        .ok_or("PAGES_SCHEMA_UNSUPPORTED")?;
    if items.is_empty() || items.len() > 128 {
        return Err("PAGES_LIMIT".into());
    }
    let mut pages = Vec::new();
    for item in items {
        let page = item.as_str().ok_or("PAGE_PATH_INVALID")?;
        if page.len() > 256
            || page.starts_with('/')
            || page.contains('\\')
            || page.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
            || pages.iter().any(|existing| existing == page)
        {
            return Err("PAGE_PATH_INVALID".into());
        }
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
    let name = manifest
        .get("module")
        .and_then(|value| value.get("pages"))
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("$profile:"))
        .ok_or("STAGE_PAGE_PROFILE_REQUIRED")?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("PAGE_PROFILE_INVALID".into());
    }
    Ok(name.to_owned())
}

/// Source pages alone are not compiled-route evidence. Require the actual
/// PreviewBuild resource copy to agree and a nonempty compiled ABC artifact.
/// The same checked profile name must be used by the Previewer launch.
pub fn compiled_page_profile(project: &ProjectRef) -> Result<CompiledPageProfile, String> {
    let module = module_directory(project)?;
    compiled_page_profile_in(project, &module)
}

pub fn compiled_page_profile_at(
    project: &ProjectRef,
    artifacts: &super::route_adapter::BuildArtifactContext,
) -> Result<CompiledPageProfile, String> {
    let original_root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let original_module = module_directory(project)?;
    let root = artifacts
        .root
        .canonicalize()
        .map_err(|_| "BUILD_ARTIFACT_ROOT_MISSING")?;
    let module = artifacts
        .module
        .canonicalize()
        .map_err(|_| "BUILD_ARTIFACT_MODULE_MISSING")?;
    if !module.starts_with(&root)
        || module.strip_prefix(&root).ok() != original_module.strip_prefix(&original_root).ok()
    {
        return Err("BUILD_ARTIFACT_MODULE_MISMATCH".into());
    }
    compiled_page_profile_in(project, &module)
}

fn compiled_page_profile_in(
    project: &ProjectRef,
    module: &Path,
) -> Result<CompiledPageProfile, String> {
    let original_module = module_directory(project)?;
    let reference = page_profile_name(project)?;
    let filename = format!("{reference}.json");
    let source = page_list(&read_json(
        &original_module,
        &original_module
            .join("src/main/resources/base/profile")
            .join(&filename),
    )?)?;
    let intermediates = module.join(".preview/default/intermediates");
    let abc = intermediates
        .join("loader_out/default/ets/modules.abc")
        .canonicalize()
        .map_err(|_| "COMPILED_PAGES_ARTIFACT_REQUIRED")?;
    if !abc.starts_with(module)
        || fs::metadata(&abc)
            .map_err(|_| "COMPILED_PAGES_ARTIFACT_REQUIRED")?
            .len()
            == 0
    {
        return Err("COMPILED_PAGES_ARTIFACT_REQUIRED".into());
    }
    let compiled = ["res/default/resources/base/profile", "res/default/base/profile", "res/default/profile"]
        .into_iter().find_map(|directory| read_json(module, &intermediates.join(directory).join(&filename)).ok())
        .ok_or("COMPILED_PAGE_PROFILE_REQUIRED: 未找到当前 PreviewBuild 的可核验页面资源，源码列表不能单独开放导航")?;
    if page_list(&compiled)? != source {
        return Err("COMPILED_PAGE_PROFILE_CHANGED".into());
    }
    for page in &source {
        let file = module
            .join("src/main/ets")
            .join(format!("{page}.ets"))
            .canonicalize()
            .map_err(|_| "PAGE_SOURCE_MISSING")?;
        if !file.starts_with(module) || !file.is_file() {
            return Err("PAGE_SOURCE_OUTSIDE_PROJECT".into());
        }
    }
    Ok(CompiledPageProfile {
        name: reference,
        pages: source,
    })
}

pub fn compiled_pages(project: &ProjectRef) -> Result<Vec<String>, String> {
    compiled_page_profile(project).map(|profile| profile.pages)
}

pub fn compiled_pages_at(
    project: &ProjectRef,
    artifacts: &super::route_adapter::BuildArtifactContext,
) -> Result<Vec<String>, String> {
    compiled_page_profile_at(project, artifacts).map(|profile| profile.pages)
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use crate::ipc::model::Environment;
    use serde_json::json;

    fn project(root: &Path, devices: &[&str]) -> ProjectRef {
        let module = root.join("entry/src/main");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            root.join("build-profile.json5"),
            r#"{"modules":[{"name":"entry","srcPath":"entry"}]}"#,
        )
        .unwrap();
        fs::write(
            module.join("module.json5"),
            serde_json::to_vec(&json!({"module":{"deviceTypes":devices}})).unwrap(),
        )
        .unwrap();
        ProjectRef {
            id: "project".into(),
            name: "App".into(),
            environment: Environment::WindowsLocal,
            workspace_root: root.to_string_lossy().into_owned(),
            app_root: root.to_string_lossy().into_owned(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        }
    }

    fn resources(root: &Path) {
        let common = root.join("sdk/default/openharmony/previewer/common");
        for dir in ["resources", "bin/fonts", "bin/module"] {
            fs::create_dir_all(common.join(dir)).unwrap();
        }
        fs::write(
            common.join("resources/resources.index"),
            b"fixture-resource-index",
        )
        .unwrap();
    }

    #[test]
    fn profile_arguments_use_public_startup_flags_for_all_supported_rich_devices() {
        for profile in profiles(true) {
            let args = profile_arguments(&profile).unwrap();
            assert_eq!(
                &args[0..2],
                &["-device".to_owned(), profile.device_type.clone()]
            );
            assert_eq!(
                &args[2..5],
                &[
                    "-or".to_owned(),
                    profile.width.to_string(),
                    profile.height.to_string()
                ]
            );
            assert_eq!(
                &args[5..8],
                &[
                    "-cr".to_owned(),
                    profile.width.to_string(),
                    profile.height.to_string()
                ]
            );
            assert_eq!(
                &args[8..12],
                &[
                    "-sd".to_owned(),
                    profile.dpi.to_string(),
                    "-shape".to_owned(),
                    profile.shape.clone()
                ]
            );
            assert!(!args.iter().any(|arg| arg.is_empty() || arg == "DeviceType"));
        }
        let mut forged = default_profile();
        forged.width = 801;
        assert!(profile_arguments(&forged).is_err());
        let mut lite = profiles(true).remove(2);
        lite.device_type = "liteWearable".into();
        assert!(profile_arguments(&lite).is_err());
    }

    #[test]
    fn available_profiles_match_selected_module_device_types() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), &["phone", "tablet"]);
        resources(temp.path());
        let catalog = profiles_for_project(Some(temp.path()), true, Some(&project));
        assert_eq!(catalog[0].status, NativeControlAvailability::Available);
        assert_eq!(catalog[1].status, NativeControlAvailability::Available);
        assert_eq!(catalog[2].status, NativeControlAvailability::Blocked);
        assert!(admit_profile("watch-454x454", &project, temp.path(), true)
            .unwrap_err()
            .contains("PROFILE_PROJECT_DEVICE_UNSUPPORTED"));
        assert_eq!(
            admit_profile("unknown", &project, temp.path(), true).unwrap_err(),
            "PROFILE_UNKNOWN"
        );
    }

    #[test]
    fn default_device_is_phone_compatible_but_not_tablet_or_watch() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), &["default"]);
        resources(temp.path());
        let catalog = profiles_for_project(Some(temp.path()), true, Some(&project));
        assert_eq!(catalog[0].status, NativeControlAvailability::Available);
        assert!(catalog[1..]
            .iter()
            .all(|profile| profile.status == NativeControlAvailability::Blocked));
    }

    #[test]
    fn watch_requires_wearable_instead_of_litewearable() {
        let temp = tempfile::tempdir().unwrap();
        let watch = profiles(true).remove(2);
        let lite = project(temp.path(), &["liteWearable"]);
        assert!(validate_project_profile(&lite, &watch).is_err());
        let rich = project(temp.path(), &["wearable"]);
        assert!(validate_project_profile(&rich, &watch).is_ok());
    }

    #[test]
    fn staged_pages_require_staged_artifacts_and_match_current_original_profile() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        let project = project(&original, &["phone"]);
        let original_module = original.join("entry");
        fs::write(
            original_module.join("src/main/module.json5"),
            r#"{"module":{"deviceTypes":["phone"],"pages":"$profile:app_pages"}}"#,
        )
        .unwrap();
        let root = temp.path().join("snapshot");
        let module = root.join("entry");
        for source_module in [&original_module, &module] {
            fs::create_dir_all(source_module.join("src/main/ets/pages")).unwrap();
            fs::create_dir_all(source_module.join("src/main/resources/base/profile")).unwrap();
            fs::write(
                source_module.join("src/main/ets/pages/Home.ets"),
                "@Entry @Component struct Home {}",
            )
            .unwrap();
            fs::write(
                source_module.join("src/main/resources/base/profile/app_pages.json"),
                r#"{"src":["pages/Home"]}"#,
            )
            .unwrap();
        }
        let artifacts = super::super::route_adapter::BuildArtifactContext {
            root,
            module: module.clone(),
            route_count: 1,
            report_path: temp.path().join("report.json"),
            source_map_path: None,
            adapted: true,
            hvigor_home: None,
        };
        // A stale original build must not make an unbuilt snapshot navigable.
        for source_module in [&original_module, &module] {
            fs::create_dir_all(
                source_module.join(".preview/default/intermediates/loader_out/default/ets"),
            )
            .unwrap();
            fs::create_dir_all(
                source_module
                    .join(".preview/default/intermediates/res/default/resources/base/profile"),
            )
            .unwrap();
        }
        fs::write(
            original_module
                .join(".preview/default/intermediates/loader_out/default/ets/modules.abc"),
            b"stale original",
        )
        .unwrap();
        assert!(compiled_pages_at(&project, &artifacts).is_err());
        fs::write(
            module.join(".preview/default/intermediates/loader_out/default/ets/modules.abc"),
            b"snapshot build",
        )
        .unwrap();
        fs::write(
            module.join(
                ".preview/default/intermediates/res/default/resources/base/profile/app_pages.json",
            ),
            r#"{"src":["pages/Home"]}"#,
        )
        .unwrap();
        assert_eq!(
            compiled_pages_at(&project, &artifacts).unwrap(),
            vec!["pages/Home"]
        );
        fs::write(
            original_module.join("src/main/resources/base/profile/app_pages.json"),
            r#"{"src":["pages/Changed"]}"#,
        )
        .unwrap();
        assert_eq!(
            compiled_pages_at(&project, &artifacts).unwrap_err(),
            "COMPILED_PAGE_PROFILE_CHANGED"
        );
    }

    #[test]
    fn missing_or_empty_runtime_resource_blocks_switching_without_starting_sdk() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), &["phone", "tablet", "wearable"]);
        assert_eq!(
            validate_profile_resources(temp.path()),
            Err("PROFILE_RESOURCE_MISSING".into())
        );
        resources(temp.path());
        fs::write(
            temp.path()
                .join("sdk/default/openharmony/previewer/common/resources/resources.index"),
            [],
        )
        .unwrap();
        assert_eq!(
            validate_profile_resources(temp.path()),
            Err("PROFILE_RESOURCE_INVALID".into())
        );
        assert!(
            profiles_for_project(Some(temp.path()), true, Some(&project))
                .iter()
                .all(|profile| profile.status == NativeControlAvailability::Blocked)
        );
        assert!(
            profiles_for_project(Some(temp.path()), false, Some(&project))
                .iter()
                .all(|profile| profile.status == NativeControlAvailability::Unavailable)
        );
    }
}
