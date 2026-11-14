import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { Check, ChevronDown, Moon, Orbit, Sun, Globe2 } from "lucide-react";
import type { AgentModel, ModelFamily } from "./model";
import { SlidingHighlights } from "./sliding-highlights";

export const effortName = (value: string) => ({ low: "Low", medium: "Medium", high: "High", xhigh: "Extra high", max: "Max", ultra: "Ultra" }[value] ?? value);
const families = ["Astra", "Sol", "Terra", "Luna"] as const;
const icons = { Astra: Orbit, Sol: Sun, Terra: Globe2, Luna: Moon };

type Point = [number, number];
// Round the shared silhouette rather than stacking overlapping bordered boxes.
export function roundedOutline(points: Point[]) {
  const unique = points.filter((point, i) => point.some((value, axis) => value !== points[(i + points.length - 1) % points.length][axis]));
  const corners = unique.filter(([x, y], i) => {
    const before = unique[(i + unique.length - 1) % unique.length];
    const after = unique[(i + 1) % unique.length];
    return Math.abs((x - before[0]) * (after[1] - y) - (y - before[1]) * (after[0] - x)) > 0.01;
  });
  return corners.map(([x, y], i) => {
    const before = corners[(i + corners.length - 1) % corners.length];
    const after = corners[(i + 1) % corners.length];
    const incoming = Math.hypot(before[0] - x, before[1] - y);
    const outgoing = Math.hypot(after[0] - x, after[1] - y);
    const radius = Math.min(11, incoming / 2, outgoing / 2);
    return `${i ? "L" : "M"}${x + (before[0] - x) * radius / incoming},${y + (before[1] - y) * radius / incoming} Q${x},${y} ${x + (after[0] - x) * radius / outgoing},${y + (after[1] - y) * radius / outgoing}`;
  }).join(" ") + " Z";
}

