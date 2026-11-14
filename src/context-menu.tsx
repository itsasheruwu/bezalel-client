import { cloneElement, createContext, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState, type HTMLAttributes, type ReactElement, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Copy, Scissors, ClipboardPaste, TextSelect, Code, Link, ExternalLink, type LucideIcon } from "lucide-react";
import { connectedMenuShape, menuSurfacePath, type MenuShape } from "./context-menu-shape";
import { SlidingHighlights } from "./sliding-highlights";

export type ContextAction = { id: string; label: string; icon?: LucideIcon; shortcut?: string; disabled?: boolean; danger?: boolean; separator?: boolean; run: () => void | Promise<unknown> };
type OpenEvent = { preventDefault: () => void; stopPropagation: () => void; target: EventTarget | null; currentTarget: EventTarget | null; clientX?: number; clientY?: number; type: string };
type MenuRequest = { id: number; x: number; y: number; source: HTMLElement; origin: DOMRect; focus: HTMLElement | null; label: string; actions: () => ContextAction[] };
const MenuContext = createContext<(event: OpenEvent, actions?: () => ContextAction[], label?: string) => void>(() => {});
const textField = (node: Element) => node.closest("textarea, input") as HTMLInputElement | HTMLTextAreaElement | null;
const supportedInput = (field: HTMLInputElement | HTMLTextAreaElement) => field instanceof HTMLTextAreaElement || ["text", "search", "url", "tel", "password"].includes(field.type);
const copy = (text: string) => navigator.clipboard.writeText(text);

function editActions(field: HTMLInputElement | HTMLTextAreaElement): ContextAction[] {
  if (field.disabled) return [];
  // These native controls do not expose a caret range. Never guess where to paste.
  if (!supportedInput(field)) return field instanceof HTMLInputElement && ["email", "number"].includes(field.type)
    ? [{ id: "copy-value", label: "Copy value", icon: Copy, disabled: !field.value, run: () => copy(field.value) }] : [];
  const start = field.selectionStart ?? 0, end = field.selectionEnd ?? 0;
  const value = field.value, selected = value.slice(start, end);
  const secret = field instanceof HTMLInputElement && field.type === "password";
  const writable = !field.readOnly;
  const restore = () => {
    if (!field.isConnected || field.disabled || field.value !== value) throw new Error("The field changed. Open its menu again.");
    field.focus({ preventScroll: true });
    if (field.selectionStart !== null) field.setSelectionRange(start, end);
  };
  const insert = (text: string) => {
    if (field.readOnly) throw new Error("This field is read-only.");
    restore();
    const limit = field.maxLength >= 0 ? Math.max(0, field.maxLength - (value.length - (end - start))) : text.length;
    const replacement = text.slice(0, limit);
    // Native insertion preserves the browser's edit/undo history and React input events.
    if (document.execCommand?.("insertText", false, replacement)) return;
    const next = value.slice(0, start) + replacement + value.slice(end);
    const prototype = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(field, next);
    if (field.selectionStart !== null) field.setSelectionRange(start + replacement.length, start + replacement.length);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  };
  return [
    ...(writable ? [{ id: "cut", label: "Cut", icon: Scissors, shortcut: "Ctrl+X", disabled: !selected || secret, run: async () => { restore(); await copy(selected); insert(""); } }] : []),
    { id: "copy", label: "Copy", icon: Copy, shortcut: "Ctrl+C", disabled: !selected || secret, run: () => copy(selected) },
    ...(writable ? [{ id: "paste", label: "Paste", icon: ClipboardPaste, shortcut: "Ctrl+V", disabled: !navigator.clipboard?.readText, run: async () => {
      restore();
      const text = await navigator.clipboard.readText();
      if (document.activeElement !== field || field.value !== value || (field.selectionStart !== null && (field.selectionStart !== start || field.selectionEnd !== end))) throw new Error("The field changed before paste finished. Try again.");
      insert(text);
    } }] : []),
    { id: "select-all", label: "Select all", icon: TextSelect, shortcut: "Ctrl+A", separator: true, disabled: !value, run: () => { restore(); field.select(); } },
  ];
}

