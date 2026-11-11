import { Check, ChevronRight, FileCode2, LoaderCircle, Terminal, X } from "lucide-react";

export type ToolItem = { id: string; type: string; command?: string; status?: string; exitCode?: number; aggregatedOutput?: string; changes?: { path: string; diff?: string }[] };
export type ToolActivity = ToolItem & { complete: boolean };
export function ToolCalls({ items }: { items: ToolActivity[] }) {
  if (!items.length) return null;
  return <div className="tool-timeline" aria-label="Agent tool calls">{items.map(item => {
    const changed = item.type === "fileChange";
    const failed = item.exitCode != null && item.exitCode !== 0 || item.status === "failed";
    const label = changed ? `Changed ${item.changes?.length ?? 1} ${(item.changes?.length ?? 1) === 1 ? "file" : "files"}` : item.type === "commandExecution" ? "Ran command" : item.type.replace(/([A-Z])/g, " $1").replace(/^./, c => c.toUpperCase());
    return <details key={item.id} className={`tool-call ${failed ? "failed" : ""}`}><summary><ChevronRight size={13} />{changed ? <FileCode2 size={14} /> : <Terminal size={14} />}<span>{label}</span>{!item.complete ? <LoaderCircle size={12} className="spin" /> : failed ? <X size={12} className="tool-error" /> : <Check size={12} className="tool-success" />}</summary>
      <div className="tool-call-body">{item.command && <code>{item.command}</code>}{item.changes?.map(change => <details key={change.path}><summary>{change.path.replaceAll("\\", "/").split("/").slice(-2).join("/")}</summary>{change.diff && <pre>{change.diff}</pre>}</details>)}{item.aggregatedOutput && <pre>{item.aggregatedOutput.slice(-12000)}</pre>}</div>
    </details>;
  })}</div>;
}
