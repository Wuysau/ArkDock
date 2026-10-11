import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { AgentSessionList, InspectorBounds, InspectorNode, InspectorTree, InteractionMode, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import type { BinaryPreviewFrame } from '../preview/frame-types.ts';
import { PreviewStage } from '../PreviewStage.tsx';
import type { PreviewStageProps } from '../PreviewStage.tsx';
import { deriveUiState } from '../view-state.ts';
import { SelectionOverlay } from './SelectionOverlay.tsx';
import { SelectionPanel } from './SelectionPanel.tsx';
import { createHitIndex, inspectionMetadata, samePixels, treeMatchesRuntime } from './selection-model.ts';
import './inspector.css';

export function InspectorPreview({ bridge, interactionMode, exit, ...stage }: PreviewStageProps & { bridge: UiBridge; interactionMode: InteractionMode; exit: () => void }) {
  const displayed = useSyncExternalStore(stage.store.subscribeDisplayed, stage.store.getDisplayed, stage.store.getDisplayed);
  const frame: BinaryPreviewFrame | null = displayed && 'jpeg' in displayed ? displayed : null;
  const frameRef = useRef(frame); frameRef.current = frame;
  const stateRef = useRef(stage.snapshot); stateRef.current = stage.snapshot;
  const selectionSerial = useRef(0);
  const queryPending = useRef(false);
  const lastQueryAt = useRef(0);
  const [tree, setTree] = useState<InspectorTree | null>(null);
  const [treeAnchor, setTreeAnchor] = useState<BinaryPreviewFrame | null>(null);
  const observedPixels = useRef<BinaryPreviewFrame | null>(null);
  const [selection, setSelection] = useState<SelectionContext | null>(null);
  const selectionRef = useRef(selection); selectionRef.current = selection;
  const [candidates, setCandidates] = useState<InspectorNode[]>([]);
  const [sessions, setSessions] = useState<AgentSessionList | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [manualRegion, setManualRegion] = useState(false);
  const [status, setStatus] = useState('');
  const [queryRevision, setQueryRevision] = useState(0);
  const targetId = stage.snapshot?.target?.id ?? null;
  const sessionId = stage.snapshot?.sessionId ?? null;
  const scope = { targetId, sessionId };
  const current = deriveUiState(stage.snapshot, displayed).inputAllowed;
  const matched = treeMatchesRuntime(tree, frame) && samePixels(treeAnchor?.jpeg, frame?.jpeg);
  const surfaceWidth = frame?.width ?? 0;
  const surfaceHeight = frame?.height ?? 0;
  const index = useMemo(() => tree && matched ? createHitIndex(tree.nodes, surfaceWidth, surfaceHeight) : null, [tree, surfaceWidth, surfaceHeight, matched]);
  useEffect(() => {
    setSelection(null); setTree(null); setSessions(null); setCandidates([]); selectionSerial.current++;
  }, [scope.targetId, scope.sessionId, stage.snapshot?.project?.id]);
  useEffect(() => {
    if (interactionMode !== 'inspect') return;
    const esc = (event: KeyboardEvent) => { if (event.key === 'Escape' && !event.isComposing) { event.preventDefault(); exit(); } };
    window.addEventListener('keydown', esc);
    return () => window.removeEventListener('keydown', esc);
  }, [interactionMode, exit]);
  const frameIdentity = frame ? JSON.stringify([frame.workspaceId, frame.sessionId, frame.buildId, frame.generation]) : null;
  useEffect(() => { setManualRegion(false); }, [frameIdentity, interactionMode]);
  useEffect(() => {
    if (interactionMode !== 'inspect') { observedPixels.current = frame; return; }
    if (frame && observedPixels.current && !samePixels(observedPixels.current.jpeg, frame.jpeg)) {
      setTree(null); setTreeAnchor(null); setCandidates([]); setQueryRevision((value) => value + 1);
    }
    observedPixels.current = frame;
  }, [frame, interactionMode]);
  useEffect(() => {
    if (stage.visible === false || interactionMode !== 'inspect' || !frameIdentity || !current || !bridge.transfer) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    async function query() {
      const requested = frameRef.current;
      if (!live || !requested) return;
      const delay = Math.max(0, 350 - (performance.now() - lastQueryAt.current));
      if (queryPending.current || delay > 0) { timer = setTimeout(() => void query(), Math.max(delay, 100)); return; }
      queryPending.current = true;
      lastQueryAt.current = performance.now();
      setStatus('正在读取原生组件树…');
      try {
        const value = await bridge.transfer!('inspector_get_tree', { frame: inspectionMetadata(requested) }, { targetId, sessionId });
        if (!live || !treeMatchesRuntime(value, frameRef.current) || !samePixels(requested.jpeg, frameRef.current?.jpeg)) return;
        setTree(value); setTreeAnchor(requested);
        setCandidates((currentCandidates) => currentCandidates.flatMap((candidate) => { const currentNode = value.nodes.find((item) => item.nodeId === candidate.nodeId); return currentNode ? [currentNode] : []; }));
        setRegionMode(false); setStatus('元素选择 · 原生组件树与截图非原子采样');
      } catch (error) {
        if (!live) return;
        setTree(null); setRegionMode(true); setStatus(`区域选择模式 · ${String(error)}`);
      } finally { queryPending.current = false; if (live) timer = setTimeout(() => void query(), 750); }
    }
    timer = setTimeout(() => void query(), 0);
    return () => { live = false; clearTimeout(timer); };
  }, [stage.visible, interactionMode, frameIdentity, current, bridge, queryRevision, targetId, sessionId]);
  useEffect(() => {
    if (interactionMode !== 'inspect' || !sessionId || !bridge.transfer) return;
    let live = true;
    void bridge.transfer('agent_session_list', {}, { targetId, sessionId }).then((value) => { if (live) setSessions(value); }).catch((error) => { if (live) setStatus(String(error)); });
    return () => { live = false; };
  }, [interactionMode, sessionId, targetId, bridge]);
  async function select(bounds: InspectorBounds, node: InspectorNode | null, hits: InspectorNode[]) {
    const actual = frameRef.current;
    if (!actual || !current || !bridge.transfer) return;
    const serial = ++selectionSerial.current;
    setStatus('保存实际显示帧与选择信息…');
    try {
      // Only node ID enters IPC. Native cache supplies properties and source evidence.
      const created = await bridge.transfer('selection_create', { draft: { frame: inspectionMetadata(actual), bounds, nodeId: node?.nodeId ?? null, inspectionVersion: node && tree ? tree.inspectionVersion : null, userInstruction: '' } }, scope);
      if (serial !== selectionSerial.current || stateRef.current?.sessionId !== created.sessionId || stateRef.current?.project?.id !== created.workspaceId) return;
      setSelection(created); setCandidates(hits); setStatus(created.selectionKind === 'component' ? '已选择实际 ArkUI 组件' : '已保存截图区域');
    } catch (error) { if (serial === selectionSerial.current) setStatus(String(error)); }
  }
  const stale = Boolean(selection && (!current || selection.workspaceId !== stage.snapshot?.project?.id || selection.sessionId !== scope.sessionId || selection.buildId !== stage.snapshot?.build.buildId || selection.generation !== stage.snapshot?.build.generation || selection.expiresAt <= Date.now()));
  const panelSerial = selectionSerial.current;
  return <div className="inspector-preview">
    <PreviewStage {...stage} interactionMode={interactionMode} overlay={interactionMode === 'inspect' && frame && current
      ? <SelectionOverlay width={frame.width} height={frame.height} index={index} regionMode={manualRegion || regionMode || !matched} selected={selection?.componentBounds ?? null} select={(bounds, node, hits) => void select(bounds, node, hits)} /> : undefined} />
    {interactionMode === 'inspect' && <output className="inspection-status"><span>{!frame ? '等待二进制原生画面' : !current ? '当前旧画面不可选择' : manualRegion ? '区域选择模式 · 拖动框选' : status || '正在获取组件树'}</span><button onClick={() => { setManualRegion(true); setStatus('区域选择模式 · 拖动框选'); }}>框选</button><button onClick={() => { setManualRegion(false); setQueryRevision((value) => value + 1); }}>刷新树</button></output>}
    {interactionMode === 'inspect' && selection && <SelectionPanel key={selection.selectionId} selection={selection} stale={stale} candidates={candidates} sessions={sessions} scope={scope} bridge={bridge} updated={(saved) => {
      const currentSelection = selectionRef.current;
      const currentState = stateRef.current;
      if (panelSerial === selectionSerial.current && currentSelection?.selectionId === saved.selectionId && currentState?.project?.id === saved.workspaceId && currentState.sessionId === saved.sessionId
        && currentState.build.buildId === saved.buildId && currentState.build.generation === saved.generation) setSelection(saved);
    }} close={() => { selectionSerial.current++; setSelection(null); }} choose={(candidate) => {
      const confirmedNode = matched ? tree?.nodes.find((item) => item.nodeId === candidate.nodeId) : null;
      if (confirmedNode?.bounds) void select(confirmedNode.bounds, confirmedNode, candidates);
      else setStatus('组件树已过期，请重新选择。');
    }} />}
  </div>;
}
