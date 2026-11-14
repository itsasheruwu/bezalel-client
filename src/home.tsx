import { useEffect, useRef, useState, type FormEvent } from "react";
import { ArrowDownToLine, ArrowLeft, ArrowRight, CheckCircle2, Clipboard, CloudDownload, FolderOpen, Hammer, History, Play, Plus, Server, ShieldCheck, Trash2, UsersRound } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { Operate } from "./app";
import { AccessCodeDialog } from "./access";
import { Empty, Feedback, Modal, type Notice } from "./components";
import { command, packStatus, targetName, withProgress, type Candidate, type Details, type Invite, type Pack, type Release, type Snapshot } from "./model";
import type { SettingsSection } from "./settings";
import { DependencyRequests } from "./dependency-permissions";
import { ContextMenuScope } from "./context-menu";
import { versionArtwork, type VersionArtwork } from "./version-artwork";

type Props = {
  editMod: (id: string) => void; snapshot: Snapshot; packId?: string; setPackId: (id?: string) => void; busy: string; notice: Notice;
  operate: Operate; setBusy: (value: string) => void; startMod: (id: string) => void; openSettings: (section?: SettingsSection) => void;
};


export function Home({ editMod, snapshot, packId, setPackId, busy, notice, operate, setBusy, startMod, openSettings }: Props) {
  const [deleting, setDeleting] = useState<Pack>();
  const [modal, setModal] = useState<"new" | "join">();
  const [selected, setSelected] = useState<string>();
  const [code, setCode] = useState("");
  const [invite, setInvite] = useState<Invite>();
  const [trust, setTrust] = useState(false);
  const packName = useRef<HTMLInputElement>(null);
  const joinNeedsSignIn = !snapshot.syncUser && snapshot.hostedSharingConfigured;
  const sharingAvailable = Boolean(snapshot.syncUser || snapshot.hostedSharingConfigured);
  const pack = snapshot.packs.find(item => item.id === packId);
  const featured = snapshot.packs.find(item => item.id === selected) ?? snapshot.packs[0];
  const target = (id: string) => snapshot.targets.find(item => item.id === id);
  const featuredArtwork = versionArtwork(featured && target(featured.targetId)?.minecraftVersion);
  const close = () => { setModal(undefined); setInvite(undefined); setTrust(false); setCode(""); };

  const canPlay = (item: Pack) => Boolean(snapshot.microsoftName && target(item.targetId));
  const handoff = async (item: Pack) => {
    await operate("Opening pack in Minecraft Launcher…",
      () => withProgress("prepare_handoff", { id: item.id }, setBusy),
      "Minecraft Launcher opened with your pack ready. Click Play in Java Edition.");
  };
  const play = (item: Pack) => {
    if (canPlay(item)) {
      void operate("Preparing Minecraft…", () => withProgress("play", { id: item.id }, setBusy), "Minecraft started.");
    } else {
      void handoff(item);
    }
  };
  const sync = (item: Pack) => void operate("Checking release…", () => withProgress("sync_pack", { id: item.id }, setBusy), "Pack is up to date.");
  const folder = (item: Pack) => void operate("Opening instance…", () => command("open_instance", { id: item.id }));
  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const item = await operate("Creating pack…", () => command<Pack>("create_pack", {
      name: data.get("name"), targetId: data.get("target"), address: data.get("address"), shared: data.get("shared") === "on",
    }));
    if (item) { close(); setSelected(item.id); setPackId(item.id); }
  };
  const checkInvite = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (invite) {
      const item = await operate("Joining pack…", () => command<Pack>("join_pack", { code: code.trim(), ownerId: invite.owner.id, trust }));
      if (item) { close(); setPackId(item.id); sync(item); }
    } else {
      const info = await operate("Checking invite…", () => command<Invite>("invite_preview", { code: code.trim() }));
      if (info) setInvite(info);
    }
  };

  return <div className="page">
    {deleting && <Modal title="Delete pack" onClose={() => setDeleting(undefined)} dismissible={!busy}>
      <p>Remove {deleting.name} from your library? Worlds and installed files stay on this PC. Mod projects are kept as standalone projects. Shared servers are unaffected.</p>
      <div className="actions end"><button autoFocus onClick={() => setDeleting(undefined)}>Cancel</button><button className="primary" disabled={Boolean(busy)} onClick={() => void operate("Deleting pack...", async () => { await command("delete_pack", { id: deleting.id }); setDeleting(undefined); setPackId(); }, "Pack deleted. Local files were preserved.")}>Delete pack</button></div>
    </Modal>}

    {pack ? <PackDetail key={pack.id} {...{ pack, busy, notice, operate, startMod, sharingAvailable }} targetLabel={targetName(target(pack.targetId))} artwork={versionArtwork(target(pack.targetId)?.minecraftVersion)}
      sharingConnected={Boolean(snapshot.syncUser)} playDirect={canPlay(pack)}
      dependencyRecords={snapshot.dependencyRecords ?? []} projects={snapshot.drafts.filter(draft => draft.packId === pack.id)} onEdit={editMod} onDelete={() => setDeleting(pack)} onBack={() => setPackId()} onPlay={() => play(pack)} onSync={() => sync(pack)} onFolder={() => folder(pack)} onHandoff={() => void handoff(pack)} />
    : <>
      <header className="page-heading"><div><h1>Your packs</h1><p>Manage your modpacks for Minecraft Java Edition.</p></div>
        <div className="actions"><button className="secondary" onClick={() => setModal("join")}><UsersRound size={17} />Join a pack</button></div>
      </header>
      <section className="pack-cover" aria-label={featured?.name ?? "Create your first pack"}>
        {featuredArtwork && <img src={featuredArtwork.banner} alt="" fetchPriority="high" />}
        <div className="cover-content">
          <div><h2>{featured?.name ?? "Create your first modpack"}</h2>
            {featured ? <><p className="version">{targetName(target(featured.targetId))}</p><span className="status"><CheckCircle2 size={16} />{packStatus(featured)}</span></>
              : <p>Group the mods you want to use in singleplayer or on a server.</p>}</div>
          {featured ? <div className="actions"><button className="cover-button" onClick={() => setPackId(featured.id)}>Pack details<ArrowRight size={16} /></button>
            <button className="primary large" onClick={() => play(featured)} disabled={Boolean(busy)}><Play size={20} fill="currentColor" />{canPlay(featured) ? "Play" : "Play in Minecraft Launcher"}</button></div>
            : <button className="primary large" onClick={() => setModal("new")}><Plus size={22} />New pack</button>}
        </div>
      </section>
      <Feedback {...{ busy, notice }} />
      <section className="library">
        <div className="section-heading"><h2>Library <span>{snapshot.packs.length}</span></h2>{featured && <button className="secondary" onClick={() => setModal("new")}><Plus size={17} />New pack</button>}</div>
        {snapshot.packs.length ? <div className="pack-grid">{snapshot.packs.map(item => {
          const artwork = versionArtwork(target(item.targetId)?.minecraftVersion);
          return (
          <ContextMenuScope key={item.id} label={item.name} actions={[
            { id: "open", label: "Open modpack", icon: FolderOpen, run: () => setPackId(item.id) },
            { id: "folder", label: "Open game folder", icon: FolderOpen, disabled: Boolean(busy), run: () => folder(item) },
            { id: "create", label: "Create a mod", icon: Hammer, disabled: Boolean(busy), run: () => startMod(item.id) },
            { id: "copy-name", label: "Copy name", icon: Clipboard, run: () => navigator.clipboard.writeText(item.name) },
            { id: "delete", label: "Delete pack…", icon: Trash2, disabled: Boolean(busy), danger: true, separator: true, run: () => setDeleting(item) },
          ]}><button className={`pack-tile ${featured?.id === item.id ? "selected" : ""}`} onClick={() => setSelected(item.id)} onDoubleClick={() => setPackId(item.id)} aria-pressed={featured?.id === item.id}>
            <span className="tile-art" style={{ backgroundImage: artwork ? `url(${artwork.tile})` : undefined }} />
            <span className="tile-info"><strong>{item.name}</strong><span className="version">{targetName(target(item.targetId))}</span></span>
            <ArrowRight size={15} className="tile-arrow" />
          </button></ContextMenuScope>);
        })}</div> : <div className="library-empty"><FolderOpen size={19} /><span>Your new and joined packs appear here.</span></div>}
      </section>
    </>}
    {modal === "new" && <Modal title="New pack" onClose={close} initialFocus={packName}>
      <form onSubmit={create} className="form">
        {snapshot.packs.length === 0 && <div>
          <p>Choose where Bezalel stores your game files before creating your first pack.</p>
          <button type="button" className="text-button" onClick={() => openSettings("Game")}>Change game library folder</button>
        </div>}
        <label>Pack name<span className="required-hint" aria-hidden="true">Required</span>
          <input ref={packName} className="name-input" name="name" placeholder="e.g. Lantern Vale" required maxLength={64} />
        </label>
        <div>
          <label>Minecraft version and mod loader<select name="target" aria-describedby="pack-target-hint" required>{snapshot.targets.map(item => <option key={item.id} value={item.id}>{targetName(item)}</option>)}</select></label>
          <p id="pack-target-hint" className="field-hint">Match the loader and Minecraft version your mods or server require. This choice is fixed after you create the pack.</p>
        </div>
        <div>
          <label>Server address <span className="muted">(optional)</span><input name="address" aria-describedby="pack-address-hint" placeholder="play.example.com:25565" maxLength={255} /></label>
          <p id="pack-address-hint" className="field-hint">Use the address of an existing server, or leave it blank for singleplayer.</p>
        </div>
        {snapshot.syncUser && <label className="check"><input name="shared" type="checkbox" />Make this an invite-only shared pack</label>}
        {!snapshot.syncUser && <p className="field-hint">This pack will be stored locally on this PC.</p>}
        <Feedback {...{ busy, notice }} /><div className="actions end"><button type="button" className="secondary" onClick={close}>Cancel</button><button className="primary" disabled={Boolean(busy)}>Create pack</button></div>
      </form>
    </Modal>}
    {modal === "join" && joinNeedsSignIn && <AccessCodeDialog title="Sign in to join a pack" {...{ busy, notice }} onClose={close}
      description="Enter the access code for your Bezalel account first. You’ll enter your friend’s pack invite after signing in."
      onSignIn={accessCode => void operate("Signing in to Bezalel…", async () => {
        await command("sync_sign_in", { code: accessCode });
        await command("complete_onboarding");
      })} />}
    {modal === "join" && !joinNeedsSignIn && <Modal title={invite ? `Join ${invite.packName}` : "Join a pack"} onClose={close}>
      {!snapshot.syncUser ? <div className="form"><p>This build needs a test sharing service before you can use a pack invite. If you received service details, connect the service in Settings, then return here to enter the invite.</p>
        <div className="actions end"><button className="secondary" onClick={close}>Close</button><button className="primary" onClick={() => openSettings()}>Sharing settings</button></div></div>
        : <form className="form" onSubmit={checkInvite}>
          {invite ? <><p>Invited by <strong>{invite.owner.name}</strong> <span className="version">{targetName(invite.target)}</span></p>
            <p className="callout"><ShieldCheck size={22} />This pack can run Java mods published by its owner. Only join packs whose owner you trust.</p>
            <label className="check"><input type="checkbox" required checked={trust} onChange={event => setTrust(event.target.checked)} />Trust this pack owner</label></>
            : <label>Invite code<input value={code} onChange={event => setCode(event.target.value)} autoFocus required maxLength={160} /></label>}
          <Feedback {...{ busy, notice }} /><div className="actions end"><button type="button" className="secondary" onClick={close}>Cancel</button>
            <button className="primary" disabled={Boolean(busy) || Boolean(invite && !trust)}>{invite ? "Join pack" : "Find pack"}</button></div>
        </form>}
    </Modal>}

  </div>;
}

