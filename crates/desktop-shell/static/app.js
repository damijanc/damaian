const $ = (id) => document.getElementById(id);

let currentSessionId = "";
let apiToken = "";
let bootstrapPromise = null;
let bootstrapError = null;
let chatSubmitting = false;
let pinnedContextFiles = [];
let terminalCwd = "";
let terminalOpen = false;
let terminalBusy = false;
let projectPaths = [];
let expandedProjectPaths = new Set();
let projectSessionsByPath = new Map();
let projectSessionsLoading = new Set();
let projectsCollapsed = false;
let appUpdateInfo = null;
let appUpdateInstalling = false;
let currentPolicyModelOptions = null;
let toastTimer = null;

const localApiOrigin = "http://127.0.0.1:4765";
const localApiHostnames = new Set(["127.0.0.1", "localhost"]);
const apiTokenHeader = "x-damaian-api-token";
const lastRepoStorageKey = "damaian:lastRepository";
const projectsStorageKey = "damaian:projects";
const expandedProjectsStorageKey = "damaian:expandedProjects";
const projectsCollapsedStorageKey = "damaian:projectsCollapsed";
const pinnedContextStoragePrefix = "damaian:pinnedContextFiles";
const chatModelPrefsStoragePrefix = "damaian:chatModelPrefs";

const builtInProviderIds = ["openai", "deepseek", "openai-compatible"];
const builtInProviderIdSet = new Set(builtInProviderIds);
const builtInModelProviderPresets = {
  openai: {
    label: "OpenAI",
    baseUrl: "https://api.openai.com",
    apiKeyEnv: "OPENAI_API_KEY",
    defaultModel: "gpt-4.1",
    models: ["gpt-4.1", "gpt-4.1-mini", "o4-mini"],
  },
  deepseek: {
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com",
    apiKeyEnv: "DEEPSEEK_API_KEY",
    defaultModel: "deepseek-chat",
    models: ["deepseek-chat", "deepseek-reasoner"],
  },
  "openai-compatible": {
    label: "Custom",
    baseUrl: "https://api.openai.com",
    apiKeyEnv: "OPENAI_API_KEY",
    defaultModel: "configured-model",
    models: ["configured-model"],
  },
};
const modelProviderPresets = {};
const configuredProviderIds = new Set();

const validReasoningLevels = new Set(["default", "minimal", "low", "medium", "high"]);
const providerLabels = {};
const reasoningLabels = {
  default: "Default",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "Extra High",
};
const popularProviderPresets = [
  {
    id: "openai",
    label: "OpenAI",
    description: "GPT and reasoning models",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    description: "DeepSeek chat and reasoning models",
  },
  {
    id: "openai-compatible",
    label: "OpenAI compatible",
    description: "Custom hosted compatible endpoint",
  },
  {
    id: "ollama",
    label: "Ollama",
    description: "Local OpenAI-compatible runtime",
    baseUrl: "http://localhost:11434/v1",
    apiKeyEnv: "keychain:ollama-api-key",
    models: ["llama3.1", "qwen2.5-coder"],
  },
];

function repo() {
  return $("repo").value.trim();
}

function errorMessage(error, fallback = "Unexpected error") {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error && typeof error === "object") {
    if (typeof error.message === "string" && error.message.trim()) return error.message.trim();
    if (typeof error.error === "string" && error.error.trim()) return error.error.trim();
    try {
      const serialized = JSON.stringify(error);
      if (serialized && serialized !== "{}") return serialized;
    } catch {
      // Fall through to fallback.
    }
  }
  return fallback;
}

function updaterErrorMessage(error) {
  const message = errorMessage(error, "Unable to check for updates");
  if (/no endpoint|endpoint.*not.*set|endpoint.*not.*configured/i.test(message)) {
    return "Updater endpoint is not configured for this build";
  }
  if (/pubkey|public key|signature/i.test(message)) {
    return "Updater signing public key is not configured for this build";
  }
  return message;
}

function toast(message, { duration } = {}) {
  const el = $("toast");
  const text = String(message || "Unexpected error");
  el.textContent = text;
  el.classList.add("show");
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(
    () => el.classList.remove("show"),
    duration || Math.min(7000, Math.max(3200, text.length * 70)),
  );
}

async function api(path, options = {}, retriedAuth = false) {
  if (isProtectedApiPath(path)) {
    await ensureDesktopApiReady();
  }
  const response = await fetch(apiUrl(path), withApiToken(path, options));
  const text = await response.text();
  let payload;
  try {
    payload = JSON.parse(text);
  } catch {
    payload = { error: text };
  }
  if (response.status === 401 && isProtectedApiPath(path) && !retriedAuth) {
    apiToken = "";
    bootstrapError = null;
    bootstrapPromise = startBootstrap();
    await ensureDesktopApiReady();
    return api(path, options, true);
  }
  if (!response.ok || payload.error) {
    throw new Error(payload.error || response.statusText);
  }
  return payload;
}

function isProtectedApiPath(path) {
  return path.startsWith("/api/");
}

function withApiToken(path, options = {}) {
  const next = { ...options };
  const headers = new Headers(next.headers || {});
  if (isProtectedApiPath(path)) {
    if (!apiToken) throw new Error("Desktop API is still starting. Try again in a moment.");
    headers.set(apiTokenHeader, apiToken);
  }
  next.headers = headers;
  return next;
}

function apiUrl(path) {
  if (!path.startsWith("/api/")) return path;
  if (isLocalShellOrigin()) return path;
  return `${localApiOrigin}${path}`;
}

function isLocalShellOrigin() {
  return (
    (window.location.protocol === "http:" || window.location.protocol === "https:") &&
    localApiHostnames.has(window.location.hostname)
  );
}

async function ensureDesktopApiReady() {
  if (apiToken) return;
  if (!bootstrapPromise || bootstrapError) {
    bootstrapPromise = startBootstrap();
  }
  if (bootstrapPromise) await bootstrapPromise;
  if (apiToken) return;
  throw bootstrapError || new Error("Desktop API is still starting. Try again in a moment.");
}

function startBootstrap() {
  bootstrapError = null;
  return Promise.resolve()
    .then(async () => {
      const invoke = tauriInvoke();
      if (!invoke) throw new Error("Desktop API bootstrap is available in the desktop app");
      const bootstrap = await invoke("damaian_desktop_bootstrap");
      const token = bootstrap?.apiToken || "";
      if (!token) throw new Error("Desktop API token missing from Tauri bootstrap");
      apiToken = token;
      if (!chatSubmitting) $("ask-btn").disabled = false;
      loadProjectState();
      const lastRepo = localStorage.getItem(lastRepoStorageKey);
      if (lastRepo) {
        setRepository(lastRepo, false);
      } else if (bootstrap.defaultRepo) {
        setRepository(bootstrap.defaultRepo, false);
      } else {
        loadPinnedContextFiles("");
        clearSessionList();
        clearChat();
        renderProjectList();
        void loadConfigFile().catch((error) => setModelKeyStatus(error.message, "error"));
      }
      scheduleUpdateCheck();
    })
    .catch((error) => {
      bootstrapError = error;
      if (!chatSubmitting) $("ask-btn").disabled = false;
      setChatStatus("Desktop API unavailable", "error");
      toast(`Desktop API unavailable: ${error.message}`);
    });
}

function form(data) {
  const params = new URLSearchParams();
  Object.entries(data).forEach(([key, value]) => params.set(key, value ?? ""));
  return {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: params.toString(),
  };
}

function requireRepo() {
  const value = repo();
  if (!value) throw new Error("Repository is required");
  return value;
}

function setRepoState(message) {
  $("repo-state").textContent = message;
}

function normalizeProjectPath(value) {
  const path = String(value || "").trim();
  if (path.length <= 1) return path;
  return path.replace(/[\\/]+$/, "");
}

function projectName(projectPath) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized) return "Untitled";
  const parts = normalized.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || normalized;
}

function loadProjectState() {
  try {
    const stored = JSON.parse(localStorage.getItem(projectsStorageKey) || "[]");
    projectPaths = Array.isArray(stored)
      ? stored.map(normalizeProjectPath).filter(Boolean)
      : [];
  } catch {
    projectPaths = [];
  }
  const legacyRepo = normalizeProjectPath(localStorage.getItem(lastRepoStorageKey));
  if (legacyRepo && !projectPaths.includes(legacyRepo)) {
    projectPaths.push(legacyRepo);
  }
  projectPaths = [...new Set(projectPaths)];

  try {
    const storedExpanded = JSON.parse(localStorage.getItem(expandedProjectsStorageKey) || "[]");
    expandedProjectPaths = new Set(
      Array.isArray(storedExpanded)
        ? storedExpanded.map(normalizeProjectPath).filter((path) => projectPaths.includes(path))
        : [],
    );
  } catch {
    expandedProjectPaths = new Set();
  }
  projectsCollapsed = localStorage.getItem(projectsCollapsedStorageKey) === "true";
  setProjectsCollapsed(projectsCollapsed, false);
}

function saveProjectState() {
  localStorage.setItem(projectsStorageKey, JSON.stringify(projectPaths));
  localStorage.setItem(expandedProjectsStorageKey, JSON.stringify([...expandedProjectPaths]));
}

function rememberProject(projectPath) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized) return "";
  if (!projectPaths.includes(normalized)) {
    projectPaths.push(normalized);
  }
  expandedProjectPaths.add(normalized);
  saveProjectState();
  return normalized;
}

function setProjectsCollapsed(collapsed, persist = true) {
  projectsCollapsed = collapsed;
  $("projects-toggle-btn").setAttribute("aria-expanded", collapsed ? "false" : "true");
  $("projects-toggle-btn").classList.toggle("is-collapsed", collapsed);
  document.querySelector(".projects-panel").classList.toggle("is-collapsed", collapsed);
  if (persist) {
    localStorage.setItem(projectsCollapsedStorageKey, collapsed ? "true" : "false");
  }
  renderProjectList();
}

function lastSessionStorageKey(repoPath = repo()) {
  return `damaian:lastSession:${repoPath}`;
}

function pinnedContextStorageKey(sessionId = currentSessionId, repoPath = repo()) {
  return `${pinnedContextStoragePrefix}:${repoPath}:${sessionId || "draft"}`;
}

function loadPinnedContextFiles(sessionId = currentSessionId) {
  try {
    const stored = JSON.parse(localStorage.getItem(pinnedContextStorageKey(sessionId)) || "[]");
    pinnedContextFiles = Array.isArray(stored)
      ? stored.filter((path) => typeof path === "string" && path.trim()).map((path) => path.trim())
      : [];
  } catch {
    pinnedContextFiles = [];
  }
  renderPinnedContextFiles();
}

