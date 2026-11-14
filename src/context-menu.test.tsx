// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { ContextMenuProvider, ContextMenuScope, type ContextAction } from "./context-menu";

const writeText = vi.fn().mockResolvedValue(undefined);
const readText = vi.fn().mockResolvedValue("pasted");
beforeEach(() => { vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} }); Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText, readText } }); window.getSelection()?.removeAllRanges(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); document.documentElement.classList.remove("reduce-motion"); });
function Item({ name, actions }: { name: string; actions: ContextAction[] }) { return <ContextMenuScope label={name} actions={actions}><button>{name}</button></ContextMenuScope>; }
function open(name: string) { fireEvent.contextMenu(screen.getByText(name), { clientX: 40, clientY: 90 }); }

it("uses the clicked item, dismisses after activation and never runs disabled actions", async () => {
  const first = vi.fn(), second = vi.fn(), forbidden = vi.fn();
  render(<ContextMenuProvider><Item name="First pack" actions={[{ id: "open", label: "Open pack", run: first }]} /><Item name="Second pack" actions={[{ id: "open", label: "Open pack", run: second }, { id: "delete", label: "Delete pack", disabled: true, run: forbidden }]} /></ContextMenuProvider>);
  open("Second pack");
  expect(screen.getByRole("menu", { name: "Second pack" })).toBeTruthy();
  fireEvent.click(screen.getByRole("menuitem", { name: "Delete pack" }));
  expect(forbidden).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("menuitem", { name: "Open pack" }));
  await waitFor(() => expect(second).toHaveBeenCalledOnce());
  expect(first).not.toHaveBeenCalled();expect(screen.queryByRole("menu")).toBeNull();
});

it("supports keyboard open, arrows, Home, End, and Escape restoring focus", () => {
  render(<ContextMenuProvider><Item name="Conversation" actions={[{ id: "open", label: "Open", run: vi.fn() }, { id: "disabled", label: "Disabled", disabled: true, run: vi.fn() }, { id: "copy", label: "Copy title", run: vi.fn() }]} /></ContextMenuProvider>);
  const source = screen.getByText("Conversation");source.focus();fireEvent.keyDown(source, { key: "F10", shiftKey: true });
  expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Open" }));
  fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Copy title" }));
  fireEvent.keyDown(document.activeElement!, { key: "Home" });expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Open" }));
  fireEvent.keyDown(document.activeElement!, { key: "End" });expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Copy title" }));
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });expect(screen.queryByRole("menu")).toBeNull();expect(document.activeElement).toBe(source);
});

