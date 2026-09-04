const $ = (id) => document.getElementById(id);

let currentSessionId = "";
let apiToken = "";
let bootstrapPromise = null;
let bootstrapError = null;
let chatSubmitting = false;
let pinnedContextFiles = [];
let contextChipsDismissed = false;
let terminalOpen = false;
let term = null;
let termFit = null;
let termId = "";
let termSessionSeq = 0;
let termResizeObserver = null;
let termInputChain = Promise.resolve();
let projectPaths = [];
let projectDisplayNames = new Map();
let expandedProjectPaths = new Set();
const projectSessionsByPath = new Map();
const projectSessionsLoading = new Set();
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
    defaultModel: "deepseek-v4-flash",
    models: ["deepseek-v4-flash", "deepseek-v4-pro"],
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
  Object.entries(data).forEach(([key, value]) => {
    params.set(key, value ?? "");
  });
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
    projectPaths = Array.isArray(stored) ? stored.map(normalizeProjectPath).filter(Boolean) : [];
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
function showAppDialog({
  title,
  message = "",
  inputValue = null,
  confirmLabel = "OK",
  danger = false,
  dismissOnly = false,
}) {
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
    // A notice has nothing to decline, so it gets one button.
    cancelBtn.hidden = dismissOnly;

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

function noticeDialog(title, message) {
  return showAppDialog({ title, message, confirmLabel: "OK", dismissOnly: true });
}

// Repository config is untrusted input: it may add restrictions but never
// remove one. The engine refuses the rest and reports it once per repository;
// these two dialogs are where the user hears about it.
const reviewedRepositoryConfigs = new Set();

async function reviewRepositoryConfig(repoPath) {
  const path = normalizeProjectPath(repoPath);
  if (!path || reviewedRepositoryConfigs.has(path)) return;
  reviewedRepositoryConfigs.add(path);
  let review;
  try {
    review = await api(`/api/repository-config-review?repo=${encodeURIComponent(path)}`);
  } catch {
    // A review that cannot be read must not stop the project from opening.
    // The engine has already refused the keys either way.
    return;
  }
  const rejected = Array.isArray(review.rejectedKeys) ? review.rejectedKeys : [];
  if (rejected.length) {
    await noticeDialog(
      "This repository tried to change Damaian's settings",
      `${projectName(path)} ships a .damaian/config.conf that sets ${rejected
        .map((item) => item.key)
        .join(", ")}. Damaian ignored ${
        rejected.length === 1 ? "that key" : "those keys"
      }: a repository cannot change where commands run, where model traffic goes, where your ` +
        "data is written, or which approvals you see. Its other settings were applied.",
    );
  }
  const entries = Array.isArray(review.allowlistEntries) ? review.allowlistEntries : [];
  if (entries.length) {
    const kept = await showAllowlistMigrationDialog(path, entries);
    if (kept === null) {
      // Dismissed rather than answered — ask again next time.
      reviewedRepositoryConfigs.delete(path);
      return;
    }
    try {
      await api("/api/repository-config-allowlist", form({ repo: path, keep: kept.join("|") }));
      toast(
        kept.length
          ? `Kept ${kept.length} allowed command${kept.length === 1 ? "" : "s"}`
          : "Discarded this repository's allowed commands",
      );
    } catch (error) {
      reviewedRepositoryConfigs.delete(path);
      toast(error.message);
    }
  }
}

// Resolves to the commands to keep (possibly empty), or `null` if the user
// dismissed the question without answering it.
function showAllowlistMigrationDialog(repoPath, entries) {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "app-dialog-backdrop";
    backdrop.innerHTML = `
      <div class="app-dialog app-dialog-wide" role="dialog" aria-modal="true">
        <p class="app-dialog-title">Commands this repository lists as always allowed</p>
        <p class="app-dialog-message"></p>
        <div class="app-dialog-checklist"></div>
        <div class="app-dialog-actions">
          <button type="button" class="app-dialog-btn app-dialog-discard">Discard all</button>
          <button type="button" class="app-dialog-btn app-dialog-confirm">Keep selected</button>
        </div>
      </div>
    `;
    backdrop.querySelector(".app-dialog-message").textContent =
      `${projectName(repoPath)} carries these in .damaian/config.conf. Damaian cannot tell which ` +
      "you allowed yourself and which arrived with the repository, so none of them run without " +
      "asking until you choose. Keep only the ones you recognise.";
    const list = backdrop.querySelector(".app-dialog-checklist");
    entries.forEach((entry, index) => {
      const row = document.createElement("label");
      row.className = "app-dialog-checkitem";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.value = entry;
      box.id = `allowlist-migration-${index}`;
      const text = document.createElement("code");
      text.textContent = entry;
      row.append(box, text);
      list.append(row);
    });
    document.body.append(backdrop);

    const cleanup = (result) => {
      document.removeEventListener("keydown", onKeydown, true);
      backdrop.remove();
      resolve(result);
    };
    const onKeydown = (event) => {
      if (event.key === "Escape") cleanup(null);
    };
    backdrop.querySelector(".app-dialog-confirm").addEventListener("click", () => {
      cleanup(Array.from(list.querySelectorAll("input:checked")).map((input) => input.value));
    });
    backdrop.querySelector(".app-dialog-discard").addEventListener("click", () => cleanup([]));
    document.addEventListener("keydown", onKeydown, true);
    backdrop.querySelector(".app-dialog-confirm").focus();
  });
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
  const left = Math.min(Math.max(8, rect.right - width), window.innerWidth - width - 8);
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
  const parts = String(path || "")
    .split(/[\\/]/)
    .filter(Boolean);
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
  // Switching repositories restarts the shell in the new working folder.
  if (terminalOpen) {
    void restartTerminal().catch((error) => toast(error.message));
  } else {
    void closeTerminalSession();
  }
  renderProjectList();
  return projectPath;
}

function setRepository(value, persist = true) {
  const projectPath = applyRepositoryState(value, persist);
  if (projectPath) {
    void reviewRepositoryConfig(projectPath);
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
  void reviewRepositoryConfig(normalized);
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
  const target = ["general", "shortcuts", "checkpoints", "mcp", "providers", "models"].includes(
    page,
  )
    ? page
    : "providers";
  if (target === "mcp") renderMcpConfigSelect();
  if (target === "checkpoints") void renderCheckpointList();
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

const TERMINAL_THEME = {
  background: "#ffffff",
  foreground: "#1f2428",
  cursor: "#1f2428",
  cursorAccent: "#ffffff",
  selectionBackground: "rgba(31, 36, 40, 0.18)",
  black: "#1f2428",
  red: "#a33737",
  green: "#2e7d32",
  yellow: "#8a6d00",
  blue: "#1565c0",
  magenta: "#8e24aa",
  cyan: "#00838f",
  white: "#d8d8d4",
  brightBlack: "#5c6672",
  brightRed: "#c62828",
  brightGreen: "#388e3c",
  brightYellow: "#a97b00",
  brightBlue: "#1976d2",
  brightMagenta: "#ab47bc",
  brightCyan: "#0097a7",
  brightWhite: "#1f2428",
};

function setTerminalOpen(open) {
  terminalOpen = open;
  document.body.classList.toggle("terminal-open", open);
  $("terminal-panel").hidden = !open;
  const button = $("terminal-toggle-btn");
  button.setAttribute("aria-pressed", open ? "true" : "false");
  button.setAttribute("aria-label", open ? "Hide terminal" : "Show terminal");
  button.title = open ? "Hide terminal" : "Show terminal";
  if (open) {
    void ensureTerminal().catch((error) => toast(error.message));
  }
}

// Lazily build the xterm.js instance and wire it to the pty transport.
function createTerminalInstance() {
  if (term) return;
  term = new Terminal({
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: 13,
    cursorBlink: true,
    scrollback: 5000,
    theme: TERMINAL_THEME,
  });
  termFit = new FitAddon.FitAddon();
  term.loadAddon(termFit);
  term.open($("terminal-xterm"));
  // Every keystroke, escape sequence and paste flows straight to the shell.
  term.onData((data) => sendTerminalInput(data));
  // Some webviews deliver keydown events with keyCode 0 / empty `code`, which
  // stops xterm from mapping special keys (arrows, Ctrl+<letter>, Tab, …) even
  // though plain letters still work. When we see that, translate the key from
  // `event.key` ourselves. Environments that report a real keyCode fall
  // through to xterm unchanged, so there is no double-send or regression.
  term.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    // Ctrl+<letter> maps to a fixed control byte (0x01–0x1a) in every terminal
    // mode, so handle it ourselves unconditionally — this makes Ctrl+R, Ctrl+C,
    // etc. work even if the webview swallows the shortcut or drops keyCode.
    // Returning false stops xterm from also sending it.
    if (event.ctrlKey && !event.metaKey && !event.altKey && event.key && event.key.length === 1) {
      const code = event.key.toLowerCase().charCodeAt(0);
      if (code >= 97 && code <= 122) {
        event.preventDefault();
        sendTerminalInput(String.fromCharCode(code - 96));
        return false;
      }
    }
    // For other special keys (arrows, Tab, …) only step in when the webview
    // failed to populate keyCode, so xterm couldn't map them. When keyCode is
    // present, xterm handles them (preserving application-cursor mode, etc.).
    if (event.keyCode) return true;
    const seq = terminalKeySequence(event);
    if (seq == null) return true;
    event.preventDefault();
    sendTerminalInput(seq);
    return false;
  });
  // Keep the pty window size in step with the visible viewport.
  termResizeObserver = new ResizeObserver(() => fitTerminal());
  termResizeObserver.observe($("terminal-xterm"));
  // Any click in the terminal area focuses the shell so the cursor shows and
  // keys reach the pty — the webview doesn't always route the click to xterm's
  // own handler on its own.
  const body = $("terminal-xterm").closest(".terminal-body") || $("terminal-xterm");
  body.addEventListener("mousedown", () => {
    if (term) requestAnimationFrame(() => term.focus());
  });
}

