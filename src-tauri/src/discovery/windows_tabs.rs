//! Reads selected tab metadata and positively scoped Codex project metadata. No Text/Value patterns,
//! focus changes, UI actions, conversations, drafts or document contents are accessed.
use super::resolver::{title_paths, WorkspaceResolver};
use super::{path_environment, registration::process_started_at};
use crate::ipc::model::{Evidence, EvidenceSource, TargetInfo, WorkspaceObservation, WorkspaceStatus};
use std::path::Path;
use std::time::{Duration, Instant};
use windows::core::Result as WinResult;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub(crate) struct WindowsTabResolver {
    result: Result<Vec<Evidence>, String>,
    observation: Option<WorkspaceObservation>,
}

impl WindowsTabResolver {
    pub fn collect(handle: isize, target: &TargetInfo, now: u64) -> Self {
        if let Err(reason) = verify_window_lifecycle(handle, target) {
            return Self { result: Err(reason), observation: None };
        }
        if codex_package(handle, target) {
            return Self::codex(handle, target, now);
        }
        Self {
            result: collect(handle, target, now),
            observation: None,
        }
    }

    /// The periodic refresh never performs generic tab discovery or a full UIA content-tree scan.
    pub fn collect_lightweight(handle: isize, target: &TargetInfo, now: u64) -> Option<Self> {
        codex_package(handle, target).then(|| Self::codex(handle, target, now))
    }

    pub fn observation(&self) -> Option<WorkspaceObservation> {
        self.observation.clone()
    }

    fn codex(handle: isize, target: &TargetInfo, now: u64) -> Self {
        let collected = verify_window_lifecycle(handle, target)
            .and_then(|_| unsafe { codex_projects(handle, target, now) })
            .and_then(|result| {
                verify_window_lifecycle(handle, target)?;
                if !codex_package(handle, target) {
                    return Err("Codex package identity changed while reading metadata".into());
                }
                Ok(result)
            });
        match collected {
            Ok((evidence, observation)) => Self { result: Ok(evidence), observation: Some(observation) },
            Err(reason) => Self {
                result: Err(reason.clone()),
                observation: Some(WorkspaceObservation {
                    status: WorkspaceStatus::Unknown,
                    project_name: None,
                    active_path: None,
                    reason,
                    observed_at: now,
                }),
            },
        }
    }
}
impl WorkspaceResolver for WindowsTabResolver {
    fn evidence(&self, _target: &TargetInfo, _now: u64) -> Result<Vec<Evidence>, String> {
        self.result.clone()
    }
}

fn collect(handle: isize, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String> {
    verify_window_lifecycle(handle, target)?;
    let result = unsafe { selected_tabs(handle, target, now) }
        .map_err(|error| format!("UIA selected-tab metadata unavailable: {}", error.code()))?;
    verify_window_lifecycle(handle, target)?;
    Ok(result)
}

fn verify_window_lifecycle(handle: isize, target: &TargetInfo) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindowThreadProcessId, IsWindow, GA_ROOT};
    let expected_start = target.process.started_at.parse::<u64>()
        .map_err(|_| "Invalid UIA process lifecycle")?;
    unsafe {
        let hwnd = handle as *mut std::ffi::c_void;
        let mut pid = 0;
        if IsWindow(hwnd) == 0 || GetAncestor(hwnd, GA_ROOT) != hwnd
            || GetWindowThreadProcessId(hwnd, &mut pid) == 0 || pid != target.process.pid
            || process_started_at(pid)? != expected_start
        {
            return Err("UIA target HWND or process lifecycle changed".into());
        }
    }
    Ok(())
}

