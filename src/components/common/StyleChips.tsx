import { useRef, type KeyboardEvent } from "react";
import { useApp } from "../../app/view";
import { STYLES } from "./labels";

/** Brief / Balanced / Detailed — a radio group with roving focus + arrow keys. */
export function StyleChips({ compact = false }: { compact?: boolean }) {
  const { view, actions } = useApp();
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const current = STYLES.findIndex((s) => s.id === view.answerStyle);

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    let dir = 0;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") dir = 1;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") dir = -1;
    if (!dir) return;
    e.preventDefault();
    const from = current < 0 ? 0 : current;
    const next = (from + dir + STYLES.length) % STYLES.length;
    const style = STYLES[next];
    if (!style) return;
    actions.setStyle(style.id);
    refs.current[next]?.focus();
  }

  return (
    <div className={`chips${compact ? " chips--compact" : ""}`} role="radiogroup" aria-label="Answer style" onKeyDown={onKeyDown}>
      {STYLES.map((s, i) => {
        const checked = s.id === view.answerStyle;
        return (
          <button
            key={s.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked || (current < 0 && i === 0) ? 0 : -1}
            className={`chip${checked ? " chip--on" : ""}`}
            onClick={() => actions.setStyle(s.id)}
          >
            {s.label}
          </button>
        );
      })}
    </div>
  );
}
