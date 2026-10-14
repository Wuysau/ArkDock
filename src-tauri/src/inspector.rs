//! Bounded reads of the owned Previewer command pipe. Native admission is separate
//! from parser tests and from the public OpenHarmony source snapshot.
use crate::ipc::model::{
    BackendStatus, BinaryFrameMetadata, CheckStatus, InspectorAttribute, InspectorBounds,
    InspectorCapabilities, InspectorNode, InspectorTree,
};
use crate::scheduler::BuildTask;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const MAX_INSPECTION_BYTES: usize = 1024 * 1024;
pub const INSPECTION_TIMEOUT_MS: u64 = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectorCommand {
    Inspector,
    Default,
}

impl InspectorCommand {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Inspector => "inspector",
            Self::Default => "inspectorDefault",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RawInspectionResponse {
    pub command: InspectorCommand,
    pub protocol_version: String,
    pub result: Value,
    pub frame_sequence: u64,
    pub requested_at: u64,
    pub received_at: u64,
    pub query_id: u64,
    pub task: BuildTask,
}

pub type InspectionReceiver = std::sync::mpsc::Receiver<Result<RawInspectionResponse, String>>;

#[cfg(windows)]
pub(crate) struct InspectionRequest {
    pub command: InspectorCommand,
    pub reply: std::sync::mpsc::SyncSender<Result<RawInspectionResponse, String>>,
}

/// Admission is limited to the exact binary fingerprint already checked by PreviewBackend.
pub fn capabilities(backend: &BackendStatus) -> InspectorCapabilities {
    let available = backend.available && backend.sdk_version.as_deref() == Some("26.0.0.105");
    InspectorCapabilities {
        available,
        status: if available {
            CheckStatus::Pass
        } else {
            CheckStatus::Blocked
        },
        reason: if available {
            "HarmonyOS SDK 26.0.0.105 Windows native inspector action and component geometry were measured; screenshot correspondence is observational, not an atomic frame proof".into()
        } else {
            format!("INSPECTOR_UNAVAILABLE: {}", backend.reason)
        },
    }
}

const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_ATTRIBUTES: usize = 512;
const MAX_ATTRIBUTE_BYTES: usize = 16 * 1024;

/// Parse only the actual commercial SDK26 `$children/$ID/$rect/$attrs` schema.
/// `frame_sequence` identifies the caller's displayed image, while the reason
/// preserves the range observed by the host during this non-atomic query.
pub fn parse_tree(
    raw: &RawInspectionResponse,
    displayed: &BinaryFrameMetadata,
) -> Result<InspectorTree, String> {
    if raw.command != InspectorCommand::Inspector || raw.protocol_version != "1.0.1" {
        return Err("INSPECTOR_COMMAND_NOT_COMPONENT_TREE".into());
    }
    if raw.task.workspace_id != displayed.workspace_id
        || raw.task.session_id != displayed.session_id
        || raw.task.build_id != displayed.build_id
        || raw.task.generation != displayed.generation
        || displayed.sequence == 0
        || raw.frame_sequence < displayed.sequence
        || displayed.protocol_version != 1
        || !(50..=crate::preview::frames::MAX_SURFACE_DIMENSION).contains(&displayed.width)
        || !(50..=crate::preview::frames::MAX_SURFACE_DIMENSION).contains(&displayed.height)
        || u64::from(displayed.width) * u64::from(displayed.height)
            > crate::preview::frames::MAX_SURFACE_PIXELS
        || raw.received_at < raw.requested_at
        || raw.query_id == 0
    {
        return Err("INSPECTOR_STALE_FRAME_IDENTITY".into());
    }
    let text = raw.result.as_str().ok_or("INSPECTOR_RESULT_NOT_STRING")?;
    if text.len() > MAX_INSPECTION_BYTES {
        return Err("INSPECTOR_PAYLOAD_LIMIT".into());
    }
    if text.is_empty() || text == "null" {
        return Err("INSPECTOR_EMPTY_TREE".into());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| "INSPECTOR_TREE_JSON")?;
    if value.get("$type").and_then(Value::as_str) != Some("root")
        || scalar_number(value.get("width")) != Some(f64::from(displayed.width))
        || scalar_number(value.get("height")) != Some(f64::from(displayed.height))
        || scalar_number(value.get("$resolution")) != Some(1.0)
    {
        return Err("INSPECTOR_SURFACE_PROFILE_UNVERIFIED".into());
    }
    let children = value
        .get("$children")
        .and_then(Value::as_array)
        .ok_or("INSPECTOR_CHILDREN_SCHEMA")?;
    let mut nodes = Vec::new();
    let mut ids = HashSet::new();
    for child in children {
        parse_node(child, None, 0, &mut nodes, &mut ids)?;
    }
    if nodes.is_empty() {
        return Err("INSPECTOR_EMPTY_TREE".into());
    }
    Ok(InspectorTree {
        workspace_id: displayed.workspace_id.clone(), session_id: displayed.session_id.clone(),
        build_id: displayed.build_id.clone(), generation: displayed.generation,
        frame_sequence: displayed.sequence, inspection_version: raw.query_id,
        queried_at: raw.requested_at, received_at: raw.received_at, exact_frame_match: false,
        reason: format!("Native component tree from the same owned runtime/build; displayed frame {}, last host-observed frame {}. Previewer provides no atomic frame ID or paint order; moving/occluded content requires confirmation. Source locations require independent source validation.",displayed.sequence,raw.frame_sequence),
        nodes,
    })
}

fn scalar_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let number = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    number.is_finite().then_some(number)
}
fn native_id(node: &Value) -> Result<String, String> {
    match node.get("$ID") {
        Some(Value::Number(id)) if id.as_u64().is_some() => Ok(id.to_string()),
        _ => Err("INSPECTOR_NODE_ID_SCHEMA".into()),
    }
}
fn parse_bounds(value: Option<&Value>) -> Result<Option<InspectorBounds>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let text = value.as_str().ok_or("INSPECTOR_RECT_SCHEMA")?;
    let coordinates = text
        .split(',')
        .map(|part| part.parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "INSPECTOR_RECT_SCHEMA")?;
    if coordinates.len() != 4
        || coordinates
            .iter()
            .any(|number| !number.is_finite() || number.abs() > 65536.0)
        || coordinates[2] < 0.0
        || coordinates[3] < 0.0
    {
        return Err("INSPECTOR_RECT_RANGE".into());
    }
    Ok(Some(InspectorBounds {
        x: coordinates[0],
        y: coordinates[1],
        width: coordinates[2],
        height: coordinates[3],
    }))
}
fn attribute(name: &str, value: &Value) -> Result<InspectorAttribute, String> {
    let text = match value {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).map_err(|_| "INSPECTOR_ATTRIBUTE_JSON")?,
    };
    if name.len() > 256 || text.len() > MAX_ATTRIBUTE_BYTES {
        return Err("INSPECTOR_ATTRIBUTE_LIMIT".into());
    }
    Ok(InspectorAttribute {
        name: name.into(),
        value: text,
    })
}
fn parse_node(
    value: &Value,
    parent: Option<&str>,
    depth: usize,
    nodes: &mut Vec<InspectorNode>,
    ids: &mut HashSet<String>,
) -> Result<String, String> {
    if nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
        return Err("INSPECTOR_TREE_LIMIT".into());
    }
    let node_id = native_id(value)?;
    if !ids.insert(node_id.clone()) {
        return Err("INSPECTOR_DUPLICATE_NODE_ID".into());
    }
    let component_type = value
        .get("$type")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 128)
        .ok_or("INSPECTOR_COMPONENT_SCHEMA")?;
    let attrs = value
        .get("$attrs")
        .and_then(Value::as_object)
        .ok_or("INSPECTOR_ATTRIBUTES_SCHEMA")?;
    if attrs.len() > MAX_ATTRIBUTES {
        return Err("INSPECTOR_ATTRIBUTE_LIMIT".into());
    }
    let mut attributes = attrs
        .iter()
        .map(|(name, value)| attribute(name, value))
        .collect::<Result<Vec<_>, _>>()?;
    for field in ["$debugLine", "$viewID"] {
        if let Some(value) = value.get(field) {
            attributes.push(attribute(field, value)?);
        }
    }
    let visible = match attrs.get("visibility").and_then(Value::as_str) {
        Some("Visibility.Visible") => Some(true),
        Some("Visibility.Hidden" | "Visibility.None") => Some(false),
        _ => None,
    };
    let index = nodes.len();
    nodes.push(InspectorNode {
        node_id: node_id.clone(),
        component_type: Some(component_type.into()),
        parent_id: parent.map(str::to_string),
        children: Vec::new(),
        bounds: parse_bounds(value.get("$rect"))?,
        visible,
        attributes,
        page: None,
        depth: depth as u32,
        paint_order: None,
    });
    if let Some(children) = value.get("$children") {
        let children = children.as_array().ok_or("INSPECTOR_CHILDREN_SCHEMA")?;
        for child in children {
            let child_id = parse_node(child, Some(&node_id), depth + 1, nodes, ids)?;
            nodes[index].children.push(child_id);
        }
    }
    Ok(node_id)
}