function savePinnedContextFiles(sessionId = currentSessionId) {
  const key = pinnedContextStorageKey(sessionId);
  if (pinnedContextFiles.length) {
    localStorage.setItem(key, JSON.stringify(pinnedContextFiles));
  } else {
    localStorage.removeItem(key);
  }
}

function persistPinnedContextForSession(sessionId) {
  if (!sessionId) return;
  savePinnedContextFiles(sessionId);
  localStorage.removeItem(pinnedContextStorageKey("", repo()));
}

function addPinnedContextFile(path) {
  const normalized = String(path || "").trim();
  if (!normalized || pinnedContextFiles.includes(normalized)) return;
  pinnedContextFiles.push(normalized);
  savePinnedContextFiles();
  renderPinnedContextFiles();
}

function removePinnedContextFile(path) {
  pinnedContextFiles = pinnedContextFiles.filter((item) => item !== path);
  savePinnedContextFiles();
  renderPinnedContextFiles();
}

function clearPinnedContextFiles() {
  pinnedContextFiles = [];
  savePinnedContextFiles();
  renderPinnedContextFiles();
}

function renderPinnedContextFiles() {
  const container = $("pinned-context-files");
  container.innerHTML = "";
  $("clear-context-files-btn").disabled = pinnedContextFiles.length === 0;
  if (!pinnedContextFiles.length) {
    return;
  }
  pinnedContextFiles.forEach((path) => {
    const chip = document.createElement("span");
    chip.className = "context-chip";
    chip.title = path;
    const label = document.createElement("span");
    label.textContent = path;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${path} from context`);
    remove.textContent = "×";
    remove.addEventListener("click", () => removePinnedContextFile(path));
    chip.append(label, remove);
    container.append(chip);
  });
}

function applyRepositoryState(value, persist = true) {
  const projectPath = normalizeProjectPath(value);
  $("repo").value = projectPath;
  setRepoState(projectPath ? projectName(projectPath) : "No repository selected");
  if (projectPath) {
    rememberProject(projectPath);
  }
  if (persist && projectPath) {
    localStorage.setItem(lastRepoStorageKey, projectPath);
  }
  currentSessionId = "";
  loadPinnedContextFiles("");
  if (terminalOpen) {
    void resetTerminalCwd().catch((error) => appendTerminalLine(error.message, "stderr"));
  } else {
    terminalCwd = "";
  }
  renderProjectList();
  return projectPath;
}

function setRepository(value, persist = true) {
  const projectPath = applyRepositoryState(value, persist);
  if (projectPath) {
    void loadSessions("", true).catch((error) => toast(error.message));
  } else {
    clearSessionList();
    clearChat();
  }
  void loadConfigFile().catch((error) => setModelKeyStatus(error.message, "error"));
}

async function switchProject(projectPath, options = {}) {
  const normalized = applyRepositoryState(projectPath, options.persist !== false);
  if (!normalized) {
    clearSessionList();
    clearChat();
    return;
  }
  await loadSessions(options.preferredSessionId || "", options.reloadSelected !== false);
  void loadConfigFile().catch((error) => setModelKeyStatus(error.message, "error"));
}

function tauriDialogOpen() {
  return window.__TAURI__?.dialog?.open;
}

function tauriInvoke() {
  return window.__TAURI__?.core?.invoke;
}

function tauriUpdater() {
  return window.__TAURI__?.updater;
}

function isDesktopApp() {
  return Boolean(window.__TAURI__);
}

async function addContextFilesFromPicker() {
  const open = tauriDialogOpen();
  if (!open) throw new Error("File picker is available in the desktop app");
  const selected = await open({
    directory: false,
    multiple: true,
    title: "Add Context File",
    defaultPath: requireRepo(),
  });
  const selectedFiles = Array.isArray(selected) ? selected : selected ? [selected] : [];
  for (const path of selectedFiles) {
    const payload = await api("/api/context-file", form({ repo: requireRepo(), path }));
    addPinnedContextFile(payload.path);
  }
  if (selectedFiles.length) {
    toast(`Added ${selectedFiles.length} context file(s)`);
  }
}

function scheduleUpdateCheck() {
  if (!isDesktopApp()) return;
  resetUpdateButton("Check Updates");
  window.setTimeout(() => {
    void checkForAppUpdate(false);
  }, 1200);
}

function resetUpdateButton(title = "Check Updates") {
  $("update-app-footer").hidden = false;
  const button = $("update-app-btn");
  button.hidden = false;
  button.disabled = false;
  button.textContent = "Check Updates";
  button.title = title;
}

async function checkForAppUpdate(showCurrent = true) {
  const updater = tauriUpdater();
  if (!updater?.check) {
    if (showCurrent) toast("Updater is not available in this build");
    return null;
  }
  const button = $("update-app-btn");
  try {
    button.hidden = false;
    button.disabled = true;
    button.textContent = "Checking...";
    const update = await updater.check();
    if (update !== null && typeof update !== "object") {
      throw new Error("Updater returned an invalid response");
    }
    if (!update) {
      resetUpdateButton("Damaian is up to date");
      appUpdateInfo = null;
      if (showCurrent) toast("Damaian is up to date");
      return null;
    }
    const version = update.version || "";
    appUpdateInfo = {
      available: true,
      currentVersion: update.currentVersion || "",
      version,
      update,
    };
    const versionLabel = version || "latest version";
    button.hidden = false;
    button.disabled = false;
    button.textContent = version ? `Update ${version}` : "Update";
    button.title = `Install Damaian ${versionLabel}`;
    toast(`Damaian ${versionLabel} is available`);
    return appUpdateInfo;
  } catch (error) {
    const message = updaterErrorMessage(error);
    appUpdateInfo = null;
    resetUpdateButton(`Update check failed: ${message}`);
    if (showCurrent) toast(`Update check failed: ${message}`, { duration: 7000 });
    return null;
  }
}

async function installAppUpdate() {
  if (appUpdateInstalling) return;
  if (!appUpdateInfo?.available) {
    await checkForAppUpdate(true);
    if (!appUpdateInfo?.available) return;
  }
  const version = appUpdateInfo.version || "the latest version";
  if (!window.confirm(`Install Damaian ${version}? Restart Damaian after the update to finish.`)) {
    return;
  }
  const update = appUpdateInfo.update;
  if (!update?.downloadAndInstall) {
    toast("Updater is not available in this build");
    return;
  }
  const button = $("update-app-btn");
  try {
    appUpdateInstalling = true;
    button.disabled = true;
    button.textContent = "Installing...";
    toast("Downloading update...");
    await update.downloadAndInstall();
    appUpdateInstalling = false;
    appUpdateInfo = null;
    resetUpdateButton("Restart Damaian to finish the update");
    toast("Update installed. Restart Damaian to finish.", { duration: 7000 });
  } catch (error) {
    appUpdateInstalling = false;
    button.disabled = false;
    button.textContent = appUpdateInfo?.available
      ? appUpdateInfo.version
        ? `Update ${appUpdateInfo.version}`
        : "Update"
      : "Check Updates";
    toast(`Update failed: ${updaterErrorMessage(error)}`, { duration: 7000 });
  }
}

function setSettingsPage(page) {
  const target = ["general", "shortcuts", "servers", "providers", "models"].includes(page)
    ? page
    : "providers";
  document.querySelectorAll(".settings-nav-item").forEach((button) => {
    button.classList.toggle("active", button.dataset.settingsPage === target);
  });
  document.querySelectorAll(".settings-page").forEach((section) => {
    section.classList.toggle("active", section.dataset.page === target);
  });
  if (target === "models") renderSettingsModels();
}

function openSettings(page = "providers") {
  setSettingsPage(page);
  $("settings-shell").hidden = false;
  document.body.classList.add("settings-open");
  renderSettingsProviderLists();
  renderSettingsModels();
  void loadConfigFile().catch((error) => setModelKeyStatus(error.message, "error"));
}

function closeSettings() {
  $("settings-shell").hidden = true;
  document.body.classList.remove("settings-open");
}

function setTerminalOpen(open) {
  terminalOpen = open;
  document.body.classList.toggle("terminal-open", open);
  $("terminal-panel").hidden = !open;
  const button = $("terminal-toggle-btn");
  button.setAttribute("aria-pressed", open ? "true" : "false");
  button.setAttribute("aria-label", open ? "Hide terminal" : "Show terminal");
  button.title = open ? "Hide terminal" : "Show terminal";
  if (open) {
    void ensureTerminalReady()
      .then(() => $("terminal-input").focus())
      .catch((error) => {
        appendTerminalLine(error.message, "stderr");
        toast(error.message);
      });
  }
}

async function ensureTerminalReady() {
  if (terminalCwd) {
    renderTerminalPrompt();
    return;
  }
  await resetTerminalCwd();
}

async function resetTerminalCwd() {
  const payload = await api(`/api/terminal-cwd?repo=${encodeURIComponent(repo())}`);
  terminalCwd = payload.cwd;
  renderTerminalPrompt();
  if (terminalOpen) {
    appendTerminalLine(`cwd ${terminalCwd}`, "meta");
  }
}

function renderTerminalPrompt() {
  $("terminal-cwd").textContent = terminalCwd || "Not started";
  $("terminal-title").textContent = terminalCwd ? terminalTitleForPath(terminalCwd) : "Terminal";
  $("terminal-prompt").textContent = "$";
}

function terminalTitleForPath(path) {
  const trimmed = String(path || "").replace(/[\\/]+$/, "");
  if (!trimmed) return "/";
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || trimmed || "Terminal";
}

function stripAnsi(value) {
  return String(value || "").replace(/\x1B\[[0-?]*[ -/]*[@-~]/g, "");
}

function appendTerminalLine(text, kind = "stdout") {
  const output = $("terminal-output");
  const clean = stripAnsi(text);
  const lines = clean.endsWith("\n") ? clean.slice(0, -1).split(/\r?\n/) : clean.split(/\r?\n/);
  lines.forEach((line) => {
    const row = document.createElement("div");
    row.className = `terminal-line ${kind}`;
    row.textContent = line || " ";
    output.append(row);
  });
  output.scrollTop = output.scrollHeight;
}

function appendTerminalCommand(command) {
  appendTerminalLine(`${terminalCwd} $ ${command}`, "command");
}

async function runTerminalCommand(command) {
  if (!command.trim() || terminalBusy) return;
  if (command.trim() === "clear") {
    $("terminal-output").innerHTML = "";
    return;
  }
  terminalBusy = true;
  $("terminal-input").disabled = true;
  try {
    await ensureTerminalReady();
    appendTerminalCommand(command);
    const payload = await api(
      "/api/terminal-run",
      form({
        cwd: terminalCwd,
        command,
      }),
    );
    terminalCwd = payload.cwd || terminalCwd;
    renderTerminalPrompt();
    if (payload.stdout) appendTerminalLine(payload.stdout, "stdout");
    if (payload.stderr) appendTerminalLine(payload.stderr, "stderr");
    if (payload.exitCode !== 0) {
      appendTerminalLine(`exit ${payload.exitCode}`, "meta");
    }
  } catch (error) {
    appendTerminalLine(error.message, "stderr");
    toast(error.message);
  } finally {
    terminalBusy = false;
    $("terminal-input").disabled = false;
    $("terminal-input").focus();
  }
}

function configScope() {
  return "user";
}

function configRepo() {
  return repo();
}

function renderConfigPolicy(payload) {
  $("config-output").textContent = payload.effectiveError
    ? `Effective policy could not be loaded:\n${payload.effectiveError}`
    : payload.effectivePolicy;
  if (!payload.effectiveError) {
    syncProviderCatalogFromPolicy(payload.effectivePolicy);
    currentPolicyModelOptions = modelOptionsFromPolicy(payload.effectivePolicy);
    syncChatModelControlsFromPolicy(payload.effectivePolicy);
    renderProviderConfigSelect();
  }
}

function configValue(content, key) {
  const prefix = `${key}=`;
  const line = String(content || "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .find((item) => item.startsWith(prefix));
  return line ? line.slice(prefix.length).trim() : "";
}

function configEntries(content) {
  return String(content || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"))
    .map((line) => {
      const index = line.indexOf("=");
      return index >= 0 ? [line.slice(0, index).trim(), line.slice(index + 1).trim()] : null;
    })
    .filter(Boolean);
}

function normalizeChatProvider(value) {
  const provider = String(value || "").trim().toLowerCase().replaceAll("_", "-");
  if (provider === "open-ai" || provider === "openai") return "openai";
  if (provider === "deep-seek" || provider === "deepseek" || provider === "deedseek") {
    return "deepseek";
  }
  if (provider === "custom" || provider === "open-ai-compatible" || provider === "openai-compatible") {
    return "openai-compatible";
  }
  return /^[a-z0-9.-]+$/.test(provider) ? provider : "openai-compatible";
}

function normalizeChatReasoning(value) {
  const reasoning = String(value || "").trim().toLowerCase();
  return validReasoningLevels.has(reasoning) ? reasoning : "default";
}

function chatModelPrefsStorageKey() {
  return `${chatModelPrefsStoragePrefix}:${repo() || "global"}`;
}

function readChatModelPrefs() {
  try {
    const stored = JSON.parse(localStorage.getItem(chatModelPrefsStorageKey()) || "{}");
    if (!stored || typeof stored !== "object") return {};
    const prefs = {};
    if (stored.provider) prefs.provider = normalizeChatProvider(stored.provider);
    if (typeof stored.model === "string" && stored.model.trim()) prefs.model = stored.model.trim();
    if (stored.reasoning) prefs.reasoning = normalizeChatReasoning(stored.reasoning);
    return prefs;
  } catch {
    return {};
  }
}

function selectedChatModelOptions() {
  const provider = normalizeChatProvider($("chat-provider").value);
  const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
  return {
    provider,
    model: $("chat-model").value.trim() || preset.defaultModel,
    reasoning: normalizeChatReasoning($("chat-reasoning").value),
  };
}

function saveChatModelPrefs() {
  localStorage.setItem(chatModelPrefsStorageKey(), JSON.stringify(selectedChatModelOptions()));
}

function providerIds() {
  const ids = Object.keys(modelProviderPresets);
  return [
    ...builtInProviderIds.filter((id) => ids.includes(id)),
    ...ids.filter((id) => !builtInProviderIdSet.has(id)).sort((a, b) => a.localeCompare(b)),
  ];
}

function configuredProviderList() {
  return [
    ...builtInProviderIds.filter((id) => configuredProviderIds.has(id)),
    ...[...configuredProviderIds]
      .filter((id) => !builtInProviderIdSet.has(id))
      .sort((a, b) => a.localeCompare(b)),
  ];
}

function splitModelList(value) {
  return String(value || "")
    .split(/[\n,|]/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function syncConfiguredProvidersFromConfig(content) {
  configuredProviderIds.clear();
  configEntries(content).forEach(([key]) => {
    const match = key.match(/^model_provider\.([a-zA-Z0-9_.-]+)\.(label|base_url|api_key_env|models)$/);
    if (match) configuredProviderIds.add(normalizeChatProvider(match[1]));
  });
}

function syncProviderCatalogFromPolicy(policyText) {
  Object.keys(modelProviderPresets).forEach((key) => delete modelProviderPresets[key]);
  Object.keys(providerLabels).forEach((key) => delete providerLabels[key]);
  Object.entries(builtInModelProviderPresets).forEach(([id, preset]) => {
    modelProviderPresets[id] = { ...preset, models: [...preset.models] };
  });

  const providers = {};
  configEntries(policyText).forEach(([key, value]) => {
    const match = key.match(/^model_provider\.([a-zA-Z0-9_.-]+)\.(label|base_url|api_key_env|models)$/);
    if (!match) return;
    const id = normalizeChatProvider(match[1]);
    providers[id] = providers[id] || { id };
    const field = match[2];
    if (field === "label") providers[id].label = value;
    if (field === "base_url") providers[id].baseUrl = value;
    if (field === "api_key_env") providers[id].apiKeyEnv = value;
    if (field === "models") providers[id].models = splitModelList(value);
  });

  Object.entries(providers).forEach(([id, provider]) => {
    const existing = modelProviderPresets[id] || {};
    const models = provider.models?.length ? provider.models : existing.models || [];
    modelProviderPresets[id] = {
      label: provider.label || existing.label || id,
      baseUrl: provider.baseUrl || existing.baseUrl || "",
      apiKeyEnv: provider.apiKeyEnv || existing.apiKeyEnv || "",
      defaultModel: models[0] || existing.defaultModel || "",
      models,
    };
  });

  Object.entries(modelProviderPresets).forEach(([id, provider]) => {
    providerLabels[id] = provider.label || id;
  });
}

function modelOptionsFromPolicy(policyText) {
  const provider = normalizeChatProvider(configValue(policyText, "model_provider") || "openai");
  const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
  return {
    provider,
    model: configValue(policyText, "model_name") || preset.defaultModel,
    reasoning: normalizeChatReasoning(configValue(policyText, "model_reasoning_level")),
  };
}

function applyChatModelOptions(options, { resetModel = false, persist = false } = {}) {
  const provider = normalizeChatProvider(options.provider);
  const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
  $("chat-provider").value = provider;
  if (resetModel || options.model !== undefined || !$("chat-model").value.trim()) {
    $("chat-model").value = options.model || preset.defaultModel;
  }
  $("chat-reasoning").value = normalizeChatReasoning(options.reasoning);
  if (persist) saveChatModelPrefs();
  renderChatModelMenu();
}

function syncChatModelControlsFromPolicy(policyText) {
  const policyOptions = modelOptionsFromPolicy(policyText);
  const storedOptions = readChatModelPrefs();
  applyChatModelOptions({ ...policyOptions, ...storedOptions });
}

function chatModelFormFields() {
  const options = selectedChatModelOptions();
  return {
    model_provider: options.provider,
    model: options.model,
    reasoning_level: options.reasoning,
  };
}

function modelSummaryLabel(options = selectedChatModelOptions()) {
  const model = options.model || "Configured";
  return `${model} ${reasoningLabels[options.reasoning] || "Default"}`;
}

function modelOptionValues(provider) {
  const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
  const selected = selectedChatModelOptions().model;
  return [...new Set([...preset.models, selected].filter(Boolean))];
}

function renderChatModelMenu() {
  const options = selectedChatModelOptions();
  $("model-menu-summary").textContent = modelSummaryLabel(options);
  $("model-provider-value").textContent = providerLabels[options.provider] || options.provider;
  $("model-name-value").textContent = options.model || "Configured";
  $("model-reasoning-value").textContent = reasoningLabels[options.reasoning] || "Default";
  $("custom-model-input").value = options.model;
  renderProviderOptions(options.provider);
  renderModelOptions(options.provider, options.model);
  renderReasoningOptions(options.reasoning);
}

function renderProviderOptions(selectedProvider) {
  const container = $("model-provider-options");
  container.innerHTML = "";
  providerIds().forEach((provider) => {
    container.append(
      modelOptionButton(providerLabels[provider], selectedProvider === provider, () => {
        const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
        const fallbackModel = preset.defaultModel || $("chat-model").value.trim();
        applyChatModelOptions(
          {
            provider,
            model: fallbackModel,
            reasoning: $("chat-reasoning").value,
          },
          { resetModel: true, persist: true },
        );
        showModelMenuPanel("root");
        void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
      }),
    );
  });
}

function renderModelOptions(provider, selectedModel) {
  const container = $("model-options");
  container.innerHTML = "";
  const models = modelOptionValues(provider);
  if (!models.length) {
    const empty = document.createElement("div");
    empty.className = "model-empty-state";
    empty.textContent = "Use a custom model name.";
    container.append(empty);
  }
  models.forEach((model) => {
    container.append(
      modelOptionButton(model, selectedModel === model, () => {
        applyChatModelOptions(
          {
            provider,
            model,
            reasoning: $("chat-reasoning").value,
          },
          { persist: true },
        );
        showModelMenuPanel("root");
      }),
    );
  });
}

function renderReasoningOptions(selectedReasoning) {
  const container = $("model-reasoning-options");
  container.innerHTML = "";
  ["default", "minimal", "low", "medium", "high"].forEach((reasoning) => {
    container.append(
      modelOptionButton(reasoningLabels[reasoning], selectedReasoning === reasoning, () => {
        applyChatModelOptions(
          {
            provider: $("chat-provider").value,
            model: $("chat-model").value,
            reasoning,
          },
          { persist: true },
        );
        showModelMenuPanel("root");
      }),
    );
  });
}

function modelOptionButton(label, selected, onClick) {
  const button = document.createElement("button");
  button.className = "model-option";
  button.type = "button";
  button.dataset.selected = selected ? "true" : "false";
  const text = document.createElement("span");
  text.textContent = label;
  button.append(text);
  button.addEventListener("click", onClick);
  return button;
}

function toggleModelMenu() {
  if ($("chat-model-popover").hidden) {
    openModelMenu();
  } else {
    closeModelMenu();
  }
}

function openModelMenu(panel = "root") {
  renderChatModelMenu();
  $("chat-model-popover").hidden = false;
  $("chat-model-menu-btn").setAttribute("aria-expanded", "true");
  showModelMenuPanel(panel);
}

function closeModelMenu() {
  $("chat-model-popover").hidden = true;
  $("chat-model-menu-btn").setAttribute("aria-expanded", "false");
}

function showModelMenuPanel(panel) {
  document.querySelectorAll(".model-menu-panel").forEach((element) => {
    element.hidden = element.id !== `model-menu-${panel}`;
  });
}

function resetChatModelPrefs() {
  localStorage.removeItem(chatModelPrefsStorageKey());
  applyChatModelOptions(currentPolicyModelOptions || modelOptionsFromPolicy(""), {
    resetModel: true,
  });
  showModelMenuPanel("root");
  void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
}

function applyCustomModel() {
  const model = $("custom-model-input").value.trim();
  if (!model) return;
  applyChatModelOptions(
    {
      provider: $("chat-provider").value,
      model,
      reasoning: $("chat-reasoning").value,
    },
    { persist: true },
  );
  showModelMenuPanel("root");
}

function providerSlug(value) {
  const slug = String(value || "")
    .trim()
    .toLowerCase()
    .replace(/[_\s]+/g, "-")
    .replace(/[^a-z0-9.-]/g, "")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
  return slug ? normalizeChatProvider(slug) : "";
}

function providerConfigFromForm() {
  const label = $("provider-label-input").value.trim();
  const id = providerSlug($("provider-id-input").value || label);
  const baseUrl = $("provider-base-url-input").value.trim().replace(/\/+$/, "");
  const apiKeyEnv = $("provider-key-ref-input").value.trim();
  const models = splitModelList($("provider-models-input").value);
  if (!label) throw new Error("Provider name is required");
  if (!id) throw new Error("Provider ID is required");
  if (!baseUrl) throw new Error("Provider base URL is required");
  if (!apiKeyEnv) throw new Error("Provider API key reference is required");
  if (apiKeyEnv === "keychain:") throw new Error("Keychain account is required");
  if (!models.length) throw new Error("At least one model is required");
  return { id, label, baseUrl, apiKeyEnv, models };
}

function renderProviderConfigSelect(selectedId = $("provider-config-select").value) {
  const select = $("provider-config-select");
  if (!select) return;
  const ids = configuredProviderList();
  select.innerHTML = "";
  select.disabled = !ids.length;
  if (!ids.length) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "No configured providers";
    select.append(option);
    clearProviderConfigForm();
    renderSettingsProviderLists();
    renderSettingsModels();
    return;
  }
  ids.forEach((id) => {
    const option = document.createElement("option");
    option.value = id;
    option.textContent = providerLabels[id] || id;
    select.append(option);
  });
  const nextId = ids.includes(selectedId) ? selectedId : ids[0];
  select.value = nextId;
  renderProviderConfigForm(nextId);
  renderSettingsProviderLists();
  renderSettingsModels();
}

function renderProviderConfigForm(providerId = $("provider-config-select").value) {
  const id = normalizeChatProvider(providerId || "openai");
  const provider = modelProviderPresets[id] || {
    label: "",
    baseUrl: "",
    apiKeyEnv: "",
    models: [],
  };
  $("provider-config-select").value = providerId;
  $("provider-label-input").value = provider.label || "";
  $("provider-id-input").value = id;
  $("provider-id-input").disabled = builtInProviderIdSet.has(id);
  $("provider-id-input").dataset.originalId = id;
  $("provider-base-url-input").value = provider.baseUrl || "";
  $("provider-key-ref-input").value = provider.apiKeyEnv || `keychain:${id}-api-key`;
  $("provider-api-key-input").value = "";
  $("provider-models-input").value = (provider.models || []).join("\n");
  $("provider-remove-btn").disabled = !configuredProviderIds.has(id);
}

function clearProviderConfigForm() {
  $("provider-config-select").value = "";
  $("provider-label-input").value = "";
  $("provider-id-input").value = "";
  $("provider-id-input").disabled = false;
  $("provider-id-input").dataset.originalId = "";
  $("provider-base-url-input").value = "";
  $("provider-key-ref-input").value = "keychain:";
  $("provider-api-key-input").value = "";
  $("provider-models-input").value = "";
  $("provider-remove-btn").disabled = true;
}

function newProviderConfigForm() {
  clearProviderConfigForm();
  $("provider-config-select").disabled = configuredProviderList().length === 0;
  $("provider-label-input").focus();
}

function providerBadge(provider) {
  return provider.apiKeyEnv?.startsWith("keychain:") ? "API key" : "Env";
}

function providerDescription(provider) {
  const models = provider.models || [];
  if (!models.length) return provider.baseUrl || "Custom provider";
  return models.slice(0, 3).join(", ");
}

function providerMark(label) {
  return String(label || "?").trim().slice(0, 1).toUpperCase() || "?";
}

function renderSettingsProviderLists() {
  renderConnectedProviders();
  renderPopularProviders();
}

function renderConnectedProviders() {
  const container = $("connected-provider-list");
  if (!container) return;
  container.innerHTML = "";
  const ids = configuredProviderList();
  if (!ids.length) {
    const empty = document.createElement("div");
    empty.className = "provider-empty-row";
    empty.textContent = "No providers configured.";
    container.append(empty);
    return;
  }
  ids.forEach((id) => {
    const provider = modelProviderPresets[id];
    if (!provider) return;
    const row = document.createElement("div");
    row.className = "provider-list-row";
    row.dataset.provider = id;

    const identity = document.createElement("div");
    identity.className = "provider-identity";
    const mark = document.createElement("span");
    mark.className = "provider-mark";
    mark.textContent = providerMark(provider.label || id);
    const copy = document.createElement("div");
    const title = document.createElement("div");
    title.className = "provider-title";
    const name = document.createElement("strong");
    name.textContent = provider.label || id;
    const badge = document.createElement("span");
    badge.className = "provider-badge";
    badge.textContent = providerBadge(provider);
    title.append(name, badge);
    const description = document.createElement("p");
    description.textContent = providerDescription(provider);
    copy.append(title, description);
    identity.append(mark, copy);

    const actions = document.createElement("div");
    actions.className = "provider-row-actions";
    const editButton = document.createElement("button");
    editButton.type = "button";
    editButton.textContent = "Configure";
    editButton.addEventListener("click", () => {
      renderProviderConfigForm(id);
      $("provider-label-input").focus();
    });
    actions.append(editButton);

    const removeButton = document.createElement("button");
    removeButton.type = "button";
    removeButton.textContent = "Disconnect";
    removeButton.addEventListener("click", async () => {
      if (!window.confirm(`Remove provider ${id}?`)) return;
      renderProviderConfigForm(id);
      try {
        await removeProviderConfigFromSettings();
        toast("LLM provider removed");
      } catch (error) {
        toast(error.message);
      }
    });
    actions.append(removeButton);

    row.append(identity, actions);
    container.append(row);
  });
}

function renderPopularProviders() {
  const container = $("popular-provider-list");
  if (!container) return;
  container.innerHTML = "";
  const presets = popularProviderPresets.filter((preset) => !configuredProviderIds.has(preset.id));
  if (!presets.length) {
    const empty = document.createElement("div");
    empty.className = "provider-empty-row";
    empty.textContent = "All popular providers are configured.";
    container.append(empty);
    return;
  }
  presets.forEach((preset) => {
    const row = document.createElement("div");
    row.className = "provider-list-row";
    const identity = document.createElement("div");
    identity.className = "provider-identity";
    const mark = document.createElement("span");
    mark.className = "provider-mark";
    mark.textContent = providerMark(preset.label);
    const copy = document.createElement("div");
    const title = document.createElement("div");
    title.className = "provider-title";
    const name = document.createElement("strong");
    name.textContent = preset.label;
    title.append(name);
    const description = document.createElement("p");
    description.textContent = preset.description;
    copy.append(title, description);
    identity.append(mark, copy);

    const button = document.createElement("button");
    button.type = "button";
    button.className = "provider-connect-btn";
    button.textContent = "+ Connect";
    button.addEventListener("click", () => connectPopularProvider(preset));

    row.append(identity, button);
    container.append(row);
  });
}

function connectPopularProvider(preset) {
  if (configuredProviderIds.has(preset.id)) {
    renderProviderConfigForm(preset.id);
  } else {
    const provider = {
      ...(builtInModelProviderPresets[preset.id] || {}),
      ...preset,
    };
    newProviderConfigForm();
    $("provider-label-input").value = provider.label;
    $("provider-id-input").value = provider.id;
    $("provider-id-input").disabled = builtInProviderIdSet.has(provider.id);
    $("provider-base-url-input").value = provider.baseUrl || "";
    $("provider-key-ref-input").value = provider.apiKeyEnv || `keychain:${provider.id}-api-key`;
    $("provider-models-input").value = (provider.models || []).join("\n");
  }
  $("provider-label-input").focus();
}

function renderSettingsModels() {
  const container = $("settings-model-list");
  if (!container) return;
  container.innerHTML = "";
  const ids = configuredProviderList();
  if (!ids.length) {
    const empty = document.createElement("div");
    empty.className = "provider-empty-row";
    empty.textContent = "No configured models.";
    container.append(empty);
    return;
  }
  ids.forEach((id) => {
    const provider = modelProviderPresets[id];
    if (!provider) return;
    const row = document.createElement("div");
    row.className = "settings-row";
    const copy = document.createElement("div");
    const title = document.createElement("strong");
    title.textContent = provider.label || id;
    const description = document.createElement("p");
    description.textContent = (provider.models || []).join(", ") || "No models configured";
    copy.append(title, description);
    const configureButton = document.createElement("button");
    configureButton.type = "button";
    configureButton.textContent = "Configure";
    configureButton.addEventListener("click", () => {
      setSettingsPage("providers");
      renderProviderConfigForm(id);
      $("provider-label-input").focus();
    });
    row.append(copy, configureButton);
    container.append(row);
  });
}

async function saveProviderConfig() {
  let provider = providerConfigFromForm();
  const apiKey = $("provider-api-key-input").value.trim();
  if (apiKey) {
    if (!provider.apiKeyEnv.startsWith("keychain:")) {
      throw new Error("API key can only be saved when the reference starts with keychain:");
    }
    const account = provider.apiKeyEnv.slice("keychain:".length).trim();
    if (!account) throw new Error("Keychain account is required");
    const payload = await api("/api/provider-key", form({ account, api_key: apiKey }));
    provider = { ...provider, apiKeyEnv: payload.reference };
  }

  const originalId = $("provider-id-input").dataset.originalId;
  let content = $("config-editor").value;
  if (originalId && originalId !== provider.id) {
    content = removeProviderConfig(content, originalId);
  }
  content = upsertProviderConfig(content, provider);
  $("config-editor").value = content;
  const payload = await saveConfigFile();
  $("provider-api-key-input").value = "";
  renderProviderConfigSelect(provider.id);
  return payload;
}

async function removeProviderConfigFromSettings() {
  const id = normalizeChatProvider($("provider-id-input").dataset.originalId || $("provider-id-input").value);
  if (!id || !configuredProviderIds.has(id)) return;
  $("config-editor").value = removeProviderConfig($("config-editor").value, id);
  if (selectedChatModelOptions().provider === id) {
    localStorage.removeItem(chatModelPrefsStorageKey());
    applyChatModelOptions(modelOptionsFromPolicy(""), { resetModel: true, persist: true });
  }
  const payload = await saveConfigFile();
  renderProviderConfigSelect();
  return payload;
}

function upsertProviderConfig(content, provider) {
  let next = removeProviderConfig(content, provider.id);
  next = upsertConfigValue(next, `model_provider.${provider.id}.label`, provider.label);
  next = upsertConfigValue(next, `model_provider.${provider.id}.base_url`, provider.baseUrl);
  next = upsertConfigValue(next, `model_provider.${provider.id}.api_key_env`, provider.apiKeyEnv);
  next = upsertConfigValue(next, `model_provider.${provider.id}.models`, provider.models.join("|"));
  return next;
}

function removeProviderConfig(content, providerId) {
  const prefix = `model_provider.${providerId}.`;
  return String(content || "")
    .split(/\r?\n/)
    .filter((line) => !line.trim().startsWith(prefix))
    .join("\n")
    .replace(/\n*$/, "\n");
}

function upsertConfigValue(content, key, value) {
  const prefix = `${key}=`;
  const lines = String(content || "").split(/\r?\n/);
  let replaced = false;
  const next = lines.map((line) => {
    if (line.trim().startsWith(prefix)) {
      replaced = true;
      return `${key}=${value}`;
    }
    return line;
  });
  if (!replaced) {
    if (next.length && next[next.length - 1].trim()) next.push("");
    next.push(`${key}=${value}`);
  }
  return next.join("\n").replace(/\n*$/, "\n");
}

function modelKeyAccountFromReference(reference) {
  return reference.startsWith("keychain:") ? reference.slice("keychain:".length).trim() : "";
}

function syncModelKeyAccountFromConfig(content) {
  const account = modelKeyAccountFromReference(configValue(content, "model_api_key_env"));
  if (account) {
    $("model-key-account").value = account;
  } else if (!$("model-key-account").value.trim()) {
    $("model-key-account").value = "model-api-key";
  }
}

function setModelKeyStatus(message, state = "") {
  const el = $("model-key-status");
  el.textContent = message;
  el.dataset.state = state;
}

async function refreshModelKeyStatus() {
  const provider = encodeURIComponent(selectedChatModelOptions().provider);
  const payload = await api(
    `/api/model-key-status?repo=${encodeURIComponent(repo())}&model_provider=${provider}`,
  );
  if (payload.kind === "keychain") {
    if (payload.account) $("model-key-account").value = payload.account;
    setModelKeyStatus(payload.configured ? "Saved" : "Missing", payload.configured ? "ok" : "warn");
  } else {
    setModelKeyStatus(
      payload.configured ? `${payload.reference} set` : `${payload.reference} missing`,
      payload.configured ? "ok" : "warn",
    );
  }
  return payload;
}

function keyOverrideWarning(savedReference, effectiveReference) {
  if (!effectiveReference || effectiveReference === savedReference) return "";
  return `Saved user key, but effective config still uses ${effectiveReference}. Remove or update the model_api_key_env override in repository or admin config.`;
}

async function loadConfigFile() {
  const payload = await api(
    `/api/config-file?scope=${encodeURIComponent(configScope())}&repo=${encodeURIComponent(
      configRepo(),
    )}`,
  );
  $("config-editor").value = payload.content;
  syncConfiguredProvidersFromConfig(payload.content);
  syncModelKeyAccountFromConfig(payload.content);
  renderConfigPolicy(payload);
  $("config-path").textContent = payload.exists ? payload.path : `${payload.path} (new)`;
  void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
  return payload;
}

async function saveConfigFile() {
  const content = $("config-editor").value;
  syncConfiguredProvidersFromConfig(content);
  const payload = await api(
    "/api/config-file",
    form({
      scope: configScope(),
      repo: configRepo(),
      content,
    }),
  );
  renderConfigPolicy(payload);
  $("config-path").textContent = payload.path;
  syncModelKeyAccountFromConfig($("config-editor").value);
  void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
  return payload;
}

async function saveModelApiKey() {
  const account = $("model-key-account").value.trim();
  const apiKey = $("model-api-key").value.trim();
  if (!account) throw new Error("Keychain account is required");
  if (!apiKey) throw new Error("API key is required");
  const payload = await api(
    "/api/model-key",
    form({
      scope: configScope(),
      repo: configRepo(),
      account,
      api_key: apiKey,
    }),
  );
  $("model-api-key").value = "";
  renderConfigPolicy(payload);
  $("config-path").textContent = payload.path;
  $("config-editor").value = upsertConfigValue(
    $("config-editor").value,
    "model_api_key_env",
    payload.reference,
  );
  syncModelKeyAccountFromConfig($("config-editor").value);
  const status = await refreshModelKeyStatus();
  const warning = keyOverrideWarning(payload.reference, status.reference);
  if (warning) {
    setModelKeyStatus("Overridden", "warn");
    payload.warning = warning;
  }
  return payload;
}

async function deleteModelApiKey() {
  const account = $("model-key-account").value.trim();
  if (!account) throw new Error("Keychain account is required");
  const payload = await api("/api/model-key-delete", form({ account }));
  $("model-api-key").value = "";
  await refreshModelKeyStatus();
  return payload;
}

function clearSessionList() {
  $("session-select").innerHTML = '<option value="">New session</option>';
  projectSessionsByPath.set(repo(), []);
  renderProjectList();
  currentSessionId = "";
}

function clearChat() {
  $("chat-log").innerHTML = "";
  $("chat-context").innerHTML = "";
  setChatStatus("Idle");
}

function setChatStatus(message, state = "") {
  const el = $("chat-status");
  el.textContent = message;
  el.dataset.state = state;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function renderInlineMarkdown(value) {
  return escapeHtml(value).replace(/`([^`]+)`/g, "<code>$1</code>");
}

function parseTableRow(line) {
  let content = line.trim();
  if (!content.includes("|")) return null;
  if (content.startsWith("|")) content = content.slice(1);
  if (content.endsWith("|")) content = content.slice(0, -1);
  const cells = content.split("|").map((cell) => cell.trim());
  return cells.length >= 2 ? cells : null;
}

function isTableSeparator(cells) {
  return cells?.every((cell) => /^:?-{3,}:?$/.test(cell.replace(/\s+/g, "")));
}

function normalizeTableCells(cells, length) {
  return Array.from({ length }, (_, index) => cells[index] || "");
}

function renderTable(headers, rows) {
  const headerHtml = normalizeTableCells(headers, headers.length)
    .map((cell) => `<th>${renderInlineMarkdown(cell)}</th>`)
    .join("");
  const rowsHtml = rows
    .map((row) => {
      const cells = normalizeTableCells(row, headers.length)
        .map((cell) => `<td>${renderInlineMarkdown(cell)}</td>`)
        .join("");
      return `<tr>${cells}</tr>`;
    })
    .join("");
  return `<div class="table-wrap"><table><thead><tr>${headerHtml}</tr></thead><tbody>${rowsHtml}</tbody></table></div>`;
}

function renderMarkdown(markdown) {
  const lines = String(markdown || "").split(/\r?\n/);
  let html = "";
  let paragraph = [];
  let listOpen = false;
  let codeOpen = false;
  let codeLines = [];
  let codeLanguage = "";

  const closeParagraph = () => {
    if (!paragraph.length) return;
    html += `<p>${paragraph.map(renderInlineMarkdown).join("<br>")}</p>`;
    paragraph = [];
  };
  const closeList = () => {
    if (!listOpen) return;
    html += "</ul>";
    listOpen = false;
  };
  const closeCode = () => {
    const languageClass = codeLanguage ? ` class="language-${escapeHtml(codeLanguage)}"` : "";
    html += `<pre><code${languageClass}>${escapeHtml(codeLines.join("\n"))}</code></pre>`;
    codeLines = [];
    codeLanguage = "";
    codeOpen = false;
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.startsWith("```")) {
      if (codeOpen) {
        closeCode();
      } else {
        closeParagraph();
        closeList();
        codeOpen = true;
        codeLanguage = line.slice(3).trim().replace(/[^a-z0-9_-]/gi, "");
      }
      continue;
    }
    if (codeOpen) {
      codeLines.push(line);
      continue;
    }
    const trimmed = line.trim();
    if (!trimmed) {
      closeParagraph();
      closeList();
      continue;
    }
    const tableHeaders = parseTableRow(line);
    const tableSeparator = parseTableRow(lines[index + 1] || "");
    if (
      tableHeaders &&
      tableSeparator &&
      tableHeaders.length === tableSeparator.length &&
      isTableSeparator(tableSeparator)
    ) {
      closeParagraph();
      closeList();
      const rows = [];
      index += 2;
      while (index < lines.length) {
        const row = parseTableRow(lines[index]);
        if (!row) break;
        rows.push(row);
        index += 1;
      }
      index -= 1;
      html += renderTable(tableHeaders, rows);
      continue;
    }
    const heading = /^(#{1,4})\s+(.+)$/.exec(trimmed);
    if (heading) {
      closeParagraph();
      closeList();
      const level = Math.min(heading[1].length + 2, 5);
      html += `<h${level}>${renderInlineMarkdown(heading[2])}</h${level}>`;
      continue;
    }
    const bullet = /^[-*]\s+(.+)$/.exec(trimmed);
    if (bullet) {
      closeParagraph();
      if (!listOpen) {
        html += "<ul>";
        listOpen = true;
      }
      html += `<li>${renderInlineMarkdown(bullet[1])}</li>`;
      continue;
    }
    paragraph.push(line);
  }

  if (codeOpen) closeCode();
  closeParagraph();
  closeList();
  return html;
}

function appendChatMessage(role, content) {
  const message = document.createElement("article");
  message.className = `message ${role}`;

  const label = document.createElement("div");
  label.className = "message-role";
  label.textContent = role === "assistant" ? "Assistant" : role === "user" ? "You" : "System";

  const body = document.createElement("div");
  body.className = "message-body";
  body.innerHTML = renderMarkdown(content);

  message.append(label, body);
  $("chat-log").append(message);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
  return { message, body };
}

function updateChatMessage(target, content) {
  target.body.innerHTML = renderMarkdown(content);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

function renderMessages(messages) {
  $("chat-log").innerHTML = "";
  messages.forEach((message) => appendChatMessage(message.role, message.content));
}

function renderContextFiles(files = []) {
  const container = $("chat-context");
  container.innerHTML = "";
  files.forEach((path) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "context-file";
    button.textContent = path;
    button.title = "Open in Visual Studio Code";
    button.addEventListener("click", async () => {
      try {
        const payload = await api("/api/open-vscode-file", form({ repo: requireRepo(), path }));
        toast(`Opened ${payload.path}`);
      } catch (error) {
        toast(error.message);
      }
    });
    container.append(button);
  });
}

function renderProjectList() {
  const list = $("project-list");
  list.innerHTML = "";
  if (projectsCollapsed) {
    return;
  }
  if (!projectPaths.length) {
    const empty = document.createElement("p");
    empty.className = "sidebar-empty";
    empty.textContent = "Use + to add a working folder";
    list.append(empty);
    return;
  }

  projectPaths.forEach((projectPath) => {
    const expanded = expandedProjectPaths.has(projectPath);
    const activeProject = projectPath === repo();
    const group = document.createElement("section");
    group.className = "project-group";
    group.classList.toggle("active", activeProject);
    group.classList.toggle("expanded", expanded);

    const row = document.createElement("div");
    row.className = "project-row";
    row.title = projectPath;
    row.dataset.projectPath = projectPath;
    const projectButton = document.createElement("button");
    projectButton.type = "button";
    projectButton.className = "project-select-btn";
    projectButton.innerHTML = `
      <span class="folder-icon" aria-hidden="true"></span>
      <span class="project-name"></span>
      <span class="project-chevron" aria-hidden="true"></span>
    `;
    projectButton.querySelector(".project-name").textContent = projectName(projectPath);
    projectButton.addEventListener("click", async () => {
      try {
        if (activeProject && expanded) {
          expandedProjectPaths.delete(projectPath);
          saveProjectState();
          renderProjectList();
          return;
        }
        expandedProjectPaths.add(projectPath);
        saveProjectState();
        await switchProject(projectPath);
      } catch (error) {
        toast(error.message);
      }
    });
    const addSessionButton = document.createElement("button");
    addSessionButton.type = "button";
    addSessionButton.className = "project-session-add-btn";
    addSessionButton.textContent = "+";
    addSessionButton.title = `New session in ${projectName(projectPath)}`;
    addSessionButton.setAttribute("aria-label", `New session in ${projectName(projectPath)}`);
    addSessionButton.addEventListener("click", async () => {
      try {
        await startNewSession(projectPath);
      } catch (error) {
        toast(error.message);
      }
    });
    row.append(projectButton, addSessionButton);
    group.append(row);

    if (expanded) {
      const sessions = projectSessionsByPath.get(projectPath);
      const sessionsList = document.createElement("div");
      sessionsList.className = "project-sessions";
      if (!sessions && projectSessionsLoading.has(projectPath)) {
        const loading = document.createElement("p");
        loading.className = "project-session-empty";
        loading.textContent = "Loading sessions...";
        sessionsList.append(loading);
      } else if (!sessions) {
        const loading = document.createElement("p");
        loading.className = "project-session-empty";
        loading.textContent = "Loading sessions...";
        sessionsList.append(loading);
        void loadProjectSessions(projectPath)
          .then(renderProjectList)
          .catch((error) => {
            projectSessionsByPath.set(projectPath, []);
            toast(error.message);
            renderProjectList();
          });
      } else if (!sessions.length) {
        const empty = document.createElement("p");
        empty.className = "project-session-empty";
        empty.textContent = "No sessions yet";
        sessionsList.append(empty);
      } else {
        sessions.forEach((session) => sessionsList.append(renderProjectSession(projectPath, session)));
      }
      group.append(sessionsList);
    }

    list.append(group);
  });
}

function renderProjectSession(projectPath, session) {
  const row = document.createElement("div");
  row.className = "session-item project-session-item";
  row.dataset.sessionId = session.id;
  row.dataset.projectPath = projectPath;
  if (projectPath === repo() && session.id === currentSessionId) {
    row.classList.add("active");
  }
  const button = document.createElement("button");
  button.type = "button";
  button.className = "project-session-open";
  button.textContent = session.title;
  button.title = `${session.title} - double-click to rename`;
  button.addEventListener("click", async () => {
    try {
      expandedProjectPaths.add(projectPath);
      saveProjectState();
      await switchProject(projectPath, { preferredSessionId: session.id, reloadSelected: false });
      await loadSession(session.id);
      renderProjectList();
    } catch (error) {
      toast(error.message);
    }
  });
  button.addEventListener("dblclick", async (event) => {
    event.preventDefault();
    event.stopPropagation();
    try {
      await renameSessionForProject(projectPath, session);
    } catch (error) {
      toast(error.message);
    }
  });

  const deleteButton = document.createElement("button");
  deleteButton.type = "button";
  deleteButton.className = "project-session-delete";
  deleteButton.textContent = "-";
  deleteButton.title = `Delete ${session.title}`;
  deleteButton.setAttribute("aria-label", `Delete ${session.title}`);
  deleteButton.addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      await deleteSessionForProject(projectPath, session);
    } catch (error) {
      toast(error.message);
    }
  });

  row.append(button, deleteButton);
  return row;
}

