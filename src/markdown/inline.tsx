// Inline parser: **bold** / __bold__, *italic* / _italic_, `code`, backslash
// escapes. Links are deliberately NOT rendered (`[x](y)` stays literal text)
// and raw HTML is just text. Emphasis pairing uses a delimiter stack
// (simplified CommonMark "process emphasis"), so it is linear-ish and
// deterministic. Every string ends up as a React text node.
import type { ReactNode } from "react";

export type Inline = string | { t: "strong"; c: Inline[] } | { t: "em"; c: Inline[] } | { t: "code"; v: string };

interface Delim {
  t: "delim";
  ch: "*" | "_";
  n: number;
  canOpen: boolean;
  canClose: boolean;
}

type Tok = Inline | Delim;

const PUNCT = /[!-/:-@[-`{-~]/;
const isWs = (c: string | undefined) => c === undefined || /\s/.test(c);
const isAlnum = (c: string | undefined) => c !== undefined && /[\p{L}\p{N}]/u.test(c);

function tokenize(s: string): Tok[] {
  const out: Tok[] = [];
  let text = "";
  const pushText = () => {
    if (text) out.push(text);
    text = "";
  };
  let i = 0;
  while (i < s.length) {
    const c = s[i] as string;
    if (c === "\\" && i + 1 < s.length && PUNCT.test(s[i + 1] as string)) {
      text += s[i + 1];
      i += 2;
      continue;
    }
    if (c === "`") {
      let n = 1;
      while (s[i + n] === "`") n++;
      const close = findBacktickRun(s, i + n, n);
      if (close >= 0) {
        let v = s.slice(i + n, close).replace(/\n/g, " ");
        if (v.length >= 2 && v.startsWith(" ") && v.endsWith(" ") && v.trim() !== "") v = v.slice(1, -1);
        pushText();
        out.push({ t: "code", v });
        i = close + n;
      } else {
        text += "`".repeat(n);
        i += n;
      }
      continue;
    }
    if (c === "*" || c === "_") {
      let n = 1;
      while (s[i + n] === c) n++;
      const prev = s[i - 1];
      const next = s[i + n];
      const left = !isWs(next);
      const right = !isWs(prev);
      const canOpen = c === "*" ? left : left && !isAlnum(prev);
      const canClose = c === "*" ? right : right && !isAlnum(next);
      pushText();
      out.push({ t: "delim", ch: c, n, canOpen, canClose });
      i += n;
      continue;
    }
    text += c;
    i++;
  }
  pushText();
  return out;
}

function findBacktickRun(s: string, from: number, n: number): number {
  let j = s.indexOf("`", from);
  while (j >= 0) {
    let k = 0;
    while (s[j + k] === "`") k++;
    if (k === n) return j;
    j = s.indexOf("`", j + k);
  }
  return -1;
}

function delimToText(t: Tok): Inline {
  return typeof t === "object" && t.t === "delim" ? t.ch.repeat(t.n) : (t as Inline);
}

function merge(nodes: Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const n of nodes) {
    const last = out[out.length - 1];
    if (typeof n === "string") {
      if (n === "") continue;
      if (typeof last === "string") out[out.length - 1] = last + n;
      else out.push(n);
    } else if (n.t === "code") {
      out.push(n);
    } else {
      out.push({ t: n.t, c: merge(n.c) });
    }
  }
  return out;
}

export function parseInline(s: string): Inline[] {
  const out: Tok[] = [];
  for (const tok of tokenize(s)) {
    if (typeof tok !== "object" || tok.t !== "delim") {
      out.push(tok);
      continue;
    }
    const d: Delim = { ...tok };
    if (d.canClose) {
      while (d.n > 0) {
        let k = -1;
        for (let j = out.length - 1; j >= 0; j--) {
          const o = out[j];
          if (typeof o === "object" && o.t === "delim" && o.ch === d.ch && o.canOpen && o.n > 0) {
            k = j;
            break;
          }
        }
        if (k < 0) break;
        const opener = out[k] as Delim;
        const use = opener.n >= 2 && d.n >= 2 ? 2 : 1;
        const inner = out.splice(k + 1).map(delimToText);
        opener.n -= use;
        d.n -= use;
        const node: Inline = { t: use === 2 ? "strong" : "em", c: inner };
        if (opener.n === 0) out.splice(k, 1);
        out.push(node);
      }
    }
    if (d.n > 0) out.push(d.canOpen ? d : d.ch.repeat(d.n));
  }
  return merge(out.map(delimToText));
}

export function renderNodes(nodes: Inline[]): ReactNode[] {
  return nodes.map((n, i) => {
    if (typeof n === "string") return n;
    if (n.t === "code") return <code key={i}>{n.v}</code>;
    if (n.t === "strong") return <strong key={i}>{renderNodes(n.c)}</strong>;
    return <em key={i}>{renderNodes(n.c)}</em>;
  });
}

/** Pure: inline markdown text -> React nodes (text nodes + strong/em/code). */
export function renderInline(text: string): ReactNode[] {
  return renderNodes(parseInline(text));
}
