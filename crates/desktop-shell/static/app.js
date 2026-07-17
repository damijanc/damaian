const $ = (id) => document.getElementById(id);

let currentSessionId = "";
let apiToken = "";
let bootstrapPromise = null;
let bootstrapError = null;
let chatSubmitting = false;
let pinnedContextFiles = [];
let contextChipsDismissed = false;
let terminalCwd = "";
let terminalOpen = false;
let terminalBusy = false;
let projectPaths = [];
let projectDisplayNames = new Map();
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
const projectDisplayNamesStorageKey = "damaian:projectDisplayNames";
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
  const customName = projectDisplayNames.get(normalized);
  if (customName) return customName;
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
  try {
    const storedNames = JSON.parse(localStorage.getItem(projectDisplayNamesStorageKey) || "{}");
    projectDisplayNames = new Map(
      Object.entries(storedNames || {})
        .map(([path, name]) => [normalizeProjectPath(path), String(name || "").trim()])
        .filter(([path, name]) => path && name && projectPaths.includes(path)),
    );
  } catch {
    projectDisplayNames = new Map();
  }
  projectsCollapsed = localStorage.getItem(projectsCollapsedStorageKey) === "true";
  setProjectsCollapsed(projectsCollapsed, false);
}

function saveProjectState() {
  localStorage.setItem(projectsStorageKey, JSON.stringify(projectPaths));
  localStorage.setItem(expandedProjectsStorageKey, JSON.stringify([...expandedProjectPaths]));
  localStorage.setItem(
    projectDisplayNamesStorageKey,
    JSON.stringify(Object.fromEntries(projectDisplayNames)),
  );
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

// `window.prompt`/`window.confirm` do not work in this app's Tauri WebView
// on macOS (a known WKWebView limitation — the JS call returns without ever
// showing a dialog), so text-input and yes/no confirmations use this small
// in-app modal instead. Single reusable element, promise-based like the
// project menu above.
let appDialogEl = null;

function ensureAppDialog() {
  if (appDialogEl) return appDialogEl;
  const backdrop = document.createElement("div");
  backdrop.className = "app-dialog-backdrop";
  backdrop.hidden = true;
  backdrop.innerHTML = `
    <div class="app-dialog" role="dialog" aria-modal="true">
      <p class="app-dialog-title"></p>
      <p class="app-dialog-message" hidden></p>
      <input type="text" class="app-dialog-input" hidden />
      <div class="app-dialog-actions">
        <button type="button" class="app-dialog-btn app-dialog-cancel">Cancel</button>
        <button type="button" class="app-dialog-btn app-dialog-confirm">OK</button>
      </div>
    </div>
  `;
  document.body.append(backdrop);
  appDialogEl = backdrop;
  return backdrop;
}

// Resolves to the entered string (or `null` if cancelled) when `inputValue`
// is given; otherwise behaves like `confirm` and resolves to a boolean.
function showAppDialog({ title, message = "", inputValue = null, confirmLabel = "OK", danger = false }) {
  return new Promise((resolve) => {
    const backdrop = ensureAppDialog();
    const titleEl = backdrop.querySelector(".app-dialog-title");
    const messageEl = backdrop.querySelector(".app-dialog-message");
    const inputEl = backdrop.querySelector(".app-dialog-input");
    const confirmBtn = backdrop.querySelector(".app-dialog-confirm");
    const cancelBtn = backdrop.querySelector(".app-dialog-cancel");
    const usesInput = inputValue !== null;

    titleEl.textContent = title;
    messageEl.hidden = !message;
    messageEl.textContent = message;
    inputEl.hidden = !usesInput;
    inputEl.value = usesInput ? inputValue : "";
    confirmBtn.textContent = confirmLabel;
    confirmBtn.classList.toggle("app-dialog-btn-danger", danger);

    const cleanup = (result) => {
      backdrop.hidden = true;
      confirmBtn.removeEventListener("click", onConfirm);
      cancelBtn.removeEventListener("click", onCancel);
      backdrop.removeEventListener("keydown", onKeydown);
      resolve(result);
    };
    const onConfirm = () => cleanup(usesInput ? inputEl.value : true);
    const onCancel = () => cleanup(usesInput ? null : false);
    const onKeydown = (event) => {
      if (event.key === "Escape") onCancel();
      if (event.key === "Enter" && usesInput) onConfirm();
    };

    confirmBtn.addEventListener("click", onConfirm);
    cancelBtn.addEventListener("click", onCancel);
    backdrop.addEventListener("keydown", onKeydown);
    backdrop.hidden = false;
    if (usesInput) {
      inputEl.focus();
      inputEl.select();
    } else {
      confirmBtn.focus();
    }
  });
}

function promptDialog(title, initialValue) {
  return showAppDialog({ title, inputValue: initialValue ?? "" });
}

function confirmDialog(title, message, { danger = false, confirmLabel = "Delete" } = {}) {
  return showAppDialog({ title, message, confirmLabel, danger });
}

// Renames a project within damaian only: it changes `projectName()`'s
// display label via `projectDisplayNames`, never the folder on disk.
async function renameProject(projectPath) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized) return;
  const current = projectName(normalized);
  const nextName = await promptDialog("Rename project", current);
  if (nextName === null) return;
  const trimmed = nextName.trim();
  if (!trimmed) {
    projectDisplayNames.delete(normalized);
  } else {
    projectDisplayNames.set(normalized, trimmed);
  }
  saveProjectState();
  renderProjectList();
}

