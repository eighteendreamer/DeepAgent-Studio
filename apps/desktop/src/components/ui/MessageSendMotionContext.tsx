import { createContext, useCallback, useContext, useEffect, useRef } from "react";
import { MessageSendMotion } from "./messageSendMotion";
import "./messageSendMotion.css";

export const MessageSendMotionContext = createContext<MessageSendMotion | null>(null);
export const useMessageSendMotion = () => useContext(MessageSendMotionContext);

export function useMessageSendMotionController() {
  const controller = useRef<MessageSendMotion>();
  if (!controller.current) controller.current = new MessageSendMotion();
  const motion = controller.current;
  useEffect(() => () => motion.finish(), [motion]);
  return motion;
}

export function useMessageSendTarget(id: string) {
  const motion = useMessageSendMotion();
  const release = useRef<() => void>();
  // Register through the stable row. Framer Motion 11 popLayout replaces its
  // direct child's ref, so a ref placed on that motion.div is never called.
  return useCallback((row: HTMLDivElement | null) => {
    release.current?.();
    const target = row?.querySelector<HTMLElement>("[data-user-message-id]");
    release.current = target ? motion?.register(id, target) : undefined;
  }, [motion, id]);
}
