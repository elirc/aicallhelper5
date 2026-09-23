import { useState, type FormEvent } from "react";
import { useApp } from "../../app/view";
import type { CallType } from "../../generated/CallType";
import { CALL_TYPES } from "../common/labels";

export function QuestionPanel() {
  const { view } = useApp();
  const live = view.phase !== "idle";
  const text = live ? view.liveTranscript : (view.entry?.question ?? "");
  let placeholder = "The question you record or type shows here.";
  if (view.phase === "starting" || view.phase === "recording") placeholder = "Listening for the other person…";
  return (
    <section className="panel panel--question" aria-labelledby="question-heading">
      <h2 id="question-heading" className="panel__title">
        Question heard
      </h2>
      <p className={`question-text${text ? "" : " question-text--empty"}`} data-testid="question-text">
        {text || placeholder}
      </p>
    </section>
  );
}

export function AskBox() {
  const { view, actions } = useApp();
  const [text, setText] = useState("");
  const trimmed = text.trim();

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!view.canAsk || !trimmed) return;
    actions.ask(trimmed);
    setText("");
  }

  return (
    <form className="ask" onSubmit={submit}>
      <label htmlFor="ask-input" className="visually-hidden">
        Type a question instead
      </label>
      <input
        id="ask-input"
        className="input ask__input"
        type="text"
        placeholder="Type a question instead…"
        value={text}
        maxLength={4000}
        onChange={(e) => setText(e.target.value)}
        autoComplete="off"
      />
      <button type="submit" className="btn" disabled={!view.canAsk || !trimmed}>
        Ask
      </button>
    </form>
  );
}

export function ProfileControls() {
  const { view, actions } = useApp();
  const profiles = view.settings?.profiles ?? [];
  const active = view.activeProfile;
  return (
    <div className="profile-controls">
      {profiles.length > 1 && (
        <label className="field field--inline">
          <span className="field__label">Profile</span>
          <select
            className="select"
            value={active?.id ?? ""}
            onChange={(e) => actions.setActiveProfile(e.target.value)}
          >
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="field field--inline">
        <span className="field__label">Call type</span>
        <select
          className="select"
          value={active?.callType ?? "behavioral"}
          disabled={!active}
          onChange={(e) => actions.setCallType(e.target.value as CallType)}
        >
          {CALL_TYPES.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

export function NoticeBar() {
  const { view, actions } = useApp();
  if (!view.notice) return null;
  return (
    <div className="notice" role="status" data-testid="notice">
      <p className="notice__text">{view.notice}</p>
      <button type="button" className="icon-btn" aria-label="Dismiss notice" title="Dismiss notice" onClick={() => actions.dismissNotice()}>
        ×
      </button>
    </div>
  );
}