it("copies selected text and does not confuse a selection elsewhere with the clicked item", async () => {
  const action = vi.fn();
  render(<ContextMenuProvider><p>Selected text</p><Item name="Another item" actions={[{ id: "open", label: "Open item", run: action }]} /></ContextMenuProvider>);
  const text = screen.getByText("Selected text"), range = document.createRange();range.selectNodeContents(text);window.getSelection()!.addRange(range);
  fireEvent.contextMenu(text, { clientX: 30, clientY: 60 });fireEvent.click(screen.getByRole("menuitem", { name: "Copy" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("Selected text"));
  open("Another item");expect(screen.getByRole("menuitem", { name: "Open item" })).toBeTruthy();
});

it("offers only appropriate editing actions for read-only, password and disabled fields", () => {
  render(<ContextMenuProvider><textarea aria-label="Read-only" defaultValue="readonly" readOnly /><input aria-label="Password" type="password" defaultValue="secret" /><textarea aria-label="Disabled" disabled /></ContextMenuProvider>);
  const field = screen.getByLabelText("Read-only") as HTMLTextAreaElement;field.setSelectionRange(0, 4);fireEvent.contextMenu(field);
  expect(screen.queryByRole("menuitem", { name: "Cut" })).toBeNull();expect(screen.queryByRole("menuitem", { name: "Paste" })).toBeNull();
  expect((screen.getByRole("menuitem", { name: "Copy" }) as HTMLButtonElement).disabled).toBe(false);
  const secret = screen.getByLabelText("Password") as HTMLInputElement;secret.setSelectionRange(0, 6);fireEvent.contextMenu(secret);
  expect((screen.getByRole("menuitem", { name: "Copy" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("menuitem", { name: "Cut" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.contextMenu(screen.getByLabelText("Disabled"));expect(screen.queryByRole("menu")).toBeNull();
});

function Editor() { const [value, setValue] = useState("hello world");return <textarea aria-label="Editor" maxLength={13} value={value} onChange={event => setValue(event.target.value)} />; }
it("pastes into the captured selection, updates React state and respects maxlength", async () => {
  readText.mockResolvedValueOnce("pasted-too-long");
  render(<ContextMenuProvider><Editor /></ContextMenuProvider>);
  const field = screen.getByLabelText("Editor") as HTMLTextAreaElement;field.focus();field.setSelectionRange(6, 11);fireEvent.contextMenu(field);
  fireEvent.click(screen.getByRole("menuitem", { name: "Paste" }));
  await waitFor(() => expect(field.value).toBe("hello pasted-"));
  expect(field.value.length).toBe(13);
  expect(document.activeElement).toBe(field);
});

it("cuts only the selection after copying and Select all selects the full field", async () => {
  render(<ContextMenuProvider><Editor /></ContextMenuProvider>);
  const field = screen.getByLabelText("Editor") as HTMLTextAreaElement;field.focus();field.setSelectionRange(0, 6);fireEvent.contextMenu(field);
  fireEvent.click(screen.getByRole("menuitem", { name: "Cut" }));await waitFor(() => expect(field.value).toBe("world"));expect(writeText).toHaveBeenCalledWith("hello ");
  fireEvent.contextMenu(field);fireEvent.click(screen.getByRole("menuitem", { name: "Select all" }));expect(field.selectionStart).toBe(0);expect(field.selectionEnd).toBe(5);
});

it("honors the displayed edit shortcuts while the menu has focus", async () => {
  render(<ContextMenuProvider><Editor /></ContextMenuProvider>);
  const field = screen.getByLabelText("Editor") as HTMLTextAreaElement;field.focus();field.setSelectionRange(0, 5);fireEvent.contextMenu(field);
  fireEvent.keyDown(document.activeElement!, { key: "c", ctrlKey: true });
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("hello"));expect(screen.queryByRole("menu")).toBeNull();expect(document.activeElement).toBe(field);
});

it("keeps number fields safe and menus usable inside modal dialogs", () => {
  render(<ContextMenuProvider><dialog open><input aria-label="Memory" type="number" defaultValue={2048} /></dialog></ContextMenuProvider>);
  const field = screen.getByLabelText("Memory");fireEvent.contextMenu(field);
  expect(screen.getAllByRole("menuitem")).toHaveLength(1);expect(screen.getByRole("menuitem", { name: "Copy value" })).toBeTruthy();
  expect(screen.getByRole("menu").closest("dialog")).toBeTruthy();
  fireEvent.pointerDown(field);expect(screen.queryByRole("menu")).toBeNull();
});

it("rejects a late paste if the field changed, and reports clipboard failures", async () => {
  let resolve!: (text: string) => void;readText.mockImplementationOnce(() => new Promise<string>(done => { resolve = done; }));
  render(<ContextMenuProvider><Editor /></ContextMenuProvider>);
  const field = screen.getByLabelText("Editor") as HTMLTextAreaElement;field.focus();fireEvent.contextMenu(field);fireEvent.click(screen.getByRole("menuitem", { name: "Paste" }));
  fireEvent.change(field, { target: { value: "changed" } });await act(async () => resolve("old paste"));
  expect(field.value).toBe("changed");expect(screen.getByRole("alert").textContent).toContain("field changed");
  readText.mockRejectedValueOnce(new Error("Clipboard access denied"));fireEvent.contextMenu(field);fireEvent.click(screen.getByRole("menuitem", { name: "Paste" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Clipboard access denied"));
});

it("copies code and safe link addresses without offering browser debug/reload commands", async () => {
  render(<ContextMenuProvider><pre>code here</pre><a href="https://modrinth.com/mod/trinkets">Project</a><a href="javascript:alert(1)">Unsafe</a><div>Blank area</div></ContextMenuProvider>);
  open("code here");fireEvent.click(screen.getByRole("menuitem", { name: "Copy code" }));await waitFor(() => expect(writeText).toHaveBeenCalledWith("code here"));
  open("Project");fireEvent.click(screen.getByRole("menuitem", { name: "Copy link address" }));await waitFor(() => expect(writeText).toHaveBeenCalledWith("https://modrinth.com/mod/trinkets"));
  open("Unsafe");expect(screen.queryByRole("menu")).toBeNull();open("Blank area");expect(screen.queryByRole("menu")).toBeNull();
});

it("rechecks changed permissions and dismisses when the source is removed or the page scrolls", async () => {
  const run = vi.fn();
  const { rerender } = render(<ContextMenuProvider><Item name="Mod" actions={[{ id: "remove", label: "Remove", run }]} /></ContextMenuProvider>);
  open("Mod");rerender(<ContextMenuProvider><Item name="Mod" actions={[{ id: "remove", label: "Remove", disabled: true, run }]} /></ContextMenuProvider>);
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }));expect(run).not.toHaveBeenCalled();
  fireEvent.scroll(window);expect(screen.queryByRole("menu")).toBeNull();
  open("Mod");rerender(<ContextMenuProvider><div>Gone</div></ContextMenuProvider>);await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
});

it("animates the actual attached SVG surface and its content clip together", () => {
  let frame: FrameRequestCallback | undefined;
  vi.stubGlobal("requestAnimationFrame", vi.fn((callback: FrameRequestCallback) => { frame = callback; return 1; }));
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(246);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(140);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ left: 8, right: 48, top: 98, bottom: 136, width: 40, height: 38, x: 8, y: 98, toJSON() {} });
  render(<ContextMenuProvider><Item name="Rail icon" actions={[{ id: "open", label: "Open", run: vi.fn() }]} /></ContextMenuProvider>);
  fireEvent.contextMenu(screen.getByText("Rail icon"), { clientX: 30, clientY: 116 });
  const surface = document.querySelector(".context-menu-surface > path")!;
  const clip = document.querySelector(".context-menu-surface clipPath path")!;
  const start = surface.getAttribute("d");
  act(() => frame!(0));act(() => frame!(140));const middle = surface.getAttribute("d");
  expect(middle).not.toBe(start);expect(clip.getAttribute("d")).toBe(middle);
  act(() => frame!(280));expect(surface.getAttribute("d")).not.toBe(middle);expect(clip.getAttribute("d")).toBe(surface.getAttribute("d"));
  expect((document.querySelector(".app-context-menu") as HTMLElement).style.left).toBe("58px");
});

it("shows the finished attached shape immediately with reduced motion", () => {
  document.documentElement.classList.add("reduce-motion");
  const animate = vi.fn();vi.stubGlobal("requestAnimationFrame", animate);
  render(<ContextMenuProvider><Item name="Conversation" actions={[{ id: "open", label: "Open", run: vi.fn() }]} /></ContextMenuProvider>);
  open("Conversation");expect(document.querySelector(".context-menu-surface > path")).toBeTruthy();expect(animate).not.toHaveBeenCalled();
});

it("glides a shared hover background across actions and ignores disabled entries", () => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.highlightKey, top = key === "second" ? 32 : key === "disabled" ? 64 : 0;
    return { left: 0, top, right: 234, bottom: top + 32, x: 0, y: top, width: 234, height: 32, toJSON() {} };
  });
  render(<ContextMenuProvider><Item name="Mod" actions={[{ id: "first", label: "Copy filename", run: vi.fn() }, { id: "second", label: "Open folder", run: vi.fn() }, { id: "disabled", label: "Unavailable", disabled: true, run: vi.fn() }]} /></ContextMenuProvider>);
  open("Mod");const list = document.querySelector(".context-menu-highlights")!, hover = list.querySelector<HTMLElement>(".hover")!;
  fireEvent.pointerOver(screen.getByRole("menuitem", { name: "Copy filename" }));fireEvent.pointerOver(screen.getByRole("menuitem", { name: "Open folder" }));
  expect(hover.dataset.visible).toBe("true");expect(hover.dataset.slide).toBe("true");expect(hover.style.transform).toBe("translate3d(0px, 32px, 0)");
  fireEvent.pointerOver(screen.getByRole("menuitem", { name: "Unavailable" }));expect(hover.dataset.visible).toBe("false");
  fireEvent.pointerOver(screen.getByRole("menuitem", { name: "Open folder" }));fireEvent.pointerLeave(list);expect(hover.dataset.visible).toBe("false");expect(hover.style.transform).toBe("translate3d(0px, 32px, 0)");
});