// Removes a project from damaian's sidebar only: it never touches the
// folder or any files on disk, and the folder can always be re-added later
// by picking it again.
async function forgetProject(projectPath) {
  const normalized = normalizeProjectPath(projectPath);
  if (!normalized) return;
  const label = projectName(normalized);
  const confirmed = await confirmDialog(
    "Remove project?",
    `Remove "${label}" from damaian? This only removes it from the project list — nothing is deleted on disk.`,
    { danger: true, confirmLabel: "Delete" },
  );
  if (!confirmed) return;

  projectPaths = projectPaths.filter((path) => path !== normalized);
  expandedProjectPaths.delete(normalized);
  projectDisplayNames.delete(normalized);
  projectSessionsByPath.delete(normalized);
  saveProjectState();

  if (normalizeProjectPath(repo()) === normalized) {
    currentSessionId = "";
    localStorage.removeItem(lastSessionStorageKey());
    localStorage.removeItem(lastRepoStorageKey);
    $("repo").value = "";
    loadPinnedContextFiles("");
    clearChat();
    renderContextFiles();
  }
  renderProjectList();
}

// A single reusable popover shared by every project row (rows are fully
// re-rendered on every `renderProjectList()` call, so per-row popovers
// would never keep stable open/closed state). Anchored via `position:
// fixed` against the trigger button's own rect rather than a CSS-relative
// ancestor, since rows live inside a scrollable list.
let projectMenuEl = null;
let projectMenuTargetPath = null;

function ensureProjectMenu() {
  if (projectMenuEl) return projectMenuEl;
  const el = document.createElement("div");
  el.className = "context-menu-popover";
  el.setAttribute("role", "menu");
  el.hidden = true;
  el.innerHTML = `
    <div class="context-menu-panel" data-panel="root">
      <button type="button" class="context-menu-row" data-action="open-in">
        <span>Open in</span>
        <span class="context-menu-caret" aria-hidden="true"></span>
      </button>
      <button type="button" class="context-menu-row" data-action="rename">Rename</button>
      <button type="button" class="context-menu-row context-menu-row-danger" data-action="delete">Delete</button>
    </div>
    <div class="context-menu-panel" data-panel="open-in" hidden>
      <button type="button" class="context-menu-back" data-action="back">Open in</button>
      <button type="button" class="context-menu-row" data-action="open-vscode">VS Code</button>
      <button type="button" class="context-menu-row" data-action="open-finder">Finder</button>
    </div>
  `;
  el.addEventListener("click", (event) => {
    event.stopPropagation();
    handleProjectMenuAction(event);
  });
  document.body.append(el);
  projectMenuEl = el;
  return el;
}

function showProjectMenuPanel(panel) {
  const el = ensureProjectMenu();
  el.querySelectorAll(".context-menu-panel").forEach((panelEl) => {
    panelEl.hidden = panelEl.dataset.panel !== panel;
  });
}