function renderSessionOptions(sessions = []) {
  const select = $("session-select");
  select.innerHTML = '<option value="">New session</option>';
  sessions.forEach((session) => {
    const option = document.createElement("option");
    option.value = session.id;
    option.textContent = session.title;
    select.append(option);
  });
}

function renderSessionList() {
  renderProjectList();
}

async function loadProjectSessions(projectPath) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized || projectSessionsLoading.has(normalized)) return;
  projectSessionsLoading.add(normalized);
  try {
    const payload = await api(`/api/sessions?repo=${encodeURIComponent(normalized)}`);
    projectSessionsByPath.set(normalized, payload.sessions || []);
    if (normalized === repo()) {
      renderSessionOptions(payload.sessions || []);
    }
  } finally {
    projectSessionsLoading.delete(normalized);
  }
}

async function startNewSession(projectPath = repo()) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized) throw new Error("Repository is required");
  expandedProjectPaths.add(normalized);
  saveProjectState();
  localStorage.removeItem(lastSessionStorageKey(normalized));
  await switchProject(normalized, { preferredSessionId: "__new__", reloadSelected: false });
  currentSessionId = "";
  $("session-select").value = "";
  loadPinnedContextFiles("");
  clearChat();
  renderProjectList();
  $("chat-prompt").focus();
}

