import { useCallback, useRef, useState } from 'react';
import type { MutableRefObject } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import { automaticPanel, emptyAutomaticPanels } from './navigation.ts';
import type { AutomaticPanels, Panel } from './navigation.ts';

export interface Navigation {
  panel: Panel | null;
  /** Element to restore focus to when the sheet closes. */
  anchor: MutableRefObject<HTMLElement | null>;
  /** Bumped on every user navigation so late command replies cannot navigate. */
  revision: MutableRefObject<number>;
  /** User navigation: bumps the revision, captures focus, closes nothing else. */
  navigate: (next: Panel | null) => void;
  /** Snapshot-driven panel change: never counts as user navigation. */
  open: (next: Panel | null) => void;
  /** Consumes the automatic popup for this snapshot; `cancelled` suppresses it entirely. */
  openAutomatic: (snapshot: AppSnapshot, cancelled: boolean) => void;
  /** A new target lifecycle may auto-open its panel again. */
  resetAutomatic: () => void;
}

export function useNavigation(): Navigation {
  const [panel, setPanel] = useState<Panel | null>(null);
  const anchor = useRef<HTMLElement | null>(null);
  const revision = useRef(0);
  const automatic = useRef<AutomaticPanels>(emptyAutomaticPanels());
  // Read during render so a navigation in the same tick still sees the open panel.
  const current = useRef<Panel | null>(null);
  current.current = panel;
  const open = useCallback((next: Panel | null) => { setPanel(next); }, []);
  const navigate = useCallback((next: Panel | null) => {
    revision.current += 1;
    if (current.current === null && next) anchor.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setPanel(next);
  }, []);
  const openAutomatic = useCallback((snapshot: AppSnapshot, cancelled: boolean) => {
    if (cancelled) return;
    const planned = automaticPanel(snapshot, automatic.current);
    automatic.current = planned.seen;
    if (planned.panel) setPanel(planned.panel);
  }, []);
  const resetAutomatic = useCallback(() => { automatic.current = emptyAutomaticPanels(); }, []);
  return { panel, anchor, revision, navigate, open, openAutomatic, resetAutomatic };
}