function PackDetail({ dependencyRecords, projects, onEdit, onDelete, pack, targetLabel, artwork, busy, notice, operate, startMod, playDirect, sharingAvailable, sharingConnected, onBack, onPlay, onSync, onFolder, onHandoff }: {
  pack: Pack; targetLabel: string; busy: string; notice: Notice; operate: Operate; startMod: (id: string) => void;
  artwork?: VersionArtwork;
  playDirect: boolean; sharingAvailable: boolean; sharingConnected: boolean;
  dependencyRecords: Snapshot["dependencyRecords"]; projects: Snapshot["drafts"]; onEdit: (id: string) => void; onDelete: () => void; onBack: () => void; onPlay: () => void; onSync: () => void; onFolder: () => void; onHandoff: () => void;
}) {
  const [tab, setTab] = useState("Mods");
  const [details, setDetails] = useState<Details>();
  const [detailError, setDetailError] = useState("");
  const [removingMod, setRemovingMod] = useState<string>();
  const [editingAddress, setEditingAddress] = useState(false);
  const [invite, setInvite] = useState<{ id: string; code: string }>();
  const [candidate, setCandidate] = useState<Candidate>();
  const [chosen, setChosen] = useState<string[]>([]);
  const [confirm, setConfirm] = useState(false);
  const [notes, setNotes] = useState("");
  const [restore, setRestore] = useState("");
  const [remove, setRemove] = useState("");
  const disabled = Boolean(busy);
  const owner = pack.role === "owner" && Boolean(pack.ownerId);
  const loadDetails = async () => {
    setDetailError("");
    try { setDetails(await command<Details>("pack_details", { id: pack.id })); }
    catch (error) { setDetailError(String(error)); }
  };
  useEffect(() => { void loadDetails(); }, [pack.id, pack.ownerId, pack.activeRevision]);

  const addMod = () => void operate("Adding a mod file…", async () => {
    const source = await open({ filters: [{ name: "Minecraft mod", extensions: ["jar"] }] });
    if (typeof source === "string") {
      await command("import_local_mod", { id: pack.id, source });
      await loadDetails();
    }
  });

  const stage = async () => {
    const next = await operate("Staging server candidate…", () => command<Candidate>("create_candidate", {
      id: pack.id, request: { baseRevision: pack.activeRevision, restoreRevision: restore ? Number(restore) : null, submissionIds: chosen,
        removePaths: remove.split("\n").map(value => value.trim()).filter(Boolean), notes },
    }));
    if (next) { setCandidate(next); setConfirm(false); }
  };
  const exportBundle = async () => {
    if (!candidate) return;
    const destination = await save({ defaultPath: `bezalel-server-r${candidate.release.manifest.revision}.zip`, filters: [{ name: "Server bundle", extensions: ["zip"] }] });
    if (destination) await operate("Exporting server bundle…", () => command("export_candidate", { id: pack.id, candidateId: candidate.id, destination }), "Server bundle exported.");
  };
  const publish = async () => {
    if (!candidate || !confirm) return;
    const result = await operate("Publishing release…", () => command<Release>("publish_candidate", { id: pack.id, candidateId: candidate.id, serverConfirmed: confirm }), "Release published.");
    if (result) { setCandidate(undefined); setChosen([]); setRestore(""); setNotes(""); setRemove(""); void loadDetails(); }
  };
  return <>
    {removingMod && <Modal title="Remove installed mod" dismissible={!disabled} onClose={() => setRemovingMod(undefined)}><div className="stack"><p>Remove {removingMod} from {pack.name}? The JAR will be moved to Removed Mods in the pack folder. Its conversation, project and worlds stay in place.</p><Feedback {...{ busy, notice }} /><div className="actions end"><button autoFocus disabled={disabled} onClick={() => setRemovingMod(undefined)}>Cancel</button><button className="primary" disabled={disabled} onClick={() => void operate("Removing mod...", async () => { await command("remove_local_mod", { id: pack.id, filename: removingMod }); setRemovingMod(undefined); await loadDetails(); }, "Mod removed from the pack.")}>Remove mod</button></div></div></Modal>}
    <button className="text-button" onClick={onBack}><ArrowLeft size={17} />Your packs</button>
    <header className="page-heading pack-heading"><div><h1>{pack.name}</h1><p><span className="version">{targetLabel}</span> · {packStatus(pack)}</p></div>
      <div className="actions"><button className="secondary" disabled={disabled} onClick={onDelete}><Trash2 size={17} />Delete pack</button>
        <button className="secondary" onClick={onFolder} disabled={disabled}><FolderOpen size={17} />Game folder</button>
        {pack.ownerId && <button className="secondary" onClick={onSync} disabled={disabled}><CloudDownload size={17} />Sync</button>}
        {playDirect && <button className="secondary" onClick={onHandoff} disabled={disabled}>Use Minecraft Launcher</button>}
        <button className="primary" onClick={onPlay} disabled={disabled}><Play size={17} fill="currentColor" />{playDirect ? "Play" : "Play in Minecraft Launcher"}</button></div>
    </header>
    {artwork && <img className="pack-banner" src={artwork.banner} alt="" />}
    <div className="tabbar" aria-label="Pack views">{["Mods", "Members", "Server", "History"].map(item =>
      <button key={item} aria-pressed={tab === item} onClick={() => setTab(item)}>{item}</button>)}</div>
    <Feedback {...{ busy, notice }} />
    <DependencyRequests packId={pack.id} />
    {detailError && <Feedback notice={{ kind: "error", text: detailError }} />}
    {tab === "Mods" && <section className="stack">
      <div className="section-heading"><h2>{pack.ownerId ? "Published mods" : "Installed mods"}</h2><div className="actions">
        {!pack.ownerId && <button className="secondary" disabled={disabled} onClick={addMod}><Plus size={16} />Add mod file</button>}
        <button className="secondary" onClick={() => startMod(pack.id)}><Hammer size={16} />Create a mod</button>
      </div></div>
      {details?.active?.manifest.files.length ? <ul className="data-list">{details.active.manifest.files.map(item =>
        <li key={item.path}><span>{item.path.replace(/^mods\//, "")}</span><span className="version">{item.side}</span></li>)}</ul>
        : details?.localMods.length ? <ul className="data-list">{details.localMods.map(name => {
          const project = projects.find(item => item.build?.jarPath.replaceAll("\\", "/").split("/").pop() === name);
          const records = dependencyRecords.filter(record => record.packId === pack.id && record.filename === name) ?? [];
          return <ContextMenuScope key={name} label={name} actions={[
            { id: "copy-name", label: "Copy filename", icon: Clipboard, run: () => navigator.clipboard.writeText(name) },
            ...(project ? [{ id: "edit", label: "Edit mod", icon: Hammer, disabled, run: () => onEdit(project.id) }] : []),
            { id: "folder", label: "Open game folder", icon: FolderOpen, disabled, run: onFolder },
            { id: "remove", label: "Remove mod…", icon: Trash2, separator: true, danger: true, disabled, run: () => setRemovingMod(name) },
          ]}><li><span>{name}{records.map((record, i) => <details key={i} className="dependency-record"><summary>{record.version} · {record.source}</summary><p>{record.project ? `Project ${record.project}` : "User-provided file"}{record.versionId ? ` · Release ${record.versionId}` : ""}</p><p>{record.requiredBy.length ? `Required by ${record.requiredBy.join(", ")}` : `Requested mod ${record.root}`}</p><p className="path">SHA-256: {record.sha256}</p></details>)}</span><span className="actions">{project && <button className="text-button" disabled={disabled} onClick={() => onEdit(project.id)}>Edit</button>}<button className="text-button" disabled={disabled} aria-label={`Remove ${name}`} onClick={() => setRemovingMod(name)}>Remove</button></span></li></ContextMenuScope>;
        })}</ul>
        : <Empty title="No mods yet">{pack.ownerId ? "Build a mod and send it to the pack for review." : `Add a .jar file you trust for ${targetLabel}, or create a mod for this pack.`}</Empty>}
    </section>}
    {tab === "Members" && <section className="stack">
      <div className="section-heading"><h2>Members</h2>{owner && <button className="secondary" disabled={disabled} onClick={async () => {
        const result = await operate("Creating invite…", () => command<{ id: string; code: string }>("invite_create", { id: pack.id, role: "player" }));
        if (result) setInvite(result);
      }}><UsersRound size={16} />Create invite</button>}</div>
      {owner && <label className="inline-label">Editor access<button className="text-button" disabled={disabled} onClick={async () => {
        const result = await operate("Creating editor invite…", () => command<{ id: string; code: string }>("invite_create", { id: pack.id, role: "editor" }));
        if (result) setInvite(result);
      }}>Create editor invite</button></label>}
      {invite && <div className="callout"><code>{invite.code}</code><button className="secondary" onClick={() => void navigator.clipboard.writeText(invite.code)}><Clipboard size={15} />Copy</button>
        <button className="text-button" onClick={async () => {
          await operate("Revoking invite…", () => command("invite_revoke", { id: pack.id, inviteId: invite.id }), "Invite revoked.");
          setInvite(undefined);
        }}>Revoke</button></div>}
      {details?.members.length ? <ul className="data-list">{details.members.map(member => <li key={member.account.id}><span>{member.account.name}</span><span>{member.role}</span>
        {owner && member.role !== "owner" && <button className="text-button" disabled={disabled} onClick={async () => {
          await operate("Removing member…", () => command("member_remove", { id: pack.id, userId: member.account.id }), "Member removed.");
          await loadDetails();
        }}>Remove</button>}</li>)}</ul> : pack.ownerId ? <Empty title="No members to show">Members appear here once they join the pack.</Empty>
        : <Empty title="Local pack">{sharingConnected ? "To invite friends, create a new pack with invite-only sharing enabled."
          : sharingAvailable ? "To invite friends, sign in to Bezalel in Settings, then create an invite-only shared pack."
          : "To invite friends, connect a test sharing service in Settings, then create an invite-only shared pack."}</Empty>}
    </section>}
    {tab === "Server" && <section className="stack">
      <div className="section-heading"><h2><Server size={19} />Server</h2><div className="actions">
        {pack.address && <button className="secondary" onClick={() => void navigator.clipboard.writeText(pack.address)}><Clipboard size={16} />Copy address</button>}
        {!pack.ownerId && <button className="secondary" disabled={disabled} onClick={() => setEditingAddress(true)}>{pack.address ? "Edit server address" : "Add server address"}</button>}
      </div></div>
      <p className="version">{pack.address || "No server address configured"}</p>
      {editingAddress && <Modal title="Server address" dismissible={!disabled} onClose={() => setEditingAddress(false)}>
        <form className="form" onSubmit={event => {
          event.preventDefault();
          const address = new FormData(event.currentTarget).get("address");
          void operate("Saving server address…", async () => {
            await command("set_local_server_address", { id: pack.id, address });
            setEditingAddress(false);
          }, "Server address saved.");
        }}>
          <label>Address<input name="address" defaultValue={pack.address} placeholder="play.example.com:25565" maxLength={255} aria-describedby="server-address-hint" autoFocus /></label>
          <p id="server-address-hint" className="field-hint">Use the address of an existing server, or leave it blank for singleplayer.</p>
          <Feedback {...{ busy, notice }} />
          <div className="actions end"><button type="button" className="secondary" disabled={disabled} onClick={() => setEditingAddress(false)}>Cancel</button><button className="primary" disabled={disabled}>Save address</button></div>
        </form>
      </Modal>}
      {pack.activeRevision !== null && <span className="status"><CheckCircle2 size={16} />Owner confirmed · Release {pack.activeRevision}</span>}
      {owner && (candidate ? <div className="panel stack">
        <h3>Candidate {candidate.release.manifest.revision}</h3><ol><li>Back up the server world.</li><li>Download the bundle, upload the files, and restart.</li><li>Test a join on this release.</li></ol>
        {candidate.removeServerPaths.length > 0 && <p>Remove these old server files before restart: {candidate.removeServerPaths.join(", ")}</p>}
        <div className="actions"><button className="secondary" disabled={disabled} onClick={() => void exportBundle()}><ArrowDownToLine size={16} />Download bundle</button></div>
        <label className="check"><input type="checkbox" checked={confirm} onChange={event => setConfirm(event.target.checked)} />I backed up and tested the updated server</label>
        <div className="actions"><button className="primary" disabled={disabled || !confirm} onClick={() => void publish()}>Confirm and publish</button>
          <button className="secondary" onClick={() => setCandidate(undefined)}>Back to changes</button></div>
      </div> : <div className="panel stack">
        <h3>Prepare an update</h3>
        {details?.candidates.filter(item => item.release.manifest.revision === (pack.activeRevision ?? 0) + 1).map(item =>
          <button key={item.id} className="text-button" onClick={() => { setCandidate(item); setConfirm(false); }}>Resume candidate {item.release.manifest.revision}</button>)}
        {details?.submissions.map(item => <div className="submission" key={item.id}>
          <label className="check"><input type="checkbox" checked={chosen.includes(item.id)} disabled={Boolean(restore)} onChange={event => setChosen(event.target.checked ? [...chosen, item.id] : chosen.filter(value => value !== item.id))} />{item.artifact.path}</label>
          <details><summary>Review source changes</summary><p>{item.notes}</p>{item.changes.map(change => <details key={change.path}><summary>{change.path}</summary><pre>{change.content ?? "Deleted"}</pre></details>)}</details>
        </div>)}
        <label>Restore a release<select value={restore} onChange={event => { setRestore(event.target.value); setChosen([]); setRemove(""); }}>
          <option value="">New update</option>{details?.history.filter(item => item.manifest.revision !== pack.activeRevision).map(item =>
            <option key={item.manifest.revision} value={item.manifest.revision}>Release {item.manifest.revision}</option>)}
        </select></label>
        {!restore && <label>Remove old mod paths<textarea value={remove} onChange={event => setRemove(event.target.value)} placeholder={"One path per line, e.g. mods/old-mod.jar"} /></label>}
        <label>Release notes<textarea value={notes} onChange={event => setNotes(event.target.value)} maxLength={3000} placeholder="What changed in this update?" /></label>
        <div className="actions"><button className="primary" disabled={disabled || (!chosen.length && !restore && !remove.trim())} onClick={() => void stage()}>Prepare server bundle</button></div>
      </div>)}
    </section>}
    {tab === "History" && <section className="stack">
      <div className="section-heading"><h2><History size={19} />Release history</h2></div>
      {details?.history.length ? <ul className="data-list">{details.history.map(item =>
        <li key={item.manifest.revision}><strong>Release {item.manifest.revision}</strong><span>{item.manifest.notes || "No release notes"}</span></li>)}</ul>
        : <Empty title="No releases yet">Published pack updates appear here.</Empty>}
    </section>}
  </>;
}
