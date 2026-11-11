import { useEffect, useId, useLayoutEffect, useRef, useState, type FocusEvent } from "react";
import { createPortal } from "react-dom";
import { ArrowLeft, ArrowRight, FolderInput, LoaderCircle, Trash2 } from "lucide-react";
import { roundedOutline } from "./agent-controls";
import type { Pack } from "./model";

type View = "actions" | "move" | "delete";

export function InstalledModControl({ installed, disabled, canInstall, packName, packs, onInstall, onMove, onDelete }: {
  installed: boolean; disabled: boolean; canInstall: boolean; packName: string; packs: Pack[];
  onInstall: () => Promise<unknown>; onMove: (pack: Pack) => Promise<unknown>; onDelete: () => Promise<unknown>;
}) {
  const [view, setView] = useState<View>();
  const [pending, setPending] = useState(false);
  const request = useRef(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const id = useId();
  const [layout, setLayout] = useState({ left: 0, top: 0, height: 120, buttonLeft: 218, buttonWidth: 32, buttonHeight: 30, below: false });
  const close = (restoreFocus = false) => { setView(undefined); if (restoreFocus) trigger.current?.focus({ preventScroll: true }); };
  const perform = async (action: () => Promise<unknown>) => {
    if (request.current || disabled) return;
    request.current = true; setPending(true); close();
    try { await action(); }
    finally { request.current = false; setPending(false); }
  };
  useEffect(() => { if (!installed) setView(undefined); }, [installed]);
  useEffect(() => {
    if (!view) return;
    const outside = (event: PointerEvent) => {
      const node = event.target as Node;
      if (!root.current?.contains(node) && !popover.current?.contains(node)) close();
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [view]);
  useLayoutEffect(() => {
    if (!view) return;
    const update = () => {
      const button = trigger.current?.getBoundingClientRect();
      if (!button || !menu.current) return;
      const height = menu.current.offsetHeight;
      const left = Math.max(8, Math.min(window.innerWidth - 268, button.right + 8 - 260));
      const below = button.top < height + 16;
      setLayout({ left, top: below ? button.top : button.bottom - (height + button.height + 8), height,
        buttonLeft: button.left - left, buttonWidth: button.width, buttonHeight: button.height, below });
    };
    const observer = new ResizeObserver(update);
    if (menu.current) observer.observe(menu.current);
    if (trigger.current) observer.observe(trigger.current);
    update();
    menu.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus({ preventScroll: true });
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => { observer.disconnect(); window.removeEventListener("resize", update); window.removeEventListener("scroll", update, true); };
  }, [view]);

  const { height, buttonLeft: x, buttonWidth: width, buttonHeight, below } = layout;
  const totalHeight = height + buttonHeight + 8;
  const outline = roundedOutline(below ? [
    [x - .5, .5], [x + width + .5, .5], [x + width + .5, buttonHeight + 8], [259.5, buttonHeight + 8],
    [259.5, totalHeight - .5], [.5, totalHeight - .5], [.5, buttonHeight + 8], [x - .5, buttonHeight + 8],
  ] : [
    [.5, .5], [259.5, .5], [259.5, height], [x + width + .5, height],
    [x + width + .5, totalHeight - .5], [x - .5, totalHeight - .5], [x - .5, height], [.5, height],
  ]);
  const blur = (event: FocusEvent) => {
    const next = event.relatedTarget as Node | null;
    if (next) {
      if (!root.current?.contains(next) && !popover.current?.contains(next)) close();
    } else {
      // Replacing a submenu removes its focused item. Let the layout effect
      // focus the new view before deciding that focus left the whole control.
      queueMicrotask(() => {
        const active = document.activeElement;
        if (!root.current?.contains(active) && !popover.current?.contains(active)) close();
      });
    }
  };
  return <div className="installed-mod-control" ref={root} data-expanded={!!view} onBlur={blur}>
    <button type="button" ref={trigger} className="primary installation-trigger" data-installed={installed} data-pending={pending} data-expanded={!!view}
      disabled={disabled || pending || (!installed && !canInstall)} aria-label={installed ? `Installed in ${packName}. Mod actions` : "Install mod"}
      aria-haspopup={installed ? "menu" : undefined} aria-expanded={installed ? !!view : undefined} aria-controls={view ? id : undefined} aria-busy={pending}
      onClick={() => installed ? setView(value => value ? undefined : "actions") : void perform(onInstall)}
      onKeyDown={event => { if (installed && ["ArrowDown", "ArrowUp"].includes(event.key)) { event.preventDefault(); setView("actions"); } }}>
      <span className="installation-label" aria-hidden="true">Install</span>
      <svg className="installation-check" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m5 12 4 4 10-10" pathLength="1" /></svg>
      {pending && <LoaderCircle className="installation-spinner spin" size={15} aria-hidden="true" />}
    </button>
    {view && createPortal(<div ref={popover} className="installation-popover" data-below={below} style={{ left: layout.left, top: layout.top, height: totalHeight, transformOrigin: `${x + width / 2}px ${below ? "0" : "100%"}` }} onBlur={blur}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (view !== "actions") setView("actions"); else close(true); }
        if (event.key === "Tab") { close(true); return; }
        if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const items = Array.from(menu.current!.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
        const current = items.indexOf(document.activeElement as HTMLButtonElement);
        items[event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
      }}>
      <svg className="reasoning-outline" width="260" height={totalHeight} aria-hidden="true"><path className="reasoning-shell" d={outline} /></svg>
      <div id={id} ref={menu} className="installed-menu" role="menu" aria-label="Installed mod actions" style={{ top: below ? buttonHeight + 8 : 0 }}>
        {view === "actions" ? <>
          <p className="installed-menu-caption">Installed in <strong>{packName}</strong></p>
          <button type="button" role="menuitem" onClick={() => setView("move")}><FolderInput size={15} /><span>Move to another modpack</span><ArrowRight size={13} /></button>
          <div className="installed-menu-divider" role="separator" />
          <button type="button" role="menuitem" className="installed-menu-danger" onClick={() => setView("delete")}><Trash2 size={15} /><span>Delete mod</span></button>
        </> : <>
          <button type="button" role="menuitem" className="installed-menu-back" onClick={() => setView("actions")}><ArrowLeft size={14} /><span>{view === "move" ? "Move to modpack" : "Delete installed mod?"}</span></button>
          {view === "move" ? <div className="installed-pack-list">{packs.length ? packs.map(pack => <button type="button" role="menuitem" key={pack.id} aria-label={`Move to ${pack.name}`} onClick={() => void perform(() => onMove(pack))}><FolderInput size={15} /><span>{pack.name}</span></button>) : <p className="installed-menu-note">No other local modpacks use this loader and Minecraft version.</p>}</div> : <>
            <p className="installed-menu-note">Remove it from <strong>{packName}</strong>. Your conversation and source stay here.</p>
            <button type="button" role="menuitem" className="installed-menu-danger" onClick={() => void perform(onDelete)}><Trash2 size={15} /><span>Delete mod</span></button>
          </>}
        </>}
      </div>
    </div>, document.body)}
  </div>;
}
