export type SourceBox = { left: number; top: number; right: number; bottom: number; width: number; height: number };
type Side = "right" | "left" | "bottom" | "top";
export type MenuShape = { left: number; top: number; width: number; height: number; side: Side; mouth: { x: number; y: number; half: number }; transform: string; canonicalWidth: number; canonicalHeight: number };
const clamp = (n: number, min: number, max: number) => Math.max(min, Math.min(n, Math.max(min, max)));

/** Position outside the source so a visible, short connection can join their edges. */
export function connectedMenuShape(source: SourceBox, point: { x: number; y: number }, width: number, height: number, viewport: { width: number; height: number }): MenuShape {
  const gap = 10, inset = 8;
  width = Math.min(width, viewport.width - inset * 2);
  height = Math.min(height, viewport.height - inset * 2);
  let box = source;
  const choices = (origin: SourceBox) => {
    const horizontal = clamp(point.y - Math.min(height / 2, origin.height / 2), inset, viewport.height - height - inset);
    const vertical = clamp(point.x - Math.min(width / 2, origin.width / 2), inset, viewport.width - width - inset);
    return {
      right: { left: origin.right + gap, top: horizontal },
      left: { left: origin.left - gap - width, top: horizontal },
      bottom: { left: vertical, top: origin.bottom + gap },
      top: { left: vertical, top: origin.top - gap - height },
    };
  };
  let candidates = choices(box);
  const fits = (side: Side) => {
    const item = candidates[side];
    return item.left >= inset && item.left + width <= viewport.width - inset && item.top >= inset && item.top + height <= viewport.height - inset;
  };
  const order: Side[] = source.width <= 280 && source.height <= 110 ? ["right", "left", "bottom", "top"] : ["bottom", "top", "right", "left"];
  let side = order.find(fits);
  if (!side) {
    // Large paragraphs/editors can occupy the entire viewport. Attach to the
    // clicked line instead of drawing a neck across a whole page of content.
    const x = clamp(point.x, inset + 14, viewport.width - inset - 14);
    const y = clamp(point.y, inset + 8, viewport.height - inset - 8);
    box = { left: x - 14, right: x + 14, top: y - 8, bottom: y + 8, width: 28, height: 16 };
    candidates = choices(box);
    side = order.find(fits);
  }
  if (!side) {
    // Last resort for very small windows: leave room for the attachment and
    // use a scrollable, shorter menu rather than overlapping its source.
    side = point.y > viewport.height / 2 ? "top" : "bottom";
    height = Math.max(24, side === "top" ? box.top - gap - inset : viewport.height - box.bottom - gap - inset);
    candidates = choices(box);
  }
  const position = candidates[side];
  const horizontal = side === "right" || side === "left";
  const contact = horizontal ? clamp(point.y, box.top + 4, box.bottom - 4) : clamp(point.x, box.left + 4, box.right - 4);
  const mouth = {
    x: -gap,
    y: contact - (horizontal ? position.top : position.left),
    half: Math.min(10, (horizontal ? box.height : box.width) / 3),
  };
  return { ...position, width, height, side, mouth, canonicalWidth: horizontal ? width : height, canonicalHeight: horizontal ? height : width,
    transform: side === "right" ? "" : side === "left" ? `translate(${width} 0) scale(-1 1)` : side === "bottom" ? "matrix(0 1 1 0 0 0)" : `matrix(0 -1 1 0 0 ${height})` };
}

/** One continuous outline: rounded menu, concave neck, and the source's edge. */
export function menuSurfacePath(shape: MenuShape, progress: number) {
  const t = clamp(progress, 0, 1), { mouth } = shape;
  const mix = (from: number, to: number) => from + (to - from) * t;
  const x = mix(mouth.x + 1, 0), y = mix(mouth.y - mouth.half, 0);
  const width = mix(1, shape.canonicalWidth), height = mix(mouth.half * 2, shape.canonicalHeight);
  const right = x + width, bottom = y + height;
  const radius = Math.min(9, width / 2, height / 2);
  const spread = Math.min(25, (height - radius * 2) / 2);
  const center = clamp(mouth.y, y + radius + spread, bottom - radius - spread);
  const control = mouth.x + (x - mouth.x) * .6;
  return `M${x + radius},${y} H${right - radius} Q${right},${y} ${right},${y + radius} V${bottom - radius} Q${right},${bottom} ${right - radius},${bottom} H${x + radius} Q${x},${bottom} ${x},${bottom - radius} L${x},${center + spread} C${x},${center + 10} ${control},${mouth.y + mouth.half} ${mouth.x},${mouth.y + mouth.half} L${mouth.x},${mouth.y - mouth.half} C${control},${mouth.y - mouth.half} ${x},${center - 10} ${x},${center - spread} L${x},${y + radius} Q${x},${y} ${x + radius},${y} Z`;
}
