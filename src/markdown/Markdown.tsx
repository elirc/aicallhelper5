// <Markdown source> — safe streaming renderer. No innerHTML anywhere: every
// string is a React text node, only p/h1-h6/ul/ol/li/pre/code/strong/em (+ the
// wrapper div) are produced, and the only attribute is `class`.
import { Component, memo, useMemo, type ErrorInfo, type ReactNode } from "react";
import { parseBlocks, type Block, type List } from "./parse";
import { renderInline } from "./inline";

function ListView({ list }: { list: List }): ReactNode {
  const items = list.items.map((it, i) => (
    <li key={i}>
      {renderInline(it.text)}
      {it.children.map((c, j) => (
        <ListView key={j} list={c} />
      ))}
    </li>
  ));
  return list.ordered ? <ol>{items}</ol> : <ul>{items}</ul>;
}

function renderBlock(b: Block): ReactNode {
  switch (b.kind) {
    case "paragraph":
      return <p>{renderInline(b.text)}</p>;
    case "heading": {
      const H = `h${b.level}` as const;
      return <H>{renderInline(b.text)}</H>;
    }
    case "list":
      return <ListView list={b.list} />;
    case "code":
      return (
        <pre>
          <code className={b.lang ? `lang-${b.lang}` : undefined}>{b.text}</code>
        </pre>
      );
  }
}

/** One block; re-renders only when its own source text changes. */
const BlockView = memo(
  function BlockView({ block }: { block: Block }) {
    return <>{renderBlock(block)}</>;
  },
  (a, b) => a.block.raw === b.block.raw && a.block.kind === b.block.kind,
);

function MarkdownBody({ source, className }: { source: string; className: string }) {
  const blocks = useMemo(() => parseBlocks(source), [source]);
  return (
    <div className={className}>
      {blocks.map((b, i) => (
        <BlockView key={i} block={b} />
      ))}
    </div>
  );
}

interface BoundaryProps {
  source: string;
  className: string;
  children: ReactNode;
}
interface BoundaryState {
  failed: boolean;
  source: string;
}

/** Falls back to plain text if rendering ever throws; retries on new source. */
export class MarkdownBoundary extends Component<BoundaryProps, BoundaryState> {
  override state: BoundaryState = { failed: false, source: this.props.source };

  static getDerivedStateFromProps(props: BoundaryProps, state: BoundaryState): Partial<BoundaryState> | null {
    return props.source !== state.source ? { failed: false, source: props.source } : null;
  }

  static getDerivedStateFromError(): Partial<BoundaryState> {
    return { failed: true };
  }

  override componentDidCatch(error: unknown, _info: ErrorInfo): void {
    console.error("markdown render failed; showing plain text", error);
  }

  override render(): ReactNode {
    if (this.state.failed) return <div className={`${this.props.className} md-plain`}>{this.props.source}</div>;
    return this.props.children;
  }
}

export function Markdown({ source, className }: { source: string; className?: string }) {
  const cls = className ? `markdown ${className}` : "markdown";
  return (
    <MarkdownBoundary source={source} className={cls}>
      <MarkdownBody source={source} className={cls} />
    </MarkdownBoundary>
  );
}
