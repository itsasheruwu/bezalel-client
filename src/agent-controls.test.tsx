// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AgentControls } from "./agent-controls";
import type { AgentModel } from "./model";

const models: AgentModel[] = ["Astra", "Sol", "Terra", "Luna"].map(family => ({ family: family as AgentModel["family"], version: "1", modelId: family === "Luna" ? null : family, efforts: ["low", "high"] }));
beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.highlightKey;
    const y = key ? models.findIndex(model => model.family === key) * 36 : 0;
    return { x: 100, y: 100 + y, left: 100, top: 100 + y, right: 312, bottom: 136 + y, width: 212, height: key ? 36 : 144, toJSON() {} };
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("keeps the selected model highlighted as hover glides and fades out in place", () => {
  const onChange = vi.fn(), props = { models, family: "Sol" as const, effort: "high", disabled: false, loading: false, error: "", onChange, onRefresh: vi.fn() };
  const { container, rerender } = render(<AgentControls {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "Model: Sol" }));
  const list = container.querySelector(".model-highlights")!;
  const selection = list.querySelector<HTMLElement>(".selection")!, hover = list.querySelector<HTMLElement>(".hover")!;
  expect(selection.style.transform).toBe("translate3d(0px, 36px, 0)");
  fireEvent.pointerOver(screen.getByRole("option", { name: "Terra" }));
  expect(hover.style.transform).toBe("translate3d(0px, 72px, 0)");expect(hover.dataset.slide).toBe("true");
  expect(selection.style.transform).toBe("translate3d(0px, 36px, 0)");
  fireEvent.pointerLeave(list);expect(hover.dataset.visible).toBe("false");expect(hover.style.transform).toBe("translate3d(0px, 72px, 0)");expect(selection.dataset.visible).toBe("true");
  rerender(<AgentControls {...props} family="Astra" />);expect(selection.style.transform).toBe("translate3d(0px, 0px, 0)");expect(selection.dataset.slide).toBe("true");
  expect((screen.getByRole("option", { name: "Astra" }) as HTMLElement).style.transform).toBe("");expect(onChange).not.toHaveBeenCalled();
});

it("tracks keyboard focus, skips unavailable models, and still selects the chosen model", () => {
  const onChange = vi.fn();
  const { container } = render(<AgentControls models={models} family="Sol" effort="high" disabled={false} loading={false} error="" onChange={onChange} onRefresh={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Model: Sol" }));
  fireEvent.keyDown(screen.getByRole("option", { name: "Sol" }), { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByRole("option", { name: "Terra" }));
  const hover = container.querySelector<HTMLElement>(".model-highlights .hover")!;
  expect(hover.style.transform).toBe("translate3d(0px, 72px, 0)");
  fireEvent.pointerOver(screen.getByRole("option", { name: "Luna" }));expect(hover.dataset.visible).toBe("false");
  fireEvent.click(screen.getByRole("option", { name: "Luna" }));expect(onChange).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("option", { name: "Terra" }));expect(onChange).toHaveBeenCalledWith("Terra", "high");expect(screen.queryByRole("listbox")).toBeNull();
});