async function renameSessionForProject(projectPath, session) {
  const title = window.prompt("Session name", session.title);
  if (!title || !title.trim()) return;
  const payload = await api(
    "/api/session-rename",
    form({ session_id: session.id, title: title.trim() }),
  );
  if (normalizeProjectPath(projectPath) === repo()) {
    await loadSessions(payload.session.id, false);
  } else {
    await loadProjectSessions(projectPath);
    renderProjectList();
  }
  toast("Session renamed");
}

async function deleteSessionForProject(projectPath, session) {
  if (!window.confirm("Delete this session?")) return;
  await api("/api/session-delete", form({ session_id: session.id }));
  const normalized = normalizeProjectPath(projectPath);
  if (normalized === repo() && currentSessionId === session.id) {
    localStorage.removeItem(lastSessionStorageKey(normalized));
    currentSessionId = "";
    $("session-select").value = "";
    loadPinnedContextFiles("");
    clearChat();
    await loadSessions("__new__", false);
  } else if (normalized === repo()) {
    await loadSessions(currentSessionId || "", false);
  } else {
    await loadProjectSessions(normalized);
    renderProjectList();
  }
  toast("Session deleted");
}

function syncSessionListActive() {
  document.querySelectorAll(".session-item").forEach((button) => {
    button.classList.toggle(
      "active",
      button.dataset.projectPath === repo() && button.dataset.sessionId === currentSessionId,
    );
  });
}

