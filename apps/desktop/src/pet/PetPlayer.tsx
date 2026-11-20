import { useEffect, useRef, useState } from "react";
import { petAsset, resolveAction, type Character } from "./api";

interface Props {
  character: Character;
  root: string;
  state: string;
  actionName?: string;
  variant: number;
  active: boolean;
  onReady: () => void;
  onPlaying?: (video: HTMLVideoElement, actionName: string) => void;
  onEnded: () => void;
  onFailure: (error: string) => void;
}

export function PetPlayer({ character, root, state, actionName, variant, active, onReady, onPlaying, onEnded, onFailure }: Props) {
  const first = useRef<HTMLVideoElement>(null);
  const second = useRef<HTMLVideoElement>(null);
  const front = useRef<HTMLVideoElement | null>(null);
  const requestedVideo = useRef<HTMLVideoElement | null>(null);
  const [showPoster, setShowPoster] = useState(true);
  const callbacks = useRef({ onReady, onPlaying, onEnded, onFailure });
  callbacks.current = { onReady, onPlaying, onEnded, onFailure };

  useEffect(() => {
    const slots = [first.current, second.current];
    requestedVideo.current = null;
    if (!active) {
      for (const slot of slots) slot?.pause();
      return;
    }
    const video = slots.find(slot => slot && slot !== front.current && slot.paused)
      ?? slots.find(slot => slot && slot !== front.current);
    if (!video) return;
    let outdated = false;
    let failed: string | undefined;
    const selected = actionName ? character.actions[actionName] : undefined;
    let action = actionName && selected
      ? { name: actionName, file: selected.file, loop: false }
      : resolveAction(character, state, variant);

    const poster = (reason: string) => {
      front.current?.pause();
      if (front.current) front.current.style.opacity = "0";
      front.current = null;
      setShowPoster(true);
      callbacks.current.onFailure(reason);
      callbacks.current.onReady();
      callbacks.current.onEnded();
    };
    const play = () => {
      if (!action) { poster("No playable animation in the pet catalog"); return; }
      const path = petAsset(root, action.file);
      if (!actionName && front.current?.src === path && !front.current.ended) {
        const current = front.current;
        requestedVideo.current = current;
        current.loop = action.loop;
        void current.play().catch(() => { if (!outdated) loadVideo(path); });
        return;
      }
      loadVideo(path);
    };
    const loadVideo = (path: string) => {
      requestedVideo.current = video;
      video.loop = Boolean(action?.loop);
      video.style.transform = "";
      video.src = path;
      video.load();
      void video.play().catch(reason => {
        if (!outdated && !video.error) fallback(String(reason));
      });
    };
    const fallback = (error: string) => {
      if (actionName) {
        callbacks.current.onFailure(`${actionName}: ${error}`);
        callbacks.current.onEnded();
        return;
      }
      if (!action || failed) { poster(error); return; }
      failed = action.name;
      callbacks.current.onFailure(`${failed}: ${error}`);
      action = resolveAction(character, character.states[state]?.fallback ?? "idle", 0, failed);
      play();
    };
    const playing = () => {
      if (outdated) return;
      if (front.current && front.current !== video) {
        front.current.style.opacity = "0";
        front.current.pause();
      }
      front.current = video;
      video.style.opacity = "1";
      setShowPoster(false);
      callbacks.current.onReady();
      if (action) callbacks.current.onPlaying?.(video, action.name);
    };
    const error = () => { if (!outdated) fallback(video.error?.message || "Video could not be decoded"); };
    video.addEventListener("playing", playing);
    video.addEventListener("error", error);
    play();
    return () => {
      outdated = true;
      video.removeEventListener("playing", playing);
      video.removeEventListener("error", error);
      if (front.current !== video) video.pause();
    };
  }, [active, character, root, state, actionName, variant]);

  const ended = (event: React.SyntheticEvent<HTMLVideoElement>) => {
    if (event.currentTarget === front.current && event.currentTarget === requestedVideo.current) callbacks.current.onEnded();
  };
  return <>
    {showPoster && <img src={petAsset(root, character.thumbnail)} alt="" className="pet-poster" draggable={false} />}
    <video ref={first} muted playsInline className="pet-video" onEnded={ended} />
    <video ref={second} muted playsInline className="pet-video" onEnded={ended} />
  </>;
}
