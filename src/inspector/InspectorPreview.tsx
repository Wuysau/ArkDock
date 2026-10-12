import { useEffect, useImperativeHandle, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { MutableRefObject } from 'react';
import type { InspectorBounds, InspectorNode, InspectorTree, InteractionMode, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import type { BinaryPreviewFrame } from '../preview/frame-types.ts';
import { PreviewStage } from '../PreviewStage.tsx';
import type { PreviewStageProps } from '../PreviewStage.tsx';
import { deriveUiState } from '../view-state.ts';
import { SelectionOverlay } from './SelectionOverlay.tsx';
import { SelectionPanel } from './SelectionPanel.tsx';
import { createHitIndex, inspectionMetadata, samePixels, treeMatchesRuntime } from './selection-model.ts';
import { displayedIdentity, identityEqual } from '../preview/useNativeControls.ts';
import './inspector.css';

type InspectionResult = { tree: InspectorTree; frame: BinaryPreviewFrame };

export interface InspectorActions {
  selectRegion: () => void;
  refreshTree: () => void;
}
export function InspectorPreview({ bridge, interactionMode, exit, actions, onProblem, onError, ...stage }: PreviewStageProps & {
  bridge: UiBridge; interactionMode: InteractionMode; exit: () => void;
  actions?: MutableRefObject<InspectorActions | null>; onProblem?: (reason: string | null) => void; onError?: (reason: string) => void;
}) {
  const displayed = useSyncExternalStore(stage.store.subscribeDisplayed, stage.store.getDisplayed, stage.store.getDisplayed);
  const frame: BinaryPreviewFrame | null = displayed && 'jpeg' in displayed ? displayed : null;
  const frameRef = useRef(frame); frameRef.current = frame;
  const stateRef = useRef(stage.snapshot); stateRef.current = stage.snapshot;
  const selectionSerial = useRef(0);
  const queryPending = useRef(false);
  const pendingTreeRequest = useRef<Promise<InspectionResult> | null>(null);
  const inspectionCache = useRef<InspectionResult | null>(null);
  const selectionPending = useRef<number | null>(null);
  const activity = useRef({ mode: interactionMode, visible: stage.visible !== false, inputReady: false });
  const lastQueryAt = useRef(0);
  const [tree, setTree] = useState<InspectorTree | null>(null);
  const [treeAnchor, setTreeAnchor] = useState<BinaryPreviewFrame | null>(null);
  const observedPixels = useRef<BinaryPreviewFrame | null>(null);
  const [selection, setSelection] = useState<SelectionContext | null>(null);
  const selectionRef = useRef(selection); selectionRef.current = selection;
  const [candidates, setCandidates] = useState<InspectorNode[]>([]);
  const [regionMode, setRegionMode] = useState(false);
  const [manualRegion, setManualRegion] = useState(false);
  const [queryProblem, setQueryProblem] = useState<string | null>(null);
  const [queryRevision, setQueryRevision] = useState(0);
  const targetId = stage.snapshot?.target?.id ?? null;
  const sessionId = stage.snapshot?.sessionId ?? null;
  const scope = { targetId, sessionId };
  const configurationReady = !stage.controlState?.experimentalEnabled || stage.controlState.inputReady
    && identityEqual(stage.controlState.identity, displayedIdentity(stage.snapshot, displayed));
  const current = deriveUiState(stage.snapshot, displayed).inputAllowed && !stage.inputPaused && configurationReady;
  activity.current = { mode: interactionMode, visible: stage.visible !== false, inputReady: current };
  const matched = treeMatchesRuntime(tree, frame) && samePixels(treeAnchor?.jpeg, frame?.jpeg);
  const surfaceWidth = frame?.width ?? 0;
  const surfaceHeight = frame?.height ?? 0;
  const index = useMemo(() => tree && matched ? createHitIndex(tree.nodes, surfaceWidth, surfaceHeight) : null, [tree, surfaceWidth, surfaceHeight, matched]);
  async function requestTree(requested: BinaryPreviewFrame): Promise<InspectionResult> {
    if (!bridge.transfer) throw new Error('缺少桌面桥接。');
    queryPending.current = true;
    lastQueryAt.current = performance.now();
    let request: Promise<InspectionResult> | null = null;
    try {
      request = bridge.transfer('inspector_get_tree', { frame: inspectionMetadata(requested) }, { targetId, sessionId })
        .then((value) => ({ tree: value, frame: requested }));
      pendingTreeRequest.current = request;
      return await request;
    } finally {
      if (pendingTreeRequest.current === request) pendingTreeRequest.current = null;
      queryPending.current = false;
    }
  }
  useEffect(() => () => { selectionSerial.current++; }, []);
  useEffect(() => {
    setSelection(null); setTree(null); setTreeAnchor(null); setCandidates([]); inspectionCache.current = null; selectionSerial.current++;
  }, [scope.targetId, scope.sessionId, stage.snapshot?.project?.id]);
  useEffect(() => {
    if (interactionMode !== 'inspect') {
      selectionSerial.current++;
      setSelection(null); setCandidates([]);
    } else if (stage.visible === false && selectionPending.current !== null) {
      selectionSerial.current++;
    }
  }, [interactionMode, stage.visible]);
  useEffect(() => {
    if (interactionMode !== 'inspect') return;
    const esc = (event: KeyboardEvent) => { if (event.key === 'Escape' && !event.isComposing) { event.preventDefault(); exit(); } };
    window.addEventListener('keydown', esc);
    return () => window.removeEventListener('keydown', esc);
  }, [interactionMode, exit]);
  const frameIdentity = frame ? JSON.stringify([frame.workspaceId, frame.sessionId, frame.buildId, frame.generation]) : null;
  useEffect(() => { setManualRegion(false); setQueryProblem(null); }, [frameIdentity, interactionMode, targetId]);
  useEffect(() => { onProblem?.(queryProblem); }, [queryProblem, onProblem]);
  useImperativeHandle(actions, () => ({
    selectRegion: () => setManualRegion(true),
    refreshTree: () => { setManualRegion(false); setQueryRevision(value => value + 1); },
  }), []);
  useEffect(() => {
    if (interactionMode !== 'inspect') { observedPixels.current = frame; return; }
    if (frame && observedPixels.current && !samePixels(observedPixels.current.jpeg, frame.jpeg)) {
      inspectionCache.current = null;
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
      if (queryPending.current || selectionPending.current !== null || delay > 0) { timer = setTimeout(() => void query(), Math.max(delay, 100)); return; }
      try {
        const result = await requestTree(requested);
        const value = result.tree;
        if (!live || !treeMatchesRuntime(value, frameRef.current) || !samePixels(requested.jpeg, frameRef.current?.jpeg)) return;
        inspectionCache.current = result;
        setTree(value); setTreeAnchor(requested);
        setCandidates((currentCandidates) => currentCandidates.flatMap((candidate) => { const currentNode = value.nodes.find((item) => item.nodeId === candidate.nodeId); return currentNode ? [currentNode] : []; }));
        setRegionMode(false); setQueryProblem(null);
      } catch (error) {
        if (!live) return;
        inspectionCache.current = null;
        setTree(null); setTreeAnchor(null); setRegionMode(true); setQueryProblem(String(error));
      } finally {
        if (live) timer = setTimeout(() => void query(), 750);
      }
    }
    timer = setTimeout(() => void query(), 0);
    return () => { live = false; clearTimeout(timer); };
  }, [stage.visible, interactionMode, frameIdentity, current, bridge, queryRevision, targetId, sessionId]);
  async function select(bounds: InspectorBounds, node: InspectorNode | null, hits: InspectorNode[]) {
    const actual = frameRef.current;
    if (!actual || !current || !bridge.transfer || activity.current.mode !== 'inspect' || !activity.current.visible) { onError?.('当前画面不可选择，请等待有效原生画面后重选。'); return; }
    if (selectionPending.current !== null) { onError?.('正在保存选择，请稍候。'); return; }
    const serial = ++selectionSerial.current;
    selectionPending.current = serial;
    const isActive = () => serial === selectionSerial.current && activity.current.mode === 'inspect' && activity.current.visible;
    const isCurrent = () => {
      const state = stateRef.current;
      const displayedFrame = frameRef.current;
      return Boolean(state && displayedFrame && isActive()
        && state.target?.id === targetId && state.project?.id === actual.workspaceId
        && state.sessionId === actual.sessionId && state.build.buildId === actual.buildId && state.build.generation === actual.generation
        && deriveUiState(state, actual).inputAllowed && activity.current.inputReady
        && displayedFrame.workspaceId === actual.workspaceId && displayedFrame.sessionId === actual.sessionId
        && displayedFrame.buildId === actual.buildId && displayedFrame.generation === actual.generation
        && displayedFrame.width === actual.width && displayedFrame.height === actual.height && displayedFrame.rotation === actual.rotation
        && displayedFrame.sequence >= actual.sequence && samePixels(actual.jpeg, displayedFrame.jpeg));
    };
    try {
      // An in-flight poll may replace the native tree before selection_create.
      // Pause new polls and use the version that actually finished, not a stale render closure.
      if (node && pendingTreeRequest.current) {
        try {
          const result = await pendingTreeRequest.current;
          if (isCurrent()) inspectionCache.current = treeMatchesRuntime(result.tree, frameRef.current)
            && samePixels(result.frame.jpeg, actual.jpeg) ? result : null;
        } catch { if (isCurrent()) inspectionCache.current = null; }
      }
      if (!isCurrent()) return;
      let selectedNode = node;
      let selectedTree: InspectorTree | null = null;
      if (node) {
        let cached = inspectionCache.current;
        if (!cached || !treeMatchesRuntime(cached.tree, frameRef.current) || !samePixels(cached.frame.jpeg, actual.jpeg)
          || Date.now() - cached.tree.receivedAt > 1500) {
          const delay = Math.max(0, 350 - (performance.now() - lastQueryAt.current));
          if (delay) await new Promise<void>((resolve) => setTimeout(resolve, delay));
          const requested = frameRef.current;
          if (!requested || !isCurrent()) return;
          const result = await requestTree(requested);
          if (!isCurrent()) return;
          if (!treeMatchesRuntime(result.tree, frameRef.current)) throw new Error('组件树与当前画面不一致，请刷新后重选。');
          cached = result;
          inspectionCache.current = cached;
          setTree(result.tree); setTreeAnchor(result.frame);
        }
        if (!cached) throw new Error('组件树不可用，请刷新后重选。');
        selectedTree = cached.tree;
        selectedNode = selectedTree.nodes.find((item) => item.nodeId === node.nodeId) ?? null;
        if (!selectedNode?.bounds || selectedNode.componentType !== node.componentType
          || !node.bounds || selectedNode.bounds.x !== node.bounds.x || selectedNode.bounds.y !== node.bounds.y
          || selectedNode.bounds.width !== node.bounds.width || selectedNode.bounds.height !== node.bounds.height) {
          onError?.('组件已变化，请在刷新后的轮廓上重新选择。'); return;
        }
      }
      // Native retention is bounded. Advance metadata only while the displayed
      // JPEG is byte-for-byte identical to the pixels on which the user clicked.
      const selectedFrame = frameRef.current;
      if (!selectedFrame || !isCurrent()) return;
      // Only node ID enters IPC. Native cache supplies properties and source evidence.
      const created = await bridge.transfer('selection_create', { draft: { frame: inspectionMetadata(selectedFrame), bounds: selectedNode?.bounds ?? bounds, nodeId: selectedNode?.nodeId ?? null, inspectionVersion: selectedTree?.inspectionVersion ?? null, userInstruction: '' } }, scope);
      if (!isCurrent()) return;
      if (created.sessionId !== actual.sessionId || created.workspaceId !== actual.workspaceId
        || created.buildId !== actual.buildId || created.generation !== actual.generation
        || created.frameSequence !== selectedFrame.sequence) throw new Error('返回的选择与当前工程或画面不一致，请重新选择。');
      setSelection(created);
      const confirmedTree = selectedTree;
      setCandidates(confirmedTree ? hits.flatMap((hit) => { const updated = confirmedTree.nodes.find((item) => item.nodeId === hit.nodeId); return updated ? [updated] : []; }) : hits);
    } catch (error) { if (isCurrent()) onError?.(`选择未保存：${String(error)}`); }
    finally {
      if (selectionPending.current === serial) selectionPending.current = null;
      if (isActive() && !isCurrent()) onError?.('画面或构建已变化，本次选择未显示，请重新选择。');
    }
  }
  const stale = Boolean(selection && (!current || selection.workspaceId !== stage.snapshot?.project?.id || selection.sessionId !== scope.sessionId || selection.buildId !== stage.snapshot?.build.buildId || selection.generation !== stage.snapshot?.build.generation || selection.expiresAt <= Date.now()));
  const panelSerial = selectionSerial.current;
  return <div className="inspector-preview">
    <PreviewStage {...stage} interactionMode={interactionMode} overlay={interactionMode === 'inspect' && frame && current
      ? <SelectionOverlay width={frame.width} height={frame.height} index={index} regionMode={manualRegion || regionMode || !matched} selected={selection?.componentBounds ?? null} select={(bounds, node, hits) => void select(bounds, node, hits)} reject={onError} /> : undefined} />
    {interactionMode === 'inspect' && selection && <SelectionPanel key={selection.selectionId} selection={selection} stale={stale} candidates={candidates} scope={scope} bridge={bridge} updated={(saved) => {
      const currentSelection = selectionRef.current;
      const currentState = stateRef.current;
      if (panelSerial === selectionSerial.current && currentSelection?.selectionId === saved.selectionId && currentState?.project?.id === saved.workspaceId && currentState.sessionId === saved.sessionId
        && currentState.build.buildId === saved.buildId && currentState.build.generation === saved.generation) setSelection(saved);
    }} close={() => { selectionSerial.current++; setSelection(null); }} choose={(candidate) => {
      const confirmedNode = matched ? tree?.nodes.find((item) => item.nodeId === candidate.nodeId) : null;
      if (confirmedNode?.bounds) void select(confirmedNode.bounds, confirmedNode, candidates);
      else onError?.('组件树已过期，请刷新组件树或重新选择。');
    }} />}
  </div>;
}
