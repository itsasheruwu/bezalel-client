import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Clipboard, LogIn } from "lucide-react";
import { Feedback, Modal, type Notice } from "./components";

type AccessCodeFormProps = {
  busy: string;
  notice: Notice;
  onSignIn: (code: string) => void;
  secondary?: ReactNode;
  autoFocus?: boolean;
};

export function AccessCodeForm({ busy, notice, onSignIn, secondary, autoFocus }: AccessCodeFormProps) {
  const [code, setCode] = useState("");
  const [visible, setVisible] = useState(false);
  const id = useId();
  return <form className="form" onSubmit={event => {
    event.preventDefault();
    if (!busy && code.trim()) onSignIn(code.trim());
  }}>
    <div className="access-code-field">
      <label htmlFor={id}>Access code</label>
      <div className="access-code-control">
        <input id={id} className="access-code-input" type={visible ? "text" : "password"} value={code}
          onChange={event => setCode(event.target.value)} placeholder="Paste your code" maxLength={128}
          autoComplete="off" autoCapitalize="none" spellCheck={false} required autoFocus={autoFocus} disabled={Boolean(busy)} />
        <button type="button" className="secondary" aria-controls={id} aria-label={visible ? "Hide access code" : "Show access code"}
          onClick={() => setVisible(value => !value)} disabled={Boolean(busy)}>{visible ? "Hide" : "Show"}</button>
      </div>
    </div>
    <Feedback {...{ busy, notice }} />
    <div className="actions">
      <button className="primary" disabled={Boolean(busy) || !code.trim()}><LogIn size={16} />Sign in</button>
      {secondary}
    </div>
  </form>;
}

export function AccessCodeDialog({ title, description, onClose, ...form }: Pick<AccessCodeFormProps, "busy" | "notice" | "onSignIn"> & {
  title: string;
  description?: string;
  onClose: () => void;
}) {
  return <Modal title={title} onClose={onClose} dismissible={!form.busy}>
    <div className="stack">
      {description && <p>{description}</p>}
      <AccessCodeForm {...form} autoFocus
        secondary={<button type="button" className="secondary" onClick={onClose} disabled={Boolean(form.busy)}>Cancel</button>} />
    </div>
  </Modal>;
}

export function FriendAccessCodeDialog({ code, busy, notice, onCreate, onClose }: {
  code?: string;
  busy: string;
  notice: Notice;
  onCreate: () => void;
  onClose: () => void;
}) {
  const [copyNotice, setCopyNotice] = useState<Notice>(null);
  const field = useRef<HTMLInputElement>(null);
  const id = useId();
  useEffect(() => { if (code) field.current?.focus(); }, [code]);

  return <Modal title={code ? "Friend access code" : "Create friend access code"} onClose={onClose} dismissible={!busy}>
    {code ? <div className="form">
      <p>Your friend can use this code to sign in again. Save it somewhere private and share it only with them. Send a pack invite separately.</p>
      <label htmlFor={id}>Friend access code
        <input id={id} className="access-code-input" ref={field} value={code} readOnly autoComplete="off"
          onFocus={event => event.currentTarget.select()} />
      </label>
      <Feedback notice={copyNotice} />
      <div className="actions">
        <button type="button" className="primary" onClick={async () => {
          try {
            await navigator.clipboard.writeText(code);
            setCopyNotice({ kind: "success", text: "Access code copied." });
          } catch {
            field.current?.focus();
            setCopyNotice({ kind: "error", text: "Select the code and copy it manually." });
          }
        }}><Clipboard size={16} />Copy access code</button>
        <button type="button" className="secondary" onClick={onClose} disabled={Boolean(busy)}>Done</button>
      </div>
    </div> : <div className="form">
      <Feedback {...{ busy, notice }} />
      {!busy && <button type="button" className="primary" onClick={onCreate}>Create code</button>}
    </div>}
  </Modal>;
}
