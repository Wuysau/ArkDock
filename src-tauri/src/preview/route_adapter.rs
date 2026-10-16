//! Owned preview snapshot protocol. Execution stays with the Windows Job runner.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const REPORT_LIMIT: u64 = 4 * 1024 * 1024;

pub fn helper_source() -> &'static str {
    include_str!("../../../tools/preview/route-adapter.mjs")
}

#[derive(Clone, Debug)]
pub struct BuildArtifactContext {
    pub root: PathBuf,
    pub module: PathBuf,
    pub route_count: usize,
    pub report_path: PathBuf,
    pub source_map_path: Option<PathBuf>,
    pub adapted: bool,
    pub hvigor_home: Option<PathBuf>,
}

pub fn arguments(
    project_root: &Path,
    module_name: &str,
    output_root: &Path,
    sdk_root: &Path,
    report_path: &Path,
) -> Result<Vec<String>, String> {
    if module_name.is_empty()
        || module_name.len() > 128
        || !module_name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    {
        return Err("ROUTE_ADAPTER_MODULE_INVALID".into());
    }
    let mut args = Vec::new();
    for (name, value) in [
        ("--project-root", project_root),
        ("--output-root", output_root),
        ("--sdk-root", sdk_root),
        ("--report", report_path),
    ] {
        if !value.is_absolute() || value.to_str().is_none() {
            return Err("ROUTE_ADAPTER_PATH_INVALID".into());
        }
        args.extend([name.into(), value.to_string_lossy().into_owned()]);
    }
    args.extend(["--module".into(), module_name.into()]);
    Ok(args)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    version: u32,
    adapted: bool,
    root: PathBuf,
    module: PathBuf,
    route_count: usize,
    report_path: PathBuf,
    source_map_path: Option<PathBuf>,
    hvigor_home: Option<PathBuf>,
}

pub fn read_report(
    report_path: &Path,
    original_root: &Path,
    original_module: &Path,
    output_root: &Path,
) -> Result<BuildArtifactContext, String> {
    let output = output_root
        .canonicalize()
        .map_err(|_| "ROUTE_ADAPTER_OUTPUT_MISSING")?;
    let report_file = report_path
        .canonicalize()
        .map_err(|_| "ROUTE_ADAPTER_REPORT_MISSING")?;
    if !report_file.starts_with(&output) {
        return Err("ROUTE_ADAPTER_REPORT_OUTSIDE_OUTPUT".into());
    }
    let bytes = crate::workspace::read_bounded(&report_file, REPORT_LIMIT)?;
    let report: Report =
        serde_json::from_slice(&bytes).map_err(|_| "ROUTE_ADAPTER_REPORT_INVALID")?;
    if report.version != 1
        || report.route_count > 1024
        || report.report_path.canonicalize().ok().as_ref() != Some(&report_file)
    {
        return Err("ROUTE_ADAPTER_REPORT_INVALID".into());
    }
    let root = report
        .root
        .canonicalize()
        .map_err(|_| "ROUTE_ADAPTER_ROOT_MISSING")?;
    let module = report
        .module
        .canonicalize()
        .map_err(|_| "ROUTE_ADAPTER_MODULE_MISSING")?;
    let original = original_root
        .canonicalize()
        .map_err(|_| "PROJECT_PATH_UNAVAILABLE")?;
    let original_module = original_module
        .canonicalize()
        .map_err(|_| "MODULE_PATH_MISSING")?;
    let source_map_path = if report.adapted {
        if report.route_count == 0
            || !root.starts_with(&output)
            || !module.starts_with(&root)
            || root.starts_with(&original)
            || original.starts_with(&root)
            || module.strip_prefix(&root).ok() != original_module.strip_prefix(&original).ok()
        {
            return Err("ROUTE_ADAPTER_ARTIFACT_OUTSIDE_OUTPUT".into());
        }
        let map = report
            .source_map_path
            .ok_or("ROUTE_ADAPTER_SOURCE_MAP_REQUIRED")?
            .canonicalize()
            .map_err(|_| "ROUTE_ADAPTER_SOURCE_MAP_REQUIRED")?;
        if !map.starts_with(&output)
            || !fs::metadata(&map).is_ok_and(|meta| meta.is_file() && meta.len() <= REPORT_LIMIT)
        {
            return Err("ROUTE_ADAPTER_SOURCE_MAP_INVALID".into());
        }
        Some(map)
    } else {
        if root != original
            || module != original_module
            || report.route_count != 0
            || report.source_map_path.is_some()
        {
            return Err("ROUTE_ADAPTER_ORIGINAL_IDENTITY_CHANGED".into());
        }
        None
    };
    let hvigor_home = match report.hvigor_home {
        Some(home) if report.adapted => {
            let home = home
                .canonicalize()
                .map_err(|_| "ROUTE_ADAPTER_HVIGOR_HOME_MISSING")?;
            if !home.starts_with(&output) || !home.is_dir() {
                return Err("ROUTE_ADAPTER_HVIGOR_HOME_OUTSIDE_OUTPUT".into());
            }
            Some(home)
        }
        None if !report.adapted => None,
        _ => return Err("ROUTE_ADAPTER_HVIGOR_HOME_INVALID".into()),
    };
    Ok(BuildArtifactContext {
        root,
        module,
        route_count: report.route_count,
        report_path: report_file,
        source_map_path,
        adapted: report.adapted,
        hvigor_home,
    })
}

