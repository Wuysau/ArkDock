//! Preview-only SDK task arguments and the pinned Hvigor watch-task receipt.

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
        return Err(format!(
            "BUILD_FAILED: exit {code}; {}",
            flushed_log
                .chars()
                .rev()
                .take(4096)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        ));
    }
    if build_receipt(module, flushed_log) != BuildReceipt::Succeeded {
        return Err("BUILD_RECEIPT_MISSING: PreviewBuild graph was not confirmed".into());
    }
    Ok(())
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
}
