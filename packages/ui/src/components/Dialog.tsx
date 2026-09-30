import { useEffect, useId, useRef, type ReactNode } from "react";
import { X } from "lucide-react";

export type DialogProps = {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons; put the primary action last. */
  footer?: ReactNode;
  size?: "sm" | "md" | "lg";
  /** Prevents closing by Escape/backdrop while an operation must finish. */
  dismissible?: boolean;
};

/**
 * Native modal dialog: the browser traps focus and handles Escape. Focus
 * returns to the element that opened it.
 */
export function Dialog({ open, onClose, title, description, children, footer, size = "md", dismissible = true }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const opener = useRef<Element | null>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      opener.current = document.activeElement;
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }, [open]);

  useEffect(() => {
    if (open) return;
    const target = opener.current;
    opener.current = null;
    if (target instanceof HTMLElement && target.isConnected) target.focus();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className={`layrs-dialog layrs-dialog--${size}`}
      aria-labelledby={titleId}
      aria-describedby={description ? descriptionId : undefined}
      onCancel={(event) => {
        event.preventDefault();
        if (dismissible) onClose();
      }}
      onMouseDown={(event) => {
        if (dismissible && event.target === event.currentTarget) onClose();
      }}
    >
      {open && (
        <div className="layrs-dialog__panel">
          <header className="layrs-dialog__header">
            <div>
              <h2 id={titleId}>{title}</h2>
              {description && (
                <p id={descriptionId} className="layrs-dialog__description">
                  {description}
                </p>
              )}
            </div>
            {dismissible && (
              <button type="button" className="layrs-dialog__close" aria-label="Fermer" onClick={onClose}>
                <X aria-hidden size={18} />
              </button>
            )}
          </header>
          {children && <div className="layrs-dialog__body">{children}</div>}
          {footer && <footer className="layrs-dialog__footer">{footer}</footer>}
        </div>
      )}
    </dialog>
  );
}
