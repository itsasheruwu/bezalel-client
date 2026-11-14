import { useEffect, useRef, useState } from "react";
import { ChevronDown, PackageCheck, Search, ShieldCheck } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { command } from "./model";

type Scope = { packId: string; targetId: string; action: string; sources: string[] };
type Request = { id: string; draftId: string | null; scope: Scope; modName: string; packName: string; explanation: string; items: { name: string; version: string; source: string; reason: string; filename: string }[]; changes: string[]; choices: string[]; fingerprint: string };
type Grant = { id: string; description: string; scope: Scope };

export function DependencyRequests({ draftId, packId }: { draftId?: string; packId?: string }) {
  const [requests, setRequests] = useState<Request[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    const refresh = () => command<Request[]>("dependency_requests").then(rows => { if (active) { setRequests(rows); setError(""); } }).catch(e => { if (active) setError(String(e)); });
    // Subscribe before fetching, so requests cannot fall into an event/list race on reload.
    const subscription = listen("dependency-requests-changed", () => { void refresh(); });
    void subscription.then(refresh);
    return () => { active = false; void subscription.then(stop => stop()); };
  }, []);
  const rows = requests.filter(row => draftId ? row.draftId === draftId : !row.draftId && row.scope.packId === packId);
  return <div className="dependency-requests" aria-live="polite">{error && <p role="alert">{error}</p>}{rows.map(request => <PermissionCard key={request.id} request={request} onDone={() => setRequests(current => current.filter(row => row.id !== request.id))} />)}</div>;
}

export function PermissionCard({ request, onDone }: { request: Request; onDone: () => void }) {
  const [menu, setMenu] = useState(false);
  const [busy, setBusy] = useState(false);
  const sending = useRef(false);
  const [error, setError] = useState("");
  const [choice, setChoice] = useState("");
  const [file, setFile] = useState<string>();
  const split = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menuItem = useRef<HTMLButtonElement>(null);
  const scopeId = `dependency-scope-${request.id}`;
  const canAlways = !request.choices.length && !request.changes.length;
  useEffect(() => { if (menu) menuItem.current?.focus(); }, [menu]);
  useEffect(() => {
    if (!menu) return;
    const dismiss = (event: PointerEvent) => { if (event.target instanceof Node && !split.current?.contains(event.target)) setMenu(false); };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [menu]);
  const closeMenu = () => { setMenu(false); trigger.current?.focus(); };
  const decide = async (decision: "deny" | "allow" | "always") => {
    if (sending.current) return;
    sending.current = true; setBusy(true); setError(""); setMenu(false);
    try {
      await command("dependency_decide", { id: request.id, decision: { decision, fingerprint: request.fingerprint, choice: choice || null, file: file ?? null } });
      onDone();
    } catch (e) { setError(String(e)); }
    finally { sending.current = false; setBusy(false); }
  };
  const select = async (value: string) => {
    setChoice(value); setFile(undefined);
    if (value === "Provide local JAR") {
      const selected = await open({ filters: [{ name: "Minecraft runtime mod", extensions: ["jar"] }] });
      if (typeof selected === "string") setFile(selected);
    }
  };
  return <section className="dependency-card" aria-label={`${request.scope.action} dependencies for ${request.modName}`} aria-busy={busy}>
    <div className="dependency-heading">{request.scope.action === "Install" ? <PackageCheck size={16} /> : <Search size={16} />}<strong>{request.scope.action === "Install" ? `Install ${request.items.length} ${request.items.length === 1 ? "mod" : "mods"}?` : request.choices.length ? "Official file needed" : "Find dependencies?"}</strong></div>
    <p className="dependency-context"><span><strong>{request.modName}</strong> → {request.packName}</span><span>{request.scope.sources.join(", ")}</span></p>
    {request.scope.action === "Install" && !!request.items.length && <ul className="dependency-items">{request.items.map((item, i) => <li key={`${item.filename}-${i}`}><strong>{item.name}</strong><span>{item.version}</span><span title={item.source}>{item.source.split(" project ")[0]}</span></li>)}</ul>}
    {!!request.changes.length && <div className="dependency-changes">{request.changes.map(change => <p key={change}>{change}</p>)}</div>}
    {!!request.choices.length && <div className="dependency-file"><p>{request.explanation}</p><button className="secondary" disabled={busy} onClick={() => void select("Provide local JAR")}>{file ? "Change file" : "Provide official JAR"}</button>{file && <span className="path">{file}</span>}</div>}
    {error && <p className="dependency-error" role="alert">{error}</p>}
    <footer className="dependency-actions"><details className="dependency-details"><summary>Details</summary><div><p>{request.explanation}</p><p>{request.scope.targetId}</p>{request.items.map((item, i) => <p key={i}>{item.name} {item.version} · {item.source}<br />{item.reason} · {item.filename}</p>)}</div></details><button className="secondary" disabled={busy} onClick={() => void decide("deny")}>Deny</button><div ref={split} className="dependency-split" onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setMenu(false); }} onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); closeMenu(); } }}>
      <button className="primary" disabled={busy || (!!request.choices.length && (!choice || (choice === "Provide local JAR" && !file)))} onClick={() => void decide("allow")}>Allow</button>
      {canAlways && <button ref={trigger} className="primary dependency-arrow" aria-label="More dependency permission options" aria-haspopup="menu" aria-expanded={menu} aria-controls={`dependency-menu-${request.id}`} disabled={busy} onClick={() => setMenu(!menu)} onKeyDown={event => { if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); setMenu(true); } }}><ChevronDown size={15} /></button>}
      {menu && <div className="dependency-menu" role="menu" id={`dependency-menu-${request.id}`} onKeyDown={event => { if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) { event.preventDefault(); menuItem.current?.focus(); } }}>{canAlways && <p className="dependency-menu-scope" id={scopeId}>Only {request.packName} · {request.scope.targetId} · {request.scope.sources.join(", ")}. Revoke in Settings.</p>}<button className="secondary" ref={menuItem} role="menuitem" aria-describedby={scopeId} onClick={() => void decide("always")}>Always Allow</button></div>}
    </div></footer>
  </section>;
}

export function DependencyPermissions() {
  const [grants, setGrants] = useState<Grant[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<string>();
  useEffect(() => { void command<Grant[]>("dependency_grants").then(setGrants).catch(e => setError(String(e))); }, []);
  const revoke = async (id: string) => {
    setBusy(id); setError("");
    try { await command("dependency_revoke", { id }); setGrants(current => current.filter(row => row.id !== id)); }
    catch (e) { setError(String(e)); } finally { setBusy(undefined); }
  };
  return <div className="stack"><h2 className="settings-subheading"><ShieldCheck size={20} />Dependency permissions</h2><p>Saved permissions apply only to runtime mod dependencies, the named modpack, target, action and sources. They never approve replacements or arbitrary software.</p>{error && <p role="alert">{error}</p>}{grants.length ? grants.map(grant => <div className="setting compact" key={grant.id}><div><p>{grant.description}</p><p className="path">Modpack {grant.scope.packId}</p></div><button disabled={!!busy} onClick={() => void revoke(grant.id)}>Revoke</button></div>) : <p className="muted">No saved dependency permissions.</p>}</div>;
}
