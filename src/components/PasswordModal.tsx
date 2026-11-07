import { Eye, EyeOff, Lock } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { BackupPeek } from "../bindings/BackupPeek";

/**
 * Asks for the backup password.
 *
 * Deriving the keys takes about a second by design, so the submit button shows
 * a busy state. A wrong password keeps whatever was typed, per UI_DESIGN.md.
 */
export function PasswordModal(props: {
  peek: BackupPeek;
  busy: boolean;
  /** Set when the last attempt was rejected. */
  error: string | null;
  onSubmit: (password: string) => void;
  onCancel: () => void;
}) {
  const [password, setPassword] = useState("");
  const [visible, setVisible] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // Focus the field on open, and again after a failed attempt.
  useEffect(() => {
    inputRef.current?.focus();
  }, [props.error]);

  const device =
    props.peek.info.deviceName ??
    props.peek.info.productName ??
    props.peek.info.productType ??
    "this backup";

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!props.busy) props.onSubmit(password);
  };

  return (
    <div
      className="modal__backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="password-title"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !props.busy) props.onCancel();
      }}
    >
      <form className="modal" onSubmit={submit}>
        <h2 className="modal__title" id="password-title">
          <Lock size={14} aria-hidden="true" /> Unlock {device}
        </h2>
        <p style={{ marginTop: 0, color: "var(--text-muted)" }}>
          This backup is encrypted
          {props.peek.info.productVersion ? ` (iOS ${props.peek.info.productVersion})` : ""}.
          Enter the backup password you set when encrypting it — it is not your
          device passcode or your Apple account password.
        </p>

        <div style={{ display: "flex", gap: 6 }}>
          <input
            ref={inputRef}
            className="input"
            style={{ flex: 1 }}
            type={visible ? "text" : "password"}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            placeholder="Backup password"
            autoComplete="off"
            spellCheck={false}
            disabled={props.busy}
            aria-invalid={props.error !== null}
            aria-describedby={props.error ? "password-error" : undefined}
          />
          <button
            type="button"
            className="button button--quiet"
            onClick={() => setVisible((v) => !v)}
            aria-label={visible ? "Hide the password" : "Show the password"}
            title={visible ? "Hide" : "Show"}
          >
            {visible ? <EyeOff size={14} aria-hidden="true" /> : <Eye size={14} aria-hidden="true" />}
          </button>
        </div>

        {props.error ? (
          <p id="password-error" style={{ color: "var(--danger)", marginBottom: 0 }}>
            {props.error}
          </p>
        ) : null}

        <p style={{ color: "var(--text-muted)", fontSize: 12, marginBottom: 0 }}>
          Decryption happens on this machine. Nothing is sent anywhere, and the
          password is never written to disk. Decrypted copies are cached under{" "}
          <span className="mono">~/.local/share/ibex</span> so files can be
          previewed and played; clear the cache from Overview when you are done.
        </p>

        <div className="modal__actions">
          <button
            type="button"
            className="button"
            onClick={props.onCancel}
            disabled={props.busy}
          >
            Cancel
          </button>
          <button type="submit" className="button button--primary" disabled={props.busy}>
            {props.busy ? "Unlocking…" : "Unlock"}
          </button>
        </div>
      </form>
    </div>
  );
}
