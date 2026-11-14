// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { JumpToLatest, useChatScroll } from "./chat-scroll";
let resized: () => void;
vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resized = callback; } observe() {} disconnect() {} });
afterEach(() => { cleanup(); vi.useRealTimers(); });
function Chat({ id = "one", text = "content" }) {
  const { viewport, content, detached, jump } = useChatScroll(id);
  return <><div ref={viewport} data-testid="scroll"><div ref={content}>{text}</div></div><JumpToLatest visible={detached} onJump={jump} /></>;
}
it("follows content growth until manual scrolling, then resumes after jumping", () => {
  vi.useFakeTimers();render(<Chat />);const viewport=screen.getByTestId("scroll");
  Object.defineProperties(viewport,{scrollHeight:{value:1000,configurable:true},clientHeight:{value:200,configurable:true}});
  act(()=>{resized();vi.advanceTimersByTime(20);});expect(viewport.scrollTop).toBe(1000);
  viewport.scrollTop=300;fireEvent.wheel(viewport,{deltaY:-100});fireEvent.scroll(viewport);
  expect(screen.getByRole("button",{name:"Jump to latest message"})).toBeTruthy();
  act(()=>{resized();vi.advanceTimersByTime(20);});expect(viewport.scrollTop).toBe(300);
  fireEvent.click(screen.getByRole("button",{name:"Jump to latest message"}));expect(viewport.scrollTop).toBe(1000);
  expect(screen.getByRole("button").className).toContain("pop");act(()=>vi.advanceTimersByTime(300));expect(screen.queryByRole("button")).toBeNull();
});
it("animates back into the composer when the reader reaches the bottom", () => {
  vi.useFakeTimers();render(<Chat />);const viewport=screen.getByTestId("scroll");Object.defineProperties(viewport,{scrollHeight:{value:1000},clientHeight:{value:200}});
  viewport.scrollTop=100;fireEvent.scroll(viewport);expect(screen.getByRole("button")).toBeTruthy();
  viewport.scrollTop=800;fireEvent.scroll(viewport);expect(screen.getByRole("button").className).toContain("exit");
  act(()=>vi.advanceTimersByTime(300));expect(screen.queryByRole("button")).toBeNull();
});

it("does not reverse the entrance during small upward wheel increments", () => {
  vi.useFakeTimers();render(<Chat />);const viewport=screen.getByTestId("scroll");
  Object.defineProperties(viewport,{scrollHeight:{value:1000},clientHeight:{value:200}});
  viewport.scrollTop=800;
  fireEvent.wheel(viewport,{deltaY:-4});expect(screen.queryByRole("button")).toBeNull();
  viewport.scrollTop=796;fireEvent.scroll(viewport);expect(screen.queryByRole("button")).toBeNull();
  viewport.scrollTop=790;fireEvent.scroll(viewport);expect(screen.getByRole("button").className).toContain("enter");
  viewport.scrollTop=794;fireEvent.scroll(viewport);expect(screen.getByRole("button").className).toContain("enter");
  act(()=>vi.advanceTimersByTime(430));expect(screen.getByRole("button").className).toContain("idle");
  fireEvent.wheel(viewport,{deltaY:-4});fireEvent.scroll(viewport);expect(screen.getByRole("button").className).toContain("idle");
  viewport.scrollTop=800;fireEvent.scroll(viewport);expect(screen.getByRole("button").className).toContain("exit");
});
