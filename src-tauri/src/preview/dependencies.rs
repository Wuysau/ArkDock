//! Fail-closed dependency preflight for the preview build, and the bounded
//! classification of a flushed hvigor log.
//!
//! A HarmonyOS project resolves every `import` through the `oh_modules` tree that
//! `ohpm install` (or one DevEco Studio sync) writes next to each
//! `oh-package.json5`. ArkDock never installs that tree itself, so a project that
//! declares dependencies it never installed can only fail with cascading ArkTS
//! compiler errors that look like the user's own code is broken. The preflight
//! below detects exactly that state before hvigor is started, and the log
//! classification restores the same primary cause when the flushed log is the only
//! evidence available.

use std::collections::BTreeSet;
use std::path::Path;

/// `ohpm` installs declared packages here, next to the declaring manifest.
const OH_MODULES: &str = "oh_modules";
const OH_PACKAGE: &str = "oh-package.json5";
/// Declared manifests are bounded like every other configuration read.
const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// A declared package name is a bounded identifier.
const MAX_NAME_CHARS: usize = 256;
/// The hvigor wrapper resolves its own build tooling in the ArkDock-owned hvigor
/// home (`HVIGOR_USER_HOME`), never in `oh_modules`: those declarations are the
/// build tool itself, not project dependencies a user installs with `ohpm`.
const HVIGOR_TOOLCHAIN: &str = "@ohos/hvigor";

/// The wire code for a project whose declared dependencies are not installed.
pub const DEPENDENCIES_NOT_INSTALLED: &str = "PROJECT_DEPENDENCIES_NOT_INSTALLED";
/// The single actionable, path-free sentence for that failure. The structured error
/// contract reuses it verbatim, so the user-facing message and the build-log
/// classification cannot drift apart.
pub const DEPENDENCIES_NOT_INSTALLED_MESSAGE: &str = "该项目尚未安装依赖（缺少 oh_modules）；请先在该工程中执行 ohpm install（或在 DevEco Studio 中同步一次工程），然后重新刷新预览";

/// The markers hvigor prints when a declared module cannot be resolved from
/// `oh_modules`. Each one precedes the cascading ArkTS errors that follow it.
const RESOLUTION_MARKERS: [&str; 3] = [
    "could not be resolved",
    "Cannot find module",
    "Failed to resolve OhmUrl",
];

/// Fail closed before hvigor starts when a declared dependency is not installed, and
/// never block a project that declares nothing. `root` and `module` are the already
/// resolved project root and selected module directory.
pub fn preflight(root: &Path, module: &Path) -> Result<(), String> {
    let missing = declared(root, module)
        .iter()
        .filter(|name| !installed(module, name) && !installed(root, name))
        .count();
    if missing == 0 {
        return Ok(());
    }
    Err(format!(
        "{DEPENDENCIES_NOT_INSTALLED}: {DEPENDENCIES_NOT_INSTALLED_MESSAGE}；缺失依赖包 {missing} 个"
    ))
}

/// The primary cause of a flushed build log, or `None` when the log carries no
/// module-resolution failure. Callers pass an already bounded log tail, so the
/// classification stays bounded and deterministic.
pub fn classify_build_log(log: &str) -> Option<&'static str> {
    RESOLUTION_MARKERS
        .iter()
        .any(|marker| log.contains(marker))
        .then_some(DEPENDENCIES_NOT_INSTALLED)
}

/// Every declared package name from the selected module and the project root,
/// deduplicated. A manifest that is missing, unreadable or not an object declares
/// nothing: the build's own error, not this preflight, explains a broken manifest.
fn declared(root: &Path, module: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for directory in [module, root] {
        let Some(manifest) = read_manifest(&directory.join(OH_PACKAGE)) else {
            continue;
        };
        for section in ["dependencies", "devDependencies"] {
            let Some(entries) = manifest.get(section).and_then(|value| value.as_object()) else {
                continue;
            };
            for name in entries.keys() {
                if !name.is_empty()
                    && name.len() <= MAX_NAME_CHARS
                    && !name.starts_with(HVIGOR_TOOLCHAIN)
                {
                    names.insert(name.clone());
                }
            }
        }
    }
    names
}

fn read_manifest(path: &Path) -> Option<serde_json::Value> {
    let bytes = crate::workspace::read_bounded(path, MANIFEST_LIMIT).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let manifest: serde_json::Value = json5::from_str(text).ok()?;
    manifest.is_object().then_some(manifest)
}