/// Family includes the publisher identity. This installed, registered package's AppxManifest
/// (26.1007.2314) declares app/ChatGPT.exe. Its executable name alone grants no host authority.
/// Other families/build layouts fail closed until their package identity is independently known.
fn codex_package(handle: isize, target: &TargetInfo) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::Packaging::Appx::{GetPackageFamilyName, GetPackageFullName, GetPackagePathByFullName};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    if verify_window_lifecycle(handle, target).is_err() { return false; }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.process.pid);
        if process.is_null() { return false; }
        let identity = (|| {
            let family = package_string(|length, buffer| GetPackageFamilyName(process, length, buffer))?;
            if family != "OpenAI.Codex_2p2nqsd0c76g0" { return None; }
            let full = package_string(|length, buffer| GetPackageFullName(process, length, buffer))?;
            let full_wide: Vec<u16> = full.encode_utf16().chain(Some(0)).collect();
            let package_path = package_string(|length, buffer| GetPackagePathByFullName(full_wide.as_ptr(), length, buffer))?;
            let mut image = vec![0u16; 32_768];
            let mut size = image.len() as u32;
            if QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut size) == 0 { return None; }
            let actual = String::from_utf16(&image[..size as usize]).ok()?;
            let expected = Path::new(&package_path).join("app").join("ChatGPT.exe");
            let actual = std::fs::canonicalize(actual).ok()?;
            let expected = std::fs::canonicalize(expected).ok()?;
            Some(actual.to_string_lossy().eq_ignore_ascii_case(&expected.to_string_lossy()))
        })();
        CloseHandle(process);
        identity == Some(true) && verify_window_lifecycle(handle, target).is_ok()
    }
}

fn package_string(mut query: impl FnMut(*mut u32, *mut u16) -> u32) -> Option<String> {
    let mut size = 0;
    if query(&mut size, std::ptr::null_mut()) != 122 || size == 0 || size > 32_768 { return None; }
    let mut buffer = vec![0u16; size as usize];
    if query(&mut size, buffer.as_mut_ptr()) != 0 || size == 0 || size as usize > buffer.len() { return None; }
    String::from_utf16(&buffer[..size as usize - 1]).ok()
}

struct Metadata {
    kind: UIA_CONTROLTYPE_ID,
    role: String,
    automation_id: String,
    rectangle: windows::Win32::Foundation::RECT,
}

struct MetadataScan<'a> {
    uia: &'a IUIAutomation2,
    walker: IUIAutomationTreeWalker,
    raw_walker: IUIAutomationTreeWalker,
    pid: u32,
    visited: usize,
    deadline: Instant,
}

impl MetadataScan<'_> {
    fn budget(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            Err("Codex project metadata refresh deadline elapsed".into())
        } else { Ok(()) }
    }

    fn read<T>(&self, operation: impl FnOnce() -> WinResult<T>) -> Result<T, String> {
        self.budget()?;
        let result = operation().map_err(uia_error)?;
        self.budget()?;
        Ok(result)
    }

    fn optional_element(&self, operation: impl FnOnce() -> WinResult<IUIAutomationElement>)
        -> Result<Option<IUIAutomationElement>, String>
    {
        self.budget()?;
        // windows-core 0.62.2 converts a successful null COM interface into Error::empty()
        // (HRESULT S_OK). Only that means the end of a branch. Provider failures must not be
        // mistaken for an empty tree and turn an incomplete collection into unique evidence.
        let result = match operation() {
            Ok(element) => Some(element),
            Err(error) if error.code().0 == 0 => None,
            Err(error) => return Err(uia_error(error)),
        };
        self.budget()?;
        Ok(result)
    }

    unsafe fn metadata(&mut self, element: &IUIAutomationElement) -> Result<Metadata, String> {
        self.visited += 1;
        if self.visited > 192 || Instant::now() >= self.deadline {
            return Err("Codex project metadata exceeded the bounded UIA refresh budget".into());
        }
        if self.read(|| element.CurrentProcessId())? as u32 != self.pid {
            return Err("Codex project metadata belongs to a different process".into());
        }
        Ok(Metadata {
            kind: self.read(|| element.CurrentControlType())?,
            role: self.read(|| element.CurrentAriaRole())?.to_string(),
            automation_id: self.read(|| element.CurrentAutomationId())?.to_string(),
            rectangle: self.read(|| element.CurrentBoundingRectangle())?,
        })
    }

    /// Provider-side equality at TreeScope_Element avoids retrieving any other control's Name.
    /// It does not FindAll/FindFirst over the target's descendants or collect text for regex search.
    unsafe fn named(&self, element: &IUIAutomationElement, names: &[&str]) -> bool {
        names.iter().any(|name| {
            let value = windows::Win32::System::Variant::VARIANT::from(*name);
            self.read(|| self.uia.CreatePropertyCondition(UIA_NamePropertyId, &value)).ok()
                .and_then(|condition| self.read(|| element.FindFirst(TreeScope_Element, &condition)).ok())
                .is_some()
        })
    }

    unsafe fn children(&mut self, parent: &IUIAutomationElement) -> Result<Vec<(IUIAutomationElement, Metadata)>, String> {
        let mut result = Vec::new();
        let mut next = self.optional_element(|| self.walker.GetFirstChildElement(parent))?;
        while let Some(element) = next {
            let meta = self.metadata(&element)?;
            next = self.optional_element(|| self.walker.GetNextSiblingElement(&element))?;
            if visible(&meta) && !self.read(|| element.CurrentIsOffscreen())?.as_bool() {
                result.push((element, meta));
            }
        }
        Ok(result)
    }
}