function diffStats(diff) {
  return String(diff || "")
    .split(/\r?\n/)
    .reduce(
      (stats, line) => {
        if (line.startsWith("+") && !line.startsWith("+++")) stats.additions += 1;
        if (line.startsWith("-") && !line.startsWith("---")) stats.deletions += 1;
        return stats;
      },
      { additions: 0, deletions: 0 },
    );
}

function diffLineClass(line) {
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("diff --git") || line.startsWith("index ")) return "file";
  if (line.startsWith("+++") || line.startsWith("---")) return "file";
  if (line.startsWith("+")) return "addition";
  if (line.startsWith("-")) return "deletion";
  return "context";
}

function renderColoredDiff(diff) {
  const view = document.createElement("div");
  view.className = "diff-view";
  const lines = String(diff || "").replace(/\n$/, "").split(/\r?\n/);
  if (lines.length === 1 && !lines[0]) {
    const empty = document.createElement("div");
    empty.className = "diff-line context";
    empty.textContent = "No textual diff.";
    view.append(empty);
    return view;
  }
  lines.forEach((line) => {
    const row = document.createElement("div");
    row.className = `diff-line ${diffLineClass(line)}`;
    row.textContent = line || " ";
    view.append(row);
  });
  return view;
}

function hunkLineClass(tag) {
  if (tag === "insert") return "addition";
  if (tag === "delete") return "deletion";
  return "context";
}