export function AgentControls({ models, family, effort, disabled, loading, error, onChange, onRefresh }: {
  models: AgentModel[]; family: ModelFamily; effort: string; disabled: boolean; loading: boolean; error: string;
  onChange: (family: ModelFamily, effort: string) => void; onRefresh: () => void;
}) {
  const [expanded, setExpanded] = useState<"model" | "reasoning">();
  const [openedLabels, setOpenedLabels] = useState({ family, effort: effortName(effort) });
  const [dragging, setDragging] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const modelTrigger = useRef<HTMLButtonElement>(null);
  const modelLabel = useRef<HTMLSpanElement>(null);
  const modelSlot = useRef<HTMLDivElement>(null);
  const modelMenu = useRef<HTMLDivElement>(null);
  const [modelAnchor, setModelAnchor] = useState({ center: 40, width: 68 });
  const [menuHeight, setMenuHeight] = useState(186);
  const reasoningTrigger = useRef<HTMLButtonElement>(null);
  const reasoningLabel = useRef<HTMLSpanElement>(null);
  const reasoningSlot = useRef<HTMLDivElement>(null);
  const readout = useRef<HTMLOutputElement>(null);
  const [anchor, setAnchor] = useState({ center: 24, width: 66 });
  const [readoutWidth, setReadoutWidth] = useState(76);
  const range = useRef<HTMLInputElement>(null);
  const selected = models.find(model => model.family === family);
  const levels = selected?.efforts.length ? selected.efforts : ["low", "medium", "high"];
  const current = Math.max(0, levels.indexOf(effort));
  const [position, setPosition] = useState(current);
  useEffect(() => setPosition(current), [current, family, levels.join(",")]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setExpanded(undefined); };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, []);
  useEffect(() => {
    if (expanded === "reasoning") range.current?.focus();
    if (expanded === "model") root.current?.querySelector<HTMLButtonElement>('[role="option"][aria-selected="true"]')?.focus();
  }, [expanded]);
  const Icon = icons[expanded === "model" ? openedLabels.family : family];
  const maximum = Math.max(1, levels.length - 1);
  const label = effortName(levels[Math.min(levels.length - 1, Math.round(position))]);
  const style = { "--slider-position": position / maximum } as CSSProperties;
  useLayoutEffect(() => {
    const update = () => {
      for (const [textRef, slotRef, triggerRef, set] of [
        [reasoningLabel, reasoningSlot, reasoningTrigger, setAnchor],
        [modelLabel, modelSlot, modelTrigger, setModelAnchor],
      ] as const) {
        if (!textRef.current || !slotRef.current || !triggerRef.current) continue;
        const text = textRef.current.getBoundingClientRect();
        const slot = slotRef.current.getBoundingClientRect();
        set({ center: text.left - slot.left + text.width / 2, width: triggerRef.current.offsetWidth });
      }
    };
    const observer = new ResizeObserver(update);
    if (reasoningTrigger.current) observer.observe(reasoningTrigger.current);
    if (reasoningLabel.current) observer.observe(reasoningLabel.current);
    if (modelTrigger.current) observer.observe(modelTrigger.current);
    if (modelLabel.current) observer.observe(modelLabel.current);
    update();
    return () => observer.disconnect();
  }, []);
  useLayoutEffect(() => {
    if (!readout.current) return;
    const update = () => setReadoutWidth(readout.current!.offsetWidth);
    const observer = new ResizeObserver(update);
    observer.observe(readout.current);
    update();
    return () => observer.disconnect();
  }, [expanded, label, selected?.version]);
  useLayoutEffect(() => {
    const menu = modelMenu.current;
    if (!menu) return;
    const update = () => setMenuHeight(menu.offsetHeight);
    const observer = new ResizeObserver(update);
    observer.observe(menu); update();
    return () => observer.disconnect();
  }, [expanded]);
  const thumbCenter = 24 + 206 * position / maximum;
  const labelLeft = Math.max(0.5, Math.min(253.5 - readoutWidth, thumbCenter - readoutWidth / 2));
  const buttonLeft = 127 - anchor.center - 0.5;
  const outline = roundedOutline([
    [labelLeft, 22], [labelLeft, 0.5], [labelLeft + readoutWidth, 0.5], [labelLeft + readoutWidth, 22],
    [253.5, 22], [253.5, 70], [buttonLeft + anchor.width + 1, 70], [buttonLeft + anchor.width + 1, 107.5],
    [buttonLeft, 107.5], [buttonLeft, 70], [0.5, 70], [0.5, 22],
  ]);
  const close = () => { const focus = expanded === "model" ? modelTrigger : reasoningTrigger; setExpanded(undefined); focus.current?.focus(); };
  const toggle = (control: "model" | "reasoning") => {
    if (expanded === control) { setExpanded(undefined); return; }
    setOpenedLabels({ family, effort: effortName(effort) });
    setExpanded(control);
    if (control === "model") onRefresh();
  };
  const modelButtonLeft = 112 - modelAnchor.center - 0.5;
  const modelOutline = roundedOutline([
    [0.5, 0.5], [223.5, 0.5], [223.5, menuHeight],
    [modelButtonLeft + modelAnchor.width + 1, menuHeight], [modelButtonLeft + modelAnchor.width + 1, menuHeight + 37.5],
    [modelButtonLeft, menuHeight + 37.5], [modelButtonLeft, menuHeight], [0.5, menuHeight],
  ]);
  const commit = (value: number) => { const index = Math.min(levels.length - 1, Math.max(0, Math.round(value))); setPosition(index); onChange(family, levels[index]); };
  return <div className="agent-controls" ref={root} onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); close(); } }}>
    <div className="agent-control-slot model-slot" ref={modelSlot} data-expanded={expanded === "model"}>
      <button type="button" className="composer-choice" ref={modelTrigger} disabled={disabled} aria-haspopup="listbox" aria-expanded={expanded === "model"} aria-label={`Model: ${family}`} onClick={() => toggle("model")} onKeyDown={event => { if (event.key === "ArrowDown" && expanded !== "model") { event.preventDefault(); toggle("model"); } }}><Icon size={15} /><span className="control-label" ref={modelLabel}><span className="control-label-face">{expanded === "model" ? openedLabels.family : family}</span></span><ChevronDown size={12} /></button>
      {expanded === "model" && <div className="model-popover" style={{ left: modelAnchor.center, height: menuHeight + 38 }}>
      <svg className="reasoning-outline" width="224" height={menuHeight + 38} aria-hidden="true"><path className="reasoning-shell" d={modelOutline} /></svg>
      <div className="model-menu" ref={modelMenu} role="listbox" aria-label="Model family" onKeyDown={event => {
        if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
        event.preventDefault(); const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]:not(:disabled)'));
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
        buttons[next]?.focus();
      }}>
        <div className="menu-caption" aria-busy={loading}>Model</div>
        <SlidingHighlights className="model-highlights" selectedKey={family}>{families.map(name => { const model = models.find(item => item.family === name); const FamilyIcon = icons[name]; return <button type="button" key={name} data-highlight-key={name} role="option" aria-selected={family === name} disabled={!model?.modelId} onClick={() => { onChange(name, model?.efforts.includes(effort) ? effort : model?.efforts.includes("high") ? "high" : model?.efforts[0] ?? "high"); close(); }}><FamilyIcon size={16} /><span>{name}</span>{family === name ? <Check size={13} /> : <i />}</button>; })}</SlidingHighlights>
        {error && <div className="model-error" role="alert">{error}<button type="button" onClick={onRefresh}>Retry</button></div>}
      </div></div>}
    </div>
    <div className="agent-control-slot reasoning-slot" ref={reasoningSlot} data-expanded={expanded === "reasoning"}>
      <button type="button" className="composer-choice" ref={reasoningTrigger} disabled={disabled || !selected?.modelId} aria-expanded={expanded === "reasoning"} aria-controls="reasoning-control" aria-label={`Reasoning: ${effortName(effort)}`} onClick={() => toggle("reasoning")}><span className="control-label" ref={reasoningLabel}><span className="control-label-face">{expanded === "reasoning" ? openedLabels.effort : effortName(effort)}</span></span><ChevronDown size={12} /></button>
      {expanded === "reasoning" && <div id="reasoning-control" className={`reasoning-popover ${dragging ? "dragging" : ""}`} style={{ left: anchor.center }}>
        <svg className="reasoning-outline" width="254" height="108" aria-hidden="true"><path className="reasoning-shell" d={outline} /></svg>
        <output ref={readout} className="reasoning-readout" style={{ left: labelLeft }} aria-hidden="true"><b>{selected?.version ?? "—"}</b><span>{label}</span></output>
        <div className="reasoning-slider" style={style}>
          <div className="reasoning-track" aria-hidden="true"><div className="reasoning-fill" />{levels.map((level, index) => <i key={level} className={index <= position ? "filled" : ""} style={{ left: `calc(16px + (100% - 32px) * ${index / maximum})` }} />)}</div>
          <div className="reasoning-thumb" aria-hidden="true"><i /></div>
          <input ref={range} type="range" aria-label="Reasoning effort" aria-valuetext={`${family} ${selected?.version ?? ""}, ${label}`} min={0} max={maximum} step={0.01} value={position} onChange={event => { const value = Number(event.target.value); if (dragging) setPosition(value); else commit(value); }}
            onPointerDown={event => { event.currentTarget.setPointerCapture(event.pointerId); setDragging(true); }}
            onPointerUp={event => { setDragging(false); commit(Number(event.currentTarget.value)); }}
            onPointerCancel={() => { setDragging(false); setPosition(current); }}
            onBlur={() => { if (!dragging && position !== current) commit(position); }}
            onKeyDown={event => { const delta = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0; if (delta || event.key === "Home" || event.key === "End") { event.preventDefault(); commit(event.key === "Home" ? 0 : event.key === "End" ? maximum : Math.round(position) + delta); } }} />
        </div>
      </div>}
    </div>
  </div>;
}
