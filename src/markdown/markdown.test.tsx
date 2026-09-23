/// <reference types="vite/client" />
import { describe, expect, it, vi } from "vitest";
import fc from "fast-check";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown, MarkdownBoundary, parseBlocks, parseInline } from "./index";

const html = (src: string) => renderToStaticMarkup(<Markdown source={src} />);
const body = (src: string) => html(src).replace(/^<div class="markdown">/, "").replace(/<\/div>$/, "");

describe("golden cases", () => {
  it("paragraphs split on blank lines; soft breaks stay in one paragraph", () => {
    expect(body("one\ntwo\n\nthree")).toBe("<p>one\ntwo</p><p>three</p>");
  });

  it("ATX headings h1..h6, closing hashes stripped; #tag is not a heading", () => {
    expect(body("# A\n## B ##\n###### F\n####### G")).toBe("<h1>A</h1><h2>B</h2><h6>F</h6><p>####### G</p>");
    expect(body("#tag")).toBe("<p>#tag</p>");
  });

  it("bullet lists with -, *, +", () => {
    expect(body("- a\n* b\n+ c")).toBe("<ul><li>a</li><li>b</li><li>c</li></ul>");
  });

  it("numbered lists", () => {
    expect(body("1. one\n2) two")).toBe("<ol><li>one</li><li>two</li></ol>");
  });

  it("nested lists by indent", () => {
    expect(body("- a\n  - a1\n  - a2\n- b\n    1. deep")).toBe(
      "<ul><li>a<ul><li>a1</li><li>a2</li></ul></li><li>b<ol><li>deep</li></ol></li></ul>",
    );
  });

  it("indented continuation lines join the item", () => {
    expect(body("- first\n  continued")).toBe("<ul><li>first continued</li></ul>");
  });

  it("bold, italic, inline code", () => {
    expect(body("**b** __b__ *i* _i_ `c`")).toBe(
      "<p><strong>b</strong> <strong>b</strong> <em>i</em> <em>i</em> <code>c</code></p>",
    );
    expect(body("***both***")).toBe("<p><em><strong>both</strong></em></p>");
    expect(body("*a **b** c*")).toBe("<p><em>a <strong>b</strong> c</em></p>");
  });

  it("intraword underscores and lone stars stay literal", () => {
    expect(body("snake_case_name and 2 * 3 * 4")).toBe("<p>snake_case_name and 2 * 3 * 4</p>");
  });

  it("code spans protect their content; backslash escapes", () => {
    expect(body("`**not bold**` \\*lit\\*")).toBe("<p><code>**not bold**</code> *lit*</p>");
    expect(body("`` a ` b ``")).toBe("<p><code>a ` b</code></p>");
  });

  it("fenced code blocks keep content verbatim with a language class", () => {
    expect(body("```ts\nconst a = **1**;\n<b>x</b>\n```\nafter")).toBe(
      '<pre><code class="lang-ts">const a = **1**;\n&lt;b&gt;x&lt;/b&gt;</code></pre><p>after</p>',
    );
    expect(body("~~~\nx\n~~~")).toBe("<pre><code>x</code></pre>");
  });

  it("an unterminated fence (streaming) renders as a code block so far", () => {
    expect(body("text\n\n```py\nprint(1)\nprint(")).toBe('<p>text</p><pre><code class="lang-py">print(1)\nprint(</code></pre>');
    expect(parseBlocks("```\nx")[0]).toMatchObject({ kind: "code", closed: false });
  });

  it("links are NOT rendered: [x](y) stays literal text", () => {
    expect(body("see [docs](https://example.com) now")).toBe("<p>see [docs](https://example.com) now</p>");
  });

  it("a list interrupts a paragraph; a heading ends a list", () => {
    expect(body("Points:\n- a\n# H")).toBe("<p>Points:</p><ul><li>a</li></ul><h1>H</h1>");
  });

  it("custom className is appended to the wrapper", () => {
    expect(renderToStaticMarkup(<Markdown source="x" className="answer-md" />)).toBe('<div class="markdown answer-md"><p>x</p></div>');
  });

  it("parseInline is pure and merges adjacent text", () => {
    expect(parseInline("a*b")).toEqual(["a*b"]);
    expect(parseInline("**x")).toEqual(["**x"]);
  });
});