fn uia_error(error: windows::core::Error) -> String {
    format!("Codex project metadata UIA provider unavailable: {}", error.code())
}

fn visible(meta: &Metadata) -> bool {
    meta.rectangle.right > meta.rectangle.left && meta.rectangle.bottom > meta.rectangle.top
}

fn container(meta: &Metadata) -> bool {
    // Never descend an edit, chat list item, content Text control or nested document.
    [UIA_PaneControlTypeId, UIA_GroupControlTypeId, UIA_CustomControlTypeId,
        UIA_ToolBarControlTypeId].contains(&meta.kind)
        && !["main", "article", "log", "textbox", "document", "feed", "list",
            "listitem", "tree", "treeitem", "dialog"].contains(&meta.role.as_str())
}

fn protected_content(meta: &Metadata) -> bool {
    [UIA_EditControlTypeId, UIA_TextControlTypeId, UIA_ListItemControlTypeId,
        UIA_TreeControlTypeId, UIA_TreeItemControlTypeId].contains(&meta.kind)
        || ["main", "article", "log", "textbox", "feed", "listitem", "treeitem", "dialog"]
            .contains(&meta.role.as_str())
}

struct ProjectCard {
    element: IUIAutomationElement,
    outer: IUIAutomationElement,
    name: String,
    active: bool,
    diagnostic: String,
}

