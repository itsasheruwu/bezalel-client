// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SlidingHighlights } from "./sliding-highlights";
let resize: () => void;
let rowWidth = 180;
vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
beforeEach(() => {
  rowWidth = 180;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.highlightKey;
    const top = key === "a" ? 108 : key === "b" ? 166 : 100;
    return { x: key ? 22 : 20, y: top, left: key ? 22 : 20, top, width: key ? rowWidth : 200, height: key ? 54 : 160, right: 220, bottom: top + 54, toJSON() {} };
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
function Items({ selected = "a", second = true }: { selected?: string; second?: boolean }) {
  return <SlidingHighlights selectedKey={selected}><div data-highlight-key="a"><button>Title A<span>Preview A</span></button></div>{second && <div data-highlight-key="b"><button>Title B<span>Preview B</span></button><button>Delete B</button></div>}</SlidingHighlights>;
}
it("moves hover independently and fades out at the last position", () => {
  const { container } = render(<Items />);
  const selected = container.querySelector<HTMLElement>(".selection")!;
  const hover = container.querySelector<HTMLElement>(".hover")!;
  expect(selected.style.transform).toBe("translate3d(2px, 8px, 0)");
  fireEvent.pointerOver(screen.getByText("Preview A"), { pointerType: "mouse" });
  expect(hover.dataset.visible).toBe("true");expect(hover.dataset.slide).toBe("false");
  fireEvent.pointerOver(screen.getByText("Preview B"), { pointerType: "mouse" });
  expect(hover.style.transform).toBe("translate3d(2px, 66px, 0)");expect(hover.dataset.slide).toBe("true");
  expect(selected.style.transform).toBe("translate3d(2px, 8px, 0)");
  fireEvent.pointerLeave(container.firstChild!);
  expect(hover.dataset.visible).toBe("false");expect(hover.style.transform).toBe("translate3d(2px, 66px, 0)");
  expect(selected.dataset.visible).toBe("true");
});
it("fits the entire row including preview and delete area without moving text", () => {
  const { container, rerender } = render(<Items />);
  fireEvent.pointerOver(screen.getByText("Delete B"));
  expect((container.querySelector(".hover") as HTMLElement).style.width).toBe("180px");
  const title = screen.getByText("Preview B");expect(title.style.transform).toBe("");
  rerender(<Items selected="b" />);
  expect((container.querySelector(".selection") as HTMLElement).style.transform).toBe("translate3d(2px, 66px, 0)");
  expect((container.querySelector(".selection") as HTMLElement).dataset.slide).toBe("true");
  expect(title.style.transform).toBe("");
});
it("remeasures after resizing and safely hides a removed selection", async () => {
  const { container, rerender } = render(<Items selected="b" />);
  rowWidth = 240;act(() => resize());expect((container.querySelector(".selection") as HTMLElement).style.width).toBe("240px");
  rerender(<Items selected="b" second={false} />);
  await waitFor(() => expect((container.querySelector(".selection") as HTMLElement).dataset.visible).toBe("false"));
});
it("keeps different sidebars isolated and supports keyboard focus", () => {
  const { container } = render(<><Items /><SlidingHighlights selectedKey="b"><button data-highlight-key="b">Other sidebar</button></SlidingHighlights></>);
  fireEvent.focus(screen.getByText("Other sidebar"));
  const layers = container.querySelectorAll<HTMLElement>(".hover");expect(layers[0].dataset.visible).toBe("false");expect(layers[1].dataset.visible).toBe("true");
});

it("uses local layout coordinates while the surrounding popover is scaled", () => {
  vi.spyOn(HTMLElement.prototype, "offsetParent", "get").mockImplementation(function(this: HTMLElement) { return this.dataset.highlightKey ? this.parentElement : null; });
  vi.spyOn(HTMLElement.prototype, "offsetLeft", "get").mockReturnValue(2);
  vi.spyOn(HTMLElement.prototype, "offsetTop", "get").mockImplementation(function(this: HTMLElement) { return this.dataset.highlightKey === "b" ? 66 : 8; });
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(180);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(54);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ left: 0, top: 0, width: 36, height: 11, right: 36, bottom: 11, x: 0, y: 0, toJSON() {} });
  const { container } = render(<Items />);
  fireEvent.pointerOver(screen.getByText("Preview B"));
  expect((container.querySelector(".selection") as HTMLElement).style.transform).toBe("translate3d(2px, 8px, 0)");
  expect((container.querySelector(".hover") as HTMLElement).style.transform).toBe("translate3d(2px, 66px, 0)");
  expect((container.querySelector(".hover") as HTMLElement).style.width).toBe("180px");
});
