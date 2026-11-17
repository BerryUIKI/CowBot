import { useT } from "./i18n";
import { useUiPreferences } from "./uiPreferences";

/**
 * Small `bot` tag that follows an agent's name.
 *
 * Team workspaces mix human teammates and agents in the same conversation, so
 * the tag makes the author kind explicit. It is off by default and driven by
 * the device-local preference in Settings → General.
 */
export default function BotTag() {
  const { showAgentBotTags } = useUiPreferences();
  const t = useT();
  if (!showAgentBotTags) return null;
  return <span className="agent-bot-tag">{t("app.agent.botTag")}</span>;
}
