import { Plugs, TelegramLogo } from "@phosphor-icons/react";
import googleChatLogo from "./assets/google-chat.svg";

export function NotificationConnectorIcon({type}: {type: string}) {
  if (type === "telegram") return <TelegramLogo size={24} weight="fill" aria-hidden="true" focusable="false" />;
  if (type === "google_chat") return <img src={googleChatLogo} width={24} height={24} alt="" aria-hidden="true" />;
  return <Plugs size={24} aria-hidden="true" focusable="false" />;
}
