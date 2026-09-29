import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, X, XCircle } from "lucide-react";
import type { Tone } from "./Feedback";

export type ToastInput = {
  tone?: Tone;
  title: ReactNode;
  description?: ReactNode;
  action?: { label: string; onSelect: () => void };
};

type ToastEntry = ToastInput & { id: number };

type ToastApi = {
  /** Secondary confirmation; never the only trace of a critical failure. */
  show: (toast: ToastInput) => number;
  dismiss: (id: number) => void;
};

const ToastContext = createContext<ToastApi | null>(null);

const icons: Record<Tone, ReactNode> = {
  info: <Info aria-hidden size={18} />,
  success: <CheckCircle2 aria-hidden size={18} />,
  warning: <AlertTriangle aria-hidden size={18} />,
  danger: <XCircle aria-hidden size={18} />,
};

const AUTO_DISMISS_MS = 5000;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => setToasts((all) => all.filter((toast) => toast.id !== id)), []);
  const show = useCallback((toast: ToastInput) => {
    const id = nextId.current++;
    setToasts((all) => [...all.slice(-3), { ...toast, id }]);
    return id;
  }, []);
  const api = useMemo(() => ({ show, dismiss }), [show, dismiss]);

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="layrs-toasts" aria-live="polite" aria-relevant="additions">
        {toasts.map((toast) => (
          <ToastView key={toast.id} toast={toast} dismiss={dismiss} />
        ))}
      </div>
    </ToastContext.Provider>
  );
}

function ToastView({ toast, dismiss }: { toast: ToastEntry; dismiss: (id: number) => void }) {
  const tone = toast.tone ?? "info";
  const { id } = toast;
  const [paused, setPaused] = useState(false);
  // Errors stay until closed; information leaves after a few seconds unless hovered/focused.
  useEffect(() => {
    if (tone === "danger" || paused) return;
    const timer = window.setTimeout(() => dismiss(id), AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [tone, paused, dismiss, id]);
  const onDismiss = () => dismiss(id);
  return (
    <div
      className={`layrs-toast layrs-toast--${tone}`}
      role={tone === "danger" ? "alert" : "status"}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      <span className="layrs-toast__icon">{icons[tone]}</span>
      <div className="layrs-toast__body">
        <p className="layrs-toast__title">{toast.title}</p>
        {toast.description && <p className="layrs-toast__text">{toast.description}</p>}
        {toast.action && (
          <button
            type="button"
            className="layrs-toast__action"
            onClick={() => {
              toast.action?.onSelect();
              onDismiss();
            }}
          >
            {toast.action.label}
          </button>
        )}
      </div>
      <button type="button" className="layrs-toast__close" aria-label="Fermer la notification" onClick={onDismiss}>
        <X aria-hidden size={16} />
      </button>
    </div>
  );
}

export function useToast(): ToastApi {
  const api = useContext(ToastContext);
  if (!api) throw new Error("useToast must be used inside <ToastProvider>");
  return api;
}
