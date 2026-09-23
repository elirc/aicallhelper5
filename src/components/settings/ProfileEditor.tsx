import type { CallType } from "../../generated/CallType";
import type { Profile } from "../../generated/Profile";
import { CALL_TYPES, profileFieldLabels } from "../common/labels";
import { FOCUS_MAX, MAX_PROFILES, NAME_MAX, TEXT_MAX, duplicateName, newProfileId } from "./draft";

interface Props {
  profiles: Profile[];
  activeProfileId: string;
  editingId: string;
  onEditingChange(id: string): void;
  /** Replace profiles (and optionally the active id). */
  onProfilesChange(profiles: Profile[], activeProfileId?: string): void;
}

export function ProfileEditor({ profiles, activeProfileId, editingId, onEditingChange, onProfilesChange }: Props) {
  const editing = profiles.find((p) => p.id === editingId) ?? profiles[0];
  if (!editing) return null;
  const labels = profileFieldLabels(editing.callType);
  const atLimit = profiles.length >= MAX_PROFILES;

  function update(patch: Partial<Profile>) {
    if (!editing) return;
    onProfilesChange(profiles.map((p) => (p.id === editing.id ? { ...p, ...patch } : p)));
  }

  function add() {
    if (atLimit) return;
    const p: Profile = {
      id: newProfileId(profiles),
      name: "New profile",
      callType: "behavioral",
      focus: "",
      resume: "",
      jobDescription: "",
      notes: "",
    };
    onProfilesChange([...profiles, p]);
    onEditingChange(p.id);
  }

  function duplicate() {
    if (atLimit || !editing) return;
    const p: Profile = { ...editing, id: newProfileId(profiles), name: duplicateName(editing.name) };
    onProfilesChange([...profiles, p]);
    onEditingChange(p.id);
  }

  function remove() {
    if (profiles.length <= 1 || !editing) return;
    const rest = profiles.filter((p) => p.id !== editing.id);
    const first = rest[0];
    if (!first) return;
    const nextActive = activeProfileId === editing.id ? first.id : undefined;
    onProfilesChange(rest, nextActive);
    onEditingChange(nextActive ?? first.id);
  }

  const isActive = editing.id === activeProfileId;

  return (
    <section className="settings-section" aria-labelledby="profiles-heading">
      <h2 id="profiles-heading" className="settings-section__title">
        Profiles
      </h2>
      <div className="profile-bar">
        <label className="field field--inline">
          <span className="field__label">Editing</span>
          <select className="select" value={editing.id} onChange={(e) => onEditingChange(e.target.value)}>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name || "(unnamed)"}
                {p.id === activeProfileId ? " (active)" : ""}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="btn btn--small" disabled={atLimit} onClick={add}>
          Add
        </button>
        <button type="button" className="btn btn--small" disabled={atLimit} onClick={duplicate}>
          Duplicate
        </button>
        <button type="button" className="btn btn--small btn--danger" disabled={profiles.length <= 1} onClick={remove}>
          Delete
        </button>
        <button type="button" className="btn btn--small btn--ghost" disabled={isActive} onClick={() => onProfilesChange(profiles, editing.id)}>
          {isActive ? "Active profile" : "Use for answers"}
        </button>
      </div>
      {atLimit && <p className="settings-note">You can have up to {MAX_PROFILES} profiles.</p>}

      <label className="field">
        <span className="field__label">Name</span>
        <input className="input" type="text" value={editing.name} maxLength={NAME_MAX} onChange={(e) => update({ name: e.target.value })} />
      </label>
      <label className="field">
        <span className="field__label">Call type</span>
        <select className="select" value={editing.callType} onChange={(e) => update({ callType: e.target.value as CallType })}>
          {CALL_TYPES.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
      <div className="field">
        <div className="field__label-row">
          <label className="field__label" htmlFor="profile-focus">
            Focus
          </label>
          <span id="profile-focus-count" className="field__counter" data-testid="focus-counter">
            {editing.focus.length}/{FOCUS_MAX}
          </span>
        </div>
        <textarea
          id="profile-focus"
          className="textarea"
          rows={3}
          value={editing.focus}
          maxLength={FOCUS_MAX}
          aria-describedby="profile-focus-count"
          onChange={(e) => update({ focus: e.target.value })}
        />
      </div>
      <label className="field">
        <span className="field__label">{labels.resume}</span>
        <textarea className="textarea" rows={5} value={editing.resume} maxLength={TEXT_MAX} onChange={(e) => update({ resume: e.target.value })} />
      </label>
      <label className="field">
        <span className="field__label">{labels.jobDescription}</span>
        <textarea
          className="textarea"
          rows={5}
          value={editing.jobDescription}
          maxLength={TEXT_MAX}
          onChange={(e) => update({ jobDescription: e.target.value })}
        />
      </label>
      <label className="field">
        <span className="field__label">Notes</span>
        <textarea className="textarea" rows={3} value={editing.notes} maxLength={TEXT_MAX} onChange={(e) => update({ notes: e.target.value })} />
      </label>
    </section>
  );
}
