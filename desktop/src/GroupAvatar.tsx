import Avatar from "./Avatar";

interface GroupAvatarProps {
  names: string[];
  size?: "small" | "medium" | "large";
}

/**
 * Overlapping participant avatars for a group conversation.
 *
 * Falls back to a channel avatar when the participant list is unknown, and to
 * a single agent avatar when only one participant is known.
 */
export default function GroupAvatar({ names, size = "medium" }: GroupAvatarProps) {
  if (names.length === 0) {
    return <Avatar name="Group" kind="channel" size={size} />;
  }
  if (names.length === 1) {
    return <Avatar name={names[0]} kind="agent" size={size} />;
  }
  const shown = names.slice(0, 3);
  return (
    <span className={`group-avatar group-avatar-${size}`} aria-hidden="true">
      {shown.map((name) => (
        <span className="group-avatar-item" key={name}>
          <Avatar name={name} kind="agent" size="small" />
        </span>
      ))}
      {names.length > shown.length ? (
        <span className="group-avatar-more">+{names.length - shown.length}</span>
      ) : null}
    </span>
  );
}
