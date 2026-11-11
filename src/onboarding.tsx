import { useEffect, useRef } from "react";
import { ArrowRight } from "lucide-react";
import { AccessCodeForm } from "./access";
import type { Operate } from "./app";
import { Brand, Feedback, type Notice } from "./components";
import { command, type Snapshot, type SyncProfile } from "./model";
import { ProfileForm } from "./profile";
import cover from "./assets/lantern-vale.webp";

export function Onboarding({ snapshot, busy, notice, operate }: { snapshot: Snapshot; busy: string; notice: Notice; operate: Operate }) {
  const profile = snapshot.syncProfile;
  const title = useRef<HTMLHeadingElement>(null);
  const finish = () => command("complete_onboarding");
  const useLocally = () => void operate("Opening your library…", finish);
  useEffect(() => { title.current?.focus(); }, [profile?.id]);

  return <main className="onboarding" id="main">
    <div className="onboarding-art" aria-hidden="true"><img src={cover} alt="" fetchPriority="high" /></div>
    <section className="onboarding-content" aria-labelledby="welcome-title">
      <div className="brand onboarding-brand"><Brand /><span>Bezalel</span></div>
      {profile ? <>
        <div className="onboarding-heading">
          <h1 id="welcome-title" ref={title} tabIndex={-1}>Make it yours</h1>
          <p>Choose a name for your packs and an avatar for the launcher.</p>
        </div>
        <ProfileForm key={profile.id} profile={profile} busy={busy} notice={notice} submitLabel="Save and continue"
          secondary={<button type="button" className="secondary" disabled={Boolean(busy)} onClick={() => void operate("Opening your library…", finish)}>Set up later</button>}
          onSave={(name, avatar) => void operate("Saving your profile…", async () => {
            await command("save_sync_profile", { name, avatar });
            await finish();
          })} />
      </> : <>
        <div className="onboarding-heading">
          <h1 id="welcome-title" ref={title} tabIndex={-1}>Welcome to Bezalel</h1>
          <p>Organize your Minecraft Java mods into packs and create your own mods.</p>
          <p>{snapshot.hostedSharingConfigured
            ? "Create local packs without an account. Sign in with a Bezalel access code to join or create shared packs."
            : "Create local packs on this PC. To share packs, connect a test sharing service in Settings."}</p>
        </div>
        {snapshot.hostedSharingConfigured
          ? <AccessCodeForm {...{ busy, notice }}
            secondary={<button type="button" className="secondary" disabled={Boolean(busy)} onClick={useLocally}>Continue without an account<ArrowRight size={17} /></button>}
            onSignIn={code => void operate("Signing in to Bezalel…", async () => {
              const signedIn = await command<SyncProfile>("sync_sign_in", { code });
              if (signedIn.setupComplete) await finish();
            })} />
          : <>
            <div className="onboarding-actions"><button className="primary large" disabled={Boolean(busy)} onClick={useLocally}>Continue without an account<ArrowRight size={17} /></button></div>
            <Feedback {...{ busy, notice }} />
          </>}
      </>}
    </section>
  </main>;
}
