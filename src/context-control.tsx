import { listen } from "@tauri-apps/api/event";
import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { roundedOutline } from "./agent-controls";
import type { ContextUsage } from "./model";

// Matching cubic segments let the browser morph the same track and fill,
// including when the user reverses a transition before it finishes.
const arc = "M222,257 C222,244.85 231.85,235 244,235 C256.15,235 266,244.85 266,257";
const bar = "M18,49 C59.33,49 100.67,49 142,49 C183.33,49 224.67,49 266,49";
const outline = roundedOutline([
  [0.5, 0.5], [283.5, 0.5], [283.5, 224], [276.5, 224],
  [276.5, 261.5], [211.5, 261.5], [211.5, 224], [0.5, 224],
]);
const tokens = new Intl.NumberFormat();
// The protocol reports the window, but not the resolved compaction threshold.
// This approximate readout must never change the agent's compaction policy.
const compactRatio = 0.9;

export function ContextControl({ draftId, modelId, usage: savedUsage }: {
  draftId: string; modelId?: string | null; usage?: ContextUsage | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const [liveUsage, setLiveUsage] = useState<ContextUsage>();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const titleId = useId();
  const candidate = liveUsage ?? savedUsage;
  const usage = candidate?.modelId === modelId ? candidate : undefined;
  const used = usage?.usedTokens;
  const capacity = usage?.windowTokens;
  const fraction = used !== undefined && capacity ? Math.min(1, Math.max(0, used / capacity)) : undefined;
  const percent = fraction === undefined ? "—" : `${Math.round(fraction * 100)}%`;
  const compactAt = capacity ? Math.floor(capacity * compactRatio) : undefined;
  const remaining = compactAt !== undefined && used !== undefined ? Math.max(0, compactAt - used) : undefined;

  useEffect(() => {
    const subscription = listen<{ draftId: string; contextUsage: ContextUsage }>("studio-context", event => {
      if (event.payload.draftId === draftId) setLiveUsage(event.payload.contextUsage);
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [draftId]);
  useEffect(() => {
    if (!expanded) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setExpanded(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [expanded]);

  return <div className="agent-control-slot context-control" ref={root} data-expanded={expanded}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setExpanded(false); }}
    onKeyDown={event => {
      if (event.key === "Escape" && expanded) { event.preventDefault(); event.stopPropagation(); setExpanded(false); trigger.current?.focus(); }
    }}>
    <button type="button" className="composer-choice context-trigger" ref={trigger} aria-haspopup="dialog" aria-expanded={expanded} aria-controls={panelId}
      aria-label={`Context window: ${fraction === undefined ? "usage unavailable" : `${percent} used`}`}
      onClick={() => setExpanded(value => !value)}
      onKeyDown={event => { if (event.key === "ArrowUp") { event.preventDefault(); setExpanded(true); requestAnimationFrame(() => panel.current?.focus({ preventScroll: true })); } }}>
      <span className="context-caption" aria-hidden="true">Context</span><span className="context-percent" aria-hidden="true">{percent}</span>
    </button>
    <div id={panelId} ref={panel} className="context-popover" role="dialog" aria-labelledby={titleId} aria-hidden={!expanded} inert={!expanded} tabIndex={-1}>
      <svg className="reasoning-outline" width="284" height="262" aria-hidden="true"><path className="reasoning-shell" d={outline} /></svg>
      <div className="context-details">
        <h2 id={titleId}>Context window</h2>
        <div className="context-bar-space" />
        <dl>
          <div><dt>Tokens used</dt><dd>{used === undefined ? "Unavailable" : tokens.format(used)}</dd></div>
          <div><dt>Window size</dt><dd>{capacity ? `${tokens.format(capacity)} tokens` : "Unavailable"}</dd></div>
          <div><dt>Context used</dt><dd>{fraction === undefined ? "Unavailable" : percent}</dd></div>
        </dl>
        <p className="context-compact"><strong>Auto-compact · estimate</strong><span>{compactAt === undefined || remaining === undefined
          ? "Available after the agent reports context usage."
          : remaining > 0 ? `Around 90% (${tokens.format(compactAt)} tokens), about ${tokens.format(remaining)} tokens from now.`
            : "Near the estimated compact point; the agent may compact soon."}</span></p>
        <p className="context-footnote">{usage ? "Compaction summarizes older context. Timing varies by model." : "Usage updates when this model runs in the thread."}</p>
      </div>
    </div>
    <svg className="context-gauge" width="284" height="262" aria-hidden="true" style={{ "--context-shape": `path('${expanded ? bar : arc}')` } as CSSProperties}>
      <path className="context-track" d={expanded ? bar : arc} pathLength="100" />
      <path className="context-fill" d={expanded ? bar : arc} pathLength="100" strokeDasharray={`${(fraction ?? 0) * 100} 100`} opacity={fraction ? 1 : 0} />
    </svg>
  </div>;
}