function toggleProjectMenu(projectPath, anchorEl) {
  const el = ensureProjectMenu();
  const alreadyOpenForThisRow = !el.hidden && projectMenuTargetPath === projectPath;
  if (alreadyOpenForThisRow) {
    closeProjectMenu();
    return;
  }
  projectMenuTargetPath = projectPath;
  showProjectMenuPanel("root");
  el.hidden = false;
  positionProjectMenu(anchorEl);
}

function positionProjectMenu(anchorEl) {
  const el = ensureProjectMenu();
  const rect = anchorEl.getBoundingClientRect();
  const width = el.offsetWidth || 200;
  const left = Math.min(
    Math.max(8, rect.right - width),
    window.innerWidth - width - 8,
  );
  const top = Math.min(rect.bottom + 4, window.innerHeight - el.offsetHeight - 8);
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
}

function closeProjectMenu() {
  if (!projectMenuEl) return;
  projectMenuEl.hidden = true;
  projectMenuTargetPath = null;
}

async function handleProjectMenuAction(event) {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const action = button.dataset.action;
  const projectPath = projectMenuTargetPath;

  if (action === "open-in") {
    showProjectMenuPanel("open-in");
    return;
  }
  if (action === "back") {
    showProjectMenuPanel("root");
    return;
  }
  if (!projectPath) return;
  if (action === "rename") {
    closeProjectMenu();
    await renameProject(projectPath);
    return;
  }
  if (action === "delete") {
    closeProjectMenu();
    await forgetProject(projectPath);
    return;
  }
  if (action === "open-vscode") {
    closeProjectMenu();
    try {
      const payload = await api("/api/open-vscode", form({ repo: projectPath }));
      toast(`Opened ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
    return;
  }
  if (action === "open-finder") {
    closeProjectMenu();
    try {
      const payload = await api("/api/reveal-in-finder", form({ repo: projectPath }));
      toast(`Revealed ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
  }
}

document.addEventListener("click", () => closeProjectMenu());
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeProjectMenu();
});

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
  contextChipsDismissed = false;
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
  if (!normalized) return;
  contextChipsDismissed = false;
  if (!pinnedContextFiles.includes(normalized)) {
    pinnedContextFiles.push(normalized);
    savePinnedContextFiles();
  }
  renderPinnedContextFiles();
}

function removePinnedContextFile(path) {
  pinnedContextFiles = pinnedContextFiles.filter((item) => item !== path);
  savePinnedContextFiles();
  renderPinnedContextFiles();
}

