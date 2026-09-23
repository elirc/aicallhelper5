// Owns the store (useReducer), the CoreApi (prop) and every effect. Components
// consume only `useApp()` -> {view, actions} (see ./view.ts).
import { useEffect, useMemo, useReducer, useState, type ReactNode } from "react";
import type { CoreApi } from "../ipc/types";
import { initialState, reducer } from "../state/reducer";
import { selectView } from "../state/select";
import { AppContext, type AppContextValue } from "./view";
import { createController, type Clock, type RecoveryOptions, type ScheduleFrame } from "./controller";

export interface AppProviderProps {
  api: CoreApi;
  children?: ReactNode;
  /** Delta-coalescing frame scheduler (tests inject a manual one). */
  scheduleFrame?: ScheduleFrame;
  clock?: Clock;
  recovery?: RecoveryOptions;
  statusRetries?: { tries: number; intervalMs: number };
  copyText?: (text: string) => Promise<boolean>;
}

/** Tick interval for the recording timer / countdown / silence hint. */
export const TICK_MS = 250;

export function AppProvider({ api, children, scheduleFrame, clock, recovery, statusRetries, copyText }: AppProviderProps) {
  const [state, dispatch] = useReducer(reducer, undefined, initialState);
  // Created once per provider instance; the api is expected to be stable.
  const [ctl] = useState(() => createController({ api, dispatch, scheduleFrame, clock, recovery, statusRetries, copyText }));
  const [nowClock] = useState(() => clock ?? { now: () => Date.now(), perf: () => performance.now() });
  const [now, setNow] = useState(() => nowClock.now());

  // Subscribe first, then (once) get_status + get_settings.
  useEffect(() => ctl.attach(), [ctl]);

  useEffect(() => {
    ctl.syncCloseGuard(state.settingsDirty);
  }, [ctl, state.settingsDirty]);

  // A ticking clock only while recording (timer, countdown, silence hint).
  const recording = state.session?.phase === "recording";
  useEffect(() => {
    if (!recording) return;
    setNow(nowClock.now());
    const t = setInterval(() => setNow(nowClock.now()), TICK_MS);
    return () => clearInterval(t);
  }, [recording, nowClock]);

  const view = useMemo(() => selectView(state, now), [state, now]);
  const value = useMemo<AppContextValue>(() => ({ view, actions: ctl.actions }), [view, ctl]);
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
