import { useEffect, useId, useRef, type ReactNode, type RefObject } from "react";
import { Check, CircleAlert, LoaderCircle, X } from "lucide-react";

export type Notice = { kind: "error" | "success"; text: string } | null;

export function Brand() {
  return <svg viewBox="0 0 24 24" width="26" height="26" aria-hidden="true" fill="currentColor">
    <path d="M2 2h7v7H2zM11 5h4v4h-4zM15 9h4v4h-4zM19 13h3v4h-3zM3 14h6v6H3zM11 16h6v6h-6z" />
  </svg>;
}

export function Modal({ title, onClose, children, dismissible = true, initialFocus }: {
  title: string; onClose: () => void; children: ReactNode; dismissible?: boolean;
  initialFocus?: RefObject<HTMLElement | null>;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const id = useId();
  useEffect(() => {
    const dialog = ref.current;
    if (dialog && !dialog.open) {
      dialog.showModal();
      initialFocus?.current?.focus();
    }
    return () => { if (dialog?.open) dialog.close(); };
  }, [initialFocus]);
  return <dialog ref={ref} aria-labelledby={id} onCancel={event => {
    event.preventDefault();
    if (dismissible) onClose();
  }}>
    <div className="modal-title"><h2 id={id}>{title}</h2><button className="icon-button" aria-label={`Close ${title}`} onClick={onClose} disabled={!dismissible}><X size={19} /></button></div>
    {children}
  </dialog>;
}

export function Feedback({ notice, busy }: { notice?: Notice; busy?: string }) {
  if (busy) return <div className="feedback" role="status"><LoaderCircle className="spin" size={16} />{busy}</div>;
  if (!notice) return null;
  return <div className={`feedback ${notice.kind}`} role={notice.kind === "error" ? "alert" : "status"}>
    {notice.kind === "error" ? <CircleAlert size={17} /> : <Check size={17} />}{notice.text}
  </div>;
}

export function Empty({ title, children }: { title: string; children: ReactNode }) {
  return <div className="empty"><strong>{title}</strong><p>{children}</p></div>;
}