/// Confirmed STATIC source anchors from the installed OpenAI.Codex 26.1007.2314 app.asar,
/// webview/assets/app-initial-b8e1c8014bb4.js: Gbs/Jis/Y4o/ORo render project listitem ->
/// first header button, aria-current=page iff activeProjectId===projectId. Uis/Ars/Urs render
/// project rich tooltip + Edit project; Wrs/Grs render Open {source} and a full Windows path.
/// Uis uses shared BO/w5/cSi/lSi: role=tooltip with aria-describedby on the composite trigger's
/// ancestor span (no dialogLabel). That wrapper can be omitted from the UIA control view.
/// DOM data-app-action-* attributes are NOT treated as UIA AutomationIds. Chromium's runtime
/// ControlType/ARIA/Name mapping has NOT been Windows-tested; absent mappings stay unknown.
unsafe fn codex_projects(handle: isize, target: &TargetInfo, now: u64) -> Result<(Vec<Evidence>, WorkspaceObservation), String> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(uia_error)?;
    let _apartment = Apartment;
    let uia: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).map_err(uia_error)?;
    uia.SetAutoSetFocus(false).map_err(uia_error)?;
    uia.SetConnectionTimeout(250).map_err(uia_error)?;
    uia.SetTransactionTimeout(250).map_err(uia_error)?;
    let root = uia.ElementFromHandle(HWND(handle as *mut _)).map_err(uia_error)?;
    let mut scan = MetadataScan {
        uia: &uia, walker: uia.ControlViewWalker().map_err(uia_error)?, pid: target.process.pid,
        raw_walker: uia.RawViewWalker().map_err(uia_error)?,
        visited: 0, deadline: Instant::now() + Duration::from_millis(1500),
    };
    let root_meta = scan.metadata(&root)?;
    let (regions, tooltips) = project_regions(&mut scan, &root, root_meta)?;
    let mut cards = Vec::new();
    for region in regions { collect_project_cards(&mut scan, &region, &mut cards)?; }
    let active_names: Vec<_> = cards.iter().filter(|card| card.active).map(|card| card.name.clone()).collect();
    let project_name = (active_names.len() == 1).then(|| active_names[0].clone());
    let mut evidence = Vec::new();
    let mut verified_path = None;
    let mut diagnostic = None;
    for popup_element in tooltips {
        // First identify the exact shipped static action. No arbitrary tooltip Names are read.
        let Some(popup) = project_popup(&mut scan, popup_element)? else { continue; };
        for card in &cards {
            // Exact same project Name must exist in a discrete popup control before any path read.
            // A hover popup by itself is never an active workspace marker.
            if !popup_has_project(&scan, &popup, &card.name) { continue; }
            let paths = popup_paths(&mut scan, &popup)?;
            let unique_name = cards.iter().filter(|other| other.name == card.name).count() == 1;
            let attributed = unique_name && card.active && active_names.len() == 1
                && project_trigger_owns_popup(&scan, card, &popup.element);
            // Re-read active state and discrete popup name after path collection: a project can
            // change while HWND/PID remain identical. Expanded/focused/hovered are never selection.
            let still_active = active_project_header(&scan, &card.element)?
                && scan.named(&card.element, &[card.name.as_str()])
                && popup_has_project(&scan, &popup, &card.name)
                && popup.controls.iter().any(|control| scan.named(control, &["Edit project", "编辑项目"]))
                && project_trigger_owns_popup(&scan, card, &popup.element)
                && popup_paths(&mut scan, &popup)? == paths;
            for path in &paths {
                let active_scope = attributed && still_active && paths.len() == 1;
                evidence.push(Evidence {
                    source: EvidenceSource::CodexProjectMetadata,
                    target_id: Some(target.id.clone()), process: Some(target.process.clone()),
                    session_id: Some(format!("uia-{}", target.id)), observed_at: now,
                    expires_at: now.saturating_add(15_000), environment: path_environment(Path::new(path)),
                    raw_path: path.clone(), verifiable: true, active_scope,
                });
                if active_scope { verified_path = Some(path.clone()); }
            }
            diagnostic = Some(card.diagnostic.clone());
        }
    }
    let mut current_cards = Vec::new();
    for card in &cards {
        if active_project_header(&scan, &card.element)? { current_cards.push(card); }
    }
    let active_paths: std::collections::HashSet<_> = evidence.iter().filter(|item| item.active_scope)
        .map(|item| item.raw_path.clone()).collect();
    if current_cards.len() != 1 || current_cards.first().map(|card| &card.name) != project_name.as_ref()
        || active_paths.len() > 1
    {
        // Inconsistent snapshots or more than one current path cannot become automatic evidence.
        for item in &mut evidence { item.active_scope = false; }
        verified_path = None;
    }
    scan.budget()?;
    let (status, reason) = if verified_path.is_some() {
        (WorkspaceStatus::Verified, "Verified Codex selected project, associated project popup path and current window lifecycle".to_owned())
    } else if !evidence.is_empty() {
        (WorkspaceStatus::Suggested, "Codex project popup path read; current-project selection or UIA popup ownership could not be proved".to_owned())
    } else if project_name.is_some() {
        (WorkspaceStatus::Suggested, "Codex selected project name read; no associated full-path metadata is accessible. Open project information, then refresh".to_owned())
    } else {
        (WorkspaceStatus::Unknown, "Codex current workspace is unverified: Projects section, selected project marker or project metadata popup is not exposed through scoped UIA".to_owned())
    };
    Ok((evidence, WorkspaceObservation {
        status, project_name, active_path: verified_path,
        reason: diagnostic.map_or(reason.clone(), |summary| format!("{reason}; {summary}")), observed_at: now,
    }))
}

