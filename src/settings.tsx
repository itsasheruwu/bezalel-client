import { useEffect, useState } from "react";
import { Box, Download, HardDrive, LogIn, LogOut, ShieldCheck, Sparkles, UserPlus, UserRound } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { AccessCodeDialog, FriendAccessCodeDialog } from "./access";
import type { Operate } from "./app";
import { Feedback, Modal, type Notice } from "./components";
import { APP_VERSION, command, type Settings, type Snapshot, type SyncProfile } from "./model";
import { Avatar, ProfileForm } from "./profile";

import { DependencyPermissions } from "./dependency-permissions";

const sections = ["Accounts", "Game", "Downloads", "Appearance", "Diagnostics"] as const;
export type SettingsSection = (typeof sections)[number];

export function SettingsPage({ snapshot, busy, notice, operate, initialSection }: {
  snapshot: Snapshot; busy: string; notice: Notice; operate: Operate; initialSection: SettingsSection;
}) {
  const [section, setSection] = useState(initialSection);
  const [modal, setModal] = useState<"minecraft" | "sharing" | "profile" | "sign-in" | "friend">();
  const [friendCode, setFriendCode] = useState<string>();
  const [trustPackId, setTrustPackId] = useState<string>();
  const [settings, setSettings] = useState(snapshot.settings);
  const disabled = Boolean(busy);
  const trustPack = snapshot.packs.find(pack => pack.id === trustPackId);
  useEffect(() => setSettings(snapshot.settings), [snapshot.settings]);
  const saveSettings = (next: Settings) => operate("Saving settings…", () => command("save_settings", { settings: next }), "Settings saved.");
  const connectMinecraft = async () => {
    if (!settings.microsoftClientId) { setModal("minecraft"); return; }
    await operate("Complete Microsoft sign-in in your browser…", () => command("connect_microsoft"), "Minecraft account connected.");
  };
  const signIn = async (code: string) => {
    const profile = await operate("Signing in to Bezalel…", async () => {
      const result = await command<SyncProfile>("sync_sign_in", { code });
      await command("complete_onboarding");
      return result;
    }, "Signed in to Bezalel.");
    if (profile) setModal(profile.setupComplete ? undefined : "profile");
  };
  const createFriendCode = async () => {
    await operate("Creating friend access code…", async () => {
      const result = await command<{ code: string }>("create_sync_access_code");
      setFriendCode(result.code);
    });
  };
  return <div className="page">
    <header className="page-heading"><div><h1>Settings</h1><p>Manage your accounts and how Bezalel runs.</p></div></header>
    <div className="tabbar" aria-label="Settings sections">{sections.map(item => <button key={item} aria-pressed={section === item} onClick={() => setSection(item)}>{item}</button>)}</div>
    <Feedback {...{ busy, notice }} />
    {section === "Accounts" && <div className="settings-list">
      <div className="setting">{snapshot.syncProfile ? <Avatar value={snapshot.syncProfile.avatar} /> : <UserRound />}<div>
        <h2>{snapshot.syncProfile?.name ?? snapshot.syncUser ?? "Bezalel account"}</h2>
        <p>{snapshot.syncProfile ? "Your profile for shared packs and invites." : snapshot.syncUser ? "Connected to a test sharing service."
          : snapshot.hostedSharingConfigured ? "Use a Bezalel access code to join and create shared packs."
          : "This build needs a test sharing service to join or create shared packs."}</p></div>
        <div className="actions account-actions">
          {snapshot.syncProfile && <button className="secondary" disabled={disabled} onClick={() => setModal("profile")}>{snapshot.syncProfile.setupComplete ? "Edit profile" : "Set up profile"}</button>}
          {snapshot.syncProfile?.canInvite && <button className="secondary" disabled={disabled} onClick={() => {
            setFriendCode(undefined);
            setModal("friend");
            void createFriendCode();
          }}><UserPlus size={16} />Create friend access code</button>}
          {snapshot.syncUser ? <button className="secondary" disabled={disabled} onClick={() => void operate("Signing out of Bezalel…", () => command("sync_disconnect"), "Signed out of Bezalel.")}><LogOut size={16} />Sign out</button>
            : <>{snapshot.hostedSharingConfigured && <button className="primary" disabled={disabled} onClick={() => setModal("sign-in")}><LogIn size={16} />Sign in with access code</button>}
              <button className={snapshot.hostedSharingConfigured ? "text-button" : "secondary"} disabled={disabled} onClick={() => setModal("sharing")}>Connect a test service</button></>}
        </div></div>
      <div className="setting"><Box /><div><h2>Minecraft Java Edition</h2><p>{snapshot.microsoftName ? `Playing as ${snapshot.microsoftName}`
        : settings.microsoftClientId ? "Sign in with Microsoft to launch Minecraft directly from Bezalel."
        : "You can sign in through Minecraft Launcher. Direct launch from Bezalel requires an approved application ID."}</p></div>
        {snapshot.microsoftName ? <button className="secondary" disabled={disabled} onClick={() => void operate("Disconnecting Minecraft…", () => command("disconnect_microsoft"), "Minecraft account disconnected.")}><LogOut size={16} />Disconnect</button>
          : <button className="secondary" disabled={disabled} onClick={() => void connectMinecraft()}>{settings.microsoftClientId ? "Connect Minecraft" : "Set up direct launch"}</button>}</div>
      <div className="setting"><Sparkles /><div><h2>ChatGPT</h2><p>{snapshot.chatgptConnected ? "Connected to your ChatGPT plan." : "Save mod ideas locally. Connect an eligible ChatGPT plan when you’re ready to chat with Codex or build a mod."}</p></div>
        {snapshot.chatgptConnected ? <button className="secondary" disabled={disabled} onClick={() => void operate("Disconnecting ChatGPT…", () => command("disconnect_chatgpt"), "ChatGPT disconnected.")}><LogOut size={16} />Disconnect</button>
          : <button className="secondary" disabled={disabled} onClick={() => void operate("Complete ChatGPT sign-in in your browser…", () => command("connect_chatgpt"), "ChatGPT connected.")}>Continue with ChatGPT</button>}</div>
    </div>}
    {section === "Game" && <form className="panel form settings-form" onSubmit={event => { event.preventDefault(); void saveSettings(settings); }}>
      <label>Allocated RAM (MiB)<input type="number" min={2048} max={16384} step={256} value={settings.memoryMib} onChange={event => setSettings({ ...settings, memoryMib: Number(event.target.value) })} required /></label>
      <div><h3>Game library</h3><p className="path">{settings.libraryPath ?? snapshot.dataPath}</p></div>
      <div className="actions"><button type="button" className="secondary" disabled={disabled || snapshot.packs.length > 0} onClick={async () => {
        const path = await open({ directory: true });
        if (typeof path === "string") setSettings({ ...settings, libraryPath: path });
      }}><HardDrive size={16} />Choose folder</button>{snapshot.packs.length > 0 && <span className="muted">Choose a library location before creating a pack.</span>}</div>
      <div className="actions"><button className="primary" disabled={disabled}>Save game settings</button></div>
    </form>}
    {section === "Downloads" && <div className="panel stack settings-form">
      <label className="check"><input type="checkbox" checked={settings.autoSync} disabled={disabled} onChange={event => void saveSettings({ ...settings, autoSync: event.target.checked })} />Sync trusted packs when Bezalel opens</label>
      <label className="check"><input type="checkbox" checked={settings.dependencyOffline ?? false} disabled={disabled} onChange={event => void saveSettings({ ...settings, dependencyOffline: event.target.checked })} />Offline dependency resolution (use installed mods only)</label>
      <DependencyPermissions />
      <h2 className="settings-subheading"><ShieldCheck size={20} />Pack trust</h2>
      {snapshot.packs.filter(pack => pack.ownerId).map(pack => <div className="setting compact" key={pack.id}><div><h2>{pack.name}</h2>
        <p>{pack.trustedOwnerId === pack.ownerId ? "Owner trusted to publish Java mods" : "Owner trust paused"}</p></div>
        {pack.trustedOwnerId === pack.ownerId ? <button className="secondary" disabled={disabled}
          onClick={() => void operate("Revoking pack trust…", () => command("set_pack_trust", { id: pack.id, trusted: false }), "Pack trust revoked.")}>Revoke trust</button>
          : <button className="secondary" disabled={disabled} onClick={() => setTrustPackId(pack.id)}>Trust owner</button>}</div>)}
      {!snapshot.packs.some(pack => pack.ownerId) && <p className="muted">Shared packs appear here after you join or create one.</p>}
    </div>}
    {section === "Appearance" && <form className="panel form settings-form" onSubmit={event => { event.preventDefault(); void saveSettings(settings); }}>
      <div><h3>Appearance</h3><p className="muted">Choose your theme or follow your system.</p></div>
      <label>Theme<select aria-label="App theme" value={settings.theme} disabled={disabled} onChange={event => { const next = { ...settings, theme: event.target.value as Settings["theme"] }; setSettings(next); void saveSettings(next); }}><option value="system">Sync with system</option><option value="light">Light</option><option value="dark">Dark</option></select></label>
      <label>Text size<select value={settings.textScale} onChange={event => setSettings({ ...settings, textScale: Number(event.target.value) })}>
        {[90, 100, 110, 120, 130].map(size => <option key={size} value={size}>{size}%</option>)}</select></label>
      <label className="check"><input type="checkbox" checked={settings.reducedMotion} onChange={event => setSettings({ ...settings, reducedMotion: event.target.checked })} />Reduce motion</label>
      <div className="actions"><button className="primary" disabled={disabled}>Save appearance</button></div>
    </form>}
    {section === "Diagnostics" && <div className="panel stack settings-form">
      <h3>Bezalel {APP_VERSION}</h3><p>Export target and status information for troubleshooting.</p>
      <div className="actions"><button className="secondary" disabled={disabled} onClick={async () => {
        const destination = await save({ defaultPath: "bezalel-diagnostics.json", filters: [{ name: "Diagnostics", extensions: ["json"] }] });
        if (destination) await operate("Exporting diagnostics…", () => command("export_diagnostics", { destination }), "Diagnostics exported.");
      }}><Download size={16} />Export diagnostics</button></div>
    </div>}
    {modal === "sign-in" && <AccessCodeDialog title="Sign in to Bezalel" {...{ busy, notice }} onSignIn={code => void signIn(code)} onClose={() => setModal(undefined)} />}
    {modal === "friend" && snapshot.syncProfile?.canInvite && <FriendAccessCodeDialog code={friendCode} {...{ busy, notice }}
      onCreate={() => void createFriendCode()} onClose={() => { setFriendCode(undefined); setModal(undefined); }} />}
    {modal === "profile" && snapshot.syncProfile && <Modal title={snapshot.syncProfile.setupComplete ? "Edit your profile" : "Set up your profile"} onClose={() => setModal(undefined)}>
      <ProfileForm key={snapshot.syncProfile.id} profile={snapshot.syncProfile} busy={busy} notice={notice} submitLabel="Save profile"
        secondary={<button type="button" className="secondary" onClick={() => setModal(undefined)}>Cancel</button>}
        onSave={(name, avatar) => void operate("Saving your profile…", async () => {
          await command("save_sync_profile", { name, avatar });
          setModal(undefined);
        }, "Profile saved.")} />
    </Modal>}
    {modal === "minecraft" && <Modal title="Set up direct Minecraft launch" onClose={() => setModal(undefined)}>
      <form className="form" onSubmit={async event => {
        event.preventDefault();
        const next = { ...settings, microsoftClientId: settings.microsoftClientId.trim() };
        await operate("Complete Microsoft sign-in in your browser…", async () => {
          await command("save_settings", { settings: next });
          await command("connect_microsoft");
          setModal(undefined);
        }, "Minecraft account connected.");
      }}>
        <p>Enter the Minecraft-approved Microsoft application ID from the person who provided this build. You can also open a pack and choose Play in Minecraft Launcher.</p>
        <label>Application ID<input value={settings.microsoftClientId} onChange={event => setSettings({ ...settings, microsoftClientId: event.target.value })} required autoFocus /></label>
        <Feedback {...{ busy, notice }} /><div className="actions end"><button type="button" className="secondary" onClick={() => setModal(undefined)}>Cancel</button><button className="primary" disabled={disabled}>Save and sign in</button></div>
      </form>
    </Modal>}
    {modal === "sharing" && <Modal title="Connect a test sharing service" onClose={() => setModal(undefined)}>
      <form className="form" onSubmit={async event => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        await operate("Connecting sharing…", async () => {
          await command("sync_connect", { url: data.get("url"), key: data.get("key"), token: data.get("token") });
          setModal(undefined);
        }, "Sharing connected.");
      }}>
        <p>Ask the person running the test sharing service for its address, public signing key, and an account token. Enter your pack invite in Home → Join a pack after connecting.</p>
        <label>Service address<input name="url" type="url" placeholder="https://packs.example.com" defaultValue={settings.syncUrl} required autoFocus /></label>
        <label>Public signing key<input name="key" defaultValue={settings.syncKey} required autoComplete="off" /></label>
        <label>Account token<input name="token" type="password" required autoComplete="off" /></label>
        <Feedback {...{ busy, notice }} /><div className="actions end"><button type="button" className="secondary" onClick={() => setModal(undefined)}>Cancel</button><button className="primary" disabled={disabled}>Connect</button></div>
      </form>
    </Modal>}
    {trustPack && <Modal title={`Trust the owner of ${trustPack.name}?`} onClose={() => setTrustPackId(undefined)}>
      <div className="form"><p>After you trust this owner, Bezalel can run Java mods they publish when you play this pack.</p>
        <Feedback {...{ busy, notice }} />
        <div className="actions end"><button className="secondary" onClick={() => setTrustPackId(undefined)}>Cancel</button>
          <button className="primary" disabled={disabled} onClick={() => void operate("Restoring pack trust…", async () => {
            await command("set_pack_trust", { id: trustPack.id, trusted: true, expectedOwner: trustPack.ownerId });
            setTrustPackId(undefined);
          }, "Pack owner trusted.")}>Trust owner</button></div></div>
    </Modal>}
  </div>;
}
