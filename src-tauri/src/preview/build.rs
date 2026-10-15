//! Preview-only SDK task arguments and the pinned Hvigor watch-task receipt.

use super::dependencies::{
    classify_build_log, DEPENDENCIES_NOT_INSTALLED, DEPENDENCIES_NOT_INSTALLED_MESSAGE,
};

/// Diagnostics handed to a failure message, newest last.
const FAILURE_TAIL_CHARS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildReceipt {
    Pending,
    Succeeded,
    Failed,
}

pub fn preview_build_args(module: &str) -> Result<Vec<String>, String> {
    if module.is_empty()
        || module.len() > 128
        || !module
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    {
        return Err("INVALID_MODULE".into());
    }
    Ok(vec![
        "--mode".into(),
        "module".into(),
        "-p".into(),
        format!("module={module}@default"),
        "-p".into(),
        "product=default".into(),
        "-p".into(),
        "buildMode=debug".into(),
        "-p".into(),
        "buildRoot=.preview".into(),
        "-p".into(),
        "previewMode=true".into(),
        "-p".into(),
        "pageType=page".into(),
        "PreviewBuild".into(),
        "--no-daemon".into(),
    ])
}

/// PreviewArkTS keeps a worker alive even without --watch in this exact SDK.
/// Its completed task + successful graph receipt is distinct from process exit.
pub fn build_receipt(module: &str, log: &str) -> BuildReceipt {
    let log = plain_sdk_log(log);
    if log
        .lines()
        .any(|line| line.starts_with("> hvigor ERROR:") || line.contains("BUILD FAILED"))
    {
        return BuildReceipt::Failed;
    }
    let task = format!(":{module}:PreviewBuild...");
    let completed = log.lines().any(|line| {
        (line.starts_with("> hvigor Finished ") || line.starts_with("> hvigor UP-TO-DATE "))
            && line.contains(&task)
    });
    if completed
        && log
            .lines()
            .any(|line| line.starts_with("> hvigor BUILD SUCCESSFUL in "))
    {
        BuildReceipt::Succeeded
    } else {
        BuildReceipt::Pending
    }
}

/// The pinned logger emits bounded SGR color escapes even to inherited log handles.
/// Strip only SGR syntax, preserving every other byte and line boundary.
fn plain_sdk_log(mut remaining: &str) -> String {
    let mut plain = String::with_capacity(remaining.len());
    while let Some(index) = remaining.find("\u{1b}[") {
        plain.push_str(&remaining[..index]);
        remaining = &remaining[index + 2..];
        if let Some(end) = remaining.find('m').filter(|&end| {
            end <= 24
                && remaining[..end]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b';')
        }) {
            remaining = &remaining[end + 1..];
        } else {
            plain.push_str("\u{1b}[");
        }
    }
    plain.push_str(remaining);
    plain
}

pub fn build_exit_result(module: &str, code: u32, flushed_log: &str) -> Result<(), String> {
    if code != 0 || build_receipt(module, flushed_log) == BuildReceipt::Failed {
        return Err(failure_message(flushed_log, &format!("exit {code}")));
    }
    if build_receipt(module, flushed_log) != BuildReceipt::Succeeded {
        return Err("BUILD_RECEIPT_MISSING: PreviewBuild graph was not confirmed".into());
    }
    Ok(())
}