/// A package is installed when its entry exists in that tree (a directory, or the
/// symlink/junction `ohpm` writes for a `file:` dependency) and its own manifest is
/// readable. A dangling link has an entry but no readable manifest.
fn installed(tree: &Path, name: &str) -> bool {
    let package = tree.join(OH_MODULES).join(name);
    if std::fs::symlink_metadata(&package).is_err() {
        return false;
    }
    std::fs::metadata(package.join(OH_PACKAGE)).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::error::wire_name;
    use crate::ipc::model::{ErrorCategory, RecoveryAction};
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A project with a root manifest and one `entry` module, both configurable.
    struct Project {
        root: tempfile::TempDir,
    }

    impl Project {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("temporary project");
            fs::create_dir_all(root.path().join("entry")).expect("module directory");
            Self { root }
        }

        fn path(&self) -> &Path {
            self.root.path()
        }

        fn module(&self) -> PathBuf {
            self.path().join("entry")
        }

        fn preflight(&self) -> Result<(), String> {
            preflight(self.path(), &self.module())
        }

        /// Write one `oh-package.json5`; `tree` is empty for the project root.
        fn manifest(&self, tree: &str, text: &str) {
            let directory = if tree.is_empty() {
                self.path().to_path_buf()
            } else {
                self.path().join(tree)
            };
            fs::create_dir_all(&directory).expect("manifest directory");
            fs::write(directory.join(OH_PACKAGE), text).expect("manifest");
        }

        /// Install one package into a tree's `oh_modules`, with or without its manifest.
        fn install(&self, tree: &str, name: &str, manifest: Option<&str>) {
            let package = self.tree_oh_modules(tree).join(name);
            fs::create_dir_all(&package).expect("installed package");
            if let Some(text) = manifest {
                fs::write(package.join(OH_PACKAGE), text).expect("installed manifest");
            }
        }

        fn tree_oh_modules(&self, tree: &str) -> PathBuf {
            let base = if tree.is_empty() {
                self.path().to_path_buf()
            } else {
                self.path().join(tree)
            };
            base.join(OH_MODULES)
        }

        /// Link one dependency the way `ohpm` links a `file:` package.
        fn link(&self, tree: &str, name: &str, target: &Path) -> std::io::Result<()> {
            let link = self.tree_oh_modules(tree).join(name);
            fs::create_dir_all(self.tree_oh_modules(tree)).expect("oh_modules directory");
            link_directory(target, &link)
        }
    }

    #[cfg(windows)]
    fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
        // A junction needs no privilege, unlike a directory symbolic link.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        }
    }

    #[cfg(unix)]
    fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    /// The Todo demo's real shape: no declared dependency anywhere, no `oh_modules`,
    /// only the hvigor build tooling the wrapper resolves in its own home.
    #[test]
    fn a_project_that_declares_no_dependency_has_nothing_to_preflight() {
        let project = Project::new();
        project.manifest(
            "",
            r#"{ "dependencies": {}, "devDependencies": { "@ohos/hvigor": "6.26.8", "@ohos/hvigor-ohos-plugin": "6.26.8" } }"#,
        );
        project.manifest("entry", r#"{ "dependencies": {} }"#);
        assert_eq!(project.preflight(), Ok(()));
    }

    /// A project without any `oh-package.json5` at all is not a dependency failure.
    #[test]
    fn missing_manifests_declare_nothing() {
        let project = Project::new();
        assert_eq!(project.preflight(), Ok(()));
    }

    #[test]
    fn missing_module_and_root_declarations_fail_closed_with_their_count() {
        let project = Project::new();
        project.manifest(
            "",
            r#"{ "dependencies": { "tyme4oh": "^1.3.9" }, "devDependencies": { "@ohos/hypium": "1.0.24" } }"#,
        );
        project.manifest(
            "entry",
            r#"{ "dependencies": { "home": "file:../../features/home", "constants": "file:../../commons/constants" } }"#,
        );
        let error = project.preflight().expect_err("declared but not installed");
        assert!(error.starts_with("PROJECT_DEPENDENCIES_NOT_INSTALLED: "));
        assert!(
            error.contains(DEPENDENCIES_NOT_INSTALLED_MESSAGE),
            "{error}"
        );
        assert!(error.ends_with("缺失依赖包 4 个"), "{error}");
        // Only the *declared* packages matter; the count is bounded to the names.
        assert!(
            !error.contains("home"),
            "{error:?} must stay a bounded summary"
        );
    }

    /// The exact declaration sets of the reported `jingmo-for-HarmonyOS` failure: a
    /// root dependency, a root dev dependency and nine `file:` module dependencies,
    /// none of them installed, exactly like the eleven unresolved imports hvigor
    /// reported before its cascading ArkTS errors.
    #[test]
    fn the_reported_jingmo_project_fails_with_eleven_missing_packages() {
        let project = Project::new();
        project.manifest(
            "",
            r#"{ "dependencies": { "tyme4oh": "^1.3.9" }, "devDependencies": { "@ohos/hypium": "1.0.24" } }"#,
        );
        // The real module manifest carries a trailing comma; the preflight accepts it.
        project.manifest(
            "entry",
            r#"{
  "dependencies": {
    "home": "file:../../features/home",
    "constants": "file:../../commons/constants",
    "data": "file:../../commons/data",
    "utils": "file:../../commons/utils",
    "chinese_antitheticalcouplet": "file:../../features/chinese/antitheticalcouplet",
    "chinese_idiom": "file:../../features/chinese/idiom",
    "chinese_quote": "file:../../features/chinese/quote",
    "classicalliterature_sentence": "file:../../features/classicalliterature/sentence",
    "classicalliterature_writing": "file:../../features/classicalliterature/writing",
  }
}"#,
        );
        let error = project.preflight().expect_err("declared but not installed");
        assert!(error.ends_with("缺失依赖包 11 个"), "{error}");
    }

    #[test]
    fn a_complete_dependency_tree_passes_at_module_and_root_level() {
        // Registry and `file:` packages installed into the root tree.
        let project = Project::new();
        project.manifest("", r#"{ "dependencies": { "tyme4oh": "^1.3.9" } }"#);
        project.manifest(
            "entry",
            r#"{ "dependencies": { "home": "file:../../features/home" } }"#,
        );
        project.install("", "tyme4oh", Some(r#"{ "name": "tyme4oh" }"#));
        project.install("", "home", Some(r#"{ "name": "home" }"#));
        assert_eq!(project.preflight(), Ok(()));

        // The same declarations installed only into the selected module's tree.
        let project = Project::new();
        project.manifest("", r#"{ "dependencies": { "tyme4oh": "^1.3.9" } }"#);
        project.manifest(
            "entry",
            r#"{ "dependencies": { "home": "file:../../features/home" } }"#,
        );
        project.install("entry", "tyme4oh", Some(r#"{ "name": "tyme4oh" }"#));
        project.install("entry", "home", Some(r#"{ "name": "home" }"#));
        assert_eq!(project.preflight(), Ok(()));
    }

    #[test]
    fn a_package_without_a_readable_manifest_is_not_installed() {
        let project = Project::new();
        project.manifest(
            "entry",
            r#"{ "dependencies": { "home": "file:../../home" } }"#,
        );
        project.install("entry", "home", None);
        assert!(project.preflight().is_err());
        project.install("entry", "home", Some(r#"{ "name": "home" }"#));
        assert_eq!(project.preflight(), Ok(()));
    }

    #[test]
    fn a_linked_package_is_installed_and_a_dangling_link_is_not() {
        let project = Project::new();
        project.manifest("entry", r#"{ "dependencies": { "home": "file:../../features/home", "utils": "file:../../commons/utils" } }"#);
        let source = tempfile::tempdir().expect("linked package source");
        fs::write(source.path().join(OH_PACKAGE), r#"{ "name": "home" }"#)
            .expect("linked manifest");
        // `utils` is declared and linked to a path that does not exist.
        if project
            .link("entry", "utils", &project.path().join("missing"))
            .is_err()
        {
            eprintln!("skipping: this host does not allow a directory link");
            return;
        }
        // A host that refuses the good link still counts both packages.
        let expected = if project.link("entry", "home", source.path()).is_ok() {
            1
        } else {
            2
        };
        let error = project
            .preflight()
            .expect_err("dangling link is not installed");
        assert!(
            error.ends_with(&format!("缺失依赖包 {expected} 个")),
            "{error}"
        );
    }

    #[test]
    fn the_preflight_failure_is_the_structured_dependency_error() {
        let project = Project::new();
        project.manifest(
            "entry",
            r#"{ "dependencies": { "home": "file:../../features/home" } }"#,
        );
        let error = project.preflight().expect_err("missing dependency");
        // The wire code leads, so the failure is classified at the IPC boundary.
        let structured = crate::ipc::model::IpcError::from_message(error.clone());
        assert_eq!(
            wire_name(&structured.code),
            "PROJECT_DEPENDENCIES_NOT_INSTALLED"
        );
        assert_eq!(structured.category, ErrorCategory::Environment);
        assert!(!structured.retryable);
        assert_eq!(structured.recovery, RecoveryAction::RestartPreview);
        assert_eq!(structured.message, DEPENDENCIES_NOT_INSTALLED_MESSAGE);
        assert!(structured.details.is_some(), "the count stays in details");
        // The message is product copy: no local path and no window title.
        assert!(!structured.message.contains("oh_modules/"), "{error}");
    }

    #[test]
    fn only_a_module_resolution_failure_chooses_the_dependency_cause() {
        assert_eq!(classify_build_log(""), None);
        assert_eq!(
            classify_build_log(
                "> hvigor Finished :entry:PreviewBuild... after 1 ms\n> hvigor BUILD SUCCESSFUL in 6 s 147 ms"
            ),
            None
        );
        // Unrelated ArkTS noise alone is not a dependency failure.
        assert_eq!(
            classify_build_log(
                "10605090 ArkTS Compiler Error\nError Message: Function return type inference is limited"
            ),
            None
        );
        for marker in RESOLUTION_MARKERS {
            let log = format!("> hvigor Finished :entry:PreBuild...\n{marker} in entry/src/main\n> hvigor ERROR: BUILD FAILED");
            assert_eq!(classify_build_log(&log), Some(DEPENDENCIES_NOT_INSTALLED));
        }
    }
}
