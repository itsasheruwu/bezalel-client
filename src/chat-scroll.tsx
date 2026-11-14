import { useEffect, useId, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";

export function useChatScroll(conversationId?: string) {
  const viewport = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const away = useRef(false);
  const [detached, setDetached] = useState(false);
  const jump = () => {
    following.current = true;
    away.current = false;
    setDetached(false);
    const node = viewport.current;
    if (node) node.scrollTop = node.scrollHeight;
  };
  useEffect(() => {
    const node = viewport.current;
    const body = content.current;
    if (!node || !body) return;
    following.current = true;
    away.current = false;
    setDetached(false);
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (following.current) node.scrollTop = node.scrollHeight;
      });
    };
    const scroll = () => {
      const distance = node.scrollHeight - node.scrollTop - node.clientHeight;
      // Separate appearance and dismissal thresholds so small wheel increments
      // cannot immediately reverse an entrance near the bottom edge.
      if (distance <= 1) {
        following.current = true;
        away.current = false;
      } else if (distance > 8) {
        following.current = false;
        away.current = true;
      }
      setDetached(away.current);
    };
    const wheel = (event: WheelEvent) => {
      if (event.deltaY < 0 && node.scrollHeight > node.clientHeight) {
        following.current = false;
        // Pause following immediately, but wait for actual scroll movement
        // before showing the control. Wheel fires before scrollTop changes.
      }
    };
    const mutations = new MutationObserver(update);
    const resize = new ResizeObserver(update);
    mutations.observe(body, { childList: true, subtree: true, characterData: true, attributes: true });
    resize.observe(body);
    resize.observe(node);
    node.addEventListener("scroll", scroll, { passive: true });
    node.addEventListener("wheel", wheel, { passive: true });
    update();
    return () => {
      cancelAnimationFrame(frame); mutations.disconnect(); resize.disconnect();
      node.removeEventListener("scroll", scroll); node.removeEventListener("wheel", wheel);
    };
  }, [conversationId]);
  return { viewport, content, detached, jump };
}

export function JumpToLatest({ visible, onJump }: { visible: boolean; onJump: () => void }) {
  const [phase, setPhase] = useState("hidden");
  const liquidId = useId();
  useEffect(() => {
    if (visible) { setPhase("enter"); return; }
    setPhase(current => current === "hidden" || current === "pop" ? current : "exit");
  }, [visible]);
  useEffect(() => {
    if (phase !== "enter" && phase !== "exit" && phase !== "pop") return;
    const timer = window.setTimeout(() => setPhase(phase === "enter" ? "idle" : "hidden"), phase === "enter" ? 420 : 280);
    return () => window.clearTimeout(timer);
  }, [phase]);
  if (phase === "hidden") return null;
  return <button type="button" className={`jump-latest ${phase}`} aria-label="Jump to latest message" title="Jump to latest" tabIndex={phase === "exit" || phase === "pop" ? -1 : 0} onClick={() => {
    if (phase === "pop") return;
    setPhase("pop"); onJump();
  }}><svg className="jump-emergence" viewBox="0 0 90 48" width="90" height="48" overflow="hidden" aria-hidden="true"><defs><filter id={liquidId} x="-30%" y="-50%" width="160%" height="200%" colorInterpolationFilters="sRGB"><feGaussianBlur in="SourceGraphic" stdDeviation="3.5" /><feColorMatrix type="matrix" values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 24 -12" result="liquid" /><feMorphology in="liquid" operator="dilate" radius="0.65" result="expanded" /><feComposite in="expanded" in2="liquid" operator="out" result="rim" /><feFlood floodColor="var(--text)" floodOpacity="0.2" result="rimColor" /><feComposite in="rimColor" in2="rim" operator="in" result="outline" /><feFlood x="-100" y="-100" width="300" height="144" floodColor="white" result="floatingRegion" /><feComposite in="outline" in2="floatingRegion" operator="in" result="floatingOutline" /><feMerge><feMergeNode in="liquid" /><feMergeNode in="floatingOutline" /></feMerge></filter></defs><g filter={`url(#${liquidId})`}><rect x="-45" y="48" width="180" height="22" /><circle className="jump-liquid-head" cx="45" cy="19" r="19" /></g></svg><span className="jump-skin" aria-hidden="true" /><ArrowDown size={18} /><span className="jump-droplets" aria-hidden="true">{Array.from({ length: 8 }, (_, i) => <i key={i} style={{ transform: `rotate(${i * 45}deg)` }} />)}</span></button>;
}