/// A failed preview build leads with its classified primary cause and keeps the
/// newest raw log appended, so the diagnostics and `ark details` paths still carry
/// exactly the evidence they carried before.
pub fn failure_message(log: &str, context: &str) -> String {
    let tail: String = log
        .chars()
        .rev()
        .take(FAILURE_TAIL_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if classify_build_log(log) == Some(DEPENDENCIES_NOT_INSTALLED) {
        format!(
            "{DEPENDENCIES_NOT_INSTALLED}: {DEPENDENCIES_NOT_INSTALLED_MESSAGE}\n[ArkDock] {context}\n{tail}"
        )
    } else {
        // Unrelated failures keep today's message byte for byte.
        format!("BUILD_FAILED: {context}; {tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_sdk_color_sequences_do_not_hide_success_or_compiler_failure() {
        let colored = "> hvigor \u{1b}[32mFinished :entry:PreviewBuild... after 1 ms \u{1b}[39m\n> hvigor \u{1b}[32mBUILD SUCCESSFUL in 6 s 147 ms \u{1b}[39m";
        assert_eq!(build_receipt("entry", colored), BuildReceipt::Succeeded);
        assert_eq!(build_exit_result("entry", 0, colored), Ok(()));
        assert_eq!(
            build_receipt(
                "entry",
                &format!("{colored}\n> hvigor \u{1b}[31mERROR: failed \u{1b}[39m")
            ),
            BuildReceipt::Failed
        );
    }

    #[test]
    fn cached_build_exit_requires_its_final_flushed_graph_receipt() {
        assert_eq!(build_receipt("entry", ""), BuildReceipt::Pending);
        let flushed =
            "> hvigor Finished :entry:PreviewBuild... after 1 ms\n> hvigor BUILD SUCCESSFUL in 2 s";
        assert_eq!(build_exit_result("entry", 0, flushed), Ok(()));
        assert!(build_exit_result("entry", 1, flushed)
            .unwrap_err()
            .starts_with("BUILD_FAILED"));
        assert!(build_exit_result("entry", 0, "")
            .unwrap_err()
            .starts_with("BUILD_RECEIPT_MISSING"));
    }

    #[test]
    fn preview_build_does_not_package_hap_and_selects_preview_output() {
        let args = preview_build_args("entry").unwrap();
        assert!(args.contains(&"PreviewBuild".to_owned()));
        assert!(args.contains(&"buildRoot=.preview".to_owned()));
        assert!(args.contains(&"previewMode=true".to_owned()));
        assert!(!args.iter().any(|arg| arg.contains("assembleHap")));
        assert_eq!(
            preview_build_args("entry&echo x"),
            Err("INVALID_MODULE".into())
        );
    }

    #[test]
    fn only_the_completed_pinned_sdk_task_receipt_allows_artifact_handoff() {
        assert_eq!(
            build_receipt("entry", "> hvigor BUILD SUCCESSFUL in 1 s"),
            BuildReceipt::Pending
        );
        assert_eq!(build_receipt("entry", "> hvigor Finished :entry:PreviewBuild... after 1 ms\n> hvigor BUILD SUCCESSFUL in 1 s"), BuildReceipt::Succeeded);
        assert_eq!(
            build_receipt(
                "entry",
                "> hvigor UP-TO-DATE :entry:PreviewBuild...\n> hvigor BUILD SUCCESSFUL in 1 s"
            ),
            BuildReceipt::Succeeded
        );
        assert_eq!(build_receipt("entry", "> hvigor Finished :entry:PreviewBuild...\n> hvigor BUILD SUCCESSFUL in 1 s\n> hvigor ERROR: COMPILE RESULT:FAIL"), BuildReceipt::Failed);
        assert_eq!(
            build_receipt("entry", "> hvigor BUILD FAILED in 1 s"),
            BuildReceipt::Failed
        );
    }

    #[test]
    fn a_module_resolution_failure_leads_the_build_failure_and_keeps_the_raw_log() {
        // The shape of the logged jingmo-for-HarmonyOS failure: unresolved imports
        // first, then a wall of cascading ArkTS errors.
        let log = "> hvigor WARN: \"utils\" is imported by \"entry/src/main/ets/pages/Index.ets\", but could not be resolved – treating it as an external dependency.\n> hvigor ERROR: 10505001 ArkTS Compiler Error\nError Message: Cannot find module 'utils' or its corresponding type declarations.\n> hvigor ERROR: BUILD FAILED in 14 s 126 ms";
        let error = build_exit_result("entry", 1, log).expect_err("failed build");
        assert!(
            error.starts_with("PROJECT_DEPENDENCIES_NOT_INSTALLED: 该项目尚未安装依赖"),
            "{error}"
        );
        assert!(
            error.contains("Cannot find module 'utils'"),
            "the raw log must stay available: {error}"
        );
        // Unrelated compiler failures keep today's message exactly.
        let unrelated = build_exit_result("entry", 1, "> hvigor ERROR: BUILD FAILED in 1 s")
            .expect_err("failed build");
        assert!(
            unrelated.starts_with("BUILD_FAILED: exit 1; "),
            "{unrelated}"
        );
    }
}
