import { check, type CheckOptions, type Update } from "@tauri-apps/plugin-updater";

let downloadedUpdate: Update | null = null;
let availableUpdate: Update | null = null;
let checkPromise: Promise<boolean> | null = null;
let downloadPromise: Promise<boolean> | null = null;

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function updateProxyCandidates(): Array<string | undefined> {
  const envProxy = (import.meta as unknown as { env?: Record<string, string | undefined> }).env
    ?.VITE_DEEPAGENT_UPDATE_PROXY;
  const userProxy =
    typeof localStorage === "undefined"
      ? undefined
      : localStorage.getItem("deepagent.updateProxy") ?? undefined;
  return [undefined, userProxy, envProxy].filter((value, index, arr) => {
    if (!value) return index === 0;
    return arr.indexOf(value) === index;
  });
}

async function checkWithFallbacks(): Promise<Update | null> {
  let lastError: unknown = null;
  for (const proxy of updateProxyCandidates()) {
    const options: CheckOptions = {
      timeout: 20_000,
      ...(proxy ? { proxy } : {}),
    };
    try {
      return await check(options);
    } catch (error) {
      lastError = error;
    }
  }
  if (lastError) console.warn("update check failed", lastError);
  return null;
}

export function hasDownloadedUpdate(): boolean {
  return downloadedUpdate !== null;
}

export function checkForAvailableUpdate(): Promise<boolean> {
  if (!inTauri()) return Promise.resolve(false);
  if (downloadedUpdate || availableUpdate) return Promise.resolve(availableUpdate !== null);
  if (checkPromise) return checkPromise;

  checkPromise = (async () => {
    try {
      availableUpdate = await checkWithFallbacks();
      return availableUpdate !== null;
    } finally {
      checkPromise = null;
    }
  })();

  return checkPromise;
}

export function downloadUpdateForNextShutdown(): Promise<boolean> {
  if (!inTauri()) return Promise.resolve(false);
  if (downloadedUpdate) return Promise.resolve(true);
  if (downloadPromise) return downloadPromise;

  downloadPromise = (async () => {
    try {
      const update = availableUpdate ?? (await checkWithFallbacks());
      if (!update) return false;
      await update.download(undefined, { timeout: 120_000 });
      downloadedUpdate = update;
      availableUpdate = null;
      return true;
    } catch (error) {
      console.warn("update download failed", error);
      availableUpdate = null;
      return false;
    } finally {
      downloadPromise = null;
    }
  })();

  return downloadPromise;
}

export async function installDownloadedUpdate(): Promise<boolean> {
  const update = downloadedUpdate;
  if (!update) return false;

  try {
    await update.install();
    downloadedUpdate = null;
    return true;
  } catch (error) {
    console.warn("update install failed", error);
    return false;
  } finally {
    if (downloadedUpdate === null) {
      update.close().catch(() => {});
    }
  }
}