// Byte sequence a terminal expects for a special key, derived from
// `event.key` (used only when the webview fails to populate `keyCode`).
function terminalKeySequence(event) {
  if (event.metaKey || event.altKey) return null;
  const key = event.key;
  if (event.ctrlKey) {
    if (key && key.length === 1) {
      const code = key.toLowerCase().charCodeAt(0);
      if (code >= 97 && code <= 122) return String.fromCharCode(code - 96); // Ctrl+A..Z
    }
    return null;
  }
  switch (key) {
    case "ArrowUp":
      return "\x1b[A";
    case "ArrowDown":
      return "\x1b[B";
    case "ArrowRight":
      return "\x1b[C";
    case "ArrowLeft":
      return "\x1b[D";
    case "Home":
      return "\x1b[H";
    case "End":
      return "\x1b[F";
    case "Delete":
      return "\x1b[3~";
    case "PageUp":
      return "\x1b[5~";
    case "PageDown":
      return "\x1b[6~";
    case "Tab":
      return event.shiftKey ? "\x1b[Z" : "\t";
    case "Enter":
      return "\r";
    case "Backspace":
      return "\x7f";
    case "Escape":
      return "\x1b";
    default:
      return null; // printable characters go through xterm's normal path
  }
}

async function ensureTerminal() {
  createTerminalInstance();
  fitTerminal();
  if (!termId) {
    await startTerminalSession();
  }
  focusTerminalSoon();
}

// Focus after layout settles — on a freshly-shown panel the element may not be
// focusable in the same frame it becomes visible.
function focusTerminalSoon() {
  if (!term) return;
  term.focus();
  requestAnimationFrame(() => term?.focus());
  setTimeout(() => term?.focus(), 60);
}

async function startTerminalSession() {
  const invoke = tauriInvoke();
  const Channel = window.__TAURI__?.core?.Channel;
  if (!invoke || !Channel) {
    term.write("\r\n\x1b[33mThe terminal is only available in the Damaian desktop app.\x1b[0m\r\n");
    return;
  }

  // A fresh sequence per session lets a late message from a torn-down shell
  // be ignored after the terminal has been restarted.
  const seq = ++termSessionSeq;
  const channel = new Channel();
  channel.onmessage = (message) => {
    if (seq !== termSessionSeq || !term) return;
    if (message.type === "output") {
      term.write(base64ToBytes(message.data));
    } else if (message.type === "exit") {
      term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n");
      termId = "";
    }
  };

  const payload = await invoke("terminal_open", {
    repo: repo(),
    cols: term.cols || 80,
    rows: term.rows || 24,
    onOutput: channel,
  });
  termId = payload.id;
  $("terminal-cwd").textContent = payload.cwd || "";
  $("terminal-title").textContent = payload.cwd ? terminalTitleForPath(payload.cwd) : "Terminal";
}

function base64ToBytes(value) {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

// Keystrokes go straight to the pty over IPC. Chain the invokes so bytes
// always reach the shell in the order they were typed.
function sendTerminalInput(data) {
  if (!termId) return;
  const invoke = tauriInvoke();
  if (!invoke) return;
  const id = termId;
  termInputChain = termInputChain
    .then(() => invoke("terminal_write", { id, data }))
    .catch(() => {});
}

function fitTerminal() {
  if (!term || !termFit || !terminalOpen) return;
  try {
    termFit.fit();
  } catch {
    return;
  }
  const invoke = tauriInvoke();
  if (termId && invoke) {
    void invoke("terminal_resize", { id: termId, cols: term.cols, rows: term.rows }).catch(
      () => {},
    );
  }
}

async function closeTerminalSession() {
  const id = termId;
  termId = "";
  termSessionSeq += 1;
  const invoke = tauriInvoke();
  if (id && invoke) {
    try {
      await invoke("terminal_close", { id });
    } catch {
      // best effort — the shell is reaped when its pty master is dropped
    }
  }
}

async function restartTerminal() {
  await closeTerminalSession();
  if (term) term.reset();
  await ensureTerminal();
}

function terminalTitleForPath(path) {
  const trimmed = String(path || "").replace(/[\\/]+$/, "");
  if (!trimmed) return "/";
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || trimmed || "Terminal";
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
  const provider = String(value || "")
    .trim()
    .toLowerCase()
    .replaceAll("_", "-");
  if (provider === "open-ai" || provider === "openai") return "openai";
  if (provider === "deep-seek" || provider === "deepseek" || provider === "deedseek") {
    return "deepseek";
  }
  if (
    provider === "custom" ||
    provider === "open-ai-compatible" ||
    provider === "openai-compatible"
  ) {
    return "openai-compatible";
  }
  return /^[a-z0-9.-]+$/.test(provider) ? provider : "openai-compatible";
}

function normalizeChatReasoning(value) {
  const reasoning = String(value || "")
    .trim()
    .toLowerCase();
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
    const match = key.match(
      /^model_provider\.([a-zA-Z0-9_.-]+)\.(label|base_url|api_key_env|models|supports_native_tools|max_output_tokens|context_token_budget)$/,
    );
    if (match) configuredProviderIds.add(normalizeChatProvider(match[1]));
  });
}

