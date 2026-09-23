// Block-level parser for the small safe Markdown subset (spec §13):
// paragraphs, ATX headings, bullet/numbered lists (simple nesting by indent),
// fenced code blocks (an unterminated fence renders as code so far).
// Pure: every Block is a function of its own `raw` source lines, which is what
// makes per-block memoization safe while text streams in.

export interface List {
  ordered: boolean;
  items: ListItem[];
}

export interface ListItem {
  text: string;
  children: List[];
}

export type Block =
  | { kind: "paragraph"; raw: string; text: string }
  | { kind: "heading"; raw: string; level: 1 | 2 | 3 | 4 | 5 | 6; text: string }
  | { kind: "list"; raw: string; list: List }
  | { kind: "code"; raw: string; lang: string; text: string; closed: boolean };

const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])(?:[ \t]+(.*))?$/;

function expandTabs(line: string): string {
  // Leading tabs count as 4 spaces (indentation only).
  const m = /^[ \t]+/.exec(line);
  if (!m || !m[0].includes("\t")) return line;
  return m[0].replace(/\t/g, "    ") + line.slice(m[0].length);
}

const isBlank = (l: string) => l.trim() === "";

function fenceClose(line: string, ch: string, len: number): boolean {
  const m = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line);
  return !!m && m[1] !== undefined && m[1][0] === ch && m[1].length >= len;
}

function sanitizeLang(s: string): string {
  return (s.match(/^[A-Za-z0-9_+#.-]{1,24}/)?.[0] ?? "").replace(/[^A-Za-z0-9_+-]/g, "");
}

function startsBlock(line: string): boolean {
  return FENCE_OPEN.test(line) || HEADING.test(line) || LIST_ITEM.test(line);
}

export function parseBlocks(source: string): Block[] {
  const lines = source.replace(/\r\n?/g, "\n").split("\n").map(expandTabs);
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (isBlank(line)) {
      i++;
      continue;
    }

    const fence = FENCE_OPEN.exec(line);
    if (fence && fence[1] !== undefined) {
      const ch = fence[1][0] ?? "`";
      const len = fence[1].length;
      const start = i;
      const body: string[] = [];
      let closed = false;
      i++;
      while (i < lines.length) {
        const l = lines[i] ?? "";
        if (fenceClose(l, ch, len)) {
          closed = true;
          i++;
          break;
        }
        body.push(l);
        i++;
      }
      blocks.push({
        kind: "code",
        raw: lines.slice(start, i).join("\n"),
        lang: sanitizeLang(fence[2] ?? ""),
        text: body.join("\n"),
        closed,
      });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading && heading[1] !== undefined) {
      const text = (heading[2] ?? "").replace(/(^|[ \t]+)#+$/, "").trim();
      blocks.push({ kind: "heading", raw: line, level: heading[1].length as 1 | 2 | 3 | 4 | 5 | 6, text });
      i++;
      continue;
    }

    if (LIST_ITEM.test(line)) {
      const start = i;
      const list = parseList(lines, i);
      i = list.end;
      blocks.push({ kind: "list", raw: lines.slice(start, i).join("\n"), list: list.list });
      continue;
    }

    // Paragraph: until a blank line or the start of another block.
    const start = i;
    const text: string[] = [];
    while (i < lines.length) {
      const l = lines[i] ?? "";
      if (isBlank(l) || (i > start && startsBlock(l))) break;
      text.push(l.trim());
      i++;
    }
    blocks.push({ kind: "paragraph", raw: lines.slice(start, i).join("\n"), text: text.join("\n") });
  }
  return blocks;
}

interface Level {
  indent: number;
  list: List;
}

function parseList(lines: string[], from: number): { list: List; end: number } {
  const root: List = { ordered: false, items: [] };
  const stack: Level[] = [];
  let lastItem: ListItem | null = null;
  let i = from;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (isBlank(line)) break;
    const m = LIST_ITEM.exec(line);
    if (!m) {
      // Indented continuation of the previous item; anything else ends the list.
      if (lastItem && /^ +\S/.test(line) && !FENCE_OPEN.test(line) && !HEADING.test(line)) {
        lastItem.text = lastItem.text ? `${lastItem.text} ${line.trim()}` : line.trim();
        i++;
        continue;
      }
      break;
    }
    const indent = (m[1] ?? "").length;
    const ordered = /\d/.test(m[2] ?? "");
    const item: ListItem = { text: (m[3] ?? "").trim(), children: [] };
    if (stack.length === 0) {
      root.ordered = ordered;
      stack.push({ indent, list: root });
    } else {
      while (stack.length > 1 && indent < (stack[stack.length - 1] as Level).indent) stack.pop();
      const top = stack[stack.length - 1] as Level;
      const parentItem = top.list.items[top.list.items.length - 1];
      if (indent >= top.indent + 2 && parentItem) {
        const child: List = { ordered, items: [] };
        parentItem.children.push(child);
        stack.push({ indent, list: child });
      }
    }
    (stack[stack.length - 1] as Level).list.items.push(item);
    lastItem = item;
    i++;
  }
  return { list: root, end: i };
}
