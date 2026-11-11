import { Channel, invoke } from "@tauri-apps/api/core";

export const APP_VERSION = "0.2.2";

export type Target = { id: string; loader: string; minecraftVersion: string; loaderVersion: string };
export type Pack = {
  id: string; name: string; targetId: string; address: string; ownerId: string | null; role: string;
  trustedOwnerId: string | null; activeRevision: number | null; installedRevision: number | null;
};
export type Message = { role: "user" | "assistant"; text: string; choices?: string[] };
export type Change = { path: string; content: string | null };
export type Build = { jarPath: string; sha256: string; size: number; compilePassed: boolean; clientPassed: boolean; serverPassed: boolean; diagnostics: string[] };
export type Draft = {
  threadId?: string | null; id: string; packId: string | null; title: string; targetId: string; mode: "direct" | "guided";
  idea: string | null; messages: Message[]; brief: string | null; changes: Change[]; build: Build | null;
};
export type ModelFamily = "Astra" | "Sol" | "Terra" | "Luna";
export type AgentModel = { family: ModelFamily; version: string | null; modelId: string | null; efforts: string[] };
export type Settings = {
  modelFamily: ModelFamily; reasoningEffort: string; theme: "light" | "dark" | "system";
  memoryMib: number; autoSync: boolean; reducedMotion: boolean; textScale: number;
  libraryPath: string | null; microsoftClientId: string; syncUrl: string; syncKey: string; onboardingComplete: boolean;
};
export type ProfileAvatar = "violet" | "lime" | "ember" | "sky";
export type SyncProfile = { id: string; name: string; avatar: ProfileAvatar; setupComplete: boolean; canInvite?: boolean };
export type Snapshot = {
  settings: Settings; packs: Pack[]; drafts: Draft[]; targets: Target[];
  builderEulaAccepted: boolean; chatgptConnected: boolean; microsoftName: string | null; syncUser: string | null; dataPath: string;
  hostedSharingConfigured: boolean; syncProfile: SyncProfile | null;
};
export type Release = {
  manifest: { revision: number; notes: string; files: { path: string; side: string; sha256: string }[] };
  signature: string;
};
export type Candidate = { id: string; release: Release; removeServerPaths: string[] };
export type Member = { account: { id: string; name: string }; role: string };
export type Submission = { id: string; artifact: { path: string; side: string }; notes: string; changes: Change[] };
export type Details = { active: Release | null; history: Release[]; submissions: Submission[]; members: Member[]; candidates: Candidate[]; localMods: string[] };
export type Invite = { packName: string; owner: { id: string; name: string }; target: Omit<Target, "id"> };

export function command<T>(name: string, args?: Record<string, unknown>) {
  return invoke<T>(name, args);
}

export function withProgress<T>(name: string, args: Record<string, unknown>, onProgress: (value: string) => void) {
  const progress = new Channel<string>();
  progress.onmessage = onProgress;
  return command<T>(name, { ...args, progress });
}

export function loaderName(loader: string) {
  return { fabric: "Fabric", forge: "Forge", neoforge: "NeoForge" }[loader] ?? loader;
}

export function targetName(target?: Omit<Target, "id">) {
  return target ? `${loaderName(target.loader)} · ${target.minecraftVersion}` : "Target unavailable";
}

export function packStatus(pack: Pack) {
  if (pack.ownerId && pack.ownerId !== pack.trustedOwnerId) return "Trust required";
  if (pack.ownerId && pack.activeRevision === null) return "Awaiting first release";
  if (pack.activeRevision !== pack.installedRevision) return "Update available";
  return pack.ownerId ? "Up to date" : "Local pack";
}