unsafe fn project_regions(scan: &mut MetadataScan<'_>, root: &IUIAutomationElement, root_meta: Metadata)
    -> Result<(Vec<IUIAutomationElement>, Vec<IUIAutomationElement>), String>
{
    let mut queue = std::collections::VecDeque::from([(root.clone(), root_meta, 0)]);
    let mut regions = Vec::new();
    let mut popups = Vec::new();
    let mut shell_document_seen = false;
    while let Some((element, meta, depth)) = queue.pop_front() {
        if meta.role == "navigation" {
            if let Some(region) = navigation_projects(scan, &element)? { regions.push(region); }
            continue;
        }
        if meta.kind == UIA_ToolTipControlTypeId || meta.role == "tooltip" {
            popups.push(element);
            continue;
        }
        // Only the outer Chromium application document is a shell traversal bridge. A main,
        // article, log, edit, listitem or nested document branch is pruned before visiting children.
        if protected_content(&meta) || meta.kind == UIA_ListControlTypeId || meta.role == "list" {
            continue;
        }
        let shell_document = meta.kind == UIA_DocumentControlTypeId
            && depth <= 2 && !shell_document_seen;
        if meta.kind == UIA_DocumentControlTypeId {
            if !shell_document { continue; }
            shell_document_seen = true;
        }
        if depth < 6 && (container(&meta) || shell_document || depth == 0) {
            for (child, meta) in scan.children(&element)? { queue.push_back((child, meta, depth + 1)); }
        }
    }
    Ok((regions, popups))
}

unsafe fn navigation_projects(scan: &mut MetadataScan<'_>, navigation: &IUIAutomationElement)
    -> Result<Option<IUIAutomationElement>, String>
{
    let mut queue = std::collections::VecDeque::from([(navigation.clone(), 0)]);
    while let Some((parent, depth)) = queue.pop_front() {
        for (element, meta) in scan.children(&parent)? {
            if meta.kind == UIA_ButtonControlTypeId && scan.named(&element, &["Projects", "项目"])
            {
                let properties = scan.read(|| element.CurrentAriaProperties())?.to_string();
                if aria_property(&properties, "expanded").is_some() {
                    if let Some(list) = project_list_for_header(scan, &element)? { return Ok(Some(list)); }
                }
            }
            if depth < 5 && container(&meta) && meta.role != "listitem" {
                queue.push_back((element, depth + 1));
            }
        }
    }
    Ok(None)
}

/// _xs/UF/Abs in the shipped source put the Projects button and role=list in sibling branches,
/// with several anonymous wrappers. Find their smallest enclosing section, not a fixed parent.
/// Stop before navigation/document/main; a wider sidebar can include private recent-chat lists.
unsafe fn project_list_for_header(scan: &mut MetadataScan<'_>, header: &IUIAutomationElement)
    -> Result<Option<IUIAutomationElement>, String>
{
    let mut ancestor = scan.optional_element(|| scan.walker.GetParentElement(header))?;
    for _ in 0..6 {
        let Some(section) = ancestor else { break; };
        let meta = scan.metadata(&section)?;
        if protected_content(&meta) || meta.role == "navigation"
            || meta.kind == UIA_DocumentControlTypeId || !container(&meta) { break; }
        let mut lists = Vec::new();
        let mut queue = std::collections::VecDeque::from([(section.clone(), 0)]);
        while let Some((parent, depth)) = queue.pop_front() {
            for (child, meta) in scan.children(&parent)? {
                if meta.kind == UIA_ListControlTypeId || meta.role == "list" {
                    lists.push(child);
                    // Never descend a list or read listitem names while locating the section.
                } else if depth < 6 && container(&meta) && meta.role != "navigation" {
                    queue.push_back((child, depth + 1));
                }
            }
        }
        if lists.len() == 1 { return Ok(lists.pop()); }
        if lists.len() > 1 { return Ok(None); }
        ancestor = scan.optional_element(|| scan.walker.GetParentElement(&section))?;
    }
    Ok(None)
}

unsafe fn collect_project_cards(scan: &mut MetadataScan<'_>, region: &IUIAutomationElement, cards: &mut Vec<ProjectCard>)
    -> Result<(), String>
{
    // Abs's direct role=list children are the Gbs project cards. Nested listitems are chats.
    for (element, meta) in scan.children(region)? {
        if meta.kind != UIA_ListItemControlTypeId && meta.role != "listitem" { continue; }
        // Gbs explicitly gives this outer project card aria-label=project label. Only after
        // positive section/list scoping do we read it. No nested chat row Name is requested.
        let name = scan.read(|| element.CurrentName())?.to_string();
        if name.is_empty() || name.len() > 256 || name.contains(['\n', '\r', '\0']) { continue; }
        if let Some((current, current_meta)) = project_header(scan, &element, 0)? {
            if !scan.named(&current, &[name.as_str()]) { continue; }
            let aria = scan.read(|| current.CurrentAriaProperties())?.to_string();
            // open-project-page headers legitimately omit expanded; only current=page marks
            // activity. Their project-card label and owned popup provide the remaining proof.
            let active = aria_property(&aria, "current") == Some("page");
            let rect = current_meta.rectangle;
            let id: String = current_meta.automation_id.chars().take(80).collect();
            cards.push(ProjectCard {
                element: current, outer: element, name, active,
                diagnostic: format!("Projects section > project list > project listitem > header; AutomationId={id:?}, ControlType={}, bounds=({},{},{},{}), aria-current-page={active}", current_meta.kind.0, rect.left, rect.top, rect.right, rect.bottom),
            });
        }
    }
    Ok(())
}

unsafe fn project_header(scan: &mut MetadataScan<'_>, parent: &IUIAutomationElement, depth: usize)
    -> Result<Option<(IUIAutomationElement, Metadata)>, String>
{
    if depth >= 4 { return Ok(None); }
    for (element, meta) in scan.children(parent)? {
        if meta.kind == UIA_ButtonControlTypeId { return Ok(Some((element, meta))); }
        if container(&meta) && meta.kind != UIA_ListControlTypeId
            && !["list", "listitem", "tree", "treeitem"].contains(&meta.role.as_str())
        {
            if let Some(header) = project_header(scan, &element, depth + 1)? { return Ok(Some(header)); }
        }
    }
    Ok(None)
}

fn aria_property<'a>(properties: &'a str, key: &str) -> Option<&'a str> {
    properties.split(';').filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| (name.trim().trim_start_matches("aria-") == key).then_some(value.trim()))
}

unsafe fn active_project_header(scan: &MetadataScan<'_>, element: &IUIAutomationElement) -> Result<bool, String> {
    if scan.read(|| element.CurrentIsOffscreen())?.as_bool() { return Ok(false); }
    let properties = scan.read(|| element.CurrentAriaProperties())?.to_string();
    Ok(aria_property(&properties, "current") == Some("page"))
}

unsafe fn popup_controls(scan: &mut MetadataScan<'_>, popup: &IUIAutomationElement)
    -> Result<Vec<IUIAutomationElement>, String>
{
    let mut result = Vec::new();
    let mut queue = std::collections::VecDeque::from([(popup.clone(), 0)]);
    while let Some((parent, depth)) = queue.pop_front() {
        for (element, meta) in scan.children(&parent)? {
            if [UIA_ButtonControlTypeId, UIA_TextControlTypeId].contains(&meta.kind) {
                result.push(element);
            } else if depth < 5 && container(&meta) { queue.push_back((element, depth + 1)); }
        }
    }
    Ok(result)
}

struct ProjectPopup {
    element: IUIAutomationElement,
    controls: Vec<IUIAutomationElement>,
}

unsafe fn project_popup(scan: &mut MetadataScan<'_>, popup: IUIAutomationElement) -> Result<Option<ProjectPopup>, String> {
    let controls = popup_controls(scan, &popup)?;
    let project_metadata = controls.iter().any(|control| {
        scan.read(|| control.CurrentControlType()).ok() == Some(UIA_ButtonControlTypeId)
            && scan.named(control, &["Edit project", "编辑项目"])
    });
    Ok(project_metadata.then_some(ProjectPopup { element: popup, controls }))
}

unsafe fn popup_has_project(scan: &MetadataScan<'_>, popup: &ProjectPopup, project_name: &str)
    -> bool
{
    popup.controls.iter().any(|control| scan.named(control, &[project_name]))
}

unsafe fn popup_paths(scan: &mut MetadataScan<'_>, popup: &ProjectPopup) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for control in &popup.controls {
        if scan.read(|| control.CurrentControlType())? != UIA_ButtonControlTypeId { continue; }
        // Positively scoped Ars/Grs project metadata button Names only; no aggregate popup Name,
        // UIA TextPattern, edit value, main-body Text or arbitrary regular-expression path search.
        let label = scan.read(|| control.CurrentName())?.to_string();
        let Some(path) = label.strip_prefix("Open ").or_else(|| label.strip_prefix("打开 "))
            .or_else(|| label.strip_prefix("打开")) else { continue; };
        let path = path.trim();
        let bytes = path.as_bytes();
        let absolute = (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
            && [b'\\', b'/'].contains(&bytes[2])) || path.starts_with("\\\\")
            || path.starts_with("wsl://") || path.starts_with("ssh://") || path.starts_with("vscode-remote://");
        if absolute && path.len() <= 4096 && !path.contains(['\n', '\r', '\0']) && !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_owned());
        }
    }
    Ok(paths)
}

unsafe fn project_trigger_owns_popup(scan: &MetadataScan<'_>, card: &ProjectCard, popup: &IUIAutomationElement) -> bool {
    let mut ancestors = Vec::new();
    let mut current = Some(card.element.clone());
    let mut reached_card = false;
    for _ in 0..6 {
        let Some(element) = current else { break; };
        let Ok(is_card) = scan.read(|| scan.uia.CompareElements(&element, &card.outer)) else { return false; };
        ancestors.push(element.clone());
        if is_card.as_bool() { reached_card = true; break; }
        let Ok(parent) = scan.optional_element(|| scan.raw_walker.GetParentElement(&element)) else { return false; };
        current = parent;
    }
    // Prove containment before using any wrapper's description link. Never accept a link from
    // navigation, a shared document ancestor or another card. Only ancestors are visited.
    reached_card && ancestors.iter().any(|trigger| described_by_popup(scan, trigger, popup))
}

unsafe fn described_by_popup(scan: &MetadataScan<'_>, trigger: &IUIAutomationElement, popup: &IUIAutomationElement) -> bool {
    let Ok(descriptions) = scan.read(|| trigger.CurrentDescribedBy()) else { return false; };
    let Ok(count) = scan.read(|| descriptions.Length()) else { return false; };
    if !(1..=4).contains(&count) { return false; }
    (0..count).any(|index| {
        let Ok(description) = scan.read(|| descriptions.GetElement(index)) else { return false; };
        // Some providers return the discrete description inside the tooltip instead of its root.
        // Accept only the popup or its own description descendants. A shared parent of the
        // popup is not ownership proof. No description aggregate Name/Text is requested.
        let mut current = Some(description);
        for _ in 0..4 {
            let Some(element) = current else { break; };
            if scan.read(|| scan.uia.CompareElements(popup, &element)).map(|value| value.as_bool()).unwrap_or(false) { return true; }
            current = scan.read(|| scan.walker.GetParentElement(&element)).ok();
        }
        false
    })
}

unsafe fn selected_tabs(handle: isize, target: &TargetInfo, now: u64) -> WinResult<Vec<Evidence>> {
    // This backend thread owns no windows. No automation interfaces leave this MTA apartment.
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let _apartment = Apartment;
    let uia: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
    uia.SetAutoSetFocus(false)?;
    uia.SetConnectionTimeout(500)?;
    uia.SetTransactionTimeout(500)?;
    let root = uia.ElementFromHandle(HWND(handle as *mut _))?;
    if root.CurrentProcessId()? as u32 != target.process.pid {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80070005_u32 as i32,
        )));
    }
    let walker = uia.ControlViewWalker()?;
    let mut queue = std::collections::VecDeque::from([(root, 0)]);
    let mut count = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut result = Vec::new();
    while let Some((parent, depth)) = queue.pop_front() {
        let mut child = walker.GetFirstChildElement(&parent).ok();
        while let Some(element) = child {
            count += 1;
            if count > 128 || Instant::now() >= deadline {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x800705b4_u32 as i32,
                )));
            }
            let kind = element.CurrentControlType()?;
            if kind == UIA_TabItemControlTypeId {
                if let Ok(selection) = element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                {
                    if selection.CurrentIsSelected()?.as_bool()
                        && !element.CurrentIsOffscreen()?.as_bool()
                    {
                        let name = element.CurrentName()?;
                        if name.len() <= 4096 {
                            for path in title_paths(&name.to_string()) {
                                result.push(Evidence {
                                    source: EvidenceSource::Uia,
                                    target_id: Some(target.id.clone()),
                                    process: Some(target.process.clone()),
                                    session_id: Some(format!("uia-{}", target.id)),
                                    observed_at: now,
                                    expires_at: now.saturating_add(30_000),
                                    environment: path_environment(Path::new(&path)),
                                    raw_path: path,
                                    verifiable: true,
                                    // Selection alone proves no generic application's workspace semantics.
                                    active_scope: false,
                                });
                            }
                        }
                    }
                }
            } else if depth < 4
                && [
                    UIA_PaneControlTypeId,
                    UIA_TabControlTypeId,
                    UIA_ToolBarControlTypeId,
                    UIA_GroupControlTypeId,
                ]
                .contains(&kind)
            {
                queue.push_back((element.clone(), depth + 1));
            }
            // Document, text, edit, list and browser content are never descended or read.
            child = walker.GetNextSiblingElement(&element).ok();
        }
    }
    Ok(result)
}
