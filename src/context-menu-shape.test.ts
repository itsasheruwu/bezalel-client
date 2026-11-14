import { expect, it } from "vitest";
import { connectedMenuShape, menuSurfacePath } from "./context-menu-shape";
const viewport = { width: 1182, height: 802 };
const box = (left: number, top: number, width: number, height: number) => ({ left, top, right: left + width, bottom: top + height, width, height });

it("joins a rail icon's actual edge, rather than opening at the cursor", () => {
  const source = box(8, 98, 40, 38);
  const shape = connectedMenuShape(source, { x: 30, y: 116 }, 246, 140, viewport);
  expect(shape.side).toBe("right");expect(shape.left).toBe(source.right + 10);
  expect(shape.left + shape.mouth.x).toBe(source.right);
  expect(shape.top + shape.mouth.y).toBe(116);
  const row = connectedMenuShape(box(65, 230, 191, 63), { x: 120, y: 260 }, 246, 150, viewport);
  expect(row.left + row.mouth.x).toBe(256);
});

it("flips the silhouette with the menu and keeps its attachment on the source", () => {
  const source = box(1000, 690, 160, 60);
  const shape = connectedMenuShape(source, { x: 1080, y: 716 }, 246, 182, viewport);
  expect(shape.side).toBe("left");expect(shape.left + shape.width - shape.mouth.x).toBe(source.left);
  expect(shape.top + shape.height).toBeLessThanOrEqual(viewport.height - 8);
  expect(shape.transform).toContain("scale(-1 1)");
});

it("grows above a composer field when there is no room beneath it", () => {
  const source = box(313, 664, 820, 58);
  const shape = connectedMenuShape(source, { x: 500, y: 688 }, 246, 182, viewport);
  expect(shape.side).toBe("top");expect(shape.top + shape.height - shape.mouth.x).toBe(source.top);
  expect(shape.left + shape.mouth.y).toBe(500);
});

it("uses a nearby text attachment for page-sized sources and shortens the menu in small windows", () => {
  const shape = connectedMenuShape(box(0, 0, 1182, 802), { x: 600, y: 400 }, 246, 182, viewport);
  expect(shape.side).toBe("bottom");expect(shape.top).toBe(418);
  const small = connectedMenuShape(box(100, 100, 40, 30), { x: 120, y: 115 }, 246, 182, { width: 260, height: 210 });
  expect(small.left).toBeGreaterThanOrEqual(8);expect(small.top).toBeGreaterThanOrEqual(8);
  expect(small.left + small.width).toBeLessThanOrEqual(252);expect(small.top + small.height).toBeLessThanOrEqual(202);
  expect(small.height).toBeLessThan(182);
});

it("changes the continuous surface itself from attached stub to full menu, keeping finite paths", () => {
  const shape = connectedMenuShape(box(8, 98, 40, 38), { x: 30, y: 116 }, 246, 140, viewport);
  const paths = [0, .1, .4, .7, 1].map(progress => menuSurfacePath(shape, progress));
  expect(new Set(paths).size).toBe(5);
  for (const path of paths) { expect(path).not.toMatch(/NaN|Infinity/);expect(path).toContain(`L${shape.mouth.x},${shape.mouth.y - shape.mouth.half}`); }
  expect(menuSurfacePath(shape, -1)).toBe(paths[0]);expect(menuSurfacePath(shape, 2)).toBe(paths[4]);
});
