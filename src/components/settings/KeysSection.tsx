import type { KeyStatus } from "../../generated/KeyStatus";
import type { SecretDraft } from "./draft";

export const KEY_DISCLOSURE = "Keys are encrypted with Windows DPAPI for your user account. Profiles are stored as plain text.";

function storageText(k: KeyStatus, d: SecretDraft | undefined): string {
  if (d?.remove && !d.value.trim()) return "Will be removed when you save";
  if (d?.value.trim()) return "New key will be saved when you save";
  if (k.storage === "unreadable") return "stored key couldn't be read — enter it again";
  if (k.storage === "encrypted" && k.hasKey) return "Saved (encrypted)";
  return "Not set";
}

interface Props {
  keys: KeyStatus[];
  secrets: Record<string, SecretDraft>;
  onChange(keyId: string, next: SecretDraft): void;
  onGetKey(url: string): void;
}

export function KeysSection({ keys, secrets, onChange, onGetKey }: Props) {
  return (
    <section className="settings-section" aria-labelledby="keys-heading">
      <h2 id="keys-heading" className="settings-section__title">
        API keys
      </h2>
      {keys.map((k) => {
        const d = secrets[k.id];
        const value = d?.value ?? "";
        const queuedRemove = !!d?.remove;
        const inputId = `key-${k.id}`;
        const statusId = `key-${k.id}-status`;
        const canRemove = k.hasKey || k.storage === "unreadable";
        return (
          <div className="key-row" key={k.id} data-testid={`key-row-${k.id}`}>
            <label htmlFor={inputId} className="field__label">
              {k.label} API key
            </label>
            <div className="key-row__line">
              <input
                id={inputId}
                className="input"
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={value}
                placeholder={k.hasKey ? "saved — type to replace" : "Paste your key"}
                aria-describedby={statusId}
                onChange={(e) => {
                  const v = e.target.value;
                  // Typing a new key cancels a queued Remove; emptying never removes.
                  onChange(k.id, { value: v, remove: v.trim() ? false : queuedRemove });
                }}
              />
              {canRemove && (
                <button
                  type="button"
                  className="btn btn--small btn--ghost"
                  aria-pressed={queuedRemove}
                  onClick={() => onChange(k.id, { value: queuedRemove ? value : "", remove: !queuedRemove })}
                >
                  {queuedRemove ? "Undo remove" : "Remove"}
                </button>
              )}
              <button type="button" className="btn btn--small btn--ghost" onClick={() => onGetKey(k.getKeyUrl)}>
                Get a key
              </button>
            </div>
            <p id={statusId} className={`key-row__status${k.storage === "unreadable" ? " key-row__status--warn" : ""}`}>
              {storageText(k, d)}
            </p>
          </div>
        );
      })}
      <p className="settings-note">{KEY_DISCLOSURE}</p>
    </section>
  );
}