const SOURCE_LIMIT: u64 = 4 * 1024 * 1024;
const INSPECTOR_SOURCE_BUDGET: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceMap {
    version: u32,
    original_root: PathBuf,
    staged_root: PathBuf,
    files: Vec<SourceMapFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceMapFile {
    source: PathBuf,
    staged: Option<PathBuf>,
    source_hash: Option<String>,
    staged_hash: Option<String>,
    prefix_lines: Option<usize>,
    #[serde(default)]
    insertions: Vec<SourceInsertion>,
}

#[derive(Deserialize)]
struct SourceInsertion {
    offset: usize,
    length: usize,
}

struct SourceLines {
    starts: Vec<usize>,
    ends: Vec<usize>,
}
impl SourceLines {
    fn new(units: &[u16]) -> Self {
        let mut starts = vec![0];
        let mut ends = Vec::new();
        for (offset, unit) in units.iter().enumerate() {
            if *unit == 10 {
                ends.push(if offset > 0 && units[offset - 1] == 13 {
                    offset - 1
                } else {
                    offset
                });
                starts.push(offset + 1);
            }
        }
        ends.push(units.len());
        Self { starts, ends }
    }
    fn offset(&self, line: usize, column: usize) -> Option<usize> {
        let index = line.checked_sub(1)?;
        let offset = self
            .starts
            .get(index)?
            .checked_add(column.checked_sub(1)?)?;
        (offset < *self.ends.get(index)?).then_some(offset)
    }
    fn position(&self, offset: usize) -> Option<(usize, usize)> {
        let line = self
            .starts
            .partition_point(|start| *start <= offset)
            .checked_sub(1)?;
        (offset < *self.ends.get(line)?).then_some((line + 1, offset - self.starts[line] + 1))
    }
}

struct SourceMapping {
    relative: String,
    staged_lines: SourceLines,
    original_lines: SourceLines,
    /// (staged start, staged end exclusive, original start), in JS UTF-16 units.
    segments: Vec<(usize, usize, usize)>,
}
impl SourceMapping {
    fn position(&self, line: usize, column: usize) -> Option<String> {
        let offset = self.staged_lines.offset(line, column)?;
        let (start, _, original) = self
            .segments
            .iter()
            .find(|(start, end, _)| offset >= *start && offset < *end)?;
        let (line, column) = self.original_lines.position(original + offset - start)?;
        Some(format!("{}({line}:{column})", self.relative))
    }
}

struct InspectorSourceMap {
    original: PathBuf,
    staged: PathBuf,
    module: PathBuf,
    files: HashMap<PathBuf, SourceMapFile>,
    cache: HashMap<PathBuf, Option<SourceMapping>>,
    remaining_bytes: usize,
    remaining_lines: usize,
}

fn mapping_error() -> String {
    "ROUTE_SOURCE_MAP_INVALID".into()
}

impl InspectorSourceMap {
    fn load(
        project: &crate::ipc::model::ProjectRef,
        artifacts: &BuildArtifactContext,
    ) -> Result<Self, String> {
        let original = Path::new(&project.app_root)
            .canonicalize()
            .map_err(|_| mapping_error())?;
        let original_module = super::configuration::module_directory(project)?;
        let report = artifacts
            .report_path
            .canonicalize()
            .map_err(|_| mapping_error())?;
        let run = report.parent().ok_or_else(mapping_error)?;
        let checked = read_report(&report, &original, &original_module, run)?;
        let staged = artifacts.root.canonicalize().map_err(|_| mapping_error())?;
        let module = artifacts
            .module
            .canonicalize()
            .map_err(|_| mapping_error())?;
        if !checked.adapted
            || checked.root != staged
            || checked.module != module
            || checked.source_map_path
                != artifacts
                    .source_map_path
                    .as_ref()
                    .and_then(|path| path.canonicalize().ok())
        {
            return Err(mapping_error());
        }
        let map_path = checked.source_map_path.ok_or_else(mapping_error)?;
        let bytes = crate::workspace::read_bounded(&map_path, REPORT_LIMIT)?;
        let map: SourceMap = serde_json::from_slice(&bytes).map_err(|_| mapping_error())?;
        if map.version != 1
            || map.files.len() > 1024
            || map.original_root.canonicalize().ok().as_ref() != Some(&original)
            || map.staged_root.canonicalize().ok().as_ref() != Some(&staged)
        {
            return Err(mapping_error());
        }
        let mut files = HashMap::new();
        for file in map.files {
            let Some(target) = &file.staged else {
                continue;
            }; // Preserved dispatchers were not rewritten.
            let target = target.canonicalize().map_err(|_| mapping_error())?;
            let source = file.source.canonicalize().map_err(|_| mapping_error())?;
            if !target.starts_with(&staged)
                || !source.starts_with(&original)
                || target.strip_prefix(&staged).ok() != source.strip_prefix(&original).ok()
                || files.insert(target, file).is_some()
            {
                return Err(mapping_error());
            }
        }
        Ok(Self {
            original,
            staged,
            module,
            files,
            cache: HashMap::new(),
            remaining_bytes: INSPECTOR_SOURCE_BUDGET,
            remaining_lines: 262_144,
        })
    }

    fn resolve(&self, value: &str) -> Option<PathBuf> {
        if value.is_empty()
            || value.len() > 4096
            || !value.ends_with(".ets")
            || value.chars().any(char::is_control)
        {
            return None;
        }
        let normalized = value.replace('\\', "/");
        if normalized
            .split('/')
            .any(|part| part == ".." || part == ".")
        {
            return None;
        }
        let path = Path::new(&normalized);
        let mut candidates = Vec::new();
        if path.is_absolute() {
            candidates.push(path.to_path_buf());
        } else {
            if normalized.contains(':')
                || normalized.starts_with('/')
                || normalized.split('/').any(str::is_empty)
            {
                return None;
            }
            candidates.push(self.staged.join(path));
            candidates.push(self.module.join(path));
        }
        let mut resolved: Vec<_> = candidates
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.starts_with(&self.staged) && path.is_file())
            .collect();
        resolved.sort();
        resolved.dedup();
        (resolved.len() == 1).then(|| resolved.remove(0))
    }

    fn read_source(&mut self, path: &Path) -> Option<Vec<u8>> {
        let size = fs::metadata(path).ok()?.len();
        if size > SOURCE_LIMIT || size > self.remaining_bytes as u64 {
            return None;
        }
        let bytes = crate::workspace::read_bounded(path, SOURCE_LIMIT).ok()?;
        self.remaining_bytes = self.remaining_bytes.checked_sub(bytes.len())?;
        Some(bytes)
    }

    fn mapping(&mut self, target: &Path) -> Option<SourceMapping> {
        let relative = target.strip_prefix(&self.staged).ok()?;
        let source = self.original.join(relative).canonicalize().ok()?;
        if !source.starts_with(&self.original) {
            return None;
        }
        let relative = source
            .strip_prefix(&self.original)
            .ok()?
            .to_str()?
            .replace('\\', "/");
        if relative.len() > 1024
            || relative.contains(':')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return None;
        }
        let source_bytes = self.read_source(&source)?;
        let staged_bytes = self.read_source(target)?;
        let original_text = std::str::from_utf8(&source_bytes).ok()?;
        let staged_text = std::str::from_utf8(&staged_bytes).ok()?;
        let original: Vec<_> = original_text.encode_utf16().collect();
        let staged: Vec<_> = staged_text.encode_utf16().collect();
        let mut segments = Vec::new();
        if let Some(file) = self.files.get(target) {
            if file.source_hash.as_deref()
                != Some(format!("{:x}", Sha256::digest(&source_bytes)).as_str())
                || file.staged_hash.as_deref()
                    != Some(format!("{:x}", Sha256::digest(&staged_bytes)).as_str())
            {
                return None;
            }
            let prefix_lines = file.prefix_lines?;
            if prefix_lines > 1024 || file.insertions.is_empty() || file.insertions.len() > 1024 {
                return None;
            }
            // The generator emits one newline even when all builders are local.
            let prefix_end = staged
                .iter()
                .enumerate()
                .filter(|(_, ch)| **ch == 10)
                .nth(prefix_lines.max(1) - 1)?
                .0
                + 1;
            let prefix = String::from_utf16(&staged[..prefix_end]).ok()?;
            if prefix_lines == 0 && prefix != "\n"
                || prefix_lines > 0
                    && !prefix.lines().all(|line| {
                        line.starts_with("import { ")
                            && line.contains(" as ArkDockRoute_")
                            && line.ends_with(';')
                    })
            {
                return None;
            }
            let mut insertions: Vec<_> = file.insertions.iter().collect();
            insertions.sort_by_key(|edit| edit.offset);
            let (mut original_pos, mut staged_pos) = (0usize, prefix_end);
            for edit in insertions {
                if edit.offset < original_pos
                    || edit.offset > original.len()
                    || !(1..=256).contains(&edit.length)
                {
                    return None;
                }
                let end = staged_pos.checked_add(edit.offset - original_pos)?;
                if staged.get(staged_pos..end)? != original.get(original_pos..edit.offset)? {
                    return None;
                }
                if end > staged_pos {
                    segments.push((staged_pos, end, original_pos));
                }
                let inserted_end = end.checked_add(edit.length)?;
                let inserted = String::from_utf16(staged.get(end..inserted_end)?).ok()?;
                let token = inserted
                    .strip_prefix(".navDestination(ArkDockPreviewRoutes_")?
                    .strip_suffix(')')?;
                if token.len() != 12 || !token.bytes().all(|ch| ch.is_ascii_hexdigit()) {
                    return None;
                }
                original_pos = edit.offset;
                staged_pos = inserted_end;
            }
            let end = staged_pos.checked_add(original.len() - original_pos)?;
            if staged.get(staged_pos..end)? != original.get(original_pos..)? {
                return None;
            }
            if end > staged_pos {
                segments.push((staged_pos, end, original_pos));
            }
            let suffix = String::from_utf16(staged.get(end..)?).ok()?;
            if !suffix.starts_with("\n@Builder\nexport function ArkDockPreviewRoutes_") {
                return None;
            }
        } else {
            if source_bytes != staged_bytes {
                return None;
            }
            segments.push((0, original.len(), 0));
        }
        let lines = original
            .iter()
            .chain(staged.iter())
            .filter(|unit| **unit == 10)
            .count()
            .checked_add(2)?;
        self.remaining_lines = self.remaining_lines.checked_sub(lines)?;
        Some(SourceMapping {
            relative,
            staged_lines: SourceLines::new(&staged),
            original_lines: SourceLines::new(&original),
            segments,
        })
    }

    fn debug_line(&mut self, text: &str) -> Option<String> {
        if text.len() > 4096 {
            return None;
        }
        let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
        let location = value.as_object()?.get("$line")?.as_str()?;
        let (file, position) = location.rsplit_once('(')?;
        let (line, column) = position.strip_suffix(')')?.split_once(':')?;
        let (line, column) = (line.parse::<usize>().ok()?, column.parse::<usize>().ok()?);
        let target = self.resolve(file)?;
        if !self.cache.contains_key(&target) {
            if self.cache.len() >= 256 {
                return None;
            }
            let mapping = self.mapping(&target);
            self.cache.insert(target.clone(), mapping);
        }
        let location = self.cache.get(&target)?.as_ref()?.position(line, column)?;
        value["$line"] = serde_json::Value::String(location);
        serde_json::to_string(&value)
            .ok()
            .filter(|text| text.len() <= 4096)
    }
}

