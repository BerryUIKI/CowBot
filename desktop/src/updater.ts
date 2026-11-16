import { useCallback, useEffect, useRef, useState } from "react";
import { check as checkForUpdate, type Update } from "@tauri-apps/plugin-updater";

import { getAppInfo, restartApp, setAutoCheckUpdates } from "./api";
import { errorMessage } from "./errors";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "downloading"
  | "ready"
  | "error";

export interface UpdateState {
  phase: UpdatePhase;
  currentVersion: string | null;
  /** Schema version of the workspace database this build provides. */
  currentDatabaseSchema: number | null;
  version: string | null;
  date: string | null;
  downloaded: number;
  total: number | null;
  error: string | null;
  checkedAt: string | null;
  dismissed: boolean;
}

export interface Updater {
  state: UpdateState;
  autoCheck: boolean;
  setAutoCheck: (enabled: boolean) => Promise<void>;
  check: () => Promise<void>;
  install: () => Promise<void>;
  restart: () => Promise<void>;
  dismiss: () => void;
}

const initialState: UpdateState = {
  phase: "idle",
  currentVersion: null,
  currentDatabaseSchema: null,
  version: null,
  date: null,
  downloaded: 0,
  total: null,
  error: null,
  checkedAt: null,
  dismissed: false,
};

/**
 * Update check and install state shared by the launch banner and Settings.
 *
 * The user opts in to automatic checks in Settings; a check never downloads
 * anything on its own. Installation always needs an explicit click, and
 * restarting is a separate step on macOS and Linux.
 */
export function useUpdater(): Updater {
  const [state, setState] = useState<UpdateState>(initialState);
  const [autoCheck, setAutoCheckState] = useState(true);
  const updateRef = useRef<Update | null>(null);
  const checkInFlight = useRef(false);
  const installInFlight = useRef(false);
  const autoCheckStarted = useRef(false);

  const check = useCallback(async (options?: { silent?: boolean }) => {
    const silent = options?.silent ?? false;
    if (checkInFlight.current) return;

    checkInFlight.current = true;
    if (!silent) {
      setState((current) => ({
        ...current,
        phase: "checking",
        error: null,
        dismissed: false,
      }));
    }

    try {
      const update = await checkForUpdate();
      updateRef.current = update;
      const checkedAt = new Date().toISOString();

      if (update) {
        setState((current) => ({
          ...current,
          phase: "available",
          version: update.version,
          currentVersion: update.currentVersion,
          date: update.date ?? null,
          downloaded: 0,
          total: null,
          error: null,
          checkedAt,
          dismissed: false,
        }));
      } else {
        setState((current) => ({
          ...current,
          phase: "current",
          version: null,
          date: null,
          downloaded: 0,
          total: null,
          error: null,
          checkedAt,
        }));
      }
    } catch (error) {
      if (!silent) {
        setState((current) => ({
          ...current,
          phase: "error",
          version: null,
          date: null,
          error: errorMessage(error),
          checkedAt: new Date().toISOString(),
        }));
      }
    } finally {
      checkInFlight.current = false;
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    getAppInfo()
      .then((info) => {
        if (cancelled) return;
        setState((current) => ({
          ...current,
          currentVersion: info.version,
          currentDatabaseSchema: info.databaseSchema,
        }));
        setAutoCheckState(info.autoCheckUpdates);
        if (info.autoCheckUpdates && !autoCheckStarted.current) {
          autoCheckStarted.current = true;
          void check({ silent: true });
        }
      })
      .catch(() => {
        // The About section still works with an unknown version.
      });

    return () => {
      cancelled = true;
    };
  }, [check]);

  const install = useCallback(async () => {
    const update = updateRef.current;
    if (!update || installInFlight.current) return;

    installInFlight.current = true;
    setState((current) => ({
      ...current,
      phase: "downloading",
      downloaded: 0,
      total: null,
      error: null,
    }));

    try {
      let downloaded = 0;
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          setState((current) => ({
            ...current,
            total: event.data.contentLength ?? null,
          }));
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          setState((current) => ({ ...current, downloaded }));
        }
      });
      setState((current) => ({ ...current, phase: "ready" }));
    } catch (error) {
      setState((current) => ({
        ...current,
        phase: "error",
        error: errorMessage(error),
      }));
    } finally {
      installInFlight.current = false;
    }
  }, []);

  const restart = useCallback(async () => {
    try {
      await restartApp();
    } catch (error) {
      setState((current) => ({
        ...current,
        phase: "error",
        error: errorMessage(error),
      }));
    }
  }, []);

  const dismiss = useCallback(() => {
    setState((current) => ({ ...current, dismissed: true }));
  }, []);

  const setAutoCheck = useCallback(async (enabled: boolean) => {
    setAutoCheckState(enabled);
    try {
      await setAutoCheckUpdates(enabled);
    } catch (error) {
      setAutoCheckState(!enabled);
      throw error;
    }
  }, []);

  return {
    state,
    autoCheck,
    setAutoCheck,
    check,
    install,
    restart,
    dismiss,
  };
}