function renderHunks(file, onToggle) {
  const view = document.createElement("div");
  view.className = "diff-view";
  if (!file.hunks.length) {
    return renderColoredDiff(file.diff);
  }

  file.hunks.forEach((hunk) => {
    const group = document.createElement("div");
    group.className = "diff-hunk";

    const hunkHeader = document.createElement("label");
    hunkHeader.className = "diff-hunk-header diff-line hunk";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = hunk.selected;
    checkbox.disabled = file.state !== "pending";
    checkbox.addEventListener("change", () => {
      hunk.selected = checkbox.checked;
      onToggle?.();
    });
    const label = document.createElement("span");
    label.textContent = `@@ -${hunk.oldStart + 1},${hunk.oldLines} +${hunk.newStart + 1},${hunk.newLines} @@`;
    hunkHeader.append(checkbox, label);
    group.append(hunkHeader);

    hunk.lines.forEach((line) => {
      const row = document.createElement("div");
      row.className = `diff-line ${hunkLineClass(line.tag)}`;
      const prefix = line.tag === "insert" ? "+" : line.tag === "delete" ? "-" : " ";
      row.textContent = `${prefix}${line.text}` || " ";
      group.append(row);
    });

    view.append(group);
  });

  return view;
}

function renderGitStatusText(payload) {
  if (payload.clean) {
    return "Git status: clean workspace.";
  }
  const files = payload.files || [];
  const visible = files
    .slice(0, 12)
    .map((file) => `- ${file.raw || "changed"} ${file.path || file}`)
    .join("\n");
  const hiddenCount = Math.max(0, files.length - 12);
  const suffix = hiddenCount ? `\n- ... ${hiddenCount} more` : "";
  return `Git status: ${files.length} changed path(s).\n${visible}${suffix}`;
}

