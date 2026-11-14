import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";

type Box = { x: number; y: number; width: number; height: number };
type Highlight = { box: Box | null; visible: boolean; slide: boolean };
const empty: Highlight = { box: null, visible: false, slide: false };
function sameBox(a: Box | null, b: Box) { return a?.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height; }

/** Two independent background layers. Items and their contents never move. */
export function SlidingHighlights({ selectedKey, className = "", children }: { selectedKey?: string; className?: string; children: ReactNode }) {
  const root = useRef<HTMLDivElement>(null);
  const hovered = useRef<HTMLElement | null>(null);
  const [selection, setSelection] = useState<Highlight>(empty);
  const [hover, setHover] = useState<Highlight>(empty);
  const boxFor = (item: HTMLElement): Box => {
    // Layout coordinates stay correct while a parent popover scales open.
    // Viewport rectangles would apply that scale a second time to the layer.
    if (item.offsetParent === root.current) return { x: item.offsetLeft, y: item.offsetTop, width: item.offsetWidth, height: item.offsetHeight };
    const base = root.current!.getBoundingClientRect();
    const box = item.getBoundingClientRect();
    return { x: box.left - base.left, y: box.top - base.top, width: box.width, height: box.height };
  };
  useLayoutEffect(() => {
    const node = root.current;
    if (!node) return;
    const measure = () => {
      const items = Array.from(node.querySelectorAll<HTMLElement>("[data-highlight-key]"));
      const selected = items.find(item => item.dataset.highlightKey === selectedKey);
      if (selected) {
        const box = boxFor(selected);
        setSelection(previous => sameBox(previous.box, box) && previous.visible ? previous : { box, visible: true, slide: previous.box !== null });
      } else setSelection(previous => previous.visible ? { ...previous, visible: false } : previous);
      if (hovered.current && node.contains(hovered.current) && !hovered.current.matches(':disabled, [aria-disabled="true"]')) {
        const box = boxFor(hovered.current);
        setHover(previous => sameBox(previous.box, box) ? previous : { ...previous, box });
      } else if (hovered.current) {
        hovered.current = null;
        setHover(previous => ({ ...previous, visible: false }));
      }
      for (const item of items) resize.observe(item);
    };
    const resize = new ResizeObserver(measure);
    resize.observe(node);
    const mutations = new MutationObserver(measure);
    mutations.observe(node, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["disabled", "aria-disabled"] });
    window.addEventListener("resize", measure);
    measure();
    return () => { resize.disconnect(); mutations.disconnect(); window.removeEventListener("resize", measure); };
  }, [selectedKey]);
  const enterItem = (target: EventTarget | null) => {
    if (!(target instanceof Element)) return;
    const item = target.closest<HTMLElement>("[data-highlight-key]");
    if (!item || !root.current?.contains(item) || item === hovered.current) return;
    if (item.matches(':disabled, [aria-disabled="true"]')) {
      hovered.current = null;
      setHover(previous => previous.visible ? { ...previous, visible: false } : previous);
      return;
    }
    const slide = hovered.current !== null;
    hovered.current = item;
    setHover({ box: boxFor(item), visible: true, slide });
  };
  const leave = () => {
    hovered.current = null;
    setHover(previous => previous.visible ? { ...previous, visible: false } : previous);
  };
  const layer = (kind: string, highlight: Highlight) => {
    const { box } = highlight;
    const style: CSSProperties = box ? { width: box.width, height: box.height, transform: `translate3d(${box.x}px, ${box.y}px, 0)` } : {};
    return <span aria-hidden="true" className={`sidebar-highlight ${kind}`} data-visible={highlight.visible} data-slide={highlight.slide} style={style} />;
  };
  return <div ref={root} className={`sliding-highlights ${className}`} onPointerOver={event => { if (event.pointerType !== "touch") enterItem(event.target); }} onPointerLeave={leave} onPointerCancel={leave}
    onFocusCapture={event => enterItem(event.target)} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) leave(); }}>
    {layer("selection", selection)}{layer("hover", hover)}{children}
  </div>;
}