function fileBaseName(path) {
  const parts = String(path || "").split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

function renderPinnedContextFiles() {
  const wrapper = $("composer-context");
  const container = $("pinned-context-files");
  container.innerHTML = "";
  const visible = !contextChipsDismissed && pinnedContextFiles.length > 0;
  wrapper.hidden = !visible;
  if (!visible) return;
  pinnedContextFiles.forEach((path) => {
    const chip = document.createElement("span");
    chip.className = "context-chip";
    chip.title = path;
    const icon = document.createElement("span");
    icon.className = "context-chip-icon";
    icon.setAttribute("aria-hidden", "true");
    const label = document.createElement("span");
    label.className = "context-chip-label";
    label.textContent = fileBaseName(path);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${path} from context`);
    remove.textContent = "×";
    remove.addEventListener("click", () => removePinnedContextFile(path));
    chip.append(icon, label, remove);
    container.append(chip);
  });
}

function dismissContextChips() {
  if (!pinnedContextFiles.length || contextChipsDismissed) return;
  contextChipsDismissed = true;
  renderPinnedContextFiles();
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

function tauriWebview() {
  return window.__TAURI__?.webview;
}

function isDesktopApp() {
  return Boolean(window.__TAURI__);
}

async function pinContextFilePaths(paths) {
  const selectedFiles = Array.isArray(paths) ? paths : paths ? [paths] : [];
  for (const path of selectedFiles) {
    const payload = await api("/api/context-file", form({ repo: requireRepo(), path }));
    addPinnedContextFile(payload.path);
  }
  return selectedFiles.length;
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
  const count = await pinContextFilePaths(selected);
  if (count) {
    toast(`Added ${count} context file(s)`);
  }
}

function setChatDropActive(active) {
  $("chat-drop-overlay").hidden = !active;
}

async function setupContextFileDragDrop() {
  const getCurrentWebview = tauriWebview()?.getCurrentWebview;
  if (!getCurrentWebview) return;
  const webview = getCurrentWebview();
  await webview.onDragDropEvent((event) => {
    const payload = event.payload || {};
    if (payload.type === "enter" || payload.type === "over") {
      setChatDropActive(true);
      return;
    }
    setChatDropActive(false);
    if (payload.type !== "drop") return;
    const paths = Array.isArray(payload.paths) ? payload.paths : [];
    if (!paths.length) return;
    void pinContextFilePaths(paths)
      .then((count) => {
        if (count) toast(`Added ${count} context file(s)`);
      })
      .catch((error) => toast(error.message));
  });
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
  const confirmed = await confirmDialog(
    "Install update?",
    `Install Damaian ${version}? Restart Damaian after the update to finish.`,
    { confirmLabel: "Install" },
  );
  if (!confirmed) {
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

function toggleAttachMenu() {
  if ($("composer-attach-popover").hidden) {
    openAttachMenu();
  } else {
    closeAttachMenu();
  }
}

function openAttachMenu() {
  $("composer-attach-popover").hidden = false;
  $("composer-attach-btn").setAttribute("aria-expanded", "true");
}

function closeAttachMenu() {
  $("composer-attach-popover").hidden = true;
  $("composer-attach-btn").setAttribute("aria-expanded", "false");
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
      if (!(await confirmDialog("Remove provider?", `Remove provider ${id}?`))) return;
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
  var PLACEHOLDER = "\uE000";
  var escaped = escapeHtml(value);
  var codeSpans = [];
  // Pull code spans out first so bold/italic/link patterns below never match
  // characters inside inline code (e.g. `a**b` should not become a<strong>).
  // PLACEHOLDER is a private-use-area character that cannot occur in
  // escaped HTML text, so it cannot collide with real content.
  var withoutCode = escaped.replace(/`([^`]+)`/g, function (_match, code) {
    codeSpans.push(code);
    return PLACEHOLDER + (codeSpans.length - 1) + PLACEHOLDER;
  });
  var withInline = withoutCode
    .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>')
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/\*([^*]+)\*/g, "<em>$1</em>");
  var codePattern = new RegExp(PLACEHOLDER + "(\\d+)" + PLACEHOLDER, "g");
  return withInline.replace(codePattern, function (_match, index) {
    return "<code>" + codeSpans[Number(index)] + "</code>";
  });
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

// Re-renders a finished message server-side (workspace-engine's
// `render_markdown_to_html`) to get real syntax-highlighted code blocks,
// which the fast client-side `renderMarkdown` above does not attempt. Only
// call this once a message's content is final (stream complete, or loaded
// from history) since it replaces the bubble's entire innerHTML.
async function finalizeChatMessage(target, content) {
  try {
    const payload = await api("/api/render-markdown", form({ content }));
    target.body.innerHTML = payload.html;
  } catch (error) {
    target.body.innerHTML = renderMarkdown(content);
  }
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

function renderMessages(messages) {
  $("chat-log").innerHTML = "";
  messages.forEach((message) => {
    const bubble = appendChatMessage(message.role, message.content);
    if (message.role === "assistant") {
      void finalizeChatMessage(bubble, message.content);
    }
  });
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
    const menuButton = document.createElement("button");
    menuButton.type = "button";
    menuButton.className = "project-menu-btn";
    menuButton.title = "More options";
    menuButton.setAttribute("aria-label", `More options for ${projectName(projectPath)}`);
    menuButton.setAttribute("aria-haspopup", "menu");
    menuButton.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
    menuButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleProjectMenu(projectPath, menuButton);
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
    row.append(projectButton, addSessionButton, menuButton);
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
  const title = await promptDialog("Session name", session.title);
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
  if (!(await confirmDialog("Delete this session?", ""))) return;
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

  // Approving or rejecting resumes the chat turn that raised this proposal:
  // the model sees the command's result (or the rejection) and streams back
  // an actual answer, same as a normal chat reply.
  async function resolveCommandProposal(approved) {
    runButton.disabled = true;
    rejectButton.disabled = true;
    output.hidden = false;
    output.textContent = approved ? "Running…" : "Rejecting…";

    const assistantMessage = appendChatMessage("assistant", "");
    let assistantText = "";
    let streamError = null;
    await streamResumeCommandRequest(
      {
        repo: proposalRepo,
        proposal_id: proposal.proposalId,
        approved: approved ? "true" : "false",
      },
      {
        token(token) {
          assistantText += token;
          updateChatMessage(assistantMessage, assistantText);
          setChatStatus("Streaming", "running");
        },
        done(payload) {
          if (payload.response && payload.response !== assistantText) {
            assistantText = payload.response;
            updateChatMessage(assistantMessage, assistantText);
          }
          if (payload.commandProposal) {
            assistantMessage.body.append(
              createCommandApprovalPreview(payload.commandProposal, proposalRepo),
            );
          }
          if (payload.sessionId) {
            currentSessionId = payload.sessionId;
            localStorage.setItem(lastSessionStorageKey(), currentSessionId);
          }
          renderContextFiles(payload.contextFiles || []);
          setChatStatus(payload.incomplete ? "Incomplete" : "Complete", payload.incomplete ? "warn" : "ok");
        },
        error(payload) {
          streamError = new Error(payload.error || "Command resume failed");
        },
      },
    );
    if (streamError) throw streamError;
    output.textContent = approved
      ? "Command approved — see the assistant's answer above."
      : "Command rejected — see the assistant's answer above.";
    await loadSessions(currentSessionId, false);
  }

  runButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(true);
      toast("Command completed");
    } catch (error) {
      runButton.disabled = false;
      rejectButton.disabled = false;
      output.textContent = error.message;
      toast(error.message);
    }
  });

  rejectButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(false);
      toast("Command rejected");
    } catch (error) {
      runButton.disabled = Boolean(proposal.blocked);
      rejectButton.disabled = false;
      output.textContent = error.message;
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
  return streamRequest("/api/ask-stream", "/api/ask", data, handlers);
}

async function streamResumeCommandRequest(data, handlers) {
  const fallbackPath = data.approved === "true" ? "/api/run-command" : "/api/reject-command";
  return streamRequest("/api/resume-command-stream", fallbackPath, data, handlers);
}

async function streamRequest(streamPath, fallbackPath, data, handlers) {
  const response = await fetch(apiUrl(streamPath), withApiToken(streamPath, form(data)));
  if (!response.ok) {
    throw new Error(await response.text());
  }
  if (!response.body) {
    const payload = await api(fallbackPath, form(data));
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

$("composer-attach-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleAttachMenu();
});

$("composer-attach-popover").addEventListener("click", (event) => {
  event.stopPropagation();
});

$("attach-add-file-btn").addEventListener("click", async () => {
  closeAttachMenu();
  try {
    await addContextFilesFromPicker();
  } catch (error) {
    toast(error.message);
  }
});

document.addEventListener("click", (event) => {
  if (!$("composer-attach-menu").contains(event.target)) closeAttachMenu();
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
      dismissContextChips();
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
        async done(payload) {
          currentSessionId = payload.sessionId;
          localStorage.setItem(lastSessionStorageKey(), currentSessionId);
          persistPinnedContextForSession(currentSessionId);
          if (payload.response && payload.response !== assistantText) {
            assistantText = payload.response;
          }
          // Awaited so the command-approval preview appended below survives
          // finalize's innerHTML replacement instead of being wiped by it.
          await finalizeChatMessage(assistantMessage, assistantText);
          if (payload.commandProposal) {
            assistantMessage.body.append(createCommandApprovalPreview(payload.commandProposal, chatRepo));
          }
          if (payload.patchProposal) {
            assistantMessage.body.append(createPatchPreview(payload.patchProposal, chatRepo));
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
    dismissContextChips();
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
    closeAttachMenu();
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
    if (!id || !(await confirmDialog("Remove provider?", `Remove provider ${id}?`))) return;
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
    if (!(await confirmDialog("Remove API key?", "Remove this stored API key from Keychain?"))) return;
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
void setupContextFileDragDrop();