/// Bounds intersection used by local hit tests when a parent declares clipping.
/// It does not infer z-order, transforms, scroll offsets or absent native fields.
pub fn intersect_bounds(
    child: &InspectorBounds,
    clip: &InspectorBounds,
) -> Option<InspectorBounds> {
    let x = child.x.max(clip.x);
    let y = child.y.max(clip.y);
    let right = (child.x + child.width).min(clip.x + clip.width);
    let bottom = (child.y + child.height).min(clip.y + clip.height);
    (right > x && bottom > y).then_some(InspectorBounds {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::NativeFrameFormat;
    fn example(tree: Value) -> (RawInspectionResponse, BinaryFrameMetadata) {
        let displayed = BinaryFrameMetadata {
            protocol_version: 1,
            workspace_id: "w".into(),
            session_id: "s".into(),
            build_id: "b".into(),
            generation: 1,
            sequence: 1,
            width: 360,
            height: 640,
            captured_at: 1,
            rotation: 0,
            format: NativeFrameFormat::Jpeg,
            payload_length: 8,
        };
        let raw = RawInspectionResponse {
            command: InspectorCommand::Inspector,
            protocol_version: "1.0.1".into(),
            result: Value::String(tree.to_string()),
            frame_sequence: 2,
            requested_at: 2,
            received_at: 3,
            query_id: 1,
            task: BuildTask {
                epoch: 1,
                workspace_id: "w".into(),
                session_id: "s".into(),
                build_id: "b".into(),
                generation: 1,
                started_at: 1,
            },
        };
        (raw, displayed)
    }
    fn root(children: Value) -> Value {
        serde_json::json!({"$type":"root","width":"360.000000","height":"640.000000","$resolution":"1.000000","$children":children})
    }
    fn node(id: u64) -> Value {
        serde_json::json!({"$ID":id,"$type":"Text","$rect":"-1.5,20,100.5,25","$attrs":{"visibility":"Visibility.Visible","content":"test","zIndex":2},"$debugLine":"{\"$line\":\"entry/Index.ets(4:3)\"}"})
    }
    #[test]
    fn synthetic_parser_preserves_native_fields_without_atomic_or_source_claims() {
        let mut parent = node(5);
        parent["$children"] = serde_json::json!([node(6)]);
        let (raw, frame) = example(root(serde_json::json!([parent])));
        let tree = parse_tree(&raw, &frame).unwrap();
        assert_eq!(tree.nodes.len(), 2);
        assert_eq!(tree.nodes[1].parent_id.as_deref(), Some("5"));
        assert_eq!(tree.nodes[0].children, vec!["6"]);
        assert_eq!(tree.nodes[1].depth, 1);
        assert_eq!(tree.nodes[0].bounds.as_ref().unwrap().x, -1.5);
        assert!(!tree.exact_frame_match);
        assert_eq!(tree.frame_sequence, frame.sequence);
        assert!(tree.nodes[0]
            .attributes
            .iter()
            .any(|v| v.name == "$debugLine"));
        assert_eq!(tree.nodes[0].paint_order, None);
        assert_eq!(tree.nodes[0].page, None);
    }
    #[test]
    fn synthetic_stale_duplicate_empty_and_invalid_bounds_fail_closed() {
        let (mut raw, frame) = example(root(serde_json::json!([node(5), node(5)])));
        assert_eq!(
            parse_tree(&raw, &frame).unwrap_err(),
            "INSPECTOR_DUPLICATE_NODE_ID"
        );
        raw.result = Value::String(root(serde_json::json!([])).to_string());
        assert_eq!(
            parse_tree(&raw, &frame).unwrap_err(),
            "INSPECTOR_EMPTY_TREE"
        );
        raw.task.session_id = "other".into();
        assert!(parse_tree(&raw, &frame).is_err());
        assert!(parse_bounds(Some(&serde_json::json!("NaN,0,10,10"))).is_err());
        assert!(parse_bounds(Some(&serde_json::json!("0,0,-10,10"))).is_err());
    }
    #[test]
    fn synthetic_clip_intersection_does_not_change_native_rect_or_invent_visibility() {
        let child = InspectorBounds {
            x: -5.0,
            y: 5.0,
            width: 20.0,
            height: 30.0,
        };
        let clip = InspectorBounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        assert_eq!(
            intersect_bounds(&child, &clip),
            Some(InspectorBounds {
                x: 0.0,
                y: 5.0,
                width: 10.0,
                height: 5.0
            })
        );
        assert_eq!(child.x, -5.0);
    }
}
