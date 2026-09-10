import { useLayoutEffect, useRef, useState } from "react";
import { verifyProviderModelVision, type ProviderView, type StateView, type VisionVerificationView } from "../api";
import { humanizeAppError } from "../errors";

// Keep only bounded, transient feedback. A result must not retain a full configuration snapshot.
type Report = Omit<VisionVerificationView, "state">;
interface Feedback {
  checking: string | null;
  results: Record<string, Report>;
  errors: Record<string, string>;
}
interface Session {
  identity: string;
  feedback: Feedback;
}
const emptyFeedback = (): Feedback => ({
  checking: null, results: Object.create(null), errors: Object.create(null),
});
const EMPTY = emptyFeedback();
const MAX_PROVIDERS = 32;
const MAX_RESULTS = 16;
const MAX_PENDING = 8;

function identity(provider: ProviderView): string {
  return JSON.stringify([
    provider.name, provider.provider, provider.base_url, provider.provider_call,
    provider.has_auth, provider.credential_source, provider.credential_reference,
    [...provider.models].sort(),
  ]);
}

export interface VisionVerificationTasks {
  snapshot: (provider: ProviderView) => Feedback;
  verify: (provider: ProviderView, model: string) => Promise<void>;
  clear: (name: string) => void;
}

/** The list owns this hook so modal unmounts do not detach a running request or its feedback. */
export function useVisionVerificationTasks(
  providers: ProviderView[],
  onSaved: (state: StateView) => void,
): VisionVerificationTasks {
  const sessions = useRef(new Map<string, Session>());
  const current = useRef({ providers, onSaved });
  const alive = useRef(true);
  const [, changed] = useState(0);
  const notify = () => { if (alive.current) changed(version => version + 1); };

  useLayoutEffect(() => {
    current.current = { providers, onSaved };
    let removed = false;
    for (const [name, session] of sessions.current) {
      const provider = providers.find(item => item.name === name);
      if (!provider || identity(provider) !== session.identity) {
        sessions.current.delete(name);
        removed = true;
      }
    }
    if (removed) changed(version => version + 1);
  }, [providers, onSaved]);

  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      sessions.current.clear();
    };
  }, []);

  const clear = (name: string) => {
    if (sessions.current.delete(name)) notify();
  };
  const snapshot = (provider: ProviderView) => {
    const session = sessions.current.get(provider.name);
    return session?.identity === identity(provider) ? session.feedback : EMPTY;
  };
  const verify = async (provider: ProviderView, model: string) => {
    const key = identity(provider);
    const liveProvider = current.current.providers.find(item => item.name === provider.name);
    if (!alive.current || !liveProvider || identity(liveProvider) !== key || !provider.models.includes(model)) return;
    let session = sessions.current.get(provider.name);
    if (session?.identity !== key) {
      clear(provider.name);
      session = undefined;
    }
    if (session?.feedback.checking) return;
    if (!session) {
      // Active requests are never evicted. The backend also caps concurrent checks at eight.
      if (sessions.current.size >= MAX_PROVIDERS) {
        const oldest = [...sessions.current].find(([, item]) => item.feedback.checking === null);
        if (oldest) sessions.current.delete(oldest[0]);
      }
      session = { identity: key, feedback: emptyFeedback() };
      sessions.current.set(provider.name, session);
    }
    const task = session;
    const feedback = task.feedback;
    delete feedback.results[model];
    delete feedback.errors[model];
    const rows = [...new Set([...Object.keys(feedback.results), ...Object.keys(feedback.errors)])];
    while (rows.length >= MAX_RESULTS) {
      const oldest = rows.shift()!;
      delete feedback.results[oldest];
      delete feedback.errors[oldest];
    }
    if ([...sessions.current.values()].filter(item => item.feedback.checking !== null).length >= MAX_PENDING) {
      feedback.errors[model] = "Wait for the current Provider checks before starting another.";
      notify();
      return;
    }
    feedback.checking = model;
    notify();
    const isCurrent = () => alive.current && sessions.current.get(provider.name) === task;
    try {
      const { state, ...report } = await verifyProviderModelVision(provider.name, model);
      if (!isCurrent()) return;
      feedback.results[model] = { ...report, detail: report.detail.slice(0, 600) };
      current.current.onSaved(state);
    } catch (caught) {
      if (isCurrent()) feedback.errors[model] = humanizeAppError(caught).slice(0, 600);
    } finally {
      if (isCurrent()) {
        feedback.checking = null;
        notify();
      }
    }
  };
  return { snapshot, verify, clear };
}