/// Runs only for a requested Inspector tree. Failed/unmapped generated locations
/// lose their debug attribute, leaving SourceEvidence Unavailable. Successful
/// locations use the existing Candidate parser; this never marks them Verified.
pub fn remap_inspector_sources(
    project: &crate::ipc::model::ProjectRef,
    artifacts: &BuildArtifactContext,
    tree: &mut crate::ipc::model::InspectorTree,
) -> Result<usize, String> {
    if !artifacts.adapted {
        return Ok(0);
    }
    let loaded = if tree.workspace_id == project.id {
        InspectorSourceMap::load(project, artifacts)
    } else {
        Err("ROUTE_SOURCE_MAP_SCOPE".into())
    };
    let mut mapper = match loaded {
        Ok(mapper) => mapper,
        Err(error) => {
            for node in &mut tree.nodes {
                node.attributes
                    .retain(|attribute| attribute.name != "$debugLine");
            }
            return Err(error);
        }
    };
    let mut mapped = 0;
    for node in &mut tree.nodes {
        node.attributes.retain_mut(|attribute| {
            if attribute.name != "$debugLine" {
                return true;
            }
            if let Some(value) = mapper.debug_line(&attribute.value) {
                attribute.value = value;
                mapped += 1;
                true
            } else {
                false
            }
        });
    }
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        _temporary: tempfile::TempDir,
        original: PathBuf,
        original_module: PathBuf,
        output: PathBuf,
        report: PathBuf,
        value: serde_json::Value,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let original = temporary.path().join("original");
            let original_module = original.join("features/entry");
            let output = temporary.path().join("owned");
            let staged = output.join("project");
            let module = staged.join("features/entry");
            let map = output.join("source-map.json");
            let home = output.join("hvigor-home");
            let report = output.join("report.json");
            for dir in [&original_module, &module, &home] {
                fs::create_dir_all(dir).unwrap();
            }
            fs::write(&map, "{}").unwrap();
            let value = json!({"version":1,"adapted":true,"root":staged,"module":module,
                "routeCount":2,"reportPath":report,"sourceMapPath":map,"hvigorHome":home});
            Self {
                _temporary: temporary,
                original,
                original_module,
                output,
                report,
                value,
            }
        }

        fn read(&self) -> Result<BuildArtifactContext, String> {
            fs::write(&self.report, serde_json::to_vec(&self.value).unwrap()).unwrap();
            read_report(
                &self.report,
                &self.original,
                &self.original_module,
                &self.output,
            )
        }
    }

    #[test]
    fn accepts_only_owned_staged_artifacts_with_matching_module_layout() {
        let fixture = Fixture::new();
        let context = fixture.read().unwrap();
        assert!(context.adapted);
        assert_eq!(context.route_count, 2);
        assert!(context
            .root
            .starts_with(fixture.output.canonicalize().unwrap()));
        assert!(context
            .hvigor_home
            .unwrap()
            .starts_with(fixture.output.canonicalize().unwrap()));
    }

    #[test]
    fn rejects_staged_root_pointing_at_original() {
        let mut fixture = Fixture::new();
        fixture.value["root"] = json!(fixture.original);
        fixture.value["module"] = json!(fixture.original_module);
        assert_eq!(
            fixture.read().unwrap_err(),
            "ROUTE_ADAPTER_ARTIFACT_OUTSIDE_OUTPUT"
        );
    }

    #[test]
    fn rejects_tool_home_outside_owned_output() {
        let mut fixture = Fixture::new();
        fixture.value["hvigorHome"] = json!(fixture.original);
        assert_eq!(
            fixture.read().unwrap_err(),
            "ROUTE_ADAPTER_HVIGOR_HOME_OUTSIDE_OUTPUT"
        );
    }

    #[test]
    fn no_adaptation_preserves_exact_original_identity() {
        let mut fixture = Fixture::new();
        fixture.value["adapted"] = json!(false);
        fixture.value["root"] = json!(fixture.original);
        fixture.value["module"] = json!(fixture.original_module);
        fixture.value["routeCount"] = json!(0);
        fixture.value["sourceMapPath"] = serde_json::Value::Null;
        fixture.value["hvigorHome"] = serde_json::Value::Null;
        assert!(!fixture.read().unwrap().adapted);
        fixture.value["routeCount"] = json!(1);
        assert_eq!(
            fixture.read().unwrap_err(),
            "ROUTE_ADAPTER_ORIGINAL_IDENTITY_CHANGED"
        );
    }

    struct MappingFixture {
        fixture: Fixture,
        artifacts: BuildArtifactContext,
        project: crate::ipc::model::ProjectRef,
        source: PathBuf,
        target: PathBuf,
        original: String,
        staged: String,
        map: serde_json::Value,
    }

    impl MappingFixture {
        fn new() -> Self {
            let fixture = Fixture::new();
            fs::write(
                fixture.original.join("build-profile.json5"),
                r#"{"modules":[{"name":"entry","srcPath":"features/entry"}]}"#,
            )
            .unwrap();
            let artifacts = fixture.read().unwrap();
            let source = fixture.original_module.join("src/main/ets/Page.ets");
            let target = artifacts.module.join("src/main/ets/Page.ets");
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let original = "@Component\r\nstruct Page {\r\n  build() { Navigation() { Text('中文😀') }.hideTitleBar(true) }\r\n}\r\n".to_owned();
            let insertion = ".navDestination(ArkDockPreviewRoutes_012345abcdef)";
            let offset = original.find(".hideTitleBar").unwrap();
            let prefix = "import { B as ArkDockRoute_0 } from './Detail';\n";
            let staged = format!("{prefix}{}{insertion}{}\n@Builder\nexport function ArkDockPreviewRoutes_012345abcdef(name: string) {{}}\n", &original[..offset], &original[offset..]);
            fs::write(&source, &original).unwrap();
            fs::write(&target, &staged).unwrap();
            let map = json!({ "version":1, "originalRoot":fixture.original, "stagedRoot":artifacts.root,
                "files":[{"source":source,"staged":target,"sourceHash":format!("{:x}",Sha256::digest(original.as_bytes())),
                    "stagedHash":format!("{:x}",Sha256::digest(staged.as_bytes())),"prefixLines":1,
                    "insertions":[{"offset":original[..offset].encode_utf16().count(),"length":insertion.encode_utf16().count()}]}] });
            fs::write(
                artifacts.source_map_path.as_ref().unwrap(),
                serde_json::to_vec(&map).unwrap(),
            )
            .unwrap();
            let project = crate::ipc::model::ProjectRef {
                id: "project".into(),
                name: "Fixture".into(),
                environment: crate::ipc::model::Environment::WindowsLocal,
                workspace_root: fixture.original.to_string_lossy().into_owned(),
                app_root: fixture.original.to_string_lossy().into_owned(),
                module: "entry".into(),
                variant: "default".into(),
                sdk_id: None,
            };
            Self {
                fixture,
                artifacts,
                project,
                source,
                target,
                original,
                staged,
                map,
            }
        }

        fn location(text: &str, needle: &str, path: &Path) -> String {
            let before = &text[..text.find(needle).unwrap()];
            let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap().encode_utf16().count() + 1;
            format!("{}({line}:{column})", path.display())
        }

        fn tree(&self, location: &str) -> crate::ipc::model::InspectorTree {
            use crate::ipc::model::{InspectorAttribute, InspectorNode, InspectorTree};
            InspectorTree {
                workspace_id: "project".into(),
                session_id: "session".into(),
                build_id: "build".into(),
                generation: 1,
                frame_sequence: 1,
                inspection_version: 1,
                queried_at: 1,
                received_at: 2,
                exact_frame_match: false,
                reason: "candidate only".into(),
                nodes: vec![InspectorNode {
                    node_id: "1".into(),
                    component_type: Some("Text".into()),
                    parent_id: None,
                    children: vec![],
                    bounds: None,
                    visible: Some(true),
                    page: None,
                    depth: 0,
                    paint_order: None,
                    attributes: vec![
                        InspectorAttribute {
                            name: "$debugLine".into(),
                            value: json!({"$line":location,"other":location}).to_string(),
                        },
                        InspectorAttribute {
                            name: "content".into(),
                            value: location.into(),
                        },
                    ],
                }],
            }
        }

        fn rewrite_map(&self) {
            fs::write(
                self.artifacts.source_map_path.as_ref().unwrap(),
                serde_json::to_vec(&self.map).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn inspector_mapping_handles_utf16_crlf_prefix_and_inline_insertion_without_replacing_user_text(
    ) {
        let fixture = MappingFixture::new();
        for needle in ["Text(", ".hideTitleBar"] {
            let location = MappingFixture::location(&fixture.staged, needle, &fixture.target);
            let mut tree = fixture.tree(&location);
            assert_eq!(
                remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree).unwrap(),
                1
            );
            let value: serde_json::Value =
                serde_json::from_str(&tree.nodes[0].attributes[0].value).unwrap();
            let expected = MappingFixture::location(
                &fixture.original,
                needle,
                Path::new("features/entry/src/main/ets/Page.ets"),
            );
            assert_eq!(value["$line"], expected);
            assert_eq!(value["other"], location);
            assert_eq!(tree.nodes[0].attributes[1].value, location);
            assert!(!tree.exact_frame_match);
        }
    }

    #[test]
    fn inspector_generated_import_insertion_and_dispatcher_have_no_original_location() {
        let fixture = MappingFixture::new();
        for needle in ["import {", ".navDestination", "export function"] {
            let mut tree = fixture.tree(&MappingFixture::location(
                &fixture.staged,
                needle,
                &fixture.target,
            ));
            assert_eq!(
                remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree).unwrap(),
                0
            );
            assert_eq!(tree.nodes[0].attributes.len(), 1);
            assert_eq!(tree.nodes[0].attributes[0].name, "content");
        }
    }

    #[test]
    fn inspector_modified_original_or_staged_source_refuses_mapping() {
        for modify_original in [true, false] {
            let fixture = MappingFixture::new();
            let mut tree = fixture.tree(&MappingFixture::location(
                &fixture.staged,
                "Text(",
                &fixture.target,
            ));
            fs::write(
                if modify_original {
                    &fixture.source
                } else {
                    &fixture.target
                },
                "changed after build",
            )
            .unwrap();
            assert_eq!(
                remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree).unwrap(),
                0
            );
            assert!(!tree.nodes[0]
                .attributes
                .iter()
                .any(|attribute| attribute.name == "$debugLine"));
        }
    }

    #[test]
    fn inspector_rejects_malicious_map_roots_paths_offsets_and_locations() {
        for attack in 0..5 {
            let mut fixture = MappingFixture::new();
            let location = MappingFixture::location(&fixture.staged, "Text(", &fixture.target);
            let mut tree = fixture.tree(&location);
            match attack {
                0 => fixture.map["originalRoot"] = json!(fixture.artifacts.root),
                1 => fixture.map["files"][0]["source"] = json!(fixture.target),
                2 => fixture.map["files"][0]["insertions"][0]["offset"] = json!(usize::MAX),
                3 => {
                    tree.nodes[0].attributes[0].value =
                        json!({"$line":"../Page.ets(1:1)"}).to_string()
                }
                _ => {
                    tree.nodes[0].attributes[0].value =
                        json!({"$line":"features/entry/src/main/ets/Page.ets(999999:1)"})
                            .to_string()
                }
            }
            fixture.rewrite_map();
            let _ = remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree);
            assert!(!tree.nodes[0]
                .attributes
                .iter()
                .any(|attribute| attribute.name == "$debugLine"));
        }
    }

    #[test]
    fn inspector_unmodified_copied_file_keeps_safe_original_relative_location() {
        let fixture = MappingFixture::new();
        let relative = "features/entry/src/main/ets/Detail.ets";
        let text = "@Component\nstruct Detail {}\n";
        fs::write(fixture.fixture.original.join(relative), text).unwrap();
        fs::write(fixture.artifacts.root.join(relative), text).unwrap();
        let mut tree = fixture.tree(&format!("{relative}(2:1)"));
        assert_eq!(
            remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree).unwrap(),
            1
        );
        let value: serde_json::Value =
            serde_json::from_str(&tree.nodes[0].attributes[0].value).unwrap();
        assert_eq!(value["$line"], format!("{relative}(2:1)"));
        fs::write(fixture.fixture.original.join(relative), "modified").unwrap();
        assert_eq!(
            remap_inspector_sources(&fixture.project, &fixture.artifacts, &mut tree).unwrap(),
            0
        );
    }
}
