import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText } from "../api/client";
import type { LogSource, LogView } from "../api/types";
import { DEFAULT_LINE_CHOICE, FOLLOW_MS, pickSource } from "../lib/logview";

/**
 * The state of the log viewer: which logs exist, which one is open, and its newest lines. Reading
 * a log changes nothing, so it is allowed while an operation runs; the list of logs is read again
 * when one ends, because that is when a new log file or container can appear.
 */
export function useLogViewer(recipeId: string | null, operationRunning: boolean) {
  const [sources, setSources] = useState<LogSource[] | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [lines, setLines] = useState<number>(DEFAULT_LINE_CHOICE);
  const [view, setView] = useState<LogView | null>(null);
  const [follow, setFollow] = useState(true);
  const [loading, setLoading] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // Only the newest answer is kept: a slow read of a log the person has already left must not replace the one they are looking at.
  const latest = useRef(0);
  const inFlight = useRef(false);

  const refreshSources = useCallback(async () => {
    if (!recipeId) return;
    try {
      const found = await api.logSources(recipeId);
      setSources(found);
      setSource((chosen) => pickSource(found, chosen));
    } catch (e) {
      setProblem(errorText(e));
    }
  }, [recipeId]);

  const read = useCallback(async () => {
    if (!recipeId || source === null || inFlight.current) return;
    const ticket = ++latest.current;
    inFlight.current = true;
    setLoading(true);
    try {
      const next = await api.readLog(recipeId, source, lines);
      if (ticket === latest.current) {
        setView(next);
        setProblem(null);
      }
    } catch (e) {
      if (ticket === latest.current) setProblem(errorText(e));
    } finally {
      inFlight.current = false;
      if (ticket === latest.current) setLoading(false);
    }
  }, [recipeId, source, lines]);

  useEffect(() => {
    void refreshSources();
  }, [refreshSources, operationRunning]);

  // Choosing another log or another size reads it at once, even if a read of the old one is still on its way.
  useEffect(() => {
    latest.current += 1;
    inFlight.current = false;
    setView(null);
    void read();
    // `read` changes exactly when the source or size does.
  }, [read]);

  useEffect(() => {
    if (!follow) return;
    const timer = setInterval(() => void read(), FOLLOW_MS);
    return () => clearInterval(timer);
  }, [follow, read]);

  return { sources, source, setSource, lines, setLines, view, follow, setFollow, loading, problem, reload: read };
}
