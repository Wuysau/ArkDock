import { useCallback, useEffect, useRef, useState } from 'react';
import type { AppSnapshot, MotionIntensity } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { applyMotion, createMotionPreferenceSync, motionSettings, readMotion, readMotionIntensity } from '../motion.ts';
import type { MotionPreference } from '../motion.ts';
import { createOrbAutoHidePreferenceSync, createOrbOpacityPreferenceSync, nativeOrbPreferenceChanged, readOrbAutoHide, readOrbOpacity, saveOrbAutoHide, saveOrbOpacity, settledOrbChoice } from '../orb-preferences.ts';
import { targetLifecycleKey } from '../dock-ripple.ts';
import type { DesktopSession } from './useDesktopSession.ts';
import type { Notices } from './notices.ts';

export interface Preferences {
  motion: MotionPreference;
  setMotion: (value: MotionPreference) => void;
  intensity: MotionIntensity;
  setIntensity: (value: MotionIntensity) => void;
  /** Effective decoration settings after the system preference is applied. */
  effectiveMotion: ReturnType<typeof motionSettings>;
  chooseOrbAutoHide: (enabled: boolean) => void;
  /** Adopts a native context-menu change once queued UI writes have settled. */
  adoptNativeOrbChange: (previous: AppSnapshot | null, updated: AppSnapshot) => boolean;
  /** Effective Orb opacity in percent, 20..=100; 100 keeps the default artwork. */
  orbOpacity: number;
  chooseOrbOpacity: (opacity: number) => void;
}

/**
 * Native shell preferences: animation settings, orb auto-hide and orb opacity.
 * Every write is serialized per target lifecycle, raced replies are dropped, and
 * a failure never loops a request — the same guarantees the inline effects had.
 */
