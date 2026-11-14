import { listen } from "@tauri-apps/api/event";
import { Fragment, useEffect, useRef, useState, type FormEvent } from "react";
import { ArrowDownToLine, ArrowRight, Check, Copy, FolderOpen, ExternalLink, FileCode2, Hammer, MessageCircle, Plus, Sparkles, Square, Trash2, ArrowUp, MoreHorizontal, Package, ChevronDown, X, LoaderCircle } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { Operate } from "./app";
import { Brand, Feedback, Modal, type Notice } from "./components";
import { command, loaderName, targetName, withProgress, type AgentModel, type ModelFamily, type ModEvent, type Change, type Draft, type Snapshot } from "./model";

import { JumpToLatest, useChatScroll } from "./chat-scroll";
import { SlidingHighlights } from "./sliding-highlights";
import { ContextMenuScope } from "./context-menu";
import { DependencyRequests } from "./dependency-permissions";
import { AgentControls } from "./agent-controls";
import { ContextControl } from "./context-control";
import { InstalledModControl } from "./installed-mod-control";
import { ToolCalls, type ToolItem, type ToolActivity } from "./studio-activity";
import { MessageBody } from "./message-body";

type BuildRequest = { id: string; title: string; brief: string };
type BuildResponse = { status: "complete"; draft: Draft } | { status: "consentRequired" };

export function ModEventSeparator({ event }: { event: ModEvent }) {
  const text = event.kind === "moved" ? "Mod was moved to" : "Mod was deleted from";
  return <div className="mod-event-separator" role="separator" aria-label={`${text} ${event.packName}`}><span>{text} <strong>{event.packName}</strong></span></div>;
}

