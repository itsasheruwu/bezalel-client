import Markdown from "react-markdown";

export function MessageBody({ text, onExport, disabled }: { text: string; onExport?: () => void; disabled?: boolean }) {
  return <div className="bubble markdown"><Markdown skipHtml components={{
    a: ({ href, children }) => href && /^https?:\/\//i.test(href)
      ? <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>
      : href?.endsWith(".jar") && onExport
        ? <button className="message-link" type="button" title="Export current verified build" disabled={disabled} onClick={onExport}>{children}</button>
        : <code>{children}</code>,
    img: ({ alt }) => <span>{alt}</span>,
  }}>{text}</Markdown></div>;
}
