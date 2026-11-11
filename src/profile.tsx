import { useId, useState, type ReactNode } from "react";
import { Box, Flame, Leaf, Moon } from "lucide-react";
import { Feedback, type Notice } from "./components";
import type { ProfileAvatar, SyncProfile } from "./model";

const avatars = {
  violet: { label: "Violet", Icon: Box },
  lime: { label: "Lime", Icon: Leaf },
  ember: { label: "Ember", Icon: Flame },
  sky: { label: "Sky", Icon: Moon },
} as const;

export function Avatar({ value }: { value: ProfileAvatar }) {
  const { Icon } = avatars[value] ?? avatars.violet;
  return <span className="profile-avatar" data-avatar={value} aria-hidden="true"><Icon size={17} /></span>;
}

export function ProfileForm({ profile, busy, notice, onSave, secondary, submitLabel }: {
  profile: SyncProfile;
  busy: string;
  notice: Notice;
  onSave: (name: string, avatar: ProfileAvatar) => void;
  secondary?: ReactNode;
  submitLabel: string;
}) {
  const [name, setName] = useState(profile.name);
  const [avatar, setAvatar] = useState(profile.avatar);
  const id = useId();
  return <form className="form" onSubmit={event => {
    event.preventDefault();
    if (!busy && name.trim()) onSave(name.trim(), avatar);
  }}>
    <label htmlFor={id}>Display name<input id={id} value={name} onChange={event => setName(event.target.value)} maxLength={48} required disabled={Boolean(busy)} /></label>
    <fieldset className="profile-options" disabled={Boolean(busy)}>
      <legend>Avatar</legend>
      <div className="avatar-picker">
        {(Object.keys(avatars) as ProfileAvatar[]).map(value => <label htmlFor={id + value} key={value}>
          <input id={id + value} name={id + "-avatar"} type="radio" value={value} checked={avatar === value} onChange={() => setAvatar(value)} />
          <Avatar value={value} />{avatars[value].label}
        </label>)}
      </div>
    </fieldset>
    <Feedback {...{ busy, notice }} />
    <div className="actions">{secondary}<button className="primary" disabled={Boolean(busy) || !name.trim()}>{submitLabel}</button></div>
  </form>;
}
