import { create } from "zustand";

export interface Toast {
  id: number;
  title: string;
  /** Technical detail: error code and message. Never message content. */
  detail?: string;
  kind: "info" | "error";
}

interface ToastState {
  toasts: Toast[];
  push: (toast: Omit<Toast, "id">) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToasts = create<ToastState>((set) => ({
  toasts: [],
  push: (toast) =>
    set((state) => ({ toasts: [...state.toasts, { ...toast, id: nextId++ }] })),
  dismiss: (id) =>
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),
}));

/** Report a failed command as a toast, with its code for the detail line. */
export function toastError(title: string, error: unknown): void {
  const detail =
    error instanceof Error
      ? `${(error as { code?: string }).code ?? "error"}: ${error.message}`
      : String(error);
  useToasts.getState().push({ title, detail, kind: "error" });
}

export function Toasts() {
  const toasts = useToasts((s) => s.toasts);
  const dismiss = useToasts((s) => s.dismiss);
  if (toasts.length === 0) return null;

  return (
    <div className="toasts">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={toast.kind === "error" ? "toast toast--error" : "toast"}
          role={toast.kind === "error" ? "alert" : "status"}
        >
          <div className="toast__title">{toast.title}</div>
          {toast.detail ? <p className="toast__detail">{toast.detail}</p> : null}
          <div className="modal__actions">
            <button
              type="button"
              className="button button--quiet"
              onClick={() => dismiss(toast.id)}
            >
              Dismiss
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
