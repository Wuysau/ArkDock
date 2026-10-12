import { memo, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { AppSnapshot, Diagnostic } from './ipc/model.ts';
import { diagnosticText } from './diagnostics.ts';
import { dockingStatus } from './request-state.ts';
import { Icon } from './Icon.tsx';
import './runtime-logs.css';

type LogOrder = 'latest' | 'earliest';
type LogRow = { key: string; diagnostic: Diagnostic; originalIndex: number };
type ScrollAnchor = { key: string; offset: number };
const latestThreshold = 24;
const timestampFormatter = new Intl.DateTimeFormat('zh-CN', {
  year: 'numeric', month: '2-digit', day: '2-digit',
  hour: '2-digit', minute: '2-digit', second: '2-digit',
  fractionalSecondDigits: 3, hourCycle: 'h23',
});

function sameDiagnostic(left: Diagnostic, right: Diagnostic): boolean {
  return left === right || (left.observedAt === right.observedAt && left.status === right.status
    && left.code === right.code && left.message === right.message
    && left.workspaceId === right.workspaceId && left.sessionId === right.sessionId
    && left.buildId === right.buildId && left.backendId === right.backendId);
}

function sameRecords(left: ReadonlyArray<Diagnostic>, right: ReadonlyArray<Diagnostic>): boolean {
  return left === right || (left.length === right.length
    && left.every((record, index) => sameDiagnostic(record, right[index])));
}

function timestamp(value: number): { label: string; iso: string | undefined } {
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    ? { label: timestampFormatter.format(date), iso: date.toISOString() }
    : { label: '时间未提供', iso: undefined };
}

function sortedRows(diagnostics: ReadonlyArray<Diagnostic>, order: LogOrder): LogRow[] {
  const occurrences = new Map<string, number>();
  // Record identity uses the actual fields rather than its display index. Prepending
  // a different record therefore keeps existing selectable DOM nodes mounted.
  const rows = diagnostics.map((diagnostic, originalIndex) => {
    const identity = JSON.stringify([
      diagnostic.observedAt, diagnostic.status, diagnostic.code, diagnostic.message,
      diagnostic.workspaceId, diagnostic.sessionId, diagnostic.buildId, diagnostic.backendId,
    ]);
    const occurrence = occurrences.get(identity) ?? 0;
    occurrences.set(identity, occurrence + 1);
    return { key: `${identity}:${occurrence}`, diagnostic, originalIndex };
  });
  return rows.sort((left, right) => {
    const leftTime = new Date(left.diagnostic.observedAt).getTime();
    const rightTime = new Date(right.diagnostic.observedAt).getTime();
    if (!Number.isFinite(leftTime)) return Number.isFinite(rightTime) ? 1 : left.originalIndex - right.originalIndex;
    if (!Number.isFinite(rightTime)) return -1;
    const difference = order === 'latest' ? rightTime - leftTime : leftTime - rightTime;
    return difference || left.originalIndex - right.originalIndex;
  });
}

const LogRecord = memo(function LogRecord({ diagnostic, rowKey }: { diagnostic: Diagnostic; rowKey: string }) {
  const observed = timestamp(diagnostic.observedAt);
  return <article className="runtime-log-record" data-log-key={rowKey}>
    <div className="runtime-log-heading">
      <span className={`log-status ${diagnostic.status.toLowerCase()}`}>{diagnostic.status}</span>
      <strong className="runtime-log-code">{diagnostic.code}</strong>
      <time dateTime={observed.iso} title={observed.iso}>{observed.label}</time>
    </div>
    <div className="runtime-log-source">来源：{diagnostic.backendId ?? '未提供'}</div>
    <p className="runtime-log-message">{diagnostic.message}</p>
    {(diagnostic.workspaceId || diagnostic.sessionId || diagnostic.buildId) && <dl className="runtime-log-scope">
      {diagnostic.workspaceId && <div><dt>工程</dt><dd>{diagnostic.workspaceId}</dd></div>}
      {diagnostic.sessionId && <div><dt>会话</dt><dd>{diagnostic.sessionId}</dd></div>}
      {diagnostic.buildId && <div><dt>构建</dt><dd>{diagnostic.buildId}</dd></div>}
    </dl>}
  </article>;
}, (previous, next) => previous.rowKey === next.rowKey && sameDiagnostic(previous.diagnostic, next.diagnostic));

const LogViewport = memo(function LogViewport({ diagnostics, order }: { diagnostics: ReadonlyArray<Diagnostic>; order: LogOrder }) {
  const rows = useMemo(() => sortedRows(diagnostics, order), [diagnostics, order]);
  const viewport = useRef<HTMLDivElement>(null);
  const readingLatest = useRef(true);
  const anchors = useRef<ScrollAnchor[]>([]);
  const previousRows = useRef<LogRow[]>([]);
  const previousOrder = useRef(order);
  const previousScrollTop = useRef(0);
  const [unread, setUnread] = useState(0);

  function rememberPosition() {
    const surface = viewport.current;
    if (!surface) return;
    const latest = order === 'latest' ? surface.scrollTop <= latestThreshold
      : surface.scrollHeight - surface.clientHeight - surface.scrollTop <= latestThreshold;
    readingLatest.current = latest;
    previousScrollTop.current = surface.scrollTop;
    if (latest) {
      anchors.current = [];
      setUnread((count) => count === 0 ? count : 0);
      return;
    }
    const viewportTop = surface.getBoundingClientRect().top;
    const visible: ScrollAnchor[] = [];
    for (const record of surface.querySelectorAll<HTMLElement>('[data-log-key]')) {
      const rect = record.getBoundingClientRect();
      if (rect.bottom <= viewportTop) continue;
      const key = record.dataset.logKey;
      if (key) visible.push({ key, offset: rect.top - viewportTop });
      // Keep alternatives if the backend trims the first visible record.
      if (visible.length === 6) break;
    }
    anchors.current = visible;
  }

  function returnToLatest() {
    const surface = viewport.current;
    if (!surface) return;
    readingLatest.current = true;
    surface.scrollTop = order === 'latest' ? 0 : surface.scrollHeight;
    setUnread(0);
    rememberPosition();
  }

  useLayoutEffect(() => {
    const surface = viewport.current;
    if (!surface) return;
    const orderChanged = previousOrder.current !== order;
    const previous = new Set(previousRows.current.map((row) => row.key));
    const added = rows.reduce((count, row) => count + (previous.has(row.key) ? 0 : 1), 0);
    if (readingLatest.current || orderChanged) {
      readingLatest.current = true;
      surface.scrollTop = order === 'latest' ? 0 : surface.scrollHeight;
      setUnread(0);
    } else {
      const elements = new Map<string, HTMLElement>();
      for (const element of surface.querySelectorAll<HTMLElement>('[data-log-key]')) {
        if (element.dataset.logKey) elements.set(element.dataset.logKey, element);
      }
      const anchor = anchors.current.find((candidate) => elements.has(candidate.key));
      const record = anchor ? elements.get(anchor.key) : undefined;
      if (anchor && record) {
        surface.scrollTop += record.getBoundingClientRect().top - surface.getBoundingClientRect().top - anchor.offset;
      } else {
        // All remembered records may have expired from the backend's bounded history.
        // Keep the user's position as far as the remaining content permits.
        surface.scrollTop = previousScrollTop.current;
      }
      if (added > 0) setUnread((count) => count + added);
    }
    previousRows.current = rows;
    previousOrder.current = order;
    rememberPosition();
  }, [rows, order]);

  return <div className="runtime-log-history" onWheel={(event) => event.stopPropagation()}>
    <div className="runtime-log-updates">
      <span>{unread > 0 ? `有 ${unread} 条新日志` : order === 'latest' ? '最新日志在顶部' : '最新日志在底部'}</span>
      <button type="button" onClick={returnToLatest} aria-label="返回最新运行日志">返回最新</button>
    </div>
    <div ref={viewport} className="runtime-log-viewport" role="region" aria-label="运行日志记录，可滚动浏览" tabIndex={0} onScroll={rememberPosition}>
      {rows.length > 0 ? rows.map((row) => <LogRecord key={row.key} diagnostic={row.diagnostic} rowKey={row.key} />)
        : <div className="no-results"><Icon name="log" size={30} /><p>暂无诊断记录，编译错误总数未知。</p></div>}
    </div>
  </div>;
}, (previous, next) => previous.order === next.order && sameRecords(previous.diagnostics, next.diagnostics));

const emptyDiagnostics: ReadonlyArray<Diagnostic> = [];

export function RuntimeLogs({ snapshot }: { snapshot: AppSnapshot | null }) {
  const [order, setOrder] = useState<LogOrder>('latest');
  const [copyStatus, setCopyStatus] = useState('');
  const [copying, setCopying] = useState(false);
  const diagnostics = snapshot?.diagnostics ?? emptyDiagnostics;
  const docking = dockingStatus(snapshot?.dockHint);

  async function copyDiagnostics() {
    if (copying) return;
    setCopying(true);
    try {
      await navigator.clipboard.writeText(diagnosticText(snapshot));
      setCopyStatus('脱敏诊断已复制');
    } catch {
      setCopyStatus('复制失败，请确认系统剪贴板权限后重试。');
    } finally {
      setCopying(false);
    }
  }

  return <section className="runtime-logs" aria-label="运行日志">
    <div className="runtime-log-actions">
      <span className="muted">最近 {diagnostics.length} 条记录</span>
      <label className="runtime-log-order">排序
        <select aria-label="运行日志排序" value={order} onChange={(event) => setOrder(event.target.value as LogOrder)}>
          <option value="latest">最新优先</option>
          <option value="earliest">最早优先</option>
        </select>
      </label>
      <button type="button" disabled={copying} onClick={() => void copyDiagnostics()}>{copying ? '复制中…' : '复制脱敏诊断'}</button>
    </div>
    <div className="runtime-log-context">
      <p>{snapshot?.backend.reason ?? '桌面后端未连接。'}</p>
      {docking && <p>贴附诊断：{docking.detail}</p>}
      {snapshot?.build.error && <p className="build-error">{snapshot.build.error}</p>}
    </div>
    {copyStatus && <p className="runtime-log-copy-status" role="status">{copyStatus}</p>}
    <LogViewport diagnostics={diagnostics} order={order} />
  </section>;
}
