// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PermissionCard, DependencyPermissions, DependencyRequests } from "./dependency-permissions";

const mocks = vi.hoisted(() => ({ command: vi.fn(), open: vi.fn(), listen: vi.fn() }));
vi.mock("./model", () => ({ command: mocks.command }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const request = {
  id: "request-1", draftId: "draft-1", modName: "Example mod", packName: "My pack",
  scope: { packId: "pack-1", targetId: "fabric-1.21.1", action: "Install", sources: ["Modrinth"] },
  explanation: "Install validated runtime mods in My pack.", fingerprint: "immutable-plan-hash",
  items: [{ name: "Trinkets", version: "3.10.0", source: "Modrinth project 5aaWibi9", reason: "Required by Example mod", filename: "trinkets.jar" }], changes: [] as string[], choices: [] as string[],
};
describe("dependency permission cards", () => {
  it("shows the concrete plan and sends a one-time decision", async () => {
    mocks.command.mockResolvedValue(undefined); const done = vi.fn(); render(<PermissionCard request={request} onDone={done} />);
    expect(screen.getByText("Trinkets")).toBeTruthy(); expect(screen.getByText("3.10.0")).toBeTruthy();
    expect(screen.getByText(/Required by Example mod/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect(mocks.command).toHaveBeenCalledWith("dependency_decide", { id: request.id, decision: { decision: "allow", fingerprint: request.fingerprint, choice: null, file: null } }); expect(done).toHaveBeenCalledOnce();
  });
  it("keeps sourcing cards brief and uses neutral action buttons", () => {
    render(<PermissionCard request={{...request, scope:{...request.scope,action:"Search and stage"},explanation:"Long backend explanation with hashes and inspection details"}} onDone={() => {}} />);
    expect(screen.getByRole("button",{name:"Deny"}).className).toBe("secondary");
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByText("Trinkets")).toBeNull();
    expect((screen.getByText("Details").parentElement as HTMLDetailsElement).open).toBe(false);
  });
  it("denies and prevents repeated decisions while the backend responds", async () => {
    let finish!: () => void; mocks.command.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    render(<PermissionCard request={request} onDone={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Deny" })); fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(mocks.command).toHaveBeenCalledTimes(1); expect(mocks.command.mock.calls[0][1].decision.decision).toBe("deny"); finish();
  });
  it("opens from the arrow with keyboard, dismisses with Escape and restores focus", async () => {
    render(<PermissionCard request={request} onDone={() => {}} />); const arrow = screen.getByRole("button", { name: "More dependency permission options" }); arrow.focus();
    await userEvent.keyboard("{ArrowDown}"); expect(screen.getByRole("menuitem", { name: "Always Allow" })).toBe(document.activeElement);
    await userEvent.keyboard("{Escape}"); expect(screen.queryByRole("menu")).toBeNull(); expect(document.activeElement).toBe(arrow);
  });
  it("describes Always Allow scope and submits the backend permission", async () => {
    mocks.command.mockResolvedValue(undefined); render(<PermissionCard request={request} onDone={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "More dependency permission options" }));
    const item = screen.getByRole("menuitem", { name: "Always Allow" });expect(item.getAttribute("aria-describedby")).toBe(`dependency-scope-${request.id}`);
    await userEvent.click(item);expect(mocks.command.mock.calls[0][1].decision.decision).toBe("always");
  });
  it("accepts an official file only when automatic download is unavailable", async () => {
    mocks.command.mockResolvedValue(undefined);mocks.open.mockResolvedValue("C:\\mods\\official.jar");
    render(<PermissionCard request={{ ...request, choices: ["Provide local JAR"] }} onDone={() => {}} />);
    expect((screen.getByRole("button", { name: "Allow" }) as HTMLButtonElement).disabled).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Provide official JAR" }));
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));expect(mocks.command.mock.calls[0][1].decision.file).toBe("C:\\mods\\official.jar");
    expect(screen.queryByRole("button", { name: "More dependency permission options" })).toBeNull();
  });
  it("shows replacements and stale errors without reporting success", async () => {
    mocks.command.mockRejectedValue(new Error("This dependency request is stale"));const done = vi.fn();
    render(<PermissionCard request={{ ...request, changes: ["Replace owned old.jar"] }} onDone={done} />);
    expect(screen.getByText("Replace owned old.jar")).toBeTruthy();expect(screen.queryByRole("button", { name: "More dependency permission options" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));expect(await screen.findByRole("alert")).toBeTruthy();expect(done).not.toHaveBeenCalled();
  });
  it("rehydrates pending requests after reload and isolates conversations", async () => {
    mocks.listen.mockResolvedValue(() => {});mocks.command.mockResolvedValue([request, { ...request, id: "other", draftId: "draft-2", modName: "Other mod" }]);
    render(<DependencyRequests draftId="draft-1" />);expect(await screen.findByText("Example mod")).toBeTruthy();expect(screen.queryByText("Other mod")).toBeNull();
  });
  it("lists persisted scopes and revokes a grant in settings", async () => {
    mocks.command.mockImplementation((name: string) => Promise.resolve(name === "dependency_grants" ? [{ id: "grant-1", description: "Install runtime dependencies in My pack only", scope: request.scope }] : undefined));
    render(<DependencyPermissions />);await userEvent.click(await screen.findByRole("button", { name: "Revoke" }));
    expect(mocks.command).toHaveBeenCalledWith("dependency_revoke", { id: "grant-1" });await waitFor(() => expect(screen.getByText("No saved dependency permissions.")).toBeTruthy());
  });
});