// ── XSS corpus ──
const ALLOWED = new Set(["div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "pre", "code", "strong", "em"]);

function assertSafe(markup: string) {
  const doc = new DOMParser().parseFromString(`<body>${markup}</body>`, "text/html");
  for (const el of Array.from(doc.body.querySelectorAll("*"))) {
    expect(ALLOWED.has(el.tagName.toLowerCase()), `element <${el.tagName}>`).toBe(true);
    for (const attr of Array.from(el.attributes)) expect(attr.name, `attribute on <${el.tagName}>`).toBe("class");
  }
}

/** Fast equivalent for hot loops: strip every allowed tag (class-only); any
 * remaining "<" would be a disallowed element (text is always escaped). */
function assertSafeFast(markup: string) {
  const rest = markup.replace(/<\/?(div|p|h[1-6]|ul|ol|li|pre|code|strong|em)( class="[^"<>]*")?>/g, "");
  expect(rest).not.toContain("<");
}

const XSS = [
  "<script>alert(1)</script>",
  "<img src=x onerror=alert(1)>",
  "[click](javascript:alert(1))",
  "<iframe src='https://evil'></iframe>",
  "&lt;script&gt; &amp; &#60;img&#62; &quot;",
  "‮evil‬ text",
  "**<b onclick=x>bold</b>**",
  "`<script>`",
  "```html\n<script>alert(1)</script>\n```",
  "```\"><img src=x onerror=alert(1)>\ncode\n```",
  "- <a href=javascript:x>li</a>\n  - <svg onload=alert(1)>",
  "# <style>body{display:none}</style>",
  "<div style='x'>*em*</div>",
  "javascript:alert(document.cookie)",
  "![img](x onerror=alert(1))",
];

describe("XSS corpus", () => {
  it.each(XSS)("renders %j with only allowed elements and class attributes", (src) => {
    const markup = html(src);
    assertSafe(markup);
    assertSafeFast(markup);
    expect(markup).not.toMatch(/<(script|img|iframe|a|svg|style|b)\b/i);
  });

  it("dangerous text survives as visible text (not dropped, not executed)", () => {
    const div = document.createElement("div");
    div.innerHTML = html("<script>alert(1)</script>");
    expect(div.textContent).toBe("<script>alert(1)</script>");
    expect(div.querySelector("script")).toBeNull();
  });

  it("the source never uses innerHTML / dangerouslySetInnerHTML", async () => {
    const files = import.meta.glob(["./*.ts", "./*.tsx", "!./*.test.tsx"], { query: "?raw", import: "default", eager: true });
    const sources = Object.values(files) as string[];
    expect(sources.length).toBeGreaterThanOrEqual(3);
    for (const src of sources) {
      expect(src).not.toContain("dangerouslySetInnerHTML");
      expect(src).not.toMatch(/\.innerHTML\s*=/);
    }
  });
});

// ── streaming property test ──
const TOKENS = [
  "**", "*", "_", "__", "`", "```", "~~~", "\n", "\n\n", "# ", "## ", "- ", "* ", "1. ", "  ", "    ",
  "a", "b", "word ", "x_y", " ", "\\*", "[x](y)", "<script>", "&amp;", "ts\n", "\t",
];
const mdString = fc.array(fc.constantFrom(...TOKENS), { maxLength: 40 }).map((a) => a.join(""));

function chunksOf(src: string, cuts: number[]): string[] {
  const points = [...new Set(cuts.map((c) => c % (src.length + 1)))].sort((a, b) => a - b);
  const out: string[] = [];
  let prev = 0;
  for (const p of points) {
    out.push(src.slice(prev, p));
    prev = p;
  }
  out.push(src.slice(prev));
  return out;
}

describe("streaming property", () => {
  it("every streamed prefix renders safely, and the concatenation renders exactly like the whole", () => {
    fc.assert(
      fc.property(mdString, fc.array(fc.nat(), { maxLength: 6 }), (src, cuts) => {
        let acc = "";
        for (const c of chunksOf(src, cuts)) {
          acc += c;
          assertSafeFast(html(acc)); // intermediate paints never crash or emit unsafe markup
        }
        expect(html(acc)).toBe(html(src));
      }),
      { numRuns: 300 },
    );
  });

  it("a live component updated chunk-by-chunk renders the same DOM as rendering the whole at once", () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    // act() for synchronous commits in the test env.
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      fc.assert(
        fc.property(mdString, fc.array(fc.nat(), { minLength: 1, maxLength: 8 }), (src, cuts) => {
          let acc = "";
          act(() => root.render(createElement(Markdown, { source: "" })));
          for (const c of chunksOf(src, cuts)) {
            acc += c;
            act(() => root.render(createElement(Markdown, { source: acc })));
          }
          expect(container.innerHTML).toBe(html(src));
        }),
        { numRuns: 200 },
      );
    } finally {
      act(() => root.unmount());
    }
  });

  it("per-block memoization: finished blocks are not re-rendered while the last block streams", () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    act(() => root.render(<Markdown source={"# Title\n\nfirst para\n\nsec"} />));
    const h1 = container.querySelector("h1");
    const firstText = container.querySelectorAll("p")[0]?.firstChild;
    act(() => root.render(<Markdown source={"# Title\n\nfirst para\n\nsecond **para**"} />));
    expect(container.querySelector("h1")).toBe(h1);
    expect(container.querySelectorAll("p")[0]?.firstChild).toBe(firstText);
    expect(container.innerHTML).toBe(html("# Title\n\nfirst para\n\nsecond **para**"));
    act(() => root.unmount());
  });
});

describe("error boundary", () => {
  it("falls back to plain text when rendering throws, and retries on new source", () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    function Boom({ source }: { source: string }): never {
      throw new Error(`boom ${source}`);
    }
    const container = document.createElement("div");
    const root = createRoot(container);
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    act(() =>
      root.render(
        <MarkdownBoundary source="**x** <b>" className="markdown">
          <Boom source="x" />
        </MarkdownBoundary>,
      ),
    );
    expect(container.innerHTML).toBe('<div class="markdown md-plain">**x** &lt;b&gt;</div>');
    act(() =>
      root.render(
        <MarkdownBoundary source="ok" className="markdown">
          <p>ok</p>
        </MarkdownBoundary>,
      ),
    );
    expect(container.innerHTML).toBe("<p>ok</p>");
    act(() => root.unmount());
    err.mockRestore();
  });
});