function syncProviderCatalogFromPolicy(policyText) {
  Object.keys(modelProviderPresets).forEach((key) => {
    delete modelProviderPresets[key];
  });
  Object.keys(providerLabels).forEach((key) => {
    delete providerLabels[key];
  });
  Object.entries(builtInModelProviderPresets).forEach(([id, preset]) => {
    modelProviderPresets[id] = { ...preset, models: [...preset.models] };
  });

  const providers = {};
  configEntries(policyText).forEach(([key, value]) => {
    const match = key.match(
      /^model_provider\.([a-zA-Z0-9_.-]+)\.(label|base_url|api_key_env|models|supports_native_tools|max_output_tokens|context_token_budget)$/,
    );
    if (!match) return;
    const id = normalizeChatProvider(match[1]);
    providers[id] = providers[id] || { id };
    const field = match[2];
    if (field === "label") providers[id].label = value;
    if (field === "base_url") providers[id].baseUrl = value;
    if (field === "api_key_env") providers[id].apiKeyEnv = value;
    if (field === "models") providers[id].models = splitModelList(value);
    if (field === "supports_native_tools") providers[id].supportsNativeTools = value === "true";
    if (field === "max_output_tokens") providers[id].maxOutputTokens = value.trim();
    if (field === "context_token_budget") providers[id].contextTokenBudget = value.trim();
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
      supportsNativeTools: provider.supportsNativeTools ?? existing.supportsNativeTools ?? false,
      // Blank means "use the built-in per-model default", which only the
      // engine knows, so the UI carries the value through untouched rather
      // than substituting a number of its own.
      maxOutputTokens: provider.maxOutputTokens ?? existing.maxOutputTokens ?? "",
      contextTokenBudget: provider.contextTokenBudget ?? existing.contextTokenBudget ?? "",
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

// Returns "" for a blank field (meaning "use the built-in default") and
// rejects anything the engine's own parser would refuse, so a bad value is
// caught in the form rather than surfacing as a config-save error.
//
// Takes the element, not its value, because `<input type="number">` reports
// `value === ""` for text it cannot parse ("1e", "--") while still showing
// that text to the user. Reading `.value` alone would save "use the default"
// under a field that visibly says otherwise; `validity.badInput` is the only
// way to tell that apart from a genuinely empty field.
function optionalTokenCount(input, fieldLabel) {
  if (input.validity?.badInput) throw new Error(`${fieldLabel} must be a whole number`);
  const value = String(input.value || "").trim();
  if (!value) return "";
  if (!/^\d+$/.test(value)) throw new Error(`${fieldLabel} must be a whole number`);
  const parsed = Number(value);
  if (parsed < 1) throw new Error(`${fieldLabel} must be at least 1`);
  if (parsed > 4294967295) throw new Error(`${fieldLabel} is too large`);
  return String(parsed);
}

function providerConfigFromForm() {
  const label = $("provider-label-input").value.trim();
  const id = providerSlug($("provider-id-input").value || label);
  const baseUrl = $("provider-base-url-input").value.trim().replace(/\/+$/, "");
  const apiKeyEnv = $("provider-key-ref-input").value.trim();
  const models = splitModelList($("provider-models-input").value);
  const supportsNativeTools = $("provider-native-tools-input").checked;
  const maxOutputTokens = optionalTokenCount(
    $("provider-max-output-tokens-input"),
    "Max output tokens",
  );
  const contextTokenBudget = optionalTokenCount(
    $("provider-context-budget-input"),
    "Context budget",
  );
  if (!label) throw new Error("Provider name is required");
  if (!id) throw new Error("Provider ID is required");
  if (!baseUrl) throw new Error("Provider base URL is required");
  if (!apiKeyEnv) throw new Error("Provider API key reference is required");
  if (apiKeyEnv === "keychain:") throw new Error("Keychain account is required");
  if (!models.length) throw new Error("At least one model is required");
  return {
    id,
    label,
    baseUrl,
    apiKeyEnv,
    models,
    supportsNativeTools,
    maxOutputTokens,
    contextTokenBudget,
  };
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
  $("provider-native-tools-input").checked = provider.supportsNativeTools === true;
  $("provider-max-output-tokens-input").value = provider.maxOutputTokens || "";
  $("provider-context-budget-input").value = provider.contextTokenBudget || "";
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
  $("provider-native-tools-input").checked = false;
  $("provider-max-output-tokens-input").value = "";
  $("provider-context-budget-input").value = "";
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
  return (
    String(label || "?")
      .trim()
      .slice(0, 1)
      .toUpperCase() || "?"
  );
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
  const id = normalizeChatProvider(
    $("provider-id-input").dataset.originalId || $("provider-id-input").value,
  );
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
  next = upsertConfigValue(
    next,
    `model_provider.${provider.id}.supports_native_tools`,
    provider.supportsNativeTools ? "true" : "false",
  );
  // Omitted entirely when blank: an absent key is what makes the engine fall
  // back to its per-model default, so writing an empty value would be wrong.
  if (provider.maxOutputTokens) {
    next = upsertConfigValue(
      next,
      `model_provider.${provider.id}.max_output_tokens`,
      provider.maxOutputTokens,
    );
  }
  if (provider.contextTokenBudget) {
    next = upsertConfigValue(
      next,
      `model_provider.${provider.id}.context_token_budget`,
      provider.contextTokenBudget,
    );
  }
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

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

// Parsed from the config-editor content; the config file is the source of
// truth (same approach as LLM providers).
let mcpServers = {};

const mcpServerFieldPattern =
  /^mcp_server\.([a-z0-9.-]+)\.(label|transport|command|args|env|url|auth_token_env|enabled|require_approval)$/;

function mcpSlug(value) {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function syncMcpServersFromConfig(content) {
  const servers = {};
  configEntries(content).forEach(([key, value]) => {
    const match = key.match(mcpServerFieldPattern);
    if (!match) return;
    const id = match[1];
    if (!servers[id]) {
      servers[id] = { id, transport: "stdio", requireApproval: true };
    }
    const server = servers[id];
    const field = match[2];
    if (field === "label") server.label = value;
    else if (field === "transport") server.transport = value === "http" ? "http" : "stdio";
    else if (field === "command") server.command = value;
    else if (field === "args") server.args = value;
    else if (field === "env") server.env = value;
    else if (field === "url") server.url = value;
    else if (field === "auth_token_env") server.authTokenEnv = value;
    else if (field === "enabled") server.enabled = value === "true";
    else if (field === "require_approval") server.requireApproval = value === "true";
  });
  mcpServers = servers;
}

function mcpServerIds() {
  return Object.keys(mcpServers).sort((a, b) => a.localeCompare(b));
}

// Config stores list values `|`-joined; the editor shows one per line.
function pipeToLines(value) {
  return String(value || "")
    .split("|")
    .map((item) => item.trim())
    .filter(Boolean)
    .join("\n");
}

function linesToPipe(value) {
  return String(value || "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean)
    .join("|");
}

function updateMcpTransportFields() {
  const isHttp = $("mcp-transport-select").value === "http";
  document.querySelectorAll(".mcp-http-fields").forEach((el) => {
    el.hidden = !isHttp;
  });
  document.querySelectorAll(".mcp-stdio-fields").forEach((el) => {
    el.hidden = isHttp;
  });
}

function mcpServerFromForm() {
  const label = $("mcp-label-input").value.trim();
  const id = mcpSlug($("mcp-id-input").value || label);
  const transport = $("mcp-transport-select").value === "http" ? "http" : "stdio";
  if (!label) throw new Error("Server name is required");
  if (!id) throw new Error("Server ID is required");
  const server = {
    id,
    label,
    transport,
    command: $("mcp-command-input").value.trim(),
    args: linesToPipe($("mcp-args-input").value),
    env: linesToPipe($("mcp-env-input").value),
    url: $("mcp-url-input").value.trim().replace(/\/+$/, ""),
    authTokenEnv: $("mcp-token-ref-input").value.trim(),
    enabled: $("mcp-enabled-input").checked,
    requireApproval: $("mcp-approval-input").checked,
  };
  if (transport === "stdio" && !server.command) {
    throw new Error("A command is required for a local (stdio) server");
  }
  if (transport === "http" && !server.url) {
    throw new Error("A URL is required for a remote (http) server");
  }
  return server;
}

function renderMcpConfigSelect(selectedId = $("mcp-config-select").value) {
  const select = $("mcp-config-select");
  if (!select) return;
  const ids = mcpServerIds();
  select.innerHTML = "";
  select.disabled = !ids.length;
  if (!ids.length) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "No configured servers";
    select.append(option);
    clearMcpConfigForm();
    renderMcpServerList();
    return;
  }
  ids.forEach((id) => {
    const option = document.createElement("option");
    option.value = id;
    option.textContent = mcpServers[id].label || id;
    select.append(option);
  });
  const nextId = ids.includes(selectedId) ? selectedId : ids[0];
  select.value = nextId;
  renderMcpConfigForm(nextId);
  renderMcpServerList();
}

function renderMcpConfigForm(serverId = $("mcp-config-select").value) {
  const server = mcpServers[serverId] || {
    id: serverId || "",
    label: "",
    transport: "stdio",
    requireApproval: true,
  };
  $("mcp-config-select").value = serverId || "";
  $("mcp-label-input").value = server.label || "";
  $("mcp-id-input").value = server.id || "";
  $("mcp-id-input").dataset.originalId = server.id || "";
  $("mcp-transport-select").value = server.transport === "http" ? "http" : "stdio";
  $("mcp-command-input").value = server.command || "";
  $("mcp-args-input").value = pipeToLines(server.args);
  $("mcp-env-input").value = pipeToLines(server.env);
  $("mcp-url-input").value = server.url || "";
  $("mcp-token-ref-input").value =
    server.authTokenEnv || `keychain:mcp-${server.id || "server"}-token`;
  $("mcp-token-input").value = "";
  $("mcp-enabled-input").checked = server.enabled === true;
  $("mcp-approval-input").checked = server.requireApproval !== false;
  $("mcp-remove-btn").disabled = !mcpServers[server.id];
  setMcpTestResult("", "");
  updateMcpTransportFields();
}

function clearMcpConfigForm() {
  $("mcp-config-select").value = "";
  $("mcp-label-input").value = "";
  $("mcp-id-input").value = "";
  $("mcp-id-input").dataset.originalId = "";
  $("mcp-transport-select").value = "stdio";
  $("mcp-command-input").value = "";
  $("mcp-args-input").value = "";
  $("mcp-env-input").value = "";
  $("mcp-url-input").value = "";
  $("mcp-token-ref-input").value = "keychain:";
  $("mcp-token-input").value = "";
  $("mcp-enabled-input").checked = true;
  $("mcp-approval-input").checked = true;
  $("mcp-remove-btn").disabled = true;
  setMcpTestResult("", "");
  updateMcpTransportFields();
}

function newMcpConfigForm() {
  clearMcpConfigForm();
  $("mcp-label-input").focus();
}

function setMcpTestResult(message, state = "") {
  const el = $("mcp-test-result");
  if (!el) return;
  el.textContent = message;
  el.dataset.state = state;
}

function renderMcpServerList() {
  const container = $("mcp-server-list");
  if (!container) return;
  container.innerHTML = "";
  const ids = mcpServerIds();
  if (!ids.length) {
    const empty = document.createElement("div");
    empty.className = "provider-empty-row";
    empty.textContent = "No MCP servers configured.";
    container.append(empty);
    return;
  }
  ids.forEach((id) => {
    const server = mcpServers[id];
    const row = document.createElement("div");
    row.className = "settings-row";
    const copy = document.createElement("div");
    const title = document.createElement("strong");
    title.textContent = server.label || id;
    const description = document.createElement("p");
    const transportLabel = server.transport === "http" ? "HTTP" : "stdio";
    const state = server.enabled ? "enabled" : "disabled";
    const detail = server.transport === "http" ? server.url || "" : server.command || "";
    description.textContent = `${transportLabel} · ${state}${detail ? ` · ${detail}` : ""}`;
    copy.append(title, description);
    const configureButton = document.createElement("button");
    configureButton.type = "button";
    configureButton.textContent = "Configure";
    configureButton.addEventListener("click", () => {
      setSettingsPage("mcp");
      renderMcpConfigForm(id);
      $("mcp-label-input").focus();
    });
    row.append(copy, configureButton);
    container.append(row);
  });
}

function upsertMcpConfig(content, server) {
  let next = removeMcpConfig(content, server.id);
  const set = (field, value) =>
    (next = upsertConfigValue(next, `mcp_server.${server.id}.${field}`, value));
  set("label", server.label);
  set("transport", server.transport);
  if (server.transport === "http") {
    set("url", server.url);
    if (server.authTokenEnv) set("auth_token_env", server.authTokenEnv);
  } else {
    set("command", server.command);
    if (server.args) set("args", server.args);
    if (server.env) set("env", server.env);
  }
  set("enabled", server.enabled ? "true" : "false");
  set("require_approval", server.requireApproval ? "true" : "false");
  return next;
}

function removeMcpConfig(content, serverId) {
  const prefix = `mcp_server.${serverId}.`;
  return String(content || "")
    .split(/\r?\n/)
    .filter((line) => !line.trim().startsWith(prefix))
    .join("\n")
    .replace(/\n*$/, "\n");
}

async function saveMcpServer() {
  const server = mcpServerFromForm();
  const token = $("mcp-token-input").value.trim();
  if (server.transport === "http" && token) {
    if (!server.authTokenEnv.startsWith("keychain:")) {
      throw new Error("A token can only be saved when the reference starts with keychain:");
    }
    const account = server.authTokenEnv.slice("keychain:".length).trim();
    if (!account) throw new Error("Keychain account is required");
    const payload = await api("/api/provider-key", form({ account, api_key: token }));
    server.authTokenEnv = payload.reference;
  }

  const originalId = $("mcp-id-input").dataset.originalId;
  let content = $("config-editor").value;
  if (originalId && originalId !== server.id) {
    content = removeMcpConfig(content, originalId);
  }
  content = upsertMcpConfig(content, server);
  $("config-editor").value = content;
  const payload = await saveConfigFile();
  $("mcp-token-input").value = "";
  renderMcpConfigSelect(server.id);
  return payload;
}

async function removeMcpServerFromSettings() {
  const id = mcpSlug($("mcp-id-input").dataset.originalId || $("mcp-id-input").value);
  if (!id || !mcpServers[id]) return;
  $("config-editor").value = removeMcpConfig($("config-editor").value, id);
  await saveConfigFile();
  renderMcpConfigSelect();
}

async function testMcpServer() {
  const server = mcpServerFromForm();
  const token = $("mcp-token-input").value.trim();
  setMcpTestResult("Connecting…", "");
  const payload = await api(
    "/api/mcp-test",
    form({
      id: server.id,
      transport: server.transport,
      command: server.command,
      args: server.args,
      env: server.env,
      url: server.url,
      auth_token_env: server.authTokenEnv,
      auth_token: token,
    }),
  );
  if (payload.ok) {
    const names = (payload.tools || []).slice(0, 8).join(", ");
    const more = (payload.tools || []).length > 8 ? "…" : "";
    setMcpTestResult(
      `Connected — ${payload.toolCount} tool${payload.toolCount === 1 ? "" : "s"}${
        names ? `: ${names}${more}` : ""
      }`,
      "ok",
    );
  } else {
    setMcpTestResult(payload.error || "Connection failed", "error");
  }
  return payload;
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
  syncMcpServersFromConfig(payload.content);
  syncModelKeyAccountFromConfig(payload.content);
  renderConfigPolicy(payload);
  $("config-path").textContent = payload.exists ? payload.path : `${payload.path} (new)`;
  void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
  return payload;
}

async function saveConfigFile() {
  const content = $("config-editor").value;
  syncConfiguredProvidersFromConfig(content);
  syncMcpServersFromConfig(content);
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
  var withoutCode = escaped.replace(/`([^`]+)`/g, (_match, code) => {
    codeSpans.push(code);
    return PLACEHOLDER + (codeSpans.length - 1) + PLACEHOLDER;
  });
  var withInline = withoutCode
    .replace(
      /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
      '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>',
    )
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/\*([^*]+)\*/g, "<em>$1</em>");
  var codePattern = new RegExp(`${PLACEHOLDER}(\\d+)${PLACEHOLDER}`, "g");
  return withInline.replace(
    codePattern,
    (_match, index) => `<code>${codeSpans[Number(index)]}</code>`,
  );
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
        codeLanguage = line
          .slice(3)
          .trim()
          .replace(/[^a-z0-9_-]/gi, "");
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
  appendWebDiagnosticArtifacts(body, content);

  message.append(label, body);
  $("chat-log").append(message);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
  return { message, body };
}

// Phase labels the server does not spell out. Tool phases arrive with their own
// `label` so this map never has to know tool names.
const TURN_PHASE_LABELS = {
  context: "Assembling context",
  model: "Waiting for model",
  tool: "Working",
  finalizing: "Finalizing",
};

// A live "something is happening" row inside an assistant bubble: the phase, an
// elapsed clock, and a Stop button.
//
// Owns its DOM, its interval, and its teardown so nothing outside has to track
// them. `finish` is idempotent, which lets error paths call it blindly.
function startTurnIndicator(target, onStop) {
  const row = document.createElement("p");
  row.className = "turn-indicator";
  row.dataset.state = "running";

  const dot = document.createElement("span");
  dot.className = "turn-indicator-dot";
  dot.setAttribute("aria-hidden", "true");

  // Announced on change, so a screen reader hears the phase but not the clock.
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.setAttribute("aria-live", "polite");
  label.textContent = "Starting";

  // Ticks every second. Inside the aria-live #chat-log it would otherwise be
  // read out once per second, which is unusable.
  const elapsed = document.createElement("span");
  elapsed.className = "turn-indicator-elapsed";
  elapsed.setAttribute("aria-hidden", "true");

  const stop = document.createElement("button");
  stop.type = "button";
  stop.className = "turn-indicator-stop";
  stop.textContent = "Stop";
  stop.addEventListener("click", () => onStop());

  row.append(dot, label, elapsed, stop);
  target.body.after(row);

  const startedAt = Date.now();
  const tick = () => {
    elapsed.textContent = `${Math.round((Date.now() - startedAt) / 1000)}s`;
  };
  tick();
  const timer = setInterval(tick, 1000);

  let done = false;
  let streaming = false;

  return {
    phase(payload) {
      if (done) return;
      // Any non-model phase means this round's streaming is over, so the next
      // model round is free to say "Waiting for model" again instead of leaving
      // the previous tool's label up for the rest of the turn.
      if (payload.phase !== "model") streaming = false;
      // Within a round though, tokens are already flowing by the time the model
      // phase would repeat, and "Streaming" is the truer description.
      else if (streaming) return;
      const text = payload.label || TURN_PHASE_LABELS[payload.phase] || "Working";
      const rounds =
        payload.phase !== "context" && payload.maxRounds > 1 && payload.round > 1
          ? ` · round ${payload.round}/${payload.maxRounds}`
          : "";
      label.textContent = `${text}${rounds}`;
    },
    streaming() {
      if (done || streaming) return;
      streaming = true;
      label.textContent = "Streaming";
    },
    // "stopped" | "complete" | "incomplete" | "failed"
    finish(state) {
      if (done) return;
      done = true;
      clearInterval(timer);
      if (state === "stopped") {
        row.dataset.state = "stopped";
        label.textContent = "Stopped by you";
        elapsed.remove();
        stop.remove();
        return;
      }
      row.remove();
    },
  };
}

function updateChatMessage(target, content) {
  target.body.innerHTML = renderMarkdown(content);
  appendWebDiagnosticArtifacts(target.body, content);
  delete target.body.dataset.placeholder;
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

// Progress text ("Generating a patch preview...") is a promise the bubble
// cannot keep on its own: if the turn then fails, the toast carrying the real
// error auto-dismisses and the bubble is left claiming work is still in
// flight, forever. `data-placeholder` marks a bubble whose content is such a
// promise so `renderChatMessageError` knows to replace it outright, rather
// than annotating it the way it does streamed model output worth keeping.
function setChatMessagePlaceholder(target, content) {
  updateChatMessage(target, content);
  target.body.dataset.placeholder = "true";
}

// Terminal state for a failed turn. The bubble — not the toast — is the
// authoritative record once a turn is over, so every failure has to land here.
function renderChatMessageError(target, error) {
  if (!target) return;
  const body = target.body;
  // Placeholder or nothing streamed yet: neither is worth keeping alongside
  // the error. Partial model output is, so it stays and the error follows it.
  if (body.dataset.placeholder === "true" || !body.textContent.trim()) {
    body.textContent = "";
    delete body.dataset.placeholder;
  }
  const note = document.createElement("p");
  note.className = "message-error";
  note.textContent = `Request failed: ${error.message}`;
  body.append(note);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

// Re-renders a finished message server-side (workspace-engine's
// `render_markdown_to_html`) to get real syntax-highlighted code blocks,
// which the fast client-side `renderMarkdown` above does not attempt. Only
// call this once a message's content is final (stream complete, or loaded
// from history) since it replaces the bubble's entire innerHTML.
async function finalizeChatMessage(target, content) {
  const chatRepo = repo();
  try {
    const payload = await api("/api/render-markdown", form({ content, repo: chatRepo || "" }));
    target.body.innerHTML = payload.html;
  } catch (_error) {
    target.body.innerHTML = renderMarkdown(content);
  }
  appendWebDiagnosticArtifacts(target.body, content);
  wireFileReferences(target.body, chatRepo);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

function appendWebDiagnosticArtifacts(container, content) {
  const paths = webDiagnosticArtifactPaths(content);
  if (!paths.length) return;

  const grid = document.createElement("div");
  grid.className = "web-diagnostic-artifacts";
  paths.forEach((path) => {
    const item = document.createElement("figure");
    item.className = "web-diagnostic-artifact";

    const image = document.createElement("img");
    image.alt = path.split("/").pop() || "Web diagnostic artifact";
    image.loading = "lazy";

    const caption = document.createElement("figcaption");
    caption.textContent = path;

    item.append(image, caption);
    grid.append(item);
    void loadWebDiagnosticArtifact(image, path);
  });
  container.append(grid);
}

function webDiagnosticArtifactPaths(content) {
  const paths = new Set();
  const pattern = /web-diagnostics\/[^\s)"'<>]+?\.(?:png|jpe?g|webp|gif)/gi;
  for (const match of String(content || "").matchAll(pattern)) {
    paths.add(match[0].replace(/[.,;:]+$/, ""));
  }
  return [...paths];
}

async function loadWebDiagnosticArtifact(image, path) {
  try {
    const chatRepo = repo();
    const endpoint = `/api/web-diagnostic-artifact?repo=${encodeURIComponent(
      chatRepo,
    )}&path=${encodeURIComponent(path)}`;
    const response = await fetch(apiUrl(endpoint), withApiToken(endpoint));
    if (!response.ok) throw new Error(response.statusText);
    const blob = await response.blob();
    image.src = URL.createObjectURL(blob);
  } catch (_error) {
    image.remove();
  }
}

// Wires click/Enter on the `.file-reference` elements the server-side
// renderer produced for verified in-text file paths, opening each in VS
// Code (at its line/column when present) via the same endpoint the
// context-file chips use.
function wireFileReferences(container, chatRepo) {
  if (!chatRepo) return;
  container.querySelectorAll(".file-reference").forEach((el) => {
    const open = async () => {
      try {
        const fields = { repo: chatRepo, path: el.dataset.path };
        if (el.dataset.line) fields.line = el.dataset.line;
        if (el.dataset.col) fields.col = el.dataset.col;
        const payload = await api("/api/open-vscode-file", form(fields));
        toast(`Opened ${payload.path}`);
      } catch (error) {
        toast(error.message);
      }
    };
    el.addEventListener("click", open);
    // Inline-code refs render as <code role="button">, not <button>, so
    // give them keyboard activation too.
    el.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        void open();
      }
    });
  });
}

// `tasks` carries each task's final status so a stopped turn stays marked as
// one on reload. Without it a truncated answer renders as a complete short one.
function renderMessages(messages, tasks = []) {
  $("chat-log").innerHTML = "";
  const cancelledTasks = new Set(
    tasks.filter((task) => task.status === "cancelled").map((task) => task.id),
  );
  const toolBudgetExhaustedTasks = new Set(
    tasks.filter((task) => task.status === "tool_budget_exhausted").map((task) => task.id),
  );
  messages.forEach((message) => {
    const bubble = appendChatMessage(message.role, message.content);
    if (message.role === "user") {
      const checkpoint = sessionCheckpoints.find((entry) => entry.userMessageId === message.id);
      if (checkpoint) markMessageRewindable(bubble, checkpoint);
    }
    if (message.role === "assistant") {
      void finalizeChatMessage(bubble, message.content);
      if (message.taskId && cancelledTasks.has(message.taskId)) {
        markMessageStopped(bubble);
      } else if (message.taskId && toolBudgetExhaustedTasks.has(message.taskId)) {
        markMessageToolBudgetExhausted(bubble, message.sessionId);
      }
    }
  });
}

// Checkpoints for the open session, newest first. Loaded with the session so
// a user turn can offer to rewind to the point just before it.
let sessionCheckpoints = [];

async function loadSessionCheckpoints(sessionId) {
  const repoPath = repo();
  if (!repoPath || !sessionId) {
    sessionCheckpoints = [];
    return;
  }
  try {
    const payload = await api(
      `/api/checkpoints?repo=${encodeURIComponent(repoPath)}&session_id=${encodeURIComponent(sessionId)}`,
    );
    sessionCheckpoints = Array.isArray(payload.checkpoints) ? payload.checkpoints : [];
  } catch {
    // A session whose checkpoints cannot be listed still has to open; the
    // turn simply offers no rewind.
    sessionCheckpoints = [];
  }
}

/// Every checkpoint for the open repository, newest first: when it was taken,
/// the turn it precedes, how much it covers, and whether it has been rewound to
/// already. The per-turn control in the conversation only reaches the turns
/// still on screen; this reaches the rest.
async function renderCheckpointList() {
  const container = $("checkpoint-list");
  container.innerHTML = "";
  const repoPath = repo();
  if (!repoPath) {
    container.append(emptyCheckpointNotice("Select a project to see its checkpoints."));
    return;
  }

  let checkpoints = [];
  let sessionTitles = new Map();
  try {
    const [payload, sessions] = await Promise.all([
      api(`/api/checkpoints?repo=${encodeURIComponent(repoPath)}`),
      api(`/api/sessions?repo=${encodeURIComponent(repoPath)}`).catch(() => ({ sessions: [] })),
    ]);
    checkpoints = Array.isArray(payload.checkpoints) ? payload.checkpoints : [];
    sessionTitles = new Map(
      (sessions.sessions || []).map((session) => [session.id, session.title]),
    );
  } catch (error) {
    container.append(emptyCheckpointNotice(error.message));
    return;
  }
  if (!checkpoints.length) {
    container.append(emptyCheckpointNotice("No checkpoints yet. One is taken before every turn."));
    return;
  }

  checkpoints.forEach((checkpoint) => {
    const row = document.createElement("div");
    row.className = "checkpoint-row";

    const details = document.createElement("div");
    const summary = document.createElement("p");
    summary.className = "checkpoint-row-summary";
    summary.textContent = checkpoint.summary;
    const meta = document.createElement("p");
    meta.className = "checkpoint-row-meta";
    const files = `${checkpoint.fileCount} ${checkpoint.fileCount === 1 ? "file" : "files"}`;
    const session = sessionTitles.get(checkpoint.sessionId);
    meta.textContent = [
      new Date(checkpoint.createdAtMs).toLocaleString(),
      files,
      session ? `in ${session}` : null,
      checkpoint.restoredAtMs ? "rewound to already" : null,
    ]
      .filter(Boolean)
      .join(" · ");
    details.append(summary, meta);

    // Coverage the checkpoint does not have is said out loud here too, not
    // only in the rewind dialog.
    const gaps = [
      checkpoint.commandEffectsCovered ? null : "command effects not covered",
      checkpoint.excludedCount
        ? `${checkpoint.excludedCount} path${
            checkpoint.excludedCount === 1 ? "" : "s"
          } excluded by policy`
        : null,
    ].filter(Boolean);
    if (gaps.length) {
      const warning = document.createElement("p");
      warning.className = "checkpoint-row-warning";
      warning.textContent = gaps.join(" · ");
      details.append(warning);
    }

    const action = document.createElement("button");
    action.type = "button";
    action.className = "checkpoint-row-action";
    action.textContent = "Rewind";
    action.addEventListener("click", async () => {
      action.disabled = true;
      try {
        await rewindToCheckpoint(checkpoint);
        await renderCheckpointList();
      } finally {
        action.disabled = false;
      }
    });

    row.append(details, action);
    container.append(row);
  });
}

function emptyCheckpointNotice(message) {
  const notice = document.createElement("p");
  notice.className = "checkpoint-list-empty";
  notice.textContent = message;
  return notice;
}

function markMessageRewindable(target, checkpoint) {
  const row = document.createElement("div");
  row.className = "turn-indicator";
  row.dataset.state = "rewindable";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.textContent = checkpoint.restoredAtMs
    ? "Rewound to once already"
    : `${checkpoint.fileCount} ${checkpoint.fileCount === 1 ? "file" : "files"} covered`;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "turn-indicator-action";
  button.textContent = "Rewind";
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      await rewindToCheckpoint(checkpoint);
    } finally {
      button.disabled = false;
    }
  });
  row.append(label, button);
  target.body.after(row);
}

async function rewindToCheckpoint(checkpoint) {
  const choice = await showRewindDialog(checkpoint);
  if (!choice) return;
  try {
    const result = await api(
      "/api/rewind",
      form({
        repo: requireRepo(),
        checkpoint_id: checkpoint.checkpointId,
        files: String(choice.files),
        conversation: String(choice.conversation),
        // One file is the fourth restore operation, not a fourth code path:
        // the same request with the file half narrowed to one path.
        path: choice.path || "",
      }),
    );
    if (result.conflictedFiles.length) {
      // Nothing was written: a partial restore with a conflict in the middle
      // would leave a tree that is neither the checkpoint nor what is there now.
      await noticeDialog(
        "Rewind stopped: files changed since the checkpoint",
        `${result.conflictedFiles.join(", ")} ${
          result.conflictedFiles.length === 1 ? "has" : "have"
        } changed since this turn ran, so nothing was restored. Resolve ${
          result.conflictedFiles.length === 1 ? "it" : "them"
        } by hand, or rewind the conversation only.`,
      );
      return;
    }
    await loadSession(currentSessionId);
    const restored = result.restoredFiles.length + result.deletedFiles.length;
    if (choice.path) {
      toast(restored ? `Restored ${choice.path}` : `Nothing to restore for ${choice.path}`);
      return;
    }
    toast(
      choice.files
        ? `Rewound ${restored} ${restored === 1 ? "file" : "files"}${
            choice.conversation ? " and the conversation" : ""
          }`
        : "Rewound the conversation",
    );
  } catch (error) {
    toast(error.message);
  }
}

// Files and conversation are independent switches, so the dialog offers all
// three useful combinations rather than making the user rewind twice.
function showRewindDialog(checkpoint) {
  return new Promise((resolve) => {
    const cleanupRef = { current: () => {} };
    const backdrop = document.createElement("div");
    backdrop.className = "app-dialog-backdrop";
    backdrop.innerHTML = `
      <div class="app-dialog app-dialog-wide app-dialog-rewind" role="dialog" aria-modal="true">
        <p class="app-dialog-title">Rewind this turn</p>
        <p class="app-dialog-message"></p>
        <div class="rewind-file-list"></div>
        <p class="app-dialog-footnote"></p>
        <div class="app-dialog-actions">
          <button type="button" class="app-dialog-btn app-dialog-cancel">Cancel</button>
          <button type="button" class="app-dialog-btn app-dialog-conversation">Conversation only</button>
          <button type="button" class="app-dialog-btn app-dialog-files">Files only</button>
          <button type="button" class="app-dialog-btn app-dialog-confirm">Files and conversation</button>
        </div>
      </div>
    `;
    const covered = `${checkpoint.fileCount} ${checkpoint.fileCount === 1 ? "file" : "files"}`;
    const excluded = checkpoint.excludedCount
      ? ` ${checkpoint.excludedCount} path${checkpoint.excludedCount === 1 ? " was" : "s were"} ` +
        "excluded by policy and will not be restored."
      : "";
    const uncovered = checkpoint.commandEffectsCovered
      ? ""
      : " Files changed by approved commands are not covered for this repository.";
    backdrop.querySelector(".app-dialog-message").textContent =
      `${checkpoint.summary}. Rewinding restores ${covered} Damaian changed in this turn, and ` +
      `moves the conversation back to just before it.${excluded}${uncovered}`;
    // Each covered file can go back on its own, for the common case of one
    // wrong file in an otherwise useful turn.
    const fileList = backdrop.querySelector(".rewind-file-list");
    const files = Array.isArray(checkpoint.files) ? checkpoint.files : [];
    files.forEach((file) => {
      const row = document.createElement("div");
      row.className = "rewind-file";
      const path = document.createElement("code");
      path.className = "rewind-file-path";
      path.textContent = file.path;
      const origin = document.createElement("span");
      origin.className = "rewind-file-origin";
      origin.textContent =
        file.origin === "command" ? "changed by a command" : "changed by a patch";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "rewind-file-action";
      button.textContent = "Restore this file";
      button.addEventListener("click", () =>
        cleanupRef.current({ files: true, conversation: false, path: file.path }),
      );
      row.append(path, origin, button);
      fileList.append(row);
    });
    fileList.hidden = files.length === 0;

    // Requirement 11: a user must not read this as a Git-like guarantee.
    backdrop.querySelector(".app-dialog-footnote").textContent =
      "Checkpoints cover Damaian's own changes to this repository. They are session recovery, " +
      "not version control, and are no substitute for a commit.";
    document.body.append(backdrop);

    const cleanup = (result) => {
      document.removeEventListener("keydown", onKeydown, true);
      backdrop.remove();
      resolve(result);
    };
    // The per-file rows are built before `cleanup` exists, so they reach it
    // through this rather than capturing an undefined binding.
    cleanupRef.current = cleanup;
    const onKeydown = (event) => {
      if (event.key === "Escape") cleanup(null);
    };
    backdrop
      .querySelector(".app-dialog-confirm")
      .addEventListener("click", () => cleanup({ files: true, conversation: true }));
    backdrop
      .querySelector(".app-dialog-files")
      .addEventListener("click", () => cleanup({ files: true, conversation: false }));
    backdrop
      .querySelector(".app-dialog-conversation")
      .addEventListener("click", () => cleanup({ files: false, conversation: true }));
    backdrop.querySelector(".app-dialog-cancel").addEventListener("click", () => cleanup(null));
    document.addEventListener("keydown", onKeydown, true);
    backdrop.querySelector(".app-dialog-confirm").focus();
  });
}

function markMessageStopped(target) {
  const row = document.createElement("p");
  row.className = "turn-indicator";
  row.dataset.state = "stopped";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.textContent = "Stopped by you";
  row.append(label);
  target.body.after(row);
}

function markMessageToolBudgetExhausted(target, sessionId = currentSessionId) {
  if (target.body.nextElementSibling?.dataset?.state === "incomplete") return;
  const row = document.createElement("div");
  row.className = "turn-indicator";
  row.dataset.state = "incomplete";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.textContent = "Tool budget exhausted";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "turn-indicator-action";
  button.textContent = "Continue debugging";
  button.addEventListener("click", async () => {
    try {
      button.disabled = true;
      await continueDebuggingFromExhausted(sessionId);
    } catch (error) {
      button.disabled = false;
      toast(error.message);
    }
  });
  row.append(label, button);
  target.body.after(row);
}

function chatCompletionStatus(payload) {
  if (payload.taskStatus === "tool_budget_exhausted") {
    return { label: "Tool budget exhausted", tone: "warn", indicator: "incomplete" };
  }
  if (payload.incomplete) {
    return { label: "Incomplete", tone: "warn", indicator: "incomplete" };
  }
  return { label: "Complete", tone: "ok", indicator: "complete" };
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
        sessions.forEach((session) => {
          sessionsList.append(renderProjectSession(projectPath, session));
        });
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

function _renderSessionList() {
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
  if (!title?.trim()) return;
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
  const lines = String(diff || "")
    .replace(/\n$/, "")
    .split(/\r?\n/);
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
  setRepoState(
    payload.clean
      ? projectName(repoPath)
      : `${projectName(repoPath)} - ${payload.files.length} changed`,
  );
  appendChatMessage("system", renderGitStatusText(payload));
}

// Appends whatever proposals a finished chat turn carries. Shared by every
// `done` handler: a turn can end with a command awaiting approval, a patch
// awaiting review, or neither, and each handler must treat all of them the
// same. Keeping this in one place is deliberate — the resume-after-approval
// handler previously rendered only `commandProposal`, so a patch proposed
// after the user approved a command was silently dropped and could never be
// reviewed.
function appendProposals(message, payload, repo) {
  if (payload.commandProposal) {
    message.body.append(createCommandApprovalPreview(payload.commandProposal, repo));
  }
  if (payload.patchProposal) {
    message.body.append(createPatchPreview(payload.patchProposal, repo));
  }
}

function createPatchPreview(payload, patchRepo) {
  const state = {
    patchId: payload.patchId,
    initialFileCount: (payload.files || []).length,
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

  // Shown when the secret scanner flags a selected file. The apply is not
  // refused outright: the user sees what was found and decides.
  const secretNotice = document.createElement("div");
  secretNotice.className = "patch-secret-notice";
  secretNotice.hidden = true;

  const list = document.createElement("div");
  list.className = "diff-list";
  wrapper.append(header, actions, secretNotice, list);

  function clearSecretNotice() {
    secretNotice.hidden = true;
    secretNotice.innerHTML = "";
  }

  function showSecretNotice(warnings, paths) {
    secretNotice.innerHTML = "";

    const title = document.createElement("strong");
    title.textContent =
      warnings.length === 1
        ? "1 file may contain a hardcoded secret"
        : `${warnings.length} files may contain a hardcoded secret`;

    const explanation = document.createElement("p");
    explanation.textContent =
      "Detection is not perfect — setup instructions and placeholder values can look like credentials. Review the diff below, then choose.";

    const findings = document.createElement("ul");
    warnings.forEach((warning) => {
      const item = document.createElement("li");
      const path = document.createElement("code");
      path.textContent = warning.path;
      const detail = document.createElement("span");
      const categories = (warning.categories || []).join(", ");
      detail.textContent = ` — ${warning.count} match${warning.count === 1 ? "" : "es"}${
        categories ? ` (${categories})` : ""
      }`;
      item.append(path, detail);
      findings.append(item);
    });

    const noticeActions = document.createElement("div");
    noticeActions.className = "inline-actions";
    const acceptButton = document.createElement("button");
    acceptButton.type = "button";
    acceptButton.className = "patch-secret-accept";
    acceptButton.textContent = "Apply Anyway";
    const cancelButton = document.createElement("button");
    cancelButton.type = "button";
    cancelButton.textContent = "Cancel";
    noticeActions.append(acceptButton, cancelButton);

    acceptButton.addEventListener("click", () => {
      clearSecretNotice();
      // Re-apply exactly the same selection, now carrying the user's explicit
      // consent. The override is per-apply and is not remembered.
      runApply(paths, true);
    });
    cancelButton.addEventListener("click", () => {
      clearSecretNotice();
      render();
    });

    secretNotice.append(title, explanation, findings, noticeActions);
    secretNotice.hidden = false;
  }

  function selectedPendingPaths() {
    return state.files
      .filter((file) => file.state === "pending" && file.selected)
      .map((file) => file.path);
  }

  function markFiles(paths, nextState) {
    if (nextState === "applied") {
      const appliedPaths = new Set(paths);
      state.files = state.files.filter((file) => !appliedPaths.has(file.path));
      render();
      return;
    }

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
      empty.textContent = state.initialFileCount
        ? "No patch files left to review."
        : "No patch files returned.";
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
            (result.warnings || []).forEach((warning) => {
              toast(warning);
            });
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

  async function runApply(paths, allowSecrets) {
    try {
      applyButton.disabled = true;
      const hunkSelection = {};
      state.files.forEach((file) => {
        if (paths.includes(file.path) && file.hunks.length) {
          hunkSelection[file.path] = file.hunks
            .filter((hunk) => hunk.selected)
            .map((hunk) => hunk.id);
        }
      });
      const fields = {
        repo: patchRepo,
        patch_id: state.patchId,
        paths: paths.join("\n"),
        hunk_selection: JSON.stringify(hunkSelection),
      };
      if (allowSecrets) fields.allow_secrets = "1";
      const result = await api("/api/apply-patch", form(fields));

      // Nothing was written; the server is asking whether to go ahead.
      const blocked = result.blockedBySecrets || [];
      if (blocked.length) {
        showSecretNotice(blocked, paths);
        applyButton.disabled = false;
        return;
      }

      const applied = result.appliedFiles || [];
      markFiles(applied, "applied");
      toast(
        allowSecrets
          ? `Applied ${applied.length} file(s) despite secret warning`
          : `Applied ${applied.length} file(s)`,
      );
      if (applied.length) {
        await appendGitStatusAfterChange(patchRepo).catch((error) => {
          toast(`Status unavailable: ${error.message}`);
        });
      }
    } catch (error) {
      toast(error.message);
      render();
    }
  }

  applyButton.addEventListener("click", () => {
    clearSecretNotice();
    const paths = selectedPendingPaths();
    if (!paths.length) {
      toast("No pending patch files selected");
      return;
    }
    runApply(paths, false);
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
  const isBrowserDiagnostic =
    proposal.allowBrowserDiagnosticsForSession || (proposal.risk || "").startsWith("browser");
  const wrapper = document.createElement("div");
  wrapper.className = "command-approval";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = isBrowserDiagnostic ? "Browser diagnostic approval" : "Command approval";
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
  runButton.textContent = proposal.blocked
    ? "Blocked"
    : isBrowserDiagnostic
      ? "Approve Once"
      : "Approve Run";
  runButton.disabled = Boolean(proposal.blocked);
  // The server decides eligibility (blocked, shell-control syntax, and
  // `require_approval_for_all_commands` all rule it out) so the button is
  // never shown for a command the policy would refuse to allowlist.
  const alwaysButton = document.createElement("button");
  alwaysButton.type = "button";
  alwaysButton.textContent = "Allow Always";
  alwaysButton.title = "Run this command and add it to this project's allowlist so it stops asking";
  const browserSessionButton = document.createElement("button");
  browserSessionButton.type = "button";
  browserSessionButton.textContent = "Allow browser diagnostics for this session";
  browserSessionButton.title =
    "Run this diagnostic and allow browser diagnostics in the current chat session";
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.textContent = "Reject";
  actions.append(runButton);
  if (proposal.allowAlways) actions.append(alwaysButton);
  if (proposal.allowBrowserDiagnosticsForSession) actions.append(browserSessionButton);
  actions.append(rejectButton);

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  // Approving or rejecting resumes the chat turn that raised this proposal:
  // the model sees the command's result (or the rejection) and streams back
  // an actual answer, same as a normal chat reply.
  async function resolveCommandProposal(approved, options = {}) {
    const always = options.always === true;
    const allowBrowserDiagnosticsForSession = options.allowBrowserDiagnosticsForSession === true;
    runButton.disabled = true;
    alwaysButton.disabled = true;
    browserSessionButton.disabled = true;
    rejectButton.disabled = true;
    output.hidden = false;
    output.textContent = approved
      ? isBrowserDiagnostic
        ? "Running diagnostic…"
        : "Running…"
      : "Rejecting…";

    const assistantMessage = appendChatMessage("assistant", "");
    let assistantText = "";
    let streamError = null;
    await streamResumeCommandRequest(
      {
        repo: proposalRepo,
        proposal_id: proposal.proposalId,
        approved: approved ? "true" : "false",
        always: always ? "true" : "false",
        allow_browser_diagnostics_for_session: allowBrowserDiagnosticsForSession ? "true" : "false",
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
          appendProposals(assistantMessage, payload, proposalRepo);
          if (payload.sessionId) {
            currentSessionId = payload.sessionId;
            localStorage.setItem(lastSessionStorageKey(), currentSessionId);
          }
          renderContextFiles(payload.contextFiles || []);
          const status = chatCompletionStatus(payload);
          setChatStatus(status.label, status.tone);
        },
        error(payload) {
          streamError = new Error(payload.error || "Command resume failed");
        },
      },
    );
    if (streamError) throw streamError;
    if (!approved) {
      output.textContent = isBrowserDiagnostic
        ? "Diagnostic rejected — see the assistant's answer above."
        : "Command rejected — see the assistant's answer above.";
    } else if (allowBrowserDiagnosticsForSession) {
      output.textContent =
        "Browser diagnostics allowed for this session — see the assistant's answer above.";
    } else if (always) {
      output.textContent =
        "Command approved and added to this project's allowlist — see the assistant's answer above.";
    } else {
      output.textContent = isBrowserDiagnostic
        ? "Diagnostic approved — see the assistant's answer above."
        : "Command approved — see the assistant's answer above.";
    }
    await loadSessions(currentSessionId, false);
  }

  // Re-enable after a failure so the user can retry or pick a different
  // action; a blocked proposal's run button stays disabled regardless.
  function restoreActions() {
    runButton.disabled = Boolean(proposal.blocked);
    alwaysButton.disabled = false;
    browserSessionButton.disabled = false;
    rejectButton.disabled = false;
  }

  runButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(true);
      toast("Command completed");
    } catch (error) {
      restoreActions();
      output.textContent = error.message;
      toast(error.message);
    }
  });

  alwaysButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(true, { always: true });
      toast("Command allowed for this project");
    } catch (error) {
      restoreActions();
      output.textContent = error.message;
      toast(error.message);
    }
  });

  browserSessionButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(true, { allowBrowserDiagnosticsForSession: true });
      toast("Browser diagnostics allowed for this session");
    } catch (error) {
      restoreActions();
      output.textContent = error.message;
      toast(error.message);
    }
  });

  rejectButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(false);
      toast("Command rejected");
    } catch (error) {
      restoreActions();
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
    sessionCheckpoints = [];
    clearChat();
    return;
  }
  const payload = await api(`/api/session?session_id=${encodeURIComponent(sessionId)}`);
  currentSessionId = payload.session.id;
  localStorage.setItem(lastSessionStorageKey(), currentSessionId);
  $("session-select").value = currentSessionId;
  syncSessionListActive();
  loadPinnedContextFiles(currentSessionId);
  await loadSessionCheckpoints(currentSessionId);
  renderMessages(payload.messages, payload.tasks || []);
  renderContextFiles();
  setChatStatus("Loaded");
}

async function streamChatRequest(data, handlers, signal) {
  return streamRequest("/api/ask-stream", "/api/ask", data, handlers, signal);
}

async function streamResumeCommandRequest(data, handlers) {
  const fallbackPath = data.approved === "true" ? "/api/run-command" : "/api/reject-command";
  return streamRequest("/api/resume-command-stream", fallbackPath, data, handlers);
}

async function streamRequest(streamPath, fallbackPath, data, handlers, signal) {
  const options = withApiToken(streamPath, form(data));
  // Aborting closes the socket, which is what the server notices on its next
  // keepalive write. No cancel request is sent — and none could be, since the
  // shell serves one request at a time and this turn is holding it.
  const response = await fetch(apiUrl(streamPath), { ...options, signal });
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
  if (event === "session" && handlers.session) handlers.session(payload.sessionId || "");
  if (event === "phase" && handlers.phase) handlers.phase(payload);
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
  if (term) {
    term.clear();
    term.focus();
  }
});

$("terminal-new-btn").addEventListener("click", async () => {
  try {
    await restartTerminal();
  } catch (error) {
    toast(error.message);
  }
});

// Tear the shell down when the window goes away (the pty is also reaped by
// SIGHUP when its master is dropped on app exit, so this is best-effort).
window.addEventListener("beforeunload", () => {
  if (!termId) return;
  const invoke = tauriInvoke();
  if (invoke) void invoke("terminal_close", { id: termId });
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
  setChatMessagePlaceholder(assistantMessage, "Generating a patch preview...");
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

// The turn currently in flight, so Stop (button or Escape) can reach its
// AbortController. Null whenever nothing is running, which is what keeps a late
// Stop from aborting the *next* turn.
let currentTurn = null;

function stopCurrentTurn() {
  if (!currentTurn) return;
  currentTurn.stopped = true;
  currentTurn.controller.abort();
}

function setComposerBusy(busy) {
  const button = $("ask-btn");
  button.classList.toggle("is-stopping", busy);
  button.setAttribute("aria-label", busy ? "Stop generating" : "Send message");
  // Deliberately left enabled while busy: it is the Stop control now.
  button.disabled = false;
}

// Puts a prompt back after a stop or a failure, unless the user has started
// typing something new. Either way the original is in the session history.
function restorePrompt(text) {
  const field = $("chat-prompt");
  if (field.value.trim()) return;
  field.value = text;
}

async function sendChatPrompt(options = {}) {
  let streamError = null;
  // Hoisted out of the `try` so the `catch` can write the failure into the
  // bubble this turn already added to the log.
  let assistantMessage = null;
  let indicator = null;
  let prompt = "";
  const promptOverride = (options.prompt || "").trim();
  const restoreSubmittedPrompt = options.restorePrompt !== false && !promptOverride;
  if (chatSubmitting) return;
  try {
    prompt = promptOverride || $("chat-prompt").value.trim();
    if (!prompt) throw new Error("Prompt is required");
    chatSubmitting = true;
    setComposerBusy(true);
    await ensureDesktopApiReady();
    const chatRepo = requireRepo();
    const userMessage = appendChatMessage("user", prompt);
    assistantMessage = appendChatMessage("assistant", "");
    // Cleared here rather than after success: the prompt is already echoed in
    // the log above, and leaving it in the box makes it look unsent.
    $("chat-prompt").value = "";
    if (looksLikeEditRequest(prompt)) {
      await proposePatchFromChat(prompt, assistantMessage);
      dismissContextChips();
      return;
    }

    const controller = new AbortController();
    currentTurn = { controller, stopped: false };
    indicator = startTurnIndicator(assistantMessage, stopCurrentTurn);

    let assistantText = "";
    setChatStatus("Thinking", "running");

    await streamChatRequest(
      {
        repo: chatRepo,
        prompt,
        session_id: currentSessionId,
        context_files: pinnedContextFiles.join("\n"),
        continue_debugging: options.continueDebugging ? "true" : "false",
        ...chatModelFormFields(),
      },
      {
        // Arrives before any work starts, so a stopped turn is still
        // identifiable — without it, stopping a brand-new session's first turn
        // would leave a session the UI cannot name.
        session(sessionId) {
          if (sessionId) currentSessionId = sessionId;
        },
        phase(payload) {
          if (indicator) indicator.phase(payload);
        },
        token(token) {
          assistantText += token;
          updateChatMessage(assistantMessage, assistantText);
          if (indicator) indicator.streaming();
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
          appendProposals(assistantMessage, payload, chatRepo);
          renderContextFiles(payload.contextFiles || []);
          if (payload.cancelled) {
            if (indicator) indicator.finish("stopped");
            setChatStatus("Stopped", "warn");
            return;
          }
          const status = chatCompletionStatus(payload);
          if (indicator) indicator.finish(status.indicator);
          if (payload.taskStatus === "tool_budget_exhausted") {
            markMessageToolBudgetExhausted(assistantMessage, payload.sessionId);
          }
          setChatStatus(status.label, status.tone);
        },
        error(payload) {
          streamError = new Error(payload.error || "Model request failed");
        },
      },
      controller.signal,
    );
    if (streamError) throw streamError;
    dismissContextChips();
    // The checkpoint for this turn exists now, so the turn just sent becomes
    // rewindable without waiting for a session reload.
    await loadSessionCheckpoints(currentSessionId);
    const checkpoint = sessionCheckpoints.find(
      (entry) => entry.sessionId === currentSessionId && !entry.restoredAtMs,
    );
    if (checkpoint) markMessageRewindable(userMessage, checkpoint);
    await loadSessions(currentSessionId, false);
  } catch (error) {
    // A stop is not a failure. Without this every Stop would toast "Failed"
    // and write an error into the bubble the user just chose to keep.
    if (currentTurn?.stopped || error.name === "AbortError") {
      if (indicator) indicator.finish("stopped");
      setChatStatus("Stopped", "warn");
      if (restoreSubmittedPrompt) restorePrompt(prompt);
      await loadSessions(currentSessionId, false);
    } else {
      if (indicator) indicator.finish("failed");
      setChatStatus("Failed", "error");
      toast(error.message);
      renderChatMessageError(assistantMessage, error);
      if (restoreSubmittedPrompt) restorePrompt(prompt);
    }
  } finally {
    chatSubmitting = false;
    currentTurn = null;
    setComposerBusy(false);
  }
}

async function continueDebuggingFromExhausted(sessionId) {
  if (chatSubmitting) throw new Error("A turn is already running");
  if (!sessionId && !currentSessionId) throw new Error("No session selected");
  if (sessionId && sessionId !== currentSessionId) {
    await loadSession(sessionId);
  }
  await sendChatPrompt({
    prompt:
      "Continue debugging from the last tool-budget exhaustion. Use the prior session evidence and continue from the unresolved diagnostic state.",
    continueDebugging: true,
    restorePrompt: false,
  });
}

$("ask-btn").addEventListener("click", () => {
  if (chatSubmitting) {
    stopCurrentTurn();
    return;
  }
  void sendChatPrompt();
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || !chatSubmitting) return;
  // Escape already closes dialogs and popovers; those keep priority.
  if (document.querySelector(".app-dialog-backdrop, .model-popover:not([hidden])")) return;
  stopCurrentTurn();
});

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
    $("provider-key-ref-input").value =
      `keychain:${providerSlug($("provider-label-input").value)}-api-key`;
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

$("mcp-config-select").addEventListener("change", () => {
  renderMcpConfigForm($("mcp-config-select").value);
});

$("mcp-transport-select").addEventListener("change", updateMcpTransportFields);

$("mcp-new-btn").addEventListener("click", newMcpConfigForm);

$("mcp-label-input").addEventListener("input", () => {
  if (!$("mcp-id-input").value.trim()) {
    $("mcp-token-ref-input").value = `keychain:mcp-${mcpSlug($("mcp-label-input").value)}-token`;
  }
});

$("mcp-save-btn").addEventListener("click", async () => {
  try {
    await saveMcpServer();
    toast("MCP server saved");
  } catch (error) {
    toast(error.message);
  }
});

$("mcp-test-btn").addEventListener("click", async () => {
  try {
    await testMcpServer();
  } catch (error) {
    setMcpTestResult(error.message, "error");
    toast(error.message);
  }
});

$("mcp-remove-btn").addEventListener("click", async () => {
  try {
    const id = $("mcp-id-input").dataset.originalId || $("mcp-id-input").value;
    if (!id || !(await confirmDialog("Remove server?", `Remove MCP server ${id}?`))) return;
    await removeMcpServerFromSettings();
    toast("MCP server removed");
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
    if (!(await confirmDialog("Remove API key?", "Remove this stored API key from Keychain?")))
      return;
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
