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
import './inspector.css';

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
  const current = deriveUiState(stage.snapshot, displayed).inputAllowed;
  const matched = treeMatchesRuntime(tree, frame) && samePixels(treeAnchor?.jpeg, frame?.jpeg);
  const surfaceWidth = frame?.width ?? 0;
  const surfaceHeight = frame?.height ?? 0;
  const index = useMemo(() => tree && matched ? createHitIndex(tree.nodes, surfaceWidth, surfaceHeight) : null, [tree, surfaceWidth, surfaceHeight, matched]);
  useEffect(() => {
    setSelection(null); setTree(null); setCandidates([]); selectionSerial.current++;
  }, [scope.targetId, scope.sessionId, stage.snapshot?.project?.id]);
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
      try {
        const value = await bridge.transfer!('inspector_get_tree', { frame: inspectionMetadata(requested) }, { targetId, sessionId });
        if (!live || !treeMatchesRuntime(value, frameRef.current) || !samePixels(requested.jpeg, frameRef.current?.jpeg)) return;
        setTree(value); setTreeAnchor(requested);
        setCandidates((currentCandidates) => currentCandidates.flatMap((candidate) => { const currentNode = value.nodes.find((item) => item.nodeId === candidate.nodeId); return currentNode ? [currentNode] : []; }));
        setRegionMode(false); setQueryProblem(null);
      } catch (error) {
        if (!live) return;
        setTree(null); setRegionMode(true); setQueryProblem(String(error));
      } finally { queryPending.current = false; if (live) timer = setTimeout(() => void query(), 750); }
    }
    timer = setTimeout(() => void query(), 0);
    return () => { live = false; clearTimeout(timer); };
  }, [stage.visible, interactionMode, frameIdentity, current, bridge, queryRevision, targetId, sessionId]);
  async function select(bounds: InspectorBounds, node: InspectorNode | null, hits: InspectorNode[]) {
    const actual = frameRef.current;
    if (!actual || !current || !bridge.transfer) return;
    const serial = ++selectionSerial.current;
    try {
      // Only node ID enters IPC. Native cache supplies properties and source evidence.
      const created = await bridge.transfer('selection_create', { draft: { frame: inspectionMetadata(actual), bounds, nodeId: node?.nodeId ?? null, inspectionVersion: node && tree ? tree.inspectionVersion : null, userInstruction: '' } }, scope);
      if (serial !== selectionSerial.current || stateRef.current?.sessionId !== created.sessionId || stateRef.current?.project?.id !== created.workspaceId) return;
      setSelection(created); setCandidates(hits);
    } catch (error) { if (serial === selectionSerial.current) onError?.(String(error)); }
  }
  const stale = Boolean(selection && (!current || selection.workspaceId !== stage.snapshot?.project?.id || selection.sessionId !== scope.sessionId || selection.buildId !== stage.snapshot?.build.buildId || selection.generation !== stage.snapshot?.build.generation || selection.expiresAt <= Date.now()));
  const panelSerial = selectionSerial.current;
  return <div className="inspector-preview">
    <PreviewStage {...stage} interactionMode={interactionMode} overlay={interactionMode === 'inspect' && frame && current
      ? <SelectionOverlay width={frame.width} height={frame.height} index={index} regionMode={manualRegion || regionMode || !matched} selected={selection?.componentBounds ?? null} select={(bounds, node, hits) => void select(bounds, node, hits)} /> : undefined} />
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
