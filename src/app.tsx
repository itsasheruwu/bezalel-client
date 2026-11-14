import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { FolderOpen, Hammer, House, Settings2 } from "lucide-react";
import { Brand, Empty, Feedback, Modal, type Notice } from "./components";
import { SlidingHighlights } from "./sliding-highlights";
import { ContextMenuScope } from "./context-menu";
import { Home } from "./home";
import { Onboarding } from "./onboarding";
import { Avatar } from "./profile";
import { Studio } from "./studio";
import { SettingsPage, type SettingsSection } from "./settings";
import { APP_VERSION, command, withProgress, type Snapshot } from "./model";

export type Operate = <T>(label: string, action: () => Promise<T>, success?: string) => Promise<T | undefined>;
type Route = "home" | "studio" | "settings";

const nav = [
  { id: "home", label: "Home", icon: House },
  { id: "studio", label: "Create Mod", icon: Hammer },
  { id: "settings", label: "Settings", icon: Settings2 },
] as const;

export function App() {
  const [request, setRequest] = useState<{ requestId: string; method: string; params: { questions?: { id: string; header?: string; question: string; options?: { label: string; description?: string }[] }[]; [key: string]: unknown } }>();
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [requestError, setRequestError] = useState("");
  useEffect(() => {
    const subscription = listen<NonNullable<typeof request>>("codex-request", event => { setAnswers({}); setRequestError(""); setRequest(event.payload); });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);
  const respond = async (approved: boolean) => {
    if (!request) return;
    let result: unknown;
    if (request.method === "item/tool/requestUserInput") result = { answers: Object.fromEntries(Object.entries(answers).map(([id, answer]) => [id, { answers: [answer] }])) };
    else if (request.method === "mcpServer/elicitation/request") result = { action: approved ? "accept" : "decline", content: approved ? JSON.parse(answers.content || "{}") : null };
    else if (request.method === "item/permissions/requestApproval") result = { permissions: approved ? request.params.permissions ?? {} : {}, scope: "turn" };
    else result = { decision: approved ? "accept" : "decline" };
    try { await command("studio_respond", { requestId: request.requestId, result }); setRequest(undefined); }
    catch (error) { setRequestError(String(error)); }
  };
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [route, setRoute] = useState<Route>("home");
  const [packId, setPackId] = useState<string>();
  const [draftId, setDraftId] = useState<string>();
  const [draftPackId, setDraftPackId] = useState<string>();
  const [settingsView, setSettingsView] = useState({ section: "Accounts" as SettingsSection, key: 0 });
  const [busy, setBusy] = useState("");
  const [notice, setNotice] = useState<Notice>(null);

  const refresh = async () => setSnapshot(await command<Snapshot>("snapshot"));

  const operate: Operate = async (label, action, success) => {
    setBusy(label);
    setNotice(null);
    try {
      const result = await action();
      await refresh();
      if (success) setNotice({ kind: "success", text: success });
      return result;
    } catch (error) {
      setNotice({ kind: "error", text: String(error) });
      await refresh().catch(() => {});
    } finally {
      setBusy("");
    }
  };

  useEffect(() => {
    let open = true;
    void (async () => {
      try {
        const initial = await command<Snapshot>("snapshot");
        if (!open) return;
        setSnapshot(initial);
        if (initial.syncUser) {
          const packs = await command<Snapshot["packs"]>("refresh_packs");
          if (initial.settings.autoSync) {
            for (const pack of packs) {
              if (!open) return;
              if (pack.ownerId && pack.ownerId === pack.trustedOwnerId && pack.activeRevision !== null) {
                await withProgress("sync_pack", { id: pack.id }, value => open && setBusy(value))
                  .catch(error => open && setNotice({ kind: "error", text: `${pack.name} · ${error}` }));
              }
            }
          }
          if (open) await refresh();
        }
      } catch (error) {
        if (open) setNotice({ kind: "error", text: String(error) });
      } finally {
        if (open) setBusy("");
      }
    })();
    return () => { open = false; };
  }, []);

  useEffect(() => {
    document.documentElement.style.fontSize = `${snapshot?.settings.textScale ?? 100}%`;
    document.documentElement.classList.toggle("reduce-motion", Boolean(snapshot?.settings.reducedMotion));
  }, [snapshot?.settings.textScale, snapshot?.settings.reducedMotion]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const preference = snapshot?.settings.theme ?? "system";
      const theme = preference === "system" ? (media.matches ? "dark" : "light") : preference;
      document.documentElement.dataset.theme = theme;
      document.documentElement.style.colorScheme = theme;
      void command("apply_app_theme", { theme: preference }).catch(() => {});
    };
    apply(); media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [snapshot?.settings.theme]);

  const switchRoute = (next: Route) => {
    setNotice(null);
    if (next === "home") setPackId(undefined);
    if (next === "studio") setDraftPackId(undefined);
    setRoute(next);
  };
  const openSettings = (section: SettingsSection = "Accounts") => {
    setSettingsView(view => ({ section, key: view.key + 1 }));
    switchRoute("settings");
  };

  const freshLibrary = snapshot && !snapshot.syncUser && !snapshot.microsoftName && !snapshot.chatgptConnected
    && snapshot.packs.length === 0 && snapshot.drafts.length === 0;
  const needsProfile = snapshot?.syncProfile?.setupComplete === false;
  if (snapshot && !snapshot.settings.onboardingComplete && (freshLibrary || needsProfile)) {
    return <Onboarding {...{ snapshot, busy, notice, operate }} />;
  }

  return <div className={`shell ${route === "studio" ? "studio-shell" : ""}`}>
    {request && <Modal title={request.method === "item/tool/requestUserInput" ? "Codex needs your input" : "Codex request"} dismissible={false} onClose={() => {}}>
      <div className="stack">
        {request.params.questions?.map(question => <label key={question.id}>{question.question}
          {question.options && <div className="actions">{question.options.map(option => <button key={option.label} onClick={() => setAnswers(value => ({ ...value, [question.id]: option.label }))}>{option.label}</button>)}</div>}
          <input value={answers[question.id] ?? ""} onChange={event => setAnswers(value => ({ ...value, [question.id]: event.target.value }))} />
        </label>)}
        {!request.params.questions && <pre>{JSON.stringify(request.params, null, 2)}</pre>}
        {request.method === "mcpServer/elicitation/request" && <label>Response JSON<textarea value={answers.content ?? "{}"} onChange={event => setAnswers({ content: event.target.value })} /></label>}
        {requestError && <p role="alert">{requestError}</p>}
        <div className="actions end"><button onClick={() => { if (draftId) void command("studio_cancel", { id: draftId }); setRequest(undefined); }}>Cancel turn</button>{request.method !== "item/tool/requestUserInput" && <button onClick={() => void respond(false)}>Decline</button>}<button className="primary" onClick={() => void respond(true).catch(error => setRequestError(String(error)))}>Continue</button></div>
      </div>
    </Modal>}

    <aside className="rail">
      <div className="brand"><Brand /><span>Bezalel</span></div>
      <nav aria-label="Main"><SlidingHighlights className="rail-highlights" selectedKey={route}>
        {nav.map(({ id, label, icon: Icon }) => <ContextMenuScope key={id} label={label} actions={[
          { id: "open", label: `Open ${label}`, icon: Icon, run: () => id === "settings" ? openSettings() : switchRoute(id) },
          ...(id === "studio" ? [{ id: "new", label: "New conversation", icon: Hammer, run: () => { setDraftId(undefined); switchRoute("studio"); } }] : []),
        ]}><button data-highlight-key={id} aria-current={route === id ? "page" : undefined}
          title={label} aria-label={label} onClick={() => id === "settings" && route !== "settings" ? openSettings() : switchRoute(id)}><Icon size={20} strokeWidth={2.2} /><span>{label}</span></button></ContextMenuScope>)}
      </SlidingHighlights></nav>
      <div className="rail-bottom">
        {snapshot?.syncProfile && <button type="button" className="rail-profile" title={snapshot.syncProfile.name}
          aria-label={"Account settings for " + snapshot.syncProfile.name} onClick={() => openSettings()}>
          <Avatar value={snapshot.syncProfile.avatar} /><span>{snapshot.syncProfile.name}</span>
        </button>}
        <div className="rail-foot"><FolderOpen size={14} /><span>Alpha · {APP_VERSION}</span></div>
      </div>
    </aside>
    <main className={`main route-${route}`} id="main">
      {!snapshot ? <div className="page"><h1>Bezalel</h1><Feedback notice={notice} busy={notice ? undefined : "Opening your library…"} />
        {notice && <Empty title="Desktop app required">Run Bezalel through Tauri to open the local library.</Empty>}</div>
        : route === "home" ? <Home {...{ snapshot, packId, setPackId, busy, notice, operate, setBusy, openSettings }}
            editMod={id => { setDraftId(id); setRoute("studio"); }} startMod={id => { setDraftId(undefined); setDraftPackId(id); setRoute("studio"); }} />
        : route === "studio" ? <Studio {...{ snapshot, draftId, setDraftId, draftPackId, busy, notice, operate, setBusy, openSettings, refresh }} />
        : <SettingsPage key={settingsView.key} initialSection={settingsView.section} {...{ snapshot, busy, notice, operate }} />}
    </main>
  </div>;
}
