import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";

type ToastTone = "error" | "info" | "success";
type FeedbackLanguage = "en" | "zh-CN" | "zh-TW" | "ja";

interface ErrorToastValue {
  showError: (message: string, id?: string) => void;
  showInfo: (message: string, id?: string) => void;
  showSuccess: (message: string, id?: string) => void;
  dismissToast: (id: string) => void;
}

interface ErrorToastItem {
  id: string;
  message: string;
  tone: ToastTone;
  revision: number;
}

const fallbackToastValue: ErrorToastValue = {
  showError: () => undefined,
  showInfo: () => undefined,
  showSuccess: () => undefined,
  dismissToast: () => undefined,
};

const ErrorToastContext = createContext<ErrorToastValue | null>(null);

function feedbackLanguage(): FeedbackLanguage {
  const documentLanguage = typeof document === "undefined" ? "" : document.documentElement.lang;
  if (
    documentLanguage === "en"
    || documentLanguage === "zh-CN"
    || documentLanguage === "zh-TW"
    || documentLanguage === "ja"
  ) {
    return documentLanguage;
  }
  try {
    const stored = typeof window === "undefined"
      ? null
      : window.localStorage.getItem("token-station-language");
    return stored === "zh-CN" || stored === "zh-TW" || stored === "ja" ? stored : "en";
  } catch {
    return "en";
  }
}

function feedbackCopy(
  language: FeedbackLanguage,
  english: string,
  simplifiedChinese: string,
  traditionalChinese: string,
  japanese: string,
): string {
  return {
    en: english,
    "zh-CN": simplifiedChinese,
    "zh-TW": traditionalChinese,
    ja: japanese,
  }[language];
}

export function useErrorToast() {
  return useContext(ErrorToastContext) ?? fallbackToastValue;
}

function Toast({
  toast,
  onDismiss,
  language,
}: {
  toast: ErrorToastItem;
  onDismiss: (id: string) => void;
  language: FeedbackLanguage;
}) {
  const [fading, setFading] = useState(false);

  useEffect(() => {
    setFading(false);
    const fadeTimer = window.setTimeout(() => setFading(true), 7_000);
    const removeTimer = window.setTimeout(() => onDismiss(toast.id), 8_000);
    return () => {
      window.clearTimeout(fadeTimer);
      window.clearTimeout(removeTimer);
    };
  }, [onDismiss, toast.id, toast.revision]);

  const Icon = toast.tone === "error"
    ? AlertTriangle
    : toast.tone === "success"
      ? CheckCircle2
      : Info;

  return (
    <div
      className={`error-toast is-${toast.tone}${fading ? " is-fading" : ""}`}
      role={toast.tone === "error" ? "alert" : "status"}
      aria-live={toast.tone === "error" ? "assertive" : "polite"}
      aria-atomic="true"
    >
      <Icon className="error-toast-icon" aria-hidden="true" />
      <span>{toast.message}</span>
      <button
        type="button"
        className="error-toast-close"
        aria-label={feedbackCopy(
          language,
          "Close notification",
          "关闭提示",
          "關閉通知",
          "通知を閉じる",
        )}
        onClick={() => onDismiss(toast.id)}
      >
        <X aria-hidden="true" />
      </button>
    </div>
  );
}

export function ErrorToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ErrorToastItem[]>([]);
  const [language, setLanguage] = useState<FeedbackLanguage>(feedbackLanguage);

  useEffect(() => {
    if (typeof MutationObserver === "undefined") return;
    const observer = new MutationObserver(() => setLanguage(feedbackLanguage()));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["lang"] });
    return () => observer.disconnect();
  }, []);

  const dismissToast = useCallback((id: string) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const showToast = useCallback((tone: ToastTone, message: string, id = message) => {
    if (!message) return;
    setToasts((current) => {
      const index = current.findIndex((toast) => toast.id === id);
      if (index < 0) return [...current, { id, message, tone, revision: 0 }];
      if (current[index].message === message && current[index].tone === tone) return current;
      return current.map((toast) => toast.id === id
        ? { ...toast, message, tone, revision: toast.revision + 1 }
        : toast);
    });
  }, []);

  const showError = useCallback(
    (message: string, id?: string) => showToast("error", message, id),
    [showToast],
  );
  const showInfo = useCallback(
    (message: string, id?: string) => showToast("info", message, id),
    [showToast],
  );
  const showSuccess = useCallback(
    (message: string, id?: string) => showToast("success", message, id),
    [showToast],
  );

  return (
    <ErrorToastContext.Provider value={{
      showError,
      showInfo,
      showSuccess,
      dismissToast,
    }}>
      {children}
      <div
        className="error-toast-viewport"
        data-testid="error-toast-viewport"
        aria-label={feedbackCopy(language, "Notifications", "操作提示", "通知", "通知")}
      >
        {toasts.map((toast) => (
          <Toast key={toast.id} toast={toast} onDismiss={dismissToast} language={language} />
        ))}
      </div>
    </ErrorToastContext.Provider>
  );
}

export function ErrorToastBoundary({ children }: { children: ReactNode }) {
  const value = useContext(ErrorToastContext);
  return value ? children : <ErrorToastProvider>{children}</ErrorToastProvider>;
}