async function appendGitStatusAfterChange(repoPath) {
  const payload = await api(`/api/git-status?repo=${encodeURIComponent(repoPath)}`);
  setRepoState(payload.clean ? projectName(repoPath) : `${projectName(repoPath)} - ${payload.files.length} changed`);
  appendChatMessage("system", renderGitStatusText(payload));
}

function createPatchPreview(payload, patchRepo) {
  const state = {
    patchId: payload.patchId,
    files: (payload.files || []).map((file) => {
      const stats = diffStats(file.diff);
      return {
        path: file.path,
        status: file.status,
        diff: file.diff,
        hunks: (file.hunks || []).map((hunk) => ({ ...hunk, selected: true })),
        additions: stats.additions,
        deletions: stats.deletions,
        selected: true,
        state: "pending",
      };
    }),
  };

  const wrapper = document.createElement("div");
  wrapper.className = "patch-preview";

  const header = document.createElement("div");
  header.className = "patch-preview-header";
  const title = document.createElement("strong");
  title.textContent = payload.summary || "Patch preview";
  const meta = document.createElement("span");
  meta.textContent = state.patchId;
  header.append(title, meta);

  const actions = document.createElement("div");
  actions.className = "inline-actions patch-actions";
  const applyButton = document.createElement("button");
  applyButton.type = "button";
  applyButton.textContent = "Apply Selected";
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.textContent = "Reject Selected";
  actions.append(applyButton, rejectButton);

  const list = document.createElement("div");
  list.className = "diff-list";
  wrapper.append(header, actions, list);

  function selectedPendingPaths() {
    return state.files
      .filter((file) => file.state === "pending" && file.selected)
      .map((file) => file.path);
  }

  function markFiles(paths, nextState) {
    state.files.forEach((file) => {
      if (paths.includes(file.path)) {
        file.state = nextState;
        file.selected = false;
      }
    });
    render();
  }

  function render() {
    list.innerHTML = "";
    if (!state.files.length) {
      const empty = document.createElement("p");
      empty.className = "empty-state";
      empty.textContent = "No patch files returned.";
      list.append(empty);
    }

    state.files.forEach((file) => {
      const card = document.createElement("article");
      card.className = `diff-card ${file.state}`;

      const cardHeader = document.createElement("div");
      cardHeader.className = "diff-card-header";

      const label = document.createElement("label");
      label.className = "diff-file-select";
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = file.selected;
      checkbox.disabled = file.state !== "pending";
      checkbox.addEventListener("change", () => {
        file.selected = checkbox.checked;
      });
      const name = document.createElement("span");
      name.textContent = file.path;
      label.append(checkbox, name);

      const fileState = document.createElement("span");
      fileState.className = "diff-state";
      fileState.textContent = file.state === "pending" ? file.status : file.state;

      const stats = document.createElement("span");
      stats.className = "diff-stats";
      stats.textContent = `+${file.additions} -${file.deletions}`;

      const meta = document.createElement("div");
      meta.className = "diff-meta";
      meta.append(stats, fileState);

      if (file.state === "applied") {
        const rollbackButton = document.createElement("button");
        rollbackButton.type = "button";
        rollbackButton.className = "diff-rollback";
        rollbackButton.textContent = "Rollback";
        rollbackButton.addEventListener("click", async () => {
          try {
            rollbackButton.disabled = true;
            const result = await api(
              "/api/rollback-patch",
              form({ repo: patchRepo, patch_id: state.patchId, paths: file.path }),
            );
            (result.warnings || []).forEach((warning) => toast(warning));
            if (result.restoredFiles?.includes(file.path)) {
              file.state = "rolled_back";
              toast(`Restored ${file.path}`);
            } else if (result.deletedFiles?.includes(file.path)) {
              file.state = "rolled_back";
              toast(`Deleted ${file.path}`);
            } else {
              toast(`Nothing to roll back for ${file.path}`);
            }
            render();
            await appendGitStatusAfterChange(patchRepo).catch((error) => {
              toast(`Status unavailable: ${error.message}`);
            });
          } catch (error) {
            rollbackButton.disabled = false;
            toast(error.message);
          }
        });
        meta.append(rollbackButton);
      }

      cardHeader.append(label, meta);
      card.append(cardHeader, renderHunks(file));
      list.append(card);
    });

    const hasPending = state.files.some((file) => file.state === "pending");
    applyButton.disabled = !hasPending;
    rejectButton.disabled = !hasPending;
    $("chat-log").scrollTop = $("chat-log").scrollHeight;
  }

  applyButton.addEventListener("click", async () => {
    try {
      const paths = selectedPendingPaths();
      if (!paths.length) throw new Error("No pending patch files selected");
      applyButton.disabled = true;
      const hunkSelection = {};
      state.files.forEach((file) => {
        if (paths.includes(file.path) && file.hunks.length) {
          hunkSelection[file.path] = file.hunks.filter((hunk) => hunk.selected).map((hunk) => hunk.id);
        }
      });
      const result = await api(
        "/api/apply-patch",
        form({
          repo: patchRepo,
          patch_id: state.patchId,
          paths: paths.join("\n"),
          hunk_selection: JSON.stringify(hunkSelection),
        }),
      );
      const applied = result.appliedFiles || [];
      markFiles(applied, "applied");
      toast(`Applied ${applied.length} file(s)`);
      if (applied.length) {
        await appendGitStatusAfterChange(patchRepo).catch((error) => {
          toast(`Status unavailable: ${error.message}`);
        });
      }
    } catch (error) {
      toast(error.message);
      render();
    }
  });

  rejectButton.addEventListener("click", async () => {
    try {
      const paths = selectedPendingPaths();
      if (!paths.length) throw new Error("No pending patch files selected");
      rejectButton.disabled = true;
      const result = await api(
        "/api/reject-patch-files",
        form({ repo: patchRepo, patch_id: state.patchId, paths: paths.join("\n") }),
      );
      const rejected = result.rejectedFiles || [];
      markFiles(rejected, "rejected");
      toast(`Rejected ${rejected.length} file(s)`);
    } catch (error) {
      toast(error.message);
      render();
    }
  });

  render();
  return wrapper;
}

function createCommandApprovalPreview(proposal, proposalRepo) {
  const wrapper = document.createElement("div");
  wrapper.className = "command-approval";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = "Command approval";
  const meta = document.createElement("span");
  meta.textContent = proposal.blocked ? "blocked" : proposal.risk || "review";
  header.append(title, meta);

  const command = document.createElement("code");
  command.className = "command-approval-command";
  command.textContent = proposal.command || "";

  const details = document.createElement("pre");
  details.className = "command-approval-details";
  details.textContent = proposal.prompt || "";

  const actions = document.createElement("div");
  actions.className = "inline-actions command-approval-actions";
  const runButton = document.createElement("button");
  runButton.type = "button";
  runButton.textContent = proposal.blocked ? "Blocked" : "Approve Run";
  runButton.disabled = Boolean(proposal.blocked);
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.textContent = "Reject";
  actions.append(runButton, rejectButton);

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  runButton.addEventListener("click", async () => {
    try {
      runButton.disabled = true;
      rejectButton.disabled = true;
      const result = await api(
        "/api/run-command",
        form({ repo: proposalRepo, proposal_id: proposal.proposalId }),
      );
      output.hidden = false;
      output.textContent = [
        `exit ${result.exitCode}`,
        result.stdout ? `\nstdout:\n${result.stdout}` : "",
        result.stderr ? `\nstderr:\n${result.stderr}` : "",
      ].join("");
      appendChatMessage("system", `Approved command completed: \`${proposal.command}\``);
      toast("Command completed");
    } catch (error) {
      runButton.disabled = false;
      rejectButton.disabled = false;
      toast(error.message);
    }
  });

  rejectButton.addEventListener("click", async () => {
    try {
      runButton.disabled = true;
      rejectButton.disabled = true;
      const result = await api(
        "/api/reject-command",
        form({ repo: proposalRepo, proposal_id: proposal.proposalId }),
      );
      output.hidden = false;
      output.textContent = `Rejected ${result.proposalId}`;
      toast("Command rejected");
    } catch (error) {
      runButton.disabled = Boolean(proposal.blocked);
      rejectButton.disabled = false;
      toast(error.message);
    }
  });

  wrapper.append(header, command, details, actions, output);
  return wrapper;
}

async function loadSessions(preferredSessionId = "", reloadSelected = false) {
  const repoPath = repo();
  if (!repoPath) {
    clearSessionList();
    return;
  }
  const payload = await api(`/api/sessions?repo=${encodeURIComponent(repoPath)}`);
  const sessions = payload.sessions || [];
  projectSessionsByPath.set(repoPath, sessions);
  const storedSessionId = localStorage.getItem(lastSessionStorageKey(repoPath)) || "";
  const selectedSessionId = preferredSessionId || currentSessionId || storedSessionId;
  renderSessionOptions(sessions);
  currentSessionId = sessions.some((session) => session.id === selectedSessionId)
    ? selectedSessionId
    : "";
  $("session-select").value = currentSessionId;
  renderProjectList();
  if (currentSessionId) {
    localStorage.setItem(lastSessionStorageKey(repoPath), currentSessionId);
    if (reloadSelected) {
      await loadSession(currentSessionId);
    }
  } else if (reloadSelected) {
    clearChat();
  }
}

async function loadSession(sessionId) {
  if (!sessionId) {
    currentSessionId = "";
    localStorage.removeItem(lastSessionStorageKey());
    loadPinnedContextFiles("");
    clearChat();
    return;
  }
  const payload = await api(`/api/session?session_id=${encodeURIComponent(sessionId)}`);
  currentSessionId = payload.session.id;
  localStorage.setItem(lastSessionStorageKey(), currentSessionId);
  $("session-select").value = currentSessionId;
  syncSessionListActive();
  loadPinnedContextFiles(currentSessionId);
  renderMessages(payload.messages);
  renderContextFiles();
  setChatStatus("Loaded");
}