export function Studio({ snapshot, draftId, setDraftId, draftPackId, busy, notice, operate, setBusy, openSettings, refresh }: {
  snapshot: Snapshot; draftId?: string; setDraftId: (id?: string) => void; draftPackId?: string; busy: string; notice: Notice;
  operate: Operate; setBusy: (value: string) => void; openSettings: () => void; refresh: () => Promise<void>;
}) {
  const draft = snapshot.drafts.find(item => item.id === draftId);
  const [title, setTitle] = useState("");
  const [packId, setPackId] = useState(draftPackId ?? "");
  const [targetId, setTargetId] = useState(snapshot.targets[0]?.id ?? "");
  const [mode, setMode] = useState<"guided" | "direct">("guided");
  const [prompt, setPrompt] = useState("");
  const [idea, setIdea] = useState(draft?.idea ?? "");
  const [pending, setPending] = useState("");
  const [activity, setActivity] = useState<ToolActivity[]>([]);
  const [models, setModels] = useState<AgentModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState("");
  const [family, setFamily] = useState<ModelFamily>(snapshot.settings.modelFamily ?? "Sol");
  const [effort, setEffort] = useState(snapshot.settings.reasoningEffort ?? "high");
  const [preferencesSaving, setPreferencesSaving] = useState(false);
  const [preferencesError, setPreferencesError] = useState("");
  const preferenceRevision = useRef(0);
  const preferenceQueue = useRef<Promise<unknown>>(Promise.resolve());
  const storedPreference = useRef({ family, effort });
  const [attachments, setAttachments] = useState<{ name: string; path: string }[]>([]);

  const [stream, setStream] = useState("");
  const [brief, setBrief] = useState(draft?.brief ?? "");
  const [deletingDraft, setDeletingDraft] = useState<Draft>();
  const [sourceFiles, setSourceFiles] = useState<Change[]>();
  const [sourcePath, setSourcePath] = useState("");
  const [sourceText, setSourceText] = useState("");
  const editSources = async () => {
    if (!draft) return;
    const files = await operate("Reading mod sources...", () => command<Change[]>("mod_sources", { id: draft.id }));
    if (files) { setSourceFiles(files); setSourcePath(files[0]?.path ?? "src/main/java/Mod.java"); setSourceText(files[0]?.content ?? ""); }
  };
  const [showChanges, setShowChanges] = useState(false);
  const [submit, setSubmit] = useState(false);
  const [toolsConsent, setToolsConsent] = useState(false);
  const [toolsState, setToolsState] = useState<"idle" | "preparing" | "ready" | "consentRequired" | "error">("idle");
  const [toolsProgress, setToolsProgress] = useState("");
  const [toolsError, setToolsError] = useState("");
  const toolsGeneration = useRef(0);
  const [builderConsent, setBuilderConsent] = useState<BuildRequest>();
  const [working, setWorking] = useState<"chat" | "build">();
  const { viewport, content, detached, jump } = useChatScroll(draft?.id);
  const locked = snapshot.packs.find(item => item.id === packId);
  const selection = snapshot.targets.find(item => item.id === (locked?.targetId ?? targetId));
  const target = snapshot.targets.find(item => item.id === draft?.targetId);
  const destination = snapshot.packs.find(item => item.id === draft?.packId);
  const canSubmit = destination?.ownerId && ["owner", "editor"].includes(destination.role);
  const installed = draft?.installedMod;
  const installedPack = snapshot.packs.find(item => item.id === installed?.packId);
  const modEventsAt = (afterMessage: number) => draft?.modEvents?.filter(event => event.afterMessage === afterMessage).map((event, index) => <ModEventSeparator key={`${afterMessage}-${index}`} event={event} />);
  const disabled = Boolean(busy);

  const prepareTools = async (approved = false) => {
    if (!target) return;
    const generation = ++toolsGeneration.current;
    setToolsState("preparing"); setToolsError(""); setToolsProgress("Preparing mod tools...");
    try {
      const result = await withProgress<"ready" | "consentRequired">("prepare_mod_tools", { targetId: target.id, acceptMinecraftEula: approved }, value => { if (generation === toolsGeneration.current) setToolsProgress(value); });
      if (generation === toolsGeneration.current) setToolsState(result);
    } catch (error) { if (generation === toolsGeneration.current) { setToolsError(String(error)); setToolsState("error"); } }
  };
  useEffect(() => {
    ++toolsGeneration.current;
    setToolsState(snapshot.builderEulaAccepted ? "idle" : "consentRequired"); setToolsError("");
    if (target && snapshot.builderEulaAccepted) void prepareTools();
    else if (target) setToolsConsent(true);
    return () => { ++toolsGeneration.current; };
  }, [target?.id]);

  useEffect(() => { setBrief(draft?.brief ?? ""); }, [draft?.id, draft?.brief]);
  useEffect(() => {
    setIdea(draft?.idea ?? "");
    setPrompt(draft?.idea ?? "");
  }, [draft?.id, draft?.idea]);

  useEffect(() => { if (draftPackId) setPackId(draftPackId); }, [draftPackId]);

  const loadModels = async () => {
    if (!snapshot.chatgptConnected) return;
    setModelsLoading(true); setModelsError("");
    try { setModels(await command<AgentModel[]>("agent_models")); }
    catch (error) { setModelsError(String(error)); }
    finally { setModelsLoading(false); }
  };
  useEffect(() => { void loadModels(); }, [snapshot.chatgptConnected]);
  const updatePreferences = (nextFamily: ModelFamily, nextEffort: string) => {
    setFamily(nextFamily); setEffort(nextEffort); setPreferencesError(""); setPreferencesSaving(true);
    const revision = ++preferenceRevision.current;
    preferenceQueue.current = preferenceQueue.current.catch(() => {}).then(async () => {
      await command("save_agent_preferences", { modelFamily: nextFamily, reasoningEffort: nextEffort });
      storedPreference.current = { family: nextFamily, effort: nextEffort };
      await refresh();
    });
    void preferenceQueue.current.catch(error => { if (revision === preferenceRevision.current) { setPreferencesError(String(error)); setFamily(storedPreference.current.family); setEffort(storedPreference.current.effort); } }).finally(() => { if (revision === preferenceRevision.current) setPreferencesSaving(false); });
  };
  useEffect(() => { setActivity([]); setAttachments([]); }, [draft?.id]);
  useEffect(() => {
    const subscription = listen<{ method: string; params: { threadId?: string; item?: ToolItem } }>("codex-event", event => {
      if (draft?.threadId && event.payload.params.threadId && event.payload.params.threadId !== draft.threadId) return;
      if (event.payload.method !== "item/started" && event.payload.method !== "item/completed") return;
      const item = event.payload.params.item;
      if (!item || !["commandExecution", "fileChange", "webSearch", "mcpToolCall", "dynamicToolCall"].includes(item.type)) return;
      setActivity(previous => { const entry = { ...item, complete: event.payload.method === "item/completed" }; const index = previous.findIndex(row => row.id === item.id); return index < 0 ? [...previous.slice(-79), entry] : previous.map((row, i) => i === index ? { ...row, ...entry } : row); });
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [draft?.id, draft?.threadId]);

  const attachFile = async () => {
    if (!draft || disabled) return;
    const source = await open({ multiple: false });
    if (typeof source !== "string") return;
    const file = await operate("Attaching file...", () => command<{ name: string; path: string }>("attach_studio_file", { id: draft.id, source }));
    if (file) setAttachments(value => [...value, file]);
  };

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const result = await operate("Opening a new draft…", () => command<Draft>("create_draft", {
      title, packId: locked?.id ?? null, targetId: selection?.id, mode,
    }));
    if (result) { setTitle(""); setDraftId(result.id); }
  };
  const send = async (text: string) => {
    if (!draft || disabled || preferencesSaving || !text.trim() || !models.find(model => model.family === family)?.modelId) return;
    const message = text.trim() + (attachments.length ? "\n\nAttached project files (read with tools):\n" + attachments.map(file => `- ${file.path}`).join("\n") : "");
    setActivity([]); setPrompt(""); setPending(message); setStream(""); setWorking("chat");
    const result = await operate("Codex is working…", () => withProgress<Draft>("studio_chat", { id: draft.id, message }, delta => setStream(value => value + delta)));
    if (!result) setPrompt(text);
    else setAttachments([]);
    setPending(""); setStream(""); setWorking(undefined);
  };
  const build = async (sourceOnly = false, approved?: BuildRequest) => {
    const request = approved ?? (draft && { id: draft.id, title: draft.title, brief: brief.trim() });
    if (disabled || !request?.brief) return;
    const args = { id: request.id, brief: request.brief };
    setWorking("build");
    try {
      if (sourceOnly) {
        await operate("Preparing source generation…", () => withProgress<Draft>("generate_mod_source", args, setBusy));
        return;
      }
      const result = await operate("Checking the mod builder…", () => withProgress<BuildResponse>(
        "build_mod", { ...args, acceptMinecraftEula: Boolean(approved) }, setBusy,
      ));
      if (result?.status === "consentRequired") setBuilderConsent(request);
    } finally {
      setWorking(undefined);
    }
  };
  const exportJar = async () => {
    if (!draft?.build) return;
    const path = await save({ defaultPath: draft.build.jarPath.split(/[\\/]/).pop(), filters: [{ name: "Minecraft mod", extensions: ["jar"] }] });
    if (path) await operate("Exporting JAR…", () => command("export_jar", { id: draft.id, destination: path }), "Mod JAR exported.");
  };
  const submitMod = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!draft || !destination) return;
    const data = new FormData(event.currentTarget);
    const result = await operate("Sending the mod to the pack…", () => command("submit_mod", {
      id: destination.id, draftId: draft.id, side: data.get("side"), notes: data.get("notes"),
    }), "Mod submitted for owner review.");
    if (result) setSubmit(false);
  };

  return <div className="studio-grid">
    <aside className="draft-rail">
      <div className="section-heading"><h2>Conversations</h2><button className="icon-button" aria-label="New mod" onClick={() => setDraftId()}><Plus size={21} /></button></div>
      {snapshot.drafts.length ? <SlidingHighlights className="conversation-highlights" selectedKey={draftId}>{snapshot.drafts.map(item => <ContextMenuScope key={item.id} label={item.title} actions={[
        { id: "open", label: "Open conversation", icon: MessageCircle, run: () => setDraftId(item.id) },
        { id: "copy-title", label: "Copy title", icon: Copy, run: () => navigator.clipboard.writeText(item.title) },
        { id: "folder", label: "Open project folder", icon: FolderOpen, disabled, run: () => operate("Opening project...", () => command("open_mod_workspace", { id: item.id })) },
        { id: "delete", label: "Delete conversation…", icon: Trash2, separator: true, danger: true, disabled, run: () => setDeletingDraft(item) },
      ]}><div className="draft-row" data-highlight-key={item.id}>
        <button className="draft-item" aria-pressed={item.id === draftId} onClick={() => setDraftId(item.id)}>
          <span><strong>{item.title}</strong><span className="version">{item.messages.at(-1)?.text.replace(/\[([^\]]+)\]\([^)]*\)/g, "$1").replace(/[*`#]/g, "").slice(0, 56) || targetName(snapshot.targets.find(t => t.id === item.targetId))}</span></span>
        </button>
        <button className="icon-button draft-delete" aria-label={`Delete ${item.title} conversation`} title="Delete mod conversation" disabled={disabled} onClick={() => setDeletingDraft(item)}><Trash2 size={15} /></button>
      </div></ContextMenuScope>)}</SlidingHighlights> : <p>Mod conversations are saved here.</p>}
    </aside>
    {!draft ? <div className="page studio-setup">
      <header className="page-heading"><div><h1>New mod</h1><p>Choose what your mod will run on.</p></div><Hammer className="heading-icon" size={29} /></header>
      <form className="form setup-form" onSubmit={create}>
        <label>Mod name<span className="required-hint" aria-hidden="true">Required</span>
          <input className="name-input" value={title} onChange={event => setTitle(event.target.value)} placeholder="e.g. Glowcap Lanterns" maxLength={64} required />
        </label>
        <label>Pack<select value={packId} onChange={event => setPackId(event.target.value)}><option value="">Standalone draft</option>
          {snapshot.packs.map(item => <option value={item.id} key={item.id}>{item.name}</option>)}</select></label>
        {locked ? <p className="inherited-target"><strong>{targetName(selection)}</strong><span>Inherited from {locked.name}</span></p> : <>
          <fieldset><legend>Loader</legend><div className="choice-row">{snapshot.targets.map(item =>
            <button type="button" key={item.id} className="choice" aria-pressed={selection?.loader === item.loader}
              onClick={() => setTargetId(item.id)}>{loaderName(item.loader)}</button>)}</div></fieldset>
          <label>Minecraft version<select value={selection?.id ?? ""} onChange={event => setTargetId(event.target.value)}>
            {snapshot.targets.filter(item => item.loader === selection?.loader).map(item => <option key={item.id} value={item.id}>{item.minecraftVersion}</option>)}
          </select></label>
        </>}
        <fieldset><legend>Chat mode</legend><div className="choice-row mode-choice">
          <button type="button" className="choice" aria-pressed={mode === "direct"} onClick={() => setMode("direct")}><MessageCircle size={19} /><span><strong>Direct chat</strong><span>Describe your mod in chat.</span></span></button>
          <button type="button" className="choice" aria-pressed={mode === "guided"} onClick={() => setMode("guided")}><Sparkles size={19} /><span><strong>Guided interview</strong><span>Answer a few focused questions.</span></span></button>
        </div></fieldset>
        {!snapshot.chatgptConnected && <p className="field-hint">Save a draft and write your idea on this PC. Connect an eligible ChatGPT plan to chat with Codex or build the mod.</p>}
        <Feedback {...{ busy, notice }} /><div className="actions end"><button className="primary" disabled={disabled || !selection}>Create draft<ArrowRight size={18} /></button></div>
      </form>
    </div> : <section className="chat">
      <header className="chat-head">
        <div className="chat-heading"><h1>{draft.title}</h1><span>{destination?.name ?? "Standalone"} / {targetName(target)}</span></div>
        <div className="chat-header-actions"><button className="text-button compact-build" disabled={disabled || toolsState === "preparing" || !brief.trim() || !snapshot.chatgptConnected} onClick={() => void build()}><Hammer size={14} />{toolsState === "preparing" ? "Preparing..." : "Build"}</button>
          <details className="project-menu"><summary aria-label="Project actions"><MoreHorizontal size={19} /></summary><div><button disabled={disabled} onClick={() => void editSources()}>Edit source</button><button disabled={disabled} onClick={() => void operate("Opening project...", () => command("open_mod_workspace", { id: draft.id }))}>Open project folder</button><button disabled={disabled || !brief.trim()} onClick={() => void build(true)}>Generate source</button><button onClick={() => setDraftId()}>New mod</button><button disabled={disabled} onClick={() => setDeletingDraft(draft)}>Delete conversation</button></div></details>
        </div>
      </header>
      <div ref={viewport} className="conversation" aria-label="Mod conversation"><div ref={content} className="conversation-content">
        {!draft.messages.length && !pending && !working && snapshot.chatgptConnected && <div className="chat-welcome"><Brand /><h2>What do you want to make?</h2><p>{draft.mode === "guided" ? "Share your idea and Codex will ask one question at a time." : "Describe a mechanic, item, block, or change you’d like to play."}</p></div>}
        {draft.messages.map((message, index) => <Fragment key={index}>
          {modEventsAt(index)}
          {!working && index === draft.messages.length - 1 && message.role === "assistant" && <ToolCalls items={activity} />}
          <ContextMenuScope label="Message actions" actions={[{ id: "copy-message", label: "Copy message", icon: Copy, run: () => navigator.clipboard.writeText(message.text) }]}><div className={`message ${message.role}`} tabIndex={0}><div className="message-author">{message.role === "user" ? "You" : "Bezalel"}</div><div className="message-content">{message.role === "assistant" ? <MessageBody text={message.text} disabled={disabled} onExport={draft.build ? () => void exportJar() : undefined} /> : <div className="bubble">{message.text}</div>}
            {index === draft.messages.length - 1 && !pending && message.choices?.length ? <div className="answer-row">{message.choices.map(choice => <button className="secondary" key={choice} disabled={disabled || !snapshot.chatgptConnected} onClick={() => void send(choice)}>{choice}</button>)}</div> : null}
          </div></div></ContextMenuScope>
        </Fragment>)}
        {modEventsAt(draft.messages.length)}
        {pending && <div className="message user"><div className="message-author">You</div><div className="bubble">{pending}</div></div>}
        {working && <ToolCalls items={activity} />}
        <DependencyRequests draftId={draft.id} />
        {stream && <div className="message assistant"><div className="message-author">Bezalel</div><MessageBody text={stream} /></div>}
        {draft.build && <div className="build-artifact"><Package size={21} /><div className="artifact-info"><strong>{draft.build.jarPath.replaceAll("\\", "/").split("/").pop()}</strong><span>{draft.build.compilePassed && draft.build.clientPassed && draft.build.serverPassed ? "Compile, client and server checks passed" : "Build completed with checks to review"}</span><div className="artifact-checks">{[["Compile", draft.build.compilePassed], ["Client", draft.build.clientPassed], ["Server", draft.build.serverPassed]].map(([label, passed]) => <span key={String(label)} data-passed={String(passed)}>{passed ? <Check size={11} /> : <X size={11} />}{label}</span>)}</div>{!!draft.build.diagnostics?.length && <details><summary>Build log</summary><pre>{draft.build.diagnostics.join("\n")}</pre></details>}</div>
          <button className="icon-button" aria-label="Export JAR" title="Export JAR" onClick={() => void exportJar()} disabled={disabled}><ArrowDownToLine size={16} /></button>
          {destination && !destination.ownerId && <InstalledModControl key={draft.id}
            installed={!!installed && installed.sha256 === draft.build.sha256} disabled={disabled}
            canInstall={draft.build.compilePassed && draft.build.clientPassed} packName={installedPack?.name ?? destination.name}
            packs={snapshot.packs.filter(pack => !pack.ownerId && pack.targetId === draft.targetId && pack.id !== installed?.packId)}
            onInstall={() => operate("Installing mod...", () => command<Draft>("install_local_mod", { id: destination.id, draftId: draft.id }), `Installed in ${destination.name}.`)}
            onMove={pack => operate("Moving mod...", () => command<Draft>("move_installed_mod", { draftId: draft.id, packId: pack.id }), `Mod moved to ${pack.name}.`)}
            onDelete={() => operate("Deleting installed mod...", () => command<Draft>("delete_installed_mod", { draftId: draft.id }), "Installed mod deleted. Conversation and source kept.")} />}
          {canSubmit && <button className="primary" disabled={disabled || !draft.build.clientPassed || !draft.build.serverPassed || !draft.build.compilePassed} onClick={() => setSubmit(true)}>Send to pack</button>}
        </div>}
        {(draft.brief || draft.messages.length > 0) && !pending && <details className="source-disclosure"><summary><ChevronDown size={13} />Brief & source</summary><div className="stack"><label>Mod brief<textarea value={brief} onChange={event => setBrief(event.target.value)} rows={3} placeholder="Describe what to build." /></label><div className="actions"><button className="secondary" disabled={disabled || toolsState === "preparing" || !snapshot.chatgptConnected || !brief.trim()} onClick={() => void build()}><Hammer size={14} />{draft.build ? "Rebuild" : "Build mod"}</button><button className="text-button" onClick={() => setShowChanges(true)}>View source changes</button></div></div></details>}
        {!snapshot.chatgptConnected && <div className="panel build-panel draft-idea">
          <form className="form" onSubmit={event => {
            event.preventDefault();
            void operate("Saving mod idea…", () => command("save_draft_idea", { id: draft.id, idea: idea.trim() }), "Mod idea saved on this PC.");
          }}>
            <div>
              <label>Mod idea<textarea value={idea} onChange={event => setIdea(event.target.value)} rows={4} maxLength={16_000}
                placeholder="Describe a mechanic, item, block, or change you’d like to play." aria-describedby="draft-idea-hint" disabled={disabled} /></label>
              <p id="draft-idea-hint" className="field-hint">Your saved idea will be ready to send when you connect ChatGPT.</p>
            </div>
            <div className="actions"><button className="primary" disabled={disabled || idea.trim() === (draft.idea ?? "")}>Save idea</button></div>
            <Feedback {...{ busy, notice }} />
          </form>
          <div className="draft-connection">
            <p><Sparkles size={18} aria-hidden="true" />Connect ChatGPT to chat with Codex and build mods.</p>
            <button type="button" className="secondary" onClick={() => openSettings()}>ChatGPT account settings</button>
          </div>
        </div>}
        </div>
      </div>
      {snapshot.chatgptConnected && <div className="composer-wrap">
        <Feedback {...{ busy, notice }} />
        <form className="composer" onSubmit={event => { event.preventDefault(); void send(prompt); }}>
          <JumpToLatest visible={detached} onJump={jump} />
          <textarea aria-label="Message Bezalel" placeholder="Ask Bezalel something..." value={prompt} onChange={event => setPrompt(event.target.value)} rows={2} maxLength={16_000} disabled={disabled} onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }} />
          {!!attachments.length && <div className="attachment-list">{attachments.map(file => <span key={file.path}><FileCode2 size={12} />{file.name}<button type="button" aria-label={`Remove attachment ${file.name}`} onClick={() => setAttachments(files => files.filter(item => item.path !== file.path))}><X size={12} /></button></span>)}</div>}
          <div className="composer-toolbar"><div className="composer-toolbar-left"><button type="button" className="composer-add" aria-label="Attach file" disabled={disabled} onClick={() => void attachFile()}><Plus size={18} /></button>
            <AgentControls models={models} family={family} effort={effort} disabled={disabled} loading={modelsLoading} error={modelsError} onChange={updatePreferences} onRefresh={() => void loadModels()} />
            {destination && <span className="composer-pack" title={destination.name}>{destination.name}</span>}
          </div>
          <div className="composer-toolbar-right">
            <ContextControl key={draft.id} draftId={draft.id} modelId={models.find(model => model.family === family)?.modelId} usage={draft.contextUsage} />
            {working ? <button type="button" className="send" aria-label="Stop agent" disabled={working === "build" && !busy.includes("Codex")} onClick={() => void command("studio_cancel", { id: draft.id })}><Square size={14} fill="currentColor" /></button> : <button className="send" aria-label="Send message" disabled={disabled || preferencesSaving || !prompt.trim() || !models.find(model => model.family === family)?.modelId}><ArrowUp size={19} /></button>}
          </div>
          </div>
        </form>
        <div className="composer-status" role="status">{toolsState === "ready" ? <><i />Tools ready</> : toolsState === "preparing" ? <><LoaderCircle size={11} className="spin" />{toolsProgress}</> : <button className="text-button" onClick={() => snapshot.builderEulaAccepted ? void prepareTools() : setToolsConsent(true)}>{toolsState === "error" ? "Retry tools setup" : "Prepare mod tools"}</button>}{preferencesSaving && <span>Saving preferences...</span>}</div>
        {toolsError && <details className="setup-error"><summary>Setup details</summary>{toolsError}</details>}
        {preferencesError && <p className="control-error" role="alert">{preferencesError}</p>}

      </div>}
    </section>}
    {toolsConsent && <Modal title="Prepare mod tools" onClose={() => setToolsConsent(false)}>
      <div className="stack"><p>Bezalel will download and prepare the tools for your mod automatically. This is a one-time setup for each loader; you can continue chatting while it runs.</p><p>The startup checks run a temporary Minecraft server. Review and accept the Minecraft EULA to enable them.</p>
        <button className="text-button" onClick={() => void command("open_minecraft_eula")}>Read Minecraft EULA<ExternalLink size={15} /></button>
        <div className="actions end"><button autoFocus onClick={() => setToolsConsent(false)}>Not now</button><button className="primary" onClick={() => { setToolsConsent(false); void prepareTools(true); }}>Accept EULA and prepare</button></div>
      </div>
    </Modal>}
    {builderConsent && <Modal title="Set up the mod builder" onClose={() => setBuilderConsent(undefined)}>
      <div className="stack">
        <p>Bezalel will set up the builder in the background, then build <strong>{builderConsent.title}</strong>. Setup can download several gigabytes. Windows may ask for administrator approval or a restart.</p>
        <p>The builder starts a temporary Minecraft server to check mods. Review the Minecraft EULA before continuing.</p>
        <div className="actions">
          <button type="button" className="text-button" disabled={disabled} onClick={() => void operate(
            "Opening the Minecraft EULA…", () => command("open_minecraft_eula"),
          )}>Read the Minecraft EULA<ExternalLink size={15} aria-hidden="true" /></button>
        </div>
        <Feedback {...{ busy, notice }} />
        <div className="actions end">
          <button type="button" className="secondary" onClick={() => setBuilderConsent(undefined)}>Cancel</button>
          <button type="button" className="primary" disabled={disabled} onClick={() => {
            setBuilderConsent(undefined);
            void build(false, builderConsent);
          }}>Accept and build</button>
        </div>
      </div>
    </Modal>}
    {deletingDraft && <Modal title="Delete mod conversation" onClose={() => setDeletingDraft(undefined)} dismissible={!busy}>
      <div className="stack"><p>Delete {deletingDraft.title} and its conversation from Studio? Project files will be moved to Deleted Projects. Installed mods and exported JARs stay in place.</p>
        <Feedback {...{ busy, notice }} />
        <div className="actions end"><button autoFocus disabled={disabled} onClick={() => setDeletingDraft(undefined)}>Cancel</button><button className="primary" disabled={disabled} onClick={() => void operate("Deleting conversation...", async () => { await command("delete_draft", { id: deletingDraft.id }); if (draftId === deletingDraft.id) { setDraftId(undefined); setSourceFiles(undefined); setActivity([]); } setDeletingDraft(undefined); }, "Mod conversation deleted. Project files were archived.")}>Delete conversation</button></div>
      </div>
    </Modal>}
    {sourceFiles && draft && <Modal title="Edit mod source" onClose={() => setSourceFiles(undefined)} dismissible={!busy}>
      <div className="stack">
        <label>Existing file<select value={sourcePath} onChange={event => { setSourcePath(event.target.value); setSourceText(sourceFiles.find(file => file.path === event.target.value)?.content ?? ""); }}>{sourceFiles.map(file => <option key={file.path} value={file.path}>{file.path}</option>)}</select></label>
        <label>Source path<input value={sourcePath} onChange={event => setSourcePath(event.target.value)} /></label>
        <label>Contents<textarea rows={20} spellCheck={false} value={sourceText} onChange={event => setSourceText(event.target.value)} /></label>
        <Feedback {...{ busy, notice }} />
        <div className="actions end"><button onClick={() => setSourceFiles(undefined)}>Cancel</button><button className="primary" disabled={disabled || !sourcePath} onClick={() => void operate("Saving source...", async () => { await command("save_mod_source", { id: draft.id, path: sourcePath, content: sourceText }); setSourceFiles(undefined); }, "Source saved. Rebuild to update your installed mod.")}>Save source</button></div>
      </div>
    </Modal>}
    {showChanges && draft && <Modal title="Source changes" onClose={() => setShowChanges(false)}><div className="stack">
      {draft.changes.map(change => <details key={change.path}><summary>{change.path}</summary><pre>{change.content ?? "Deleted"}</pre></details>)}
    </div></Modal>}
    {submit && draft && <Modal title="Send to pack" onClose={() => setSubmit(false)}>
      <form className="form" onSubmit={submitMod}>
        <p className="version">{draft.build?.jarPath.split(/[\\/]/).pop()}</p>
        <label>Run on<select name="side"><option value="both">Client and server</option><option value="client">Client only</option><option value="server">Server only</option></select></label>
        <label>Notes<textarea name="notes" maxLength={3000} placeholder="What should the owner test?" /></label>
        <Feedback {...{ busy, notice }} /><div className="actions end"><button type="button" className="secondary" onClick={() => setSubmit(false)}>Cancel</button><button className="primary" disabled={disabled}>Submit for review</button></div>
      </form>
    </Modal>}
  </div>;
}
