import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { message } from "../message";

import {
  pickSshIdentityFile,
  sshCreateConnection,
  sshListConnections,
  sshRemoveConnection,
  sshTestConnection,
  sshUpdateConnection,
  type SshConnection,
  type SshTestResult,
} from "../../api";

type AuthType = "password" | "file";

interface FormData {
  name: string;
  host: string;
  port: string;
  username: string;
  authType: AuthType;
  keyPath: string;
  password: string;
}

const emptyForm: FormData = {
  name: "",
  host: "",
  port: "22",
  username: "",
  authType: "password",
  keyPath: "",
  password: "",
};

export function ConnectionsSettings() {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<SshConnection[]>([]);
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState<FormData>(emptyForm);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<Record<string, SshTestResult>>({});

  const load = () => {
    sshListConnections().then(setConnections).catch(() => setConnections([]));
  };

  useEffect(() => {
    load();
  }, []);

  const openCreate = () => {
    setEditingId(null);
    setForm(emptyForm);
    setIsModalOpen(true);
    setTestResult({});
  };

  const openEdit = (conn: SshConnection) => {
    setEditingId(conn.id);
    setForm({
      name: conn.name,
      host: conn.host,
      port: String(conn.port),
      username: conn.username,
      authType: conn.key_path ? "file" : "password",
      keyPath: conn.key_path || "",
      password: "",
    });
    setIsModalOpen(true);
    setTestResult({});
  };

  const closeModal = () => {
    setIsModalOpen(false);
    setEditingId(null);
    setForm(emptyForm);
    setTestResult({});
  };

  const handleSave = async () => {
    if (!form.name || !form.host || !form.username) return;

    const port = parseInt(form.port, 10) || 22;
    const authType = form.authType === "file" ? "key_file" : "password";
    const keyPath = form.authType === "file" ? form.keyPath : undefined;
    const password = form.authType === "password" ? form.password : undefined;

    if (editingId) {
      await sshUpdateConnection(
        editingId,
        form.name,
        form.host,
        port,
        form.username,
        authType,
        keyPath,
        password,
      );
    } else {
      await sshCreateConnection(
        form.name,
        form.host,
        port,
        form.username,
        authType,
        keyPath,
        password,
      );
    }

    closeModal();
    load();
  };

  const handlePickKeyFile = async () => {
    const selected = await pickSshIdentityFile();
    if (!selected) return;
    setForm((prev) => ({ ...prev, keyPath: selected }));
  };

  const handleRemove = async (id: string) => {
    await sshRemoveConnection(id);
    setConnections((prev) => prev.filter((conn) => conn.id !== id));
    setTestResult((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  const handleTest = async (id: string) => {
    setTestingId(id);
    setConnections((prev) =>
      prev.map((conn) =>
        conn.id === id
          ? { ...conn, status: "connecting", last_error: undefined }
          : conn,
      ),
    );

    try {
      const result = await sshTestConnection(id);
      setTestResult((prev) => ({ ...prev, [id]: result }));
      if (result.ok) {
        message.success(
          `${t("settings.connections.testOk")}${
            typeof result.latency_ms === "number" ? ` (${result.latency_ms}ms)` : ""
          }`,
        );
      } else {
        message.error(
          `${t("settings.connections.testFailed")}${
            result.error ? `: ${result.error}` : ""
          }`,
        );
      }
      setConnections((prev) =>
        prev.map((conn) =>
          conn.id === id
            ? {
                ...conn,
                status: result.ok ? "connected" : "error",
                latency_ms: result.latency_ms,
                last_error: result.ok ? undefined : result.error,
              }
            : conn,
        ),
      );
    } catch (error) {
      const errorMessage =
        error instanceof Error
          ? error.message
          : t("settings.connections.testFailed");

      setTestResult((prev) => ({
        ...prev,
        [id]: { ok: false, error: errorMessage },
      }));
      message.error(
        `${t("settings.connections.testFailed")}${
          errorMessage ? `: ${errorMessage}` : ""
        }`,
      );
      setConnections((prev) =>
        prev.map((conn) =>
          conn.id === id
            ? {
                ...conn,
                status: "error",
                last_error: errorMessage,
              }
            : conn,
        ),
      );
    } finally {
      setTestingId(null);
    }
  };

  const statusLabel = (status: SshConnection["status"]) => {
    switch (status) {
      case "connected":
        return t("settings.connections.online");
      case "connecting":
        return t("settings.connections.checking");
      case "error":
        return t("settings.connections.offline");
      default:
        return t("settings.connections.unknown");
    }
  };

  const statusClassName = (status: SshConnection["status"]) => {
    switch (status) {
      case "connected":
        return "bg-green-100 text-green-700";
      case "connecting":
        return "bg-yellow-100 text-yellow-700";
      case "error":
        return "bg-red-100 text-red-700";
      default:
        return "bg-gray-100 text-text-secondary";
    }
  };

  const testMessage = (id: string) => {
    const result = testResult[id];
    if (!result) return null;

    if (result.ok) {
      return `${t("settings.connections.testOk")}${
        typeof result.latency_ms === "number" ? ` (${result.latency_ms}ms)` : ""
      }`;
    }

    return `${t("settings.connections.testFailed")}${
      result.error ? `: ${result.error}` : ""
    }`;
  };

  return (
    <>
      <div className="mb-8">
        <h1 className="text-2xl font-semibold text-text-base">
          {t("settings.connections.title")}
        </h1>
      </div>

      <div className="mb-6 max-w-[800px]">
        <h2 className="mb-4 text-[13px] font-medium text-text-base">
          {t("settings.connections.sshConnections")}
        </h2>

        {connections.length === 0 ? (
          <div className="flex flex-col items-center justify-center rounded-xl border border-border-theme bg-white p-10 shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
            <div className="mb-3 flex items-center space-x-2 text-2xl text-gray-700">
              <FontAwesomeIcon icon={["fas", "laptop"]} />
              <span className="relative -top-1 text-sm font-bold tracking-widest">
                ...
              </span>
              <FontAwesomeIcon icon={["fas", "server"]} />
            </div>
            <div className="mb-4 text-[13px] text-text-secondary">
              {t("settings.connections.sshDesc")}
            </div>
            <button
              className="rounded-full border border-transparent bg-gray-100 px-4 py-1.5 text-[13px] font-medium text-text-base transition-colors hover:bg-gray-200"
              onClick={openCreate}
            >
              {t("settings.connections.add")}
            </button>
          </div>
        ) : (
          <div className="space-y-3">
            {connections.map((conn) => (
              <div
                key={conn.id}
                className="rounded-xl border border-border-theme bg-white p-4 shadow-[0_1px_2px_rgb(0,0,0,0.02)]"
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="flex min-w-0 items-center space-x-3">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-gray-100 text-gray-600">
                      <FontAwesomeIcon icon={["fas", "server"]} />
                    </div>
                    <div className="min-w-0">
                      <div className="text-[13px] font-medium text-text-base">
                        {conn.name}
                      </div>
                      <div className="break-all text-[12px] text-text-secondary">
                        {conn.username}@{conn.host}:{conn.port}
                      </div>
                      {conn.last_error && (
                        <div className="mt-0.5 break-all text-[11px] text-red-500">
                          {conn.last_error}
                        </div>
                      )}
                    </div>
                  </div>

                  <div className="flex shrink-0 items-center space-x-2">
                    <span
                      className={`rounded-full px-2 py-0.5 text-[11px] ${statusClassName(conn.status)}`}
                    >
                      {statusLabel(conn.status)}
                    </span>
                    <button
                      className="rounded-full bg-black px-3 py-1 text-[12px] text-white hover:bg-gray-800 disabled:opacity-60"
                      onClick={() => handleTest(conn.id)}
                      disabled={testingId === conn.id}
                    >
                      {testingId === conn.id
                        ? t("settings.connections.testing")
                        : t("settings.connections.test")}
                    </button>
                    <button
                      className="rounded-full bg-gray-100 px-3 py-1 text-[12px] hover:bg-gray-200"
                      onClick={() => openEdit(conn)}
                    >
                      {t("settings.connections.edit")}
                    </button>
                    <button
                      className="rounded-full bg-red-50 px-3 py-1 text-[12px] text-red-600 hover:bg-red-100"
                      onClick={() => handleRemove(conn.id)}
                    >
                      {t("settings.connections.delete")}
                    </button>
                  </div>
                </div>

                {testResult[conn.id] && !testResult[conn.id].ok && (
                  <div
                    className={`mt-3 text-[12px] ${
                      testResult[conn.id].ok ? "text-green-600" : "text-red-500"
                    }`}
                  >
                    {testMessage(conn.id)}
                  </div>
                )}
              </div>
            ))}

            <button
              className="w-full rounded-xl border border-dashed border-border-theme py-2 text-[13px] text-text-secondary transition-colors hover:border-gray-400 hover:text-text-base"
              onClick={openCreate}
            >
              + {t("settings.connections.add")}
            </button>
          </div>
        )}
      </div>

      {isModalOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-[rgba(15,23,42,0.16)] px-6 py-8"
          onMouseDown={closeModal}
        >
          <div
            className="flex max-h-[calc(100vh-64px)] w-full max-w-[520px] flex-col overflow-hidden rounded-[20px] border border-black/5 bg-white shadow-[0_18px_44px_rgba(15,23,42,0.16)]"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <div className="flex items-start justify-between gap-3 border-b border-border-theme px-5 py-4">
              <div>
                <h3 className="text-[18px] font-semibold tracking-tight text-text-base">
                  {editingId
                    ? t("settings.connections.edit")
                    : t("settings.connections.addSsh")}
                </h3>
                <p className="mt-1 text-[11px] leading-4 text-text-secondary">
                  {t("settings.connections.sshDesc")}
                </p>
              </div>
              <button
                type="button"
                className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-gray-400 transition-colors hover:bg-gray-100 hover:text-text-base"
                onClick={closeModal}
              >
                <FontAwesomeIcon icon={["fas", "times"]} className="text-[12px]" />
              </button>
            </div>

            <form
              className="min-h-0 flex-1 overflow-y-auto"
              onSubmit={(event) => {
                event.preventDefault();
                void handleSave();
              }}
            >
              <div className="space-y-3 px-5 py-4">
                <section className="space-y-2.5">
                  <div>
                    <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                      {t("settings.connections.displayName")}
                    </label>
                    <input
                      type="text"
                      value={form.name}
                      onChange={(e) => setForm({ ...form, name: e.target.value })}
                      className="h-[34px] w-full rounded-[14px] border border-blue-400 bg-white px-3 text-[13px] text-text-base shadow-[0_0_0_1px_rgba(59,130,246,0.12)] outline-none transition-colors focus:border-blue-500"
                    />
                  </div>

                  <div>
                    <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                      {t("settings.connections.hostname")}
                    </label>
                    <input
                      type="text"
                      value={form.host}
                      onChange={(e) => setForm({ ...form, host: e.target.value })}
                      placeholder={t("settings.connections.hostPlaceholder")}
                      className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                    />
                  </div>

                  <div className="grid gap-3 sm:grid-cols-[110px_minmax(0,1fr)]">
                    <div>
                      <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                        {t("settings.connections.sshPort")}{" "}
                        <span className="font-normal text-gray-400">
                          {t("settings.connections.optional")}
                        </span>
                      </label>
                      <input
                        type="text"
                        value={form.port}
                        onChange={(e) => setForm({ ...form, port: e.target.value })}
                        className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                      />
                    </div>

                    <div>
                      <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                        {t("settings.connections.username")}
                      </label>
                      <input
                        type="text"
                        value={form.username}
                        onChange={(e) => setForm({ ...form, username: e.target.value })}
                        className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                      />
                    </div>
                  </div>
                </section>

                <section className="space-y-2.5 border-t border-border-theme pt-3">
                  <div className="text-[11px] font-medium text-text-secondary">
                    SSH
                  </div>

                  {form.authType === "password" ? (
                    <div>
                      <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                        {t("settings.connections.password")}
                      </label>
                      <input
                        type="password"
                        value={form.password}
                        onChange={(e) => setForm({ ...form, password: e.target.value })}
                        className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                      />
                    </div>
                  ) : (
                    <div>
                      <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                        {t("settings.connections.identityFilePath")}
                      </label>
                      <div className="flex items-center gap-2">
                        <input
                          type="text"
                          value={form.keyPath}
                          readOnly
                          placeholder={t("settings.connections.selectIdentityFile")}
                          className="h-[34px] min-w-0 flex-1 rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                        />
                        <button
                          type="button"
                          className="inline-flex h-[34px] shrink-0 items-center rounded-[14px] border border-border-theme bg-gray-50 px-3 text-[12px] font-medium text-text-base transition-colors hover:bg-gray-100"
                          onClick={() => void handlePickKeyFile()}
                        >
                          <FontAwesomeIcon icon={["fas", "folder-open"]} className="mr-1.5 text-[11px] text-text-secondary" />
                          {t("settings.connections.browse")}
                        </button>
                      </div>
                    </div>
                  )}

                  <div className="flex overflow-hidden rounded-full border border-border-theme bg-gray-100 p-0.5">
                    <button
                      type="button"
                      className={`flex h-[28px] flex-1 items-center justify-center rounded-full px-3 text-[12px] font-medium transition-colors ${
                        form.authType === "password"
                          ? "bg-white text-text-base shadow-sm"
                          : "text-text-secondary hover:text-text-base"
                      }`}
                      onClick={() => setForm({ ...form, authType: "password" })}
                    >
                      {t("settings.connections.password")}
                    </button>
                    <button
                      type="button"
                      className={`flex h-[28px] flex-1 items-center justify-center rounded-full px-3 text-[12px] font-medium transition-colors ${
                        form.authType === "file"
                          ? "bg-white text-text-base shadow-sm"
                          : "text-text-secondary hover:text-text-base"
                      }`}
                      onClick={() => setForm({ ...form, authType: "file" })}
                    >
                      {t("settings.connections.identityFile")}
                    </button>
                  </div>
                </section>
              </div>

              <div className="flex items-center justify-end gap-2.5 border-t border-border-theme bg-gray-50/70 px-5 py-2.5">
                <button
                  type="button"
                  className="rounded-full px-3.5 py-1 text-[12px] text-text-secondary transition-colors hover:bg-gray-200/70 hover:text-text-base"
                  onClick={closeModal}
                >
                  {t("settings.connections.cancel")}
                </button>
                <button
                  type="submit"
                  className="rounded-full bg-black px-4 py-1 text-[12px] font-medium text-white shadow-sm transition-colors hover:bg-gray-800 disabled:opacity-50"
                  disabled={!form.name || !form.host || !form.username}
                >
                  {editingId
                    ? t("settings.connections.save")
                    : t("settings.connections.add")}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </>
  );
}