export function usePreferences({ bridge, session, notices }: {
  bridge: UiBridge; session: DesktopSession; notices: Notices;
}): Preferences {
  const { stateRef, receive, connected, nativeReady, nativeReadyRef, initialSnapshotApplied } = session;
  // Destructured so the serialized writes below are not re-created on every render.
  const report = notices.report;
  const [motion, setMotion] = useState<MotionPreference>(readMotion);
  const [intensity, setIntensity] = useState(readMotionIntensity);
  const [systemReduced, setSystemReduced] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  const [orbAutoHide, setOrbAutoHide] = useState(readOrbAutoHide);
  const [orbOpacity, setOrbOpacity] = useState(readOrbOpacity);
  const orbChoice = useRef(orbAutoHide);
  const orbOpacityChoice = useRef(orbOpacity);
  const motionSync = useRef<ReturnType<typeof createMotionPreferenceSync<AppSnapshot>> | null>(null);
  const orbSync = useRef<ReturnType<typeof createOrbAutoHidePreferenceSync<AppSnapshot>> | null>(null);
  const orbOpacitySync = useRef<ReturnType<typeof createOrbOpacityPreferenceSync<AppSnapshot>> | null>(null);
  const motionTargetKey = targetLifecycleKey(session.state);
  const effectiveMotion = motionSettings(motion, intensity, systemReduced);

  useEffect(() => { applyMotion(motion, intensity, systemReduced); }, [motion, intensity, systemReduced]);
  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const changed = () => setSystemReduced(media.matches);
    media.addEventListener('change', changed);
    return () => media.removeEventListener('change', changed);
  }, []);
  useEffect(() => {
    const sync = createMotionPreferenceSync<AppSnapshot>({
      send: (job) => bridge.request('set_motion_preferences', { preferences: job.preferences }, job.scope),
      accept: (job, result) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) receive(result);
      },
      fail: (job, reason) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) report(reason);
      },
    });
    motionSync.current = sync;
    return () => { sync.dispose(); if (motionSync.current === sync) motionSync.current = null; };
  }, [bridge, receive, stateRef, report]);
  useEffect(() => {
    if (!connected || !nativeReady) return;
    motionSync.current?.enqueue({
      scope: { targetId: stateRef.current?.target?.id ?? null, sessionId: stateRef.current?.sessionId ?? null },
      targetKey: motionTargetKey, preferences: motionSettings(motion, intensity, systemReduced),
    });
  }, [connected, nativeReady, motionTargetKey, session.state?.sessionId, motion, intensity, systemReduced, stateRef]);
  useEffect(() => {
    const sync = createOrbAutoHidePreferenceSync<AppSnapshot>({
      send: (job) => bridge.request('set_orb_auto_hide', { enabled: job.enabled }, job.scope),
      accept: (job, result) => {
        if (job.targetKey !== targetLifecycleKey(stateRef.current) || job.scope.sessionId !== (stateRef.current?.sessionId ?? null)) return;
        receive(result);
        const confirmed = settledOrbChoice(job, result, stateRef.current);
        if (confirmed !== null && orbChoice.current !== confirmed) {
          // receive may already have accepted a newer native event while this invoke was pending.
          orbChoice.current = confirmed;
          setOrbAutoHide(confirmed);
        }
      },
      fail: (job, reason) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) report(reason);
      },
    });
    orbSync.current = sync;
    return () => { sync.dispose(); if (orbSync.current === sync) orbSync.current = null; };
  }, [bridge, receive, stateRef, report]);
  useEffect(() => {
    const sync = createOrbOpacityPreferenceSync<AppSnapshot>({
      send: (job) => bridge.request('set_orb_opacity', { opacity: job.opacity }, job.scope),
      accept: (job, result) => {
        if (job.targetKey !== targetLifecycleKey(stateRef.current) || job.scope.sessionId !== (stateRef.current?.sessionId ?? null)) return;
        receive(result);
        // The confirmed native value is authoritative; adopt it when it differs.
        const confirmed = result.display.orbOpacity;
        if (confirmed !== orbOpacityChoice.current) {
          orbOpacityChoice.current = confirmed;
          setOrbOpacity(confirmed);
        }
      },
      fail: (job, reason) => {
        if (job.targetKey !== targetLifecycleKey(stateRef.current) || job.scope.sessionId !== (stateRef.current?.sessionId ?? null)) return;
        report(reason);
        // A rejected write never happened natively: fall back to the committed value.
        const confirmed = stateRef.current?.display.orbOpacity;
        if (orbOpacityChoice.current === job.opacity && confirmed !== undefined && confirmed !== job.opacity) {
          orbOpacityChoice.current = confirmed;
          setOrbOpacity(confirmed);
        }
      },
    });
    orbOpacitySync.current = sync;
    return () => { sync.dispose(); if (orbOpacitySync.current === sync) orbOpacitySync.current = null; };
  }, [bridge, receive, stateRef, report]);
  useEffect(() => { if (orbAutoHide !== null) saveOrbAutoHide(orbAutoHide); }, [orbAutoHide]);
  useEffect(() => { if (orbOpacity !== null) saveOrbOpacity(orbOpacity); }, [orbOpacity]);
  useEffect(() => {
    if (!connected || !nativeReady || !stateRef.current) return;
    const enabled = orbAutoHide ?? stateRef.current.display.orbAutoHide;
    orbChoice.current = enabled;
    if (orbAutoHide === null) setOrbAutoHide(enabled);
    orbSync.current?.enqueue({
      scope: { targetId: stateRef.current.target?.id ?? null, sessionId: stateRef.current.sessionId },
      targetKey: motionTargetKey, enabled,
    });
    initialSnapshotApplied.current = false;
  }, [connected, nativeReady, motionTargetKey, session.state?.sessionId, orbAutoHide, stateRef, initialSnapshotApplied]);
  useEffect(() => {
    if (!connected || !nativeReady || !stateRef.current) return;
    const opacity = orbOpacity ?? stateRef.current.display.orbOpacity;
    orbOpacityChoice.current = opacity;
    if (orbOpacity === null) setOrbOpacity(opacity);
    orbOpacitySync.current?.enqueue({
      scope: { targetId: stateRef.current.target?.id ?? null, sessionId: stateRef.current.sessionId },
      targetKey: motionTargetKey, opacity,
    });
  }, [connected, nativeReady, motionTargetKey, session.state?.sessionId, orbOpacity, stateRef]);

  const chooseOrbAutoHide = useCallback((enabled: boolean) => {
    orbChoice.current = enabled;
    setOrbAutoHide(enabled);
    const current = stateRef.current;
    if (!connected || !nativeReady || !current) return;
    orbSync.current?.enqueue({
      scope: { targetId: current.target?.id ?? null, sessionId: current.sessionId },
      targetKey: targetLifecycleKey(current), enabled,
    }, true);
  }, [connected, nativeReady, stateRef]);

  const chooseOrbOpacity = useCallback((opacity: number) => {
    orbOpacityChoice.current = opacity;
    setOrbOpacity(opacity);
    const current = stateRef.current;
    if (!connected || !nativeReady || !current) return;
    orbOpacitySync.current?.enqueue({
      scope: { targetId: current.target?.id ?? null, sessionId: current.sessionId },
      targetKey: targetLifecycleKey(current), opacity,
    }, true);
  }, [connected, nativeReady, stateRef]);

  const adoptNativeOrbChange = useCallback((previous: AppSnapshot | null, updated: AppSnapshot) => {
    if (!nativeReadyRef.current || initialSnapshotApplied.current || orbSync.current?.pending()
      || !nativeOrbPreferenceChanged(previous, updated) || orbChoice.current === updated.display.orbAutoHide) return false;
    orbChoice.current = updated.display.orbAutoHide;
    setOrbAutoHide(updated.display.orbAutoHide);
    return true;
  }, [nativeReadyRef, initialSnapshotApplied]);

  return { motion, setMotion, intensity, setIntensity, effectiveMotion, chooseOrbAutoHide, adoptNativeOrbChange,
    orbOpacity: orbOpacity ?? session.state?.display.orbOpacity ?? 100, chooseOrbOpacity };
}