export function contextualActions(target: Element, scoped?: () => ContextAction[]): (() => ContextAction[]) | undefined {
  const field = textField(target);
  if (field) return () => editActions(field);
  const selection = window.getSelection();
  // A selection elsewhere in the window must not replace this item's actions.
  if (selection && !selection.isCollapsed && selection.rangeCount && selection.getRangeAt(0).intersectsNode(target)) {
    const text = selection.toString();
    if (text) return () => [{ id: "copy-selection", label: "Copy", icon: Copy, shortcut: "Ctrl+C", run: () => copy(text) }];
  }
  const link = target.closest<HTMLAnchorElement>("a[href]");
  if (link && /^https?:\/\//i.test(link.href)) return () => [
    { id: "open-link", label: "Open link", icon: ExternalLink, run: () => { if (link.isConnected && /^https?:\/\//i.test(link.href)) link.click(); } },
    { id: "copy-link", label: "Copy link address", icon: Link, run: () => copy(link.href) },
  ];
  const code = target.closest("pre");
  if (code) return () => [{ id: "copy-code", label: "Copy code", icon: Code, run: () => copy(code.textContent ?? "") }];
  if (scoped) return scoped;
  const message = target.closest(".message .bubble");
  if (message?.textContent) return () => [{ id: "copy-message", label: "Copy message", icon: Copy, run: () => copy(message.textContent ?? "") }];
}

/** Adds contextual actions without introducing wrappers or changing item geometry. */
export function ContextMenuScope({ actions, label, children }: { actions: ContextAction[]; label: string; children: ReactElement<HTMLAttributes<HTMLElement>> }) {
  const open = useContext(MenuContext);
  const latest = useRef(actions);
  latest.current = actions;
  return cloneElement(children, {
    onContextMenu: event => { children.props.onContextMenu?.(event); if (!event.defaultPrevented) open(event, () => latest.current, label); },
    onKeyDown: event => {
      children.props.onKeyDown?.(event);
      if (!event.defaultPrevented && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) open(event, () => latest.current, label);
    },
  });
}

export function ContextMenuProvider({ children }: { children: ReactNode }) {
  const [request, setRequest] = useState<MenuRequest>();
  const [error, setError] = useState("");
  const sequence = useRef(0);
  const executing = useRef(false);
  const open = useCallback((event: OpenEvent, scoped?: () => ContextAction[], label = "Actions") => {
    event.preventDefault(); event.stopPropagation();
    if (!(event.target instanceof Element)) return;
    const actions = contextualActions(event.target, scoped);
    if (!actions?.().length) { setRequest(undefined); return; }
    const source = (scoped ? event.currentTarget : event.target.closest("input, textarea, a, pre, .bubble, button") ?? event.target) as HTMLElement;
    const origin = (event.target.closest("input, textarea, a, pre") ?? source).getBoundingClientRect();
    const rect = source.getBoundingClientRect();
    const keyboard = event.type === "keydown" || (!event.clientX && !event.clientY);
    setError("");
    setRequest({ id: ++sequence.current, x: keyboard ? rect.left + Math.min(rect.width / 2, 24) : event.clientX!, y: keyboard ? rect.bottom - 4 : event.clientY!, source, origin, focus: document.activeElement instanceof HTMLElement ? document.activeElement : null, label, actions });
  }, []);
  const close = useCallback((restore = false) => {
    if (restore && request?.focus?.isConnected) request.focus.focus({ preventScroll: true });
    setRequest(current => current?.id === request?.id ? undefined : current);
  }, [request]);
  const activate = async (id: string) => {
    if (!request || executing.current || !request.source.isConnected) { close(); return; }
    const action = request.actions().find(item => item.id === id);
    if (!action || action.disabled) return;
    executing.current = true;
    close(true);
    try { await action.run(); }
    catch (failure) { setError(`Could not ${action.label.toLowerCase()}: ${failure instanceof Error ? failure.message : String(failure)}`); }
    finally { executing.current = false; }
  };
  useEffect(() => {
    if (!error) return;
    const timer = window.setTimeout(() => setError(""), 7000);
    return () => clearTimeout(timer);
  }, [error]);
  return <MenuContext.Provider value={open}><div className="context-menu-root" onContextMenu={event => open(event)} onKeyDown={event => {
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) open(event);
  }}>{children}</div>
    {request && <ContextMenu key={request.id} request={request} onClose={close} onActivate={id => void activate(id)} />}
    {error && createPortal(<div className="context-menu-error" role="alert">{error}<button type="button" aria-label="Dismiss menu error" onClick={() => setError("")}>×</button></div>, document.querySelector("dialog[open]") ?? document.body)}
  </MenuContext.Provider>;
}

function ContextMenu({ request, onClose, onActivate }: { request: MenuRequest; onClose: (restore?: boolean) => void; onActivate: (id: string) => void }) {
  const menu = useRef<HTMLDivElement>(null);
  const group = useRef<HTMLDivElement>(null);
  const [shape, setShape] = useState<MenuShape>();
  const clipId = `context-surface-${useId().replace(/:/g, "")}`;
  const [, invalidate] = useState(0);
  const nativeFocus = "focusGroup" in HTMLElement.prototype;
  const actions = request.actions();
  const typeahead = useRef({ text: "", time: 0 });
  const entries = () => Array.from(group.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
  useLayoutEffect(() => {
    const node = menu.current!;
    node.showPopover?.();
    const next = connectedMenuShape(request.origin, request, node.offsetWidth, node.offsetHeight, { width: window.innerWidth, height: window.innerHeight });
    node.style.left = `${next.left}px`; node.style.top = `${next.top}px`;
    node.style.width = `${next.width}px`;
    node.style.setProperty("--context-menu-height", `${next.height}px`);
    setShape(next);
    entries()[0]?.focus({ preventScroll: true });
  }, [request]);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose(); };
    const focus = (event: FocusEvent) => { if (!menu.current?.contains(event.target as Node)) onClose(); };
    const scroll = (event: Event) => { if (!(event.target instanceof Node) || !menu.current?.contains(event.target)) onClose(); };
    const dismiss = () => onClose();
    const observer = new MutationObserver(changes => {
      if (!request.source.isConnected) onClose();
      else if (changes.some(change => request.source.contains(change.target))) invalidate(value => value + 1);
    });
    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled", "aria-disabled"] });
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", focus);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("blur", dismiss);
    return () => { observer.disconnect(); document.removeEventListener("pointerdown", outside, true); document.removeEventListener("focusin", focus); window.removeEventListener("scroll", scroll, true); window.removeEventListener("resize", dismiss); window.removeEventListener("blur", dismiss); };
  }, [onClose, request]);
  const parent = request.source.closest("dialog[open]") ?? document.body;
  return createPortal(<div ref={menu} popover={"showPopover" in HTMLElement.prototype ? "auto" : undefined} className="app-context-menu" data-ready={!!shape} style={{ visibility: shape ? undefined : "hidden" }} onContextMenu={event => { event.preventDefault(); event.stopPropagation(); }} onToggle={event => { if (event.newState === "closed") onClose(true); }} onKeyDown={event => {
      const items = entries(), index = items.indexOf(document.activeElement as HTMLButtonElement);
      const shortcut = (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey ? actions.find(action => action.shortcut?.toLowerCase() === `ctrl+${event.key.toLowerCase()}`) : undefined;
      if (shortcut) { event.preventDefault(); event.stopPropagation(); if (!shortcut.disabled) onActivate(shortcut.id); }
      else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(true); }
      else if (event.key === "Tab") { event.preventDefault(); event.stopPropagation(); onClose(true); const stops = Array.from(parent.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex="0"]')).filter(item => !menu.current?.contains(item) && item.getClientRects().length); const previous = stops.indexOf(request.focus!); stops[(previous + (event.shiftKey ? -1 : 1) + stops.length) % stops.length]?.focus(); }
      else if (["Home", "End"].includes(event.key) || (!nativeFocus && ["ArrowDown", "ArrowUp"].includes(event.key))) { event.preventDefault(); event.stopPropagation(); const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowUp" ? -1 : 1) + items.length) % items.length; items[next]?.focus(); }
      else if (event.key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey && event.key !== " ") { event.preventDefault(); const now = Date.now(); const previous = typeahead.current; const text = (now - previous.time < 600 ? previous.text : "") + event.key.toLowerCase(); typeahead.current = { text, time: now }; const match = [...items.slice(index + 1), ...items.slice(0, index + 1)].find(item => item.textContent?.trim().toLowerCase().startsWith(text)); match?.focus(); }
    }}>
      {shape && <MenuSurface shape={shape} clipId={clipId} />}
      <div className="context-menu-items" ref={group} style={shape ? { clipPath: `url(#${clipId})` } : undefined} {...(nativeFocus ? { focusgroup: "menu nomemory" } : { role: "menu" })} aria-label={request.label}>
        <SlidingHighlights className="context-menu-highlights">{actions.map(action => <Action key={action.id} action={action} nativeFocus={nativeFocus} activate={() => onActivate(action.id)} />)}</SlidingHighlights>
      </div>
    </div>, parent);
}

function MenuSurface({ shape, clipId }: { shape: MenuShape; clipId: string }) {
  const surface = useRef<SVGPathElement>(null);
  const clip = useRef<SVGPathElement>(null);
  useLayoutEffect(() => {
    const reduced = document.documentElement.classList.contains("reduce-motion") || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const draw = (progress: number) => { const path = menuSurfacePath(shape, progress); surface.current?.setAttribute("d", path); clip.current?.setAttribute("d", path); };
    if (reduced) { draw(1); return; }
    draw(0);
    let frame = 0, start: number | undefined;
    const animate = (time: number) => {
      start ??= time;
      const progress = Math.min(1, (time - start) / 280);
      draw(1 - Math.pow(1 - progress, 3));
      if (progress < 1) frame = requestAnimationFrame(animate);
    };
    frame = requestAnimationFrame(animate);
    return () => cancelAnimationFrame(frame);
  }, [shape]);
  return <svg className="context-menu-surface" width={shape.width} height={shape.height} aria-hidden="true">
    <defs><clipPath id={clipId} clipPathUnits="userSpaceOnUse"><path ref={clip} transform={shape.transform} d={menuSurfacePath(shape, 1)} /></clipPath></defs>
    <path ref={surface} transform={shape.transform} d={menuSurfacePath(shape, 1)} />
  </svg>;
}

function Action({ action, nativeFocus, activate }: { action: ContextAction; nativeFocus: boolean; activate: () => void }) {
  const Icon = action.icon;
  return <>{action.separator && <hr />}<button type="button" data-highlight-key={action.id} role={nativeFocus ? undefined : "menuitem"} tabIndex={nativeFocus ? undefined : -1} disabled={action.disabled} data-danger={action.danger || undefined} onClick={activate} onPointerMove={event => { if (!action.disabled && event.pointerType !== "touch") event.currentTarget.focus({ preventScroll: true }); }}>
    {Icon ? <Icon size={15} aria-hidden="true" /> : <span className="context-menu-icon" />}<span>{action.label}</span>{action.shortcut && <kbd aria-hidden="true">{action.shortcut}</kbd>}
  </button></>;
}