async function streamChatRequest(data, handlers) {
  const response = await fetch(
    apiUrl("/api/ask-stream"),
    withApiToken("/api/ask-stream", form(data)),
  );
  if (!response.ok) {
    throw new Error(await response.text());
  }
  if (!response.body) {
    const payload = await api("/api/ask", form(data));
    handlers.done(payload);
    return;
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let separator = buffer.indexOf("\n\n");
    while (separator >= 0) {
      processSseEvent(buffer.slice(0, separator), handlers);
      buffer = buffer.slice(separator + 2);
      separator = buffer.indexOf("\n\n");
    }
  }
  buffer += decoder.decode();
  if (buffer.trim()) {
    processSseEvent(buffer, handlers);
  }
}

function processSseEvent(raw, handlers) {
  let event = "message";
  const data = [];
  raw.split(/\r?\n/).forEach((line) => {
    if (line.startsWith("event:")) {
      event = line.slice("event:".length).trim();
    } else if (line.startsWith("data:")) {
      data.push(line.slice("data:".length).trimStart());
    }
  });
  const payload = data.length ? JSON.parse(data.join("\n")) : {};
  if (event === "token") handlers.token(payload.token || "");
  if (event === "done") handlers.done(payload);
  if (event === "error") handlers.error(payload);
}

document.querySelectorAll(".settings-nav-item").forEach((button) => {
  button.addEventListener("click", () => setSettingsPage(button.dataset.settingsPage));
});

$("settings-close-btn").addEventListener("click", closeSettings);

window.addEventListener("damaian-open-settings", () => openSettings("providers"));
window.addEventListener("damaian-check-for-updates", () => {
  void installAppUpdate();
});

$("repo").addEventListener("change", () => {
  const value = repo();
  if (value) {
    setRepository(value);
  }
});

$("projects-toggle-btn").addEventListener("click", () => {
  setProjectsCollapsed(!projectsCollapsed);
});

$("pick-folder-btn").addEventListener("click", async () => {
  try {
    const open = tauriDialogOpen();
    if (!open) throw new Error("Folder picker is available in the desktop app");
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select Working Folder",
    });
    if (selected) {
      setRepository(selected);
      toast("Working folder selected");
    }
  } catch (error) {
    toast(error.message);
  }
});

$("add-context-file-btn").addEventListener("click", async () => {
  try {
    await addContextFilesFromPicker();
  } catch (error) {
    toast(error.message);
  }
});

$("clear-context-files-btn").addEventListener("click", () => {
  clearPinnedContextFiles();
});

$("open-vscode-btn").addEventListener("click", async () => {
  try {
    const payload = await api("/api/open-vscode", form({ repo: requireRepo() }));
    toast(`Opened ${payload.path}`);
  } catch (error) {
    toast(error.message);
  }
});

$("terminal-toggle-btn").addEventListener("click", () => {
  setTerminalOpen(!terminalOpen);
});

$("terminal-close-btn").addEventListener("click", () => {
  setTerminalOpen(false);
});

$("terminal-clear-btn").addEventListener("click", () => {
  $("terminal-output").innerHTML = "";
  $("terminal-input").focus();
});

$("terminal-new-btn").addEventListener("click", async () => {
  try {
    $("terminal-output").innerHTML = "";
    terminalCwd = "";
    await ensureTerminalReady();
    $("terminal-input").focus();
  } catch (error) {
    appendTerminalLine(error.message, "stderr");
    toast(error.message);
  }
});

$("terminal-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const command = $("terminal-input").value;
  $("terminal-input").value = "";
  void runTerminalCommand(command);
});

$("session-select").addEventListener("change", async () => {
  try {
    await loadSession($("session-select").value);
  } catch (error) {
    toast(error.message);
  }
});

function looksLikeEditRequest(prompt) {
  const text = prompt.trim().toLowerCase();
  if (!text) return false;
  if (/^(how\s+(do|can|would|should)\s+i|how\s+to|what\s+is|why\b|where\b)/.test(text)) {
    return false;
  }
  const editVerb =
    /\b(add|create|write|generate|implement|modify|update|change|fix|refactor|remove|delete|make)\b/;
  const codeTarget =
    /\b(file|test|code|function|component|class|module|endpoint|api|route|ui|layout|style|css|html|javascript|typescript|rust|readme|doc|docs|config|script|bug|issue|error)\b/;
  return editVerb.test(text) && codeTarget.test(text);
}

async function proposePatchFromChat(prompt, assistantMessage) {
  updateChatMessage(assistantMessage, "Generating a patch preview...");
  setChatStatus("Generating patch", "running");
  const patchRepo = requireRepo();
  const payload = await api(
    "/api/propose-edit",
    form({
      repo: patchRepo,
      prompt,
      context_files: pinnedContextFiles.join("\n"),
      ...chatModelFormFields(),
    }),
  );
  updateChatMessage(
    assistantMessage,
    `Prepared a patch preview for \`${payload.patchId}\`. Review the diff and apply selected files when ready.`,
  );
  assistantMessage.body.append(createPatchPreview(payload, patchRepo));
  renderContextFiles(payload.contextFiles || []);
  setChatStatus("Patch ready", "warn");
}

async function sendChatPrompt() {
  const button = $("ask-btn");
  let streamError = null;
  if (chatSubmitting) return;
  try {
    const prompt = $("chat-prompt").value.trim();
    if (!prompt) throw new Error("Prompt is required");
    chatSubmitting = true;
    button.disabled = true;
    await ensureDesktopApiReady();
    const chatRepo = requireRepo();
    appendChatMessage("user", prompt);
    const assistantMessage = appendChatMessage("assistant", "");
    if (looksLikeEditRequest(prompt)) {
      await proposePatchFromChat(prompt, assistantMessage);
      $("chat-prompt").value = "";
      return;
    }
    let assistantText = "";
    setChatStatus("Thinking", "running");

    await streamChatRequest(
      {
        repo: chatRepo,
        prompt,
        session_id: currentSessionId,
        context_files: pinnedContextFiles.join("\n"),
        ...chatModelFormFields(),
      },
      {
        token(token) {
          assistantText += token;
          updateChatMessage(assistantMessage, assistantText);
          setChatStatus("Streaming", "running");
        },
        done(payload) {
          currentSessionId = payload.sessionId;
          localStorage.setItem(lastSessionStorageKey(), currentSessionId);
          persistPinnedContextForSession(currentSessionId);
          if (payload.response && payload.response !== assistantText) {
            assistantText = payload.response;
            updateChatMessage(assistantMessage, assistantText);
          }
          if (payload.commandProposal) {
            assistantMessage.body.append(createCommandApprovalPreview(payload.commandProposal, chatRepo));
          }
          renderContextFiles(payload.contextFiles || []);
          setChatStatus(payload.incomplete ? "Incomplete" : "Complete", payload.incomplete ? "warn" : "ok");
        },
        error(payload) {
          streamError = new Error(payload.error || "Model request failed");
        },
      },
    );
    if (streamError) throw streamError;
    $("chat-prompt").value = "";
    await loadSessions(currentSessionId, false);
  } catch (error) {
    setChatStatus("Failed", "error");
    toast(error.message);
  } finally {
    chatSubmitting = false;
    button.disabled = false;
  }
}

$("ask-btn").addEventListener("click", sendChatPrompt);

$("chat-prompt").addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  void sendChatPrompt();
});

$("chat-model-menu-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleModelMenu();
});

$("chat-model-popover").addEventListener("click", (event) => {
  event.stopPropagation();
});

document.addEventListener("click", (event) => {
  if (!$("chat-model-menu").contains(event.target)) closeModelMenu();
});

document.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === ",") {
    event.preventDefault();
    openSettings("providers");
    return;
  }
  if (event.key === "Escape") {
    if (!$("settings-shell").hidden) {
      closeSettings();
      return;
    }
    closeModelMenu();
  }
});

document.querySelectorAll("[data-panel]").forEach((button) => {
  button.addEventListener("click", () => showModelMenuPanel(button.dataset.panel));
});

$("model-reset-btn").addEventListener("click", resetChatModelPrefs);
$("custom-model-apply-btn").addEventListener("click", applyCustomModel);
$("custom-model-input").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  applyCustomModel();
});

$("provider-config-select").addEventListener("change", () => {
  renderProviderConfigForm($("provider-config-select").value);
});

$("provider-new-btn").addEventListener("click", newProviderConfigForm);

$("provider-label-input").addEventListener("input", () => {
  if (!$("provider-id-input").disabled && !$("provider-id-input").value.trim()) {
    $("provider-key-ref-input").value = `keychain:${providerSlug($("provider-label-input").value)}-api-key`;
  }
});

$("provider-save-btn").addEventListener("click", async () => {
  try {
    await saveProviderConfig();
    toast("LLM provider saved");
  } catch (error) {
    toast(error.message);
  }
});

$("provider-remove-btn").addEventListener("click", async () => {
  try {
    const id = $("provider-id-input").dataset.originalId || $("provider-id-input").value;
    if (!id || !window.confirm(`Remove provider ${id}?`)) return;
    await removeProviderConfigFromSettings();
    toast("LLM provider removed");
  } catch (error) {
    toast(error.message);
  }
});

$("config-load-btn").addEventListener("click", async () => {
  try {
    const payload = await loadConfigFile();
    toast(payload.path);
  } catch (error) {
    toast(error.message);
  }
});

$("config-save-btn").addEventListener("click", async () => {
  try {
    const payload = await saveConfigFile();
    toast(payload.path);
  } catch (error) {
    toast(error.message);
  }
});

$("model-key-save-btn").addEventListener("click", async () => {
  try {
    const payload = await saveModelApiKey();
    toast(payload.warning || "Model API key saved");
  } catch (error) {
    setModelKeyStatus("Failed", "error");
    toast(error.message);
  }
});

$("model-key-delete-btn").addEventListener("click", async () => {
  try {
    if (!window.confirm("Remove this stored API key from Keychain?")) return;
    const payload = await deleteModelApiKey();
    toast(payload.deleted ? "Model API key removed" : "No stored key found");
  } catch (error) {
    setModelKeyStatus("Failed", "error");
    toast(error.message);
  }
});

$("update-app-btn").addEventListener("click", () => {
  void installAppUpdate();
});

$("ask-btn").disabled = true;
setChatStatus("Starting", "running");
syncProviderCatalogFromPolicy("");
applyChatModelOptions(modelOptionsFromPolicy(""));
renderProviderConfigSelect();
renderPinnedContextFiles();

bootstrapPromise = startBootstrap();
