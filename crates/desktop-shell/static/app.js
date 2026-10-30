const $ = (id) => document.getElementById(id);

let currentSessionId = "";
// The open session's working mode (spec 20), from `/api/session` or
// `/api/session-mode`. The session list carries no mode, so this is the only
// copy; `sessionId` says which session it belongs to.
let currentSessionMode = { sessionId: "", mode: "code" };
// Whether the open session has recorded browser diagnostics; see
// `appendWebDiagnosticArtifacts`.
let sessionHasRecordedDiagnostics = false;
let apiToken = "";
let bootstrapPromise = null;
let bootstrapError = null;
let chatSubmitting = false;
let pinnedContextFiles = [];
// Repository-relative root paths for the selected repository, newest map
// first. Used to qualify a basename or a patch file by the root that owns it
// (`context.md` §4, §13). Loaded from `/api/repository-map`.
let repositoryRoots = [];
let contextChipsDismissed = false;
// The newest manifest this page has seen (live `context` event, a done payload
// or `/api/session`). The panel renders this and nothing else.
let lastContextManifest = null;
// The task the panel is showing; `""` means the newest turn.
let contextPanelTaskId = "";
// The paused pre-send review, while one is open.
let currentContextReview = null;
// Review cards by proposal id. Removal is by manifest index, so a card that
// reopened the panel from the review it was created with would restore removed
// items and make the next removal hit the wrong one. Every card therefore keeps
// one live review object, and a removal refreshes it from the returned manifest.
const contextReviewCards = new Map();
// Session pins and restriction from `/api/session` or a control route.
let contextPins = [];
let contextRestriction = null;
// True while the restriction textarea holds text the user typed but has not
// applied. Re-rendering must not overwrite it with the stored restriction.
let contextRestrictionDirty = false;
// Memory statements are not in the manifest (spec 30 C3), so the panel asks
// the engine for them by id. Cached per manifest so re-rendering does not
// re-ask, and keyed by the ids so a removal or a new turn fetches again.
const memoryDisplayCache = new Map();
const memoryDisplayRequests = new Set();
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

const builtInProviderIds = ["openai", "deepseek", "openrouter", "openai-compatible"];
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
  openrouter: {
    label: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    apiKeyEnv: "OPENROUTER_API_KEY",
    defaultModel: "anthropic/claude-sonnet-5.5",
    models: [
      "anthropic/claude-sonnet-5.5",
      "anthropic/claude-opus-5.5",
      "anthropic/claude-haiku-5.5",
      "openai/gpt-6.1-sol",
      "google/gemini-3.8-flash",
      "deepseek/deepseek-v4.1-flash",
      "x-ai/grok-4.7",
      "moonshotai/kimi-k3.1-pro",
      "z-ai/glm-5.3",
      "qwen/qwen3.8-max-prime",
    ],
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
    id: "openrouter",
    label: "OpenRouter",
    description: "Many models through one API key",
  },
  {
    id: "openai-compatible",
    label: "OpenAI compatible",
    description: "Custom hosted compatible endpoint",
  },
  {
    id: "ollama-local",
    label: "Ollama (local)",
    description: "Local models on this Mac",
    baseUrl: "http://localhost:11434",
    apiKeyEnv: "keychain:ollama-local-api-key",
    models: ["llama3.1", "qwen2.5-coder"],
  },
  {
    id: "ollama-cloud",
    label: "Ollama Cloud",
    description: "Hosted models on ollama.com",
    baseUrl: "https://ollama.com",
    apiKeyEnv: "keychain:ollama-cloud-api-key",
    models: ["gpt-oss:120b", "qwen3-coder:480b", "deepseek-v3.1:671b"],
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

// The one icon set (style guide §3): 24x24 stroke glyphs that take the
// surrounding text colour. Path data is the agreed mockup's.
const ICON_PATHS = {
  code: ["M8 6 3 12l5 6", "m16 6 5 6-5 6"],
  terminal: ["m5 8 4 4-4 4", "M12 17h7"],
  paperclip: [
    "m20 11-8.5 8.5a5 5 0 0 1-7-7L13 4a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4L15 7",
  ],
  folderPlus: ["M3 7h6l2 2h10v10H3z", "M12 12v5M9.5 14.5h5"],
  compose: ["M12 20h8", "M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"],
};

function icon(name) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", "icon");
  (ICON_PATHS[name] || []).forEach((d) => {
    const path = document.createElementNS(ns, "path");
    path.setAttribute("d", d);
    svg.append(path);
  });
  return svg;
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
    // The commit dialog branches on `code` (a rewritten message, a failed
    // hook, HEAD moving) instead of parsing prose.
    const error = new Error(payload.error || response.statusText);
    error.payload = payload;
    throw error;
  }
  // An answer from the API is proof it is available, whichever way the token
  // arrived.
  setDesktopApiBanner(null);
  return payload;
}

// "Desktop API unavailable" is the app failing, not a turn, so it is a
// persistent banner above the conversation rather than a turn mark (spec 58
// §3.2). `null` clears it.
function setDesktopApiBanner(message) {
  const banner = $("desktop-api-banner");
  if (!banner) return;
  banner.textContent = message || "";
  banner.hidden = !message;
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
      setDesktopApiBanner(null);
      if (!chatSubmitting) $("ask-btn").disabled = false;
      // Before the first session renders, so its messages are drawn once with
      // the setting already known.
      await refreshWorkersEnabled();
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
      setDesktopApiBanner(`Desktop API unavailable: ${error.message}`);
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
  renderThreadHeader();
}

// The thread header names the open session; nothing else in it is state (spec
// 58 §3.2). The folder it used to show above the title moves to the composer's
// repo bar.
//
// Driven off `setRepoState`, `syncSessionListActive` and `loadSessions`, which
// between them fire on every project switch, session switch, load and rename.
function renderThreadHeader() {
  const repoPath = repo();
  const sessionLine = $("thread-session");
  renderModeControl();
  // From the last status, no request: this only keeps the folder's display
  // name (a project rename) and the bar's visibility current.
  renderRepoBar();
  renderWorkerHeaderMeta();

  if (!repoPath) {
    sessionLine.textContent = "";
    sessionLine.removeAttribute("title");
    return;
  }

  if (workersBoardProject) {
    sessionLine.textContent = `Workers · ${projectName(workersBoardProject)}`;
    sessionLine.title = workersBoardProject;
    return;
  }

  // A worker opened from its parent is not in this project's session list:
  // its sessions are keyed by its worktree (spec 57 §5.7).
  const session = currentSession() || workerRowFor(currentSessionId);
  sessionLine.textContent = session ? session.title : "New session";
  if (session) sessionLine.title = session.title;
  else sessionLine.removeAttribute("title");
}

// The open session's list entry, or null for a new session or no folder.
function currentSession() {
  const repoPath = repo();
  if (!repoPath || !currentSessionId) return null;
  return (
    (projectSessionsByPath.get(repoPath) || []).find((entry) => entry.id === currentSessionId) ||
    null
  );
}

const sessionModeLabels = { ask: "Ask", plan: "Plan", code: "Code", review: "Review" };

// The mode the pill shows: the last one the server confirmed for the open
// session, or Code when there is none.
function displayedSessionMode() {
  const hasSession = Boolean(repo() && currentSessionId);
  return hasSession && currentSessionMode.sessionId === currentSessionId
    ? currentSessionMode.mode
    : "code";
}

// A mode belongs to a session that exists, so the pill is disabled until the
// first message creates one; that session starts in Code, which is what the
// disabled pill shows. Also how a failed switch snaps back: the pill is
// redrawn from the last mode the server confirmed.
function renderModeControl() {
  const button = $("session-mode-btn");
  const hasSession = Boolean(repo() && currentSessionId);
  const mode = displayedSessionMode();
  const label = sessionModeLabels[mode] || mode;
  button.disabled = !hasSession;
  button.dataset.mode = mode;
  $("session-mode-label").textContent = label;
  button.setAttribute("aria-label", `Working mode: ${label}`);
  if (!hasSession) sessionModeMenu.close();
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
      <pre class="app-dialog-detail" tabindex="0" hidden></pre>
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
  detail = "",
  inputValue = null,
  confirmLabel = "OK",
  danger = false,
  dismissOnly = false,
}) {
  return new Promise((resolve) => {
    const backdrop = ensureAppDialog();
    const dialog = backdrop.querySelector(".app-dialog");
    const titleEl = backdrop.querySelector(".app-dialog-title");
    const messageEl = backdrop.querySelector(".app-dialog-message");
    const detailEl = backdrop.querySelector(".app-dialog-detail");
    const inputEl = backdrop.querySelector(".app-dialog-input");
    const confirmBtn = backdrop.querySelector(".app-dialog-confirm");
    const cancelBtn = backdrop.querySelector(".app-dialog-cancel");
    const usesInput = inputValue !== null;

    titleEl.textContent = title;
    messageEl.hidden = !message;
    messageEl.textContent = message;
    // A detail is content the user must review before deciding — the exact
    // repair prompt, for example. It widens the dialog and scrolls; the dialog
    // itself never grows with it.
    detailEl.hidden = !detail;
    detailEl.textContent = detail;
    dialog.classList.toggle("app-dialog-preview", Boolean(detail));
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
  // A key Damaian could not parse is almost always a typo in the repository's
  // own config, not an attempt on the user's settings. Giving it the sentence
  // below would be untrue and would train the user to click through the notice
  // that does matter, so the two get separate dialogs.
  const refused = rejected.filter((item) => item.class !== "unparsable");
  const unparsable = rejected.filter((item) => item.class === "unparsable");
  if (refused.length) {
    await noticeDialog(
      "This repository tried to change Damaian's settings",
      `${projectName(path)} ships a .damaian/config.conf that sets ${refused
        .map((item) => item.key)
        .join(", ")}. Damaian ignored ${
        refused.length === 1 ? "that key" : "those keys"
      }: a repository cannot change where commands run, where model traffic goes, where your ` +
        "data is written, or which approvals you see. Its other settings were applied.",
    );
  }
  if (unparsable.length) {
    await noticeDialog(
      "This repository has a setting Damaian could not read",
      `${projectName(path)} ships a .damaian/config.conf Damaian could not read in full: ${unparsable
        .map((item) => item.key)
        .join(", ")}. Damaian skipped ${
        unparsable.length === 1 ? "that line" : "those lines"
      } and applied the rest, so nothing here is broken — it is most likely a typo in the ` +
        "repository's own config, worth reporting to whoever maintains it.",
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
  // Spec 32: a repository-defined hook stays inactive until the user approves
  // the exact definition shown here. Declining is the default action, and a
  // dismissal asks again next time rather than recording either decision.
  const hooks = Array.isArray(review.hooks) ? review.hooks : [];
  for (const hook of hooks) {
    const decision = await showHookReviewDialog(path, hook);
    if (decision === null) {
      reviewedRepositoryConfigs.delete(path);
      break;
    }
    try {
      await api(
        "/api/hook-review",
        form({ repo: path, hook: hook.id, decision, hash: hook.hash || "" }),
      );
      toast(decision === "approve" ? `Approved hook ${hook.id}` : `Declined hook ${hook.id}`);
    } catch (error) {
      reviewedRepositoryConfigs.delete(path);
      toast(error.message);
      break;
    }
  }
}

// One pending repository hook, with its full command line in the consent
// surface (UI style guide §1: consent content is never truncated). Every
// hook-supplied string is text, never markup.
function showHookReviewDialog(repoPath, hook) {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "app-dialog-backdrop";
    backdrop.innerHTML = `
      <div class="app-dialog app-dialog-wide app-dialog-hook-review" role="dialog" aria-modal="true">
        <p class="app-dialog-title"></p>
        <p class="app-dialog-message"></p>
        <p class="hook-review-label">Command</p>
        <pre class="hook-review-command"></pre>
        <p class="hook-review-meta"></p>
        <div class="app-dialog-actions">
          <button type="button" class="app-dialog-btn app-dialog-decline">Decline</button>
          <button type="button" class="app-dialog-btn app-dialog-approve">Approve</button>
        </div>
      </div>
    `;
    const changed = hook.activation === "changed_since_review";
    backdrop.querySelector(".app-dialog-title").textContent = changed
      ? "A hook this repository defines has changed"
      : "This repository defines a hook";
    backdrop.querySelector(".app-dialog-message").textContent = changed
      ? `${projectName(repoPath)} ships a different definition of the hook "${hook.id}" from the one you approved. It is not running until you review the new definition. If it changes again, it stops again.`
      : `${projectName(repoPath)} ships a hook named "${hook.id}". Damaian will run it only after you approve this exact definition. If it changes later, it stops running until you review it again.`;
    const command = hook.command + ((hook.args || []).length ? ` ${hook.args.join(" ")}` : "");
    backdrop.querySelector(".hook-review-command").textContent = command;
    const failure = hook.mode === "mandatory" ? "blocks" : "warns";
    backdrop.querySelector(".hook-review-meta").textContent = `Events: ${(hook.events || []).join(
      ", ",
    )} · If it fails: ${failure}`;

    document.body.append(backdrop);

    const cleanup = (result) => {
      document.removeEventListener("keydown", onKeydown, true);
      backdrop.remove();
      resolve(result);
    };
    const onKeydown = (event) => {
      if (event.key === "Escape") cleanup(null);
    };
    document.addEventListener("keydown", onKeydown, true);
    backdrop.querySelector(".app-dialog-approve").addEventListener("click", () => {
      if (!hook.hash) {
        toast("This hook's definition could not be read, so it cannot be approved");
        return;
      }
      cleanup("approve");
    });
    backdrop
      .querySelector(".app-dialog-decline")
      .addEventListener("click", () => cleanup("decline"));
    // Decline is the default: it takes focus, and Approve is never the filled
    // or pre-selected choice (UI style guide §4, §5).
    backdrop.querySelector(".app-dialog-decline").focus();
  });
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
  }
  renderProjectList();
}

// The one ⋮ menu component (spec 58 §3.1). Every overflow menu is an instance:
// a single `position: fixed` popover on `document.body`, anchored to its
// trigger's rect rather than to a CSS ancestor, because triggers live inside
// scrolling lists (sidebar rows, `#chat-log`) and are often re-rendered while
// the menu is open.
//
// `sections` is an array of arrays of entries; a divider is drawn between
// non-empty sections. An entry is
//   { label, action?, hint?, danger?, hidden?, disabled?, className?,
//     onSelect?(context), items? }
// where `items` (itself sections) makes the row drill into a panel with a back
// row, and `onSelect` wins over the menu's `onAction(action, context)`.
// Danger rows are moved to the end of their panel, whatever order they were
// given in, so "danger last" cannot drift per menu.
const openContextMenus = new Set();

function closeAllContextMenus(options = {}) {
  for (const menu of [...openContextMenus]) menu.close(options);
}

function createContextMenu({ className = "", align = "end", placement = "below", onAction } = {}) {
  let el = null;
  let anchor = null;
  let openKey = null;
  let context = null;
  let rootPanel = null;

  const menu = { toggle, open, close, isOpen: () => openContextMenus.has(menu) };

  function ensureEl() {
    if (el) return el;
    el = document.createElement("div");
    el.className = `context-menu-popover ${className}`.trim();
    el.setAttribute("role", "menu");
    el.hidden = true;
    // Clicks inside (drill-in, back) must not reach the document handler that
    // closes every open menu.
    el.addEventListener("click", (event) => event.stopPropagation());
    el.addEventListener("keydown", handleKeydown);
    document.body.append(el);
    return el;
  }

  function orderedEntries(sections) {
    const flat = [];
    sections.forEach((section) => {
      const visible = section.filter((entry) => !entry.hidden);
      if (!visible.length) return;
      if (flat.length) flat.push({ divider: true });
      flat.push(...visible);
    });
    const ordered = [
      ...flat.filter((entry) => !entry.danger),
      ...flat.filter((entry) => entry.danger),
    ];
    // Moving a danger row can leave a divider leading, trailing or doubled.
    return ordered.filter(
      (entry, index) =>
        !entry.divider || (index > 0 && index < ordered.length - 1 && !ordered[index - 1].divider),
    );
  }

  function buildPanel(sections, back) {
    const panel = document.createElement("div");
    panel.className = "context-menu-panel";
    panel.hidden = true;
    if (back) {
      const backRow = document.createElement("button");
      backRow.type = "button";
      backRow.className = "context-menu-back";
      backRow.dataset.action = "back";
      backRow.setAttribute("role", "menuitem");
      backRow.textContent = back.label;
      backRow.addEventListener("click", () => showPanel(back.parent, back.opener));
      panel.append(backRow);
    }
    orderedEntries(sections).forEach((entry) => {
      if (entry.divider) {
        const divider = document.createElement("div");
        divider.className = "context-menu-divider";
        divider.setAttribute("role", "separator");
        panel.append(divider);
        return;
      }
      const row = document.createElement("button");
      row.type = "button";
      row.className = [
        "context-menu-row",
        entry.danger && "context-menu-row-danger",
        entry.className,
      ]
        .filter(Boolean)
        .join(" ");
      row.setAttribute("role", "menuitem");
      if (entry.action) row.dataset.action = entry.action;
      row.disabled = entry.disabled === true;
      const label = document.createElement("span");
      label.textContent = entry.label;
      row.append(label);
      if (entry.hint) {
        const hint = document.createElement("small");
        hint.textContent = entry.hint;
        row.append(hint);
      }
      if (entry.items) {
        const caret = document.createElement("span");
        caret.className = "context-menu-caret";
        caret.setAttribute("aria-hidden", "true");
        row.append(caret);
        row.setAttribute("aria-haspopup", "menu");
        const subpanel = buildPanel(entry.items, {
          label: entry.label,
          parent: panel,
          opener: row,
        });
        row.addEventListener("click", () => showPanel(subpanel));
      } else {
        row.addEventListener("click", () => select(entry));
      }
      panel.append(row);
    });
    el.append(panel);
    return panel;
  }

  function currentPanel() {
    return el ? [...el.children].find((panel) => !panel.hidden) : null;
  }

  function focusableRows(panel) {
    return panel ? [...panel.querySelectorAll("button:not(:disabled)")] : [];
  }

  // Drilling in focuses the panel's first real row; going back focuses the row
  // that drilled in, so a keyboard user lands where they left.
  function showPanel(panel, focusTarget) {
    [...el.children].forEach((child) => {
      child.hidden = child !== panel;
    });
    position();
    const rows = focusableRows(panel);
    const target =
      focusTarget || rows.find((row) => !row.classList.contains("context-menu-back")) || rows[0];
    target?.focus();
  }

  function select(entry) {
    const selectedContext = context;
    close({ restoreFocus: true });
    if (entry.onSelect) {
      entry.onSelect(selectedContext);
    } else if (onAction && entry.action) {
      onAction(entry.action, selectedContext);
    }
  }

  function handleKeydown(event) {
    const panel = currentPanel();
    const rows = focusableRows(panel);
    const index = rows.indexOf(document.activeElement);
    const focusAt = (next) => rows[(next + rows.length) % rows.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        focusAt(index + 1);
        break;
      case "ArrowUp":
        focusAt(index < 0 ? rows.length - 1 : index - 1);
        break;
      case "Home":
        focusAt(0);
        break;
      case "End":
        focusAt(rows.length - 1);
        break;
      case "ArrowRight":
        if (document.activeElement?.getAttribute("aria-haspopup") === "menu") {
          document.activeElement.click();
        }
        break;
      case "ArrowLeft":
        panel?.querySelector(".context-menu-back")?.click();
        break;
      case "Escape":
      case "Tab":
        close({ restoreFocus: true });
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  // Below the trigger, flipping above when the space below cannot hold the
  // menu and the space above is larger; `placement: "above"` prefers above
  // (the repo bar sits at the bottom of the window). `align: "end"` lines the
  // menu's right edge up with the trigger's, `"start"` its left edge.
  function position() {
    if (!anchor || el.hidden) return;
    const rect = anchor.getBoundingClientRect();
    const width = el.offsetWidth;
    const height = el.offsetHeight;
    const gap = 4;
    const margin = 8;
    const clamp = (value, max) => Math.max(margin, Math.min(value, max));
    const roomBelow = window.innerHeight - rect.bottom - gap - margin;
    const roomAbove = rect.top - gap - margin;
    const above =
      placement === "above"
        ? height <= roomAbove || roomAbove > roomBelow
        : height > roomBelow && roomAbove > roomBelow;
    const left = align === "start" ? rect.left : rect.right - width;
    const top = above ? rect.top - gap - height : rect.bottom + gap;
    el.style.left = `${clamp(left, window.innerWidth - width - margin)}px`;
    el.style.top = `${clamp(top, window.innerHeight - height - margin)}px`;
    el.dataset.placement = above ? "above" : "below";
  }

  // `key` identifies what the menu is open for (a project path, a proposal),
  // so clicking the same trigger again closes it even after a re-render
  // replaced the trigger element.
  function open(anchorEl, { sections, key = anchorEl, context: menuContext = null }) {
    closeAllContextMenus();
    ensureEl();
    el.replaceChildren();
    anchor = anchorEl;
    openKey = key;
    context = menuContext;
    rootPanel = buildPanel(sections, null);
    el.hidden = false;
    openContextMenus.add(menu);
    anchor.setAttribute("aria-expanded", "true");
    showPanel(rootPanel);
  }

  function toggle(anchorEl, options) {
    if (menu.isOpen() && openKey === (options.key ?? anchorEl)) {
      close({ restoreFocus: true });
      return;
    }
    open(anchorEl, options);
  }

  function close({ restoreFocus = false } = {}) {
    if (!el || !menu.isOpen()) return;
    el.hidden = true;
    openContextMenus.delete(menu);
    anchor?.setAttribute("aria-expanded", "false");
    if (restoreFocus && anchor?.isConnected) anchor.focus();
    anchor = null;
    openKey = null;
    context = null;
  }

  return menu;
}

document.addEventListener("click", () => closeAllContextMenus());
// Focus is normally inside the menu and its own handler wins; this catches an
// Escape while focus sits elsewhere (the trigger, after a mouse open).
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && openContextMenus.size) {
    closeAllContextMenus({ restoreFocus: true });
  }
});

// The project row menu. Rows are fully re-rendered on every
// `renderProjectList()`, so the project path, not the button, keys the menu.
const projectMenu = createContextMenu({
  onAction: (action, projectPath) => void handleProjectMenuAction(action, projectPath),
});

function toggleProjectMenu(projectPath, anchorEl) {
  projectMenu.toggle(anchorEl, {
    key: projectPath,
    context: projectPath,
    sections: [
      [
        {
          label: "Open in",
          action: "open-in",
          items: [
            [
              { label: "VS Code", action: "open-vscode" },
              { label: "Finder", action: "open-finder" },
            ],
          ],
        },
        { label: "Rename", action: "rename" },
        { label: "Delete", action: "delete", danger: true },
      ],
      // §5.1 Entry points: from the sidebar, empty and without a parent.
      [
        { label: "New worker", action: "new-worker", hidden: !workersEnabled },
        { label: "Workers board", action: "workers-board", hidden: !workersEnabled },
      ],
    ],
  });
}

async function handleProjectMenuAction(action, projectPath) {
  if (!projectPath) return;
  if (action === "rename") {
    await renameProject(projectPath);
    return;
  }
  if (action === "delete") {
    await forgetProject(projectPath);
    return;
  }
  if (action === "new-worker") {
    try {
      // The dialog reads the open folder, so the project opens first.
      if (normalizeProjectPath(projectPath) !== repo()) {
        expandedProjectPaths.add(normalizeProjectPath(projectPath));
        saveProjectState();
        await switchProject(projectPath);
      }
      const started = await showNewWorkerDialog({});
      if (started) refreshWorkersAfterAction(projectPath);
    } catch (error) {
      toast(error.message);
    }
    return;
  }
  if (action === "workers-board") {
    void openWorkersBoard(projectPath).catch((error) => toast(error.message));
    return;
  }
  if (action === "open-vscode") {
    try {
      const payload = await api("/api/open-vscode", form({ repo: projectPath }));
      toast(`Opened ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
    return;
  }
  if (action === "open-finder") {
    try {
      const payload = await api("/api/reveal-in-finder", form({ repo: projectPath }));
      toast(`Revealed ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
  }
}

// The approval and crash-recovery overflow menu. Approval cards live inside
// the scrolling `#chat-log` and a turn can leave more than one behind; the
// trigger's `data-menu-id` keys which card the menu is open for.
const approvalMenu = createContextMenu({ className: "approval-menu-popover", align: "start" });

// `items` is [{ label, hint, onSelect }], rebuilt on every open because the
// grants a proposal offers differ from one to the next.
function toggleApprovalMenu(items, anchorEl) {
  approvalMenu.toggle(anchorEl, {
    key: anchorEl.dataset.menuId || anchorEl,
    sections: [
      items.map((item) => ({
        label: item.label,
        hint: item.hint,
        className: "approval-menu-row",
        onSelect: () => item.onSelect(),
      })),
    ],
  });
}

// The composer's mode pill (spec 58 §1 req 18). It sits at the bottom of the
// window, so its menu prefers to open above.
const sessionModeMenu = createContextMenu({ align: "start", placement: "above" });

function toggleSessionModeMenu(anchorEl) {
  const current = displayedSessionMode();
  sessionModeMenu.toggle(anchorEl, {
    sections: [
      Object.entries(sessionModeLabels).map(([mode, label]) => ({
        label,
        action: mode,
        hint: mode === current ? "Current" : "",
        className: mode === current ? "context-menu-row-current" : "",
        onSelect: () => void selectSessionMode(mode),
      })),
    ],
  });
}

// The selection itself is the explicit user action spec 20 §5.6 asks for, in
// either direction, so there is no confirmation step. A turn already running
// keeps the mode it started with; the change applies from the next turn.
async function selectSessionMode(mode) {
  const sessionId = currentSessionId;
  if (!sessionId || mode === displayedSessionMode()) return;
  try {
    const payload = await api("/api/session-mode", form({ session_id: sessionId, mode }));
    currentSessionMode = { sessionId: payload.session.id, mode: payload.session.mode };
  } catch (error) {
    toast(error.message);
  }
  renderModeControl();
}

// ---------------------------------------------------------------------------
// Repo bar (spec 58 §3.3): the open folder, its branch and change count, and
// the contextual-action slot, inside the composer above the prompt box.

// The last `/api/git-status` answer for the open folder. `payload` is null
// until one arrives, and stays null when the request fails, which draws the
// folder alone, as for a folder that is not a repository.
let repoBarStatus = { repoPath: "", payload: null };
// Spec 35: whether the open session has anything to commit, and whether it may.
// `null` until the first answer, or when no session is open.
let repoBarCommitStatus = null;
let repoBarRefreshSeq = 0;
let repoBarRefreshedAt = 0;
// Focus refreshes are at least this far apart (§1 req 15). There is no timer:
// the bar only ever refreshes because something happened.
const repoBarFocusThrottleMs = 5000;

async function refreshRepoBar() {
  const repoPath = repo();
  const seq = ++repoBarRefreshSeq;
  repoBarRefreshedAt = Date.now();
  if (repoBarStatus.repoPath !== repoPath) {
    // Another folder: never show the previous one's branch while this loads.
    repoBarStatus = { repoPath, payload: null };
    renderRepoBar();
  }
  if (!repoPath) return;
  let payload = null;
  let commitStatus = null;
  const sessionId = currentSessionId;
  const [statusAnswer, commitAnswer] = await Promise.allSettled([
    api(`/api/git-status?repo=${encodeURIComponent(repoPath)}`),
    sessionId
      ? api(
          `/api/commit-status?repo=${encodeURIComponent(repoPath)}&session=${encodeURIComponent(sessionId)}`,
        )
      : Promise.resolve(null),
  ]);
  if (statusAnswer.status === "fulfilled") payload = statusAnswer.value;
  if (commitAnswer.status === "fulfilled") commitStatus = commitAnswer.value;
  // A later refresh, or a folder switch, owns the bar now.
  if (seq !== repoBarRefreshSeq || repoPath !== repo()) return;
  applyRepoBarStatus(repoPath, payload, commitStatus);
}

function applyRepoBarStatus(repoPath, payload, commitStatus = null) {
  if (repoPath !== repo()) return;
  // Newer than anything still in flight.
  repoBarRefreshSeq += 1;
  repoBarRefreshedAt = Date.now();
  repoBarStatus = { repoPath, payload };
  repoBarCommitStatus = commitStatus;
  renderRepoBar();
}

function renderRepoBar() {
  const bar = $("repo-bar");
  const repoPath = repo();
  bar.hidden = !repoPath;
  if (!repoPath) {
    repoFolderMenu.close();
    closeFindingsPopover();
    repoBarCommitStatus = null;
    return;
  }
  $("repo-folder-name").textContent = projectName(repoPath);
  const folderButton = $("repo-folder-btn");
  folderButton.title = repoPath;
  folderButton.setAttribute("aria-label", `Folder ${projectName(repoPath)}: more options`);

  const payload = repoBarStatus.repoPath === repoPath ? repoBarStatus.payload : null;
  // A folder that is not a repository answers with a non-zero `exitCode` and
  // null repo fields (spec 58 Task 2), not an error.
  const isGit = Boolean(payload && payload.exitCode === 0);
  const branch = $("repo-branch");
  const branchText = isGit ? payload.branch || payload.head || "" : "";
  branch.textContent = branchText;
  branch.hidden = !branchText;
  if (isGit && !payload.branch && payload.head) {
    branch.title = `Detached HEAD at ${payload.head}`;
  } else if (branchText) {
    branch.title = `Branch ${branchText}`;
  } else {
    branch.removeAttribute("title");
  }

  // Display-only in this spec; the diff spec makes it open the diff (§2).
  const insertions = isGit ? Number(payload.insertions) || 0 : 0;
  const deletions = isGit ? Number(payload.deletions) || 0 : 0;
  const changes = $("repo-changes");
  changes.hidden = !isGit || (insertions === 0 && deletions === 0);
  $("repo-insertions").textContent = `+${insertions.toLocaleString("en-US")}`;
  $("repo-deletions").textContent = `−${deletions.toLocaleString("en-US")}`;
  changes.title = `${insertions} inserted, ${deletions} deleted against HEAD, untracked files included`;
  renderRepoAction();
}

window.addEventListener("focus", () => {
  if (!repo() || Date.now() - repoBarRefreshedAt < repoBarFocusThrottleMs) return;
  void refreshRepoBar();
});

const repoFolderMenu = createContextMenu({
  className: "repo-folder-menu",
  align: "start",
  placement: "above",
});

function toggleRepoFolderMenu(anchorEl) {
  const repoPath = repo();
  if (!repoPath) return;
  const payload = repoBarStatus.repoPath === repoPath ? repoBarStatus.payload : null;
  repoFolderMenu.toggle(anchorEl, {
    sections: [
      [
        {
          label: "Show in Finder",
          action: "open-finder",
          onSelect: () => void handleProjectMenuAction("open-finder", repoPath),
        },
        {
          label: "Open repository on GitHub",
          action: "open-github",
          // Only ever the URL the server derived from `origin` (§1 req 12–13).
          hidden: !payload?.githubUrl,
          onSelect: () => void openRepositoryOnGitHub(repoPath),
        },
      ],
      [
        {
          label: "Branch…",
          action: "branch",
          // Spec 36 D8: branch entry points live in the folder menu so the
          // repo bar's single action slot stays Commit…-first.
          hidden: Boolean(payload && payload.exitCode !== 0),
          onSelect: () => void openBranchDialog(),
        },
        {
          label: "Copy path",
          action: "copy-path",
          onSelect: () => void copyRepoPath(repoPath),
        },
        {
          label: "Change folder…",
          action: "change-folder",
          onSelect: () => void pickWorkingFolder(),
        },
      ],
      [
        {
          label: "Open in terminal",
          action: "open-terminal",
          onSelect: () => setTerminalOpen(true),
        },
      ],
    ],
  });
}

// The client never sends a URL: the server rebuilds it from `origin`.
async function openRepositoryOnGitHub(repoPath) {
  try {
    const payload = await api("/api/open-github", form({ repo: repoPath }));
    toast(`Opened ${payload.url}`);
  } catch (error) {
    toast(error.message);
  }
}

async function copyRepoPath(repoPath) {
  try {
    await navigator.clipboard.writeText(repoPath);
    toast("Copied path");
  } catch {
    // No clipboard access (an unfocused or insecure context): show the path so
    // it can still be copied by hand.
    toast(repoPath, { duration: 7000 });
  }
}

// ---------------------------------------------------------------------------
// The contextual-action slot (spec 58 §3.5): one split button, filled by the
// first provider in this list that has an action. Each provider returns null
// or { key, label, tone, run, more }; `more` (optional) is { label, toggle,
// isOpen } for the chevron. Order is priority; nothing else in the bar
// changes when an entry is added.
const repoActionProviders = [
  findingsRepairAction("error"),
  findingsRepairAction("warning"),
  // #35: uncommitted accepted changes → Commit…
  commitAction,
  // #37: branch ahead of its base → Create PR
];

// What the repo bar's Commit… entry shows, per spec 35 §5.1. It is offered
// whenever the session has a candidate, because a file changed by an approved
// command is still worth committing even though it starts unselected.
function commitAction() {
  const status = repoBarCommitStatus;
  if (!status || !Number(status.candidateCount)) return null;
  return {
    key: "commit",
    label: "Commit…",
    tone: "",
    run: () => void openCommitDialog(),
  };
}

let repoActionCurrent = null;
// Covers the `/api/findings-repair` request before `sendChatPrompt` sets
// `chatSubmitting`, so a double click cannot start two repairs.
let findingsRepairPending = false;

function openFindingIds(severity) {
  return sessionFindings
    .filter((finding) => finding.status === "open" && finding.severity === severity)
    .map((finding) => finding.id);
}

function findingsRepairAction(severity) {
  return () => {
    const ids = openFindingIds(severity);
    if (!ids.length) return null;
    const noun = severity === "error" ? "error" : "warning";
    return {
      key: `fix-${severity}`,
      label: `Fix ${ids.length} ${noun}${ids.length === 1 ? "" : "s"}`,
      tone: severity === "error" ? "danger" : "warning",
      run: () => repairFindings(ids),
      more: {
        label: "Show findings",
        toggle: (anchorEl) => toggleFindingsPopover(anchorEl, severity),
        isOpen: () => findingsPanelOpen,
      },
    };
  };
}

function currentRepoAction() {
  if (!repo()) return null;
  for (const provider of repoActionProviders) {
    const action = provider();
    if (action) return action;
  }
  return null;
}

function renderRepoAction() {
  const action = currentRepoAction();
  repoActionCurrent = action;
  const slot = $("repo-action");
  slot.hidden = !action;
  if (!action) return;
  slot.dataset.action = action.key;
  slot.dataset.tone = action.tone || "";
  const main = $("repo-action-main");
  main.textContent = action.label;
  main.disabled = chatSubmitting || findingsRepairPending;
  const more = $("repo-action-more");
  more.hidden = !action.more;
  if (action.more) {
    more.setAttribute("aria-label", action.more.label);
    more.title = action.more.label;
    more.setAttribute("aria-expanded", String(action.more.isOpen()));
  }
}

// ---------------------------------------------------------------------------
// Commit preparation (docs/specs/35_commit_preparation/proposal.md §5.1).
// The diff is redacted for display; the commit contains the real content, and
// the dialog says so before the button. Nothing here decides what is
// committable: the server prepared the record, the secret scan and the hooks.

async function openCommitDialog() {
  const repoPath = repo();
  const sessionId = currentSessionId;
  if (!repoPath || !sessionId) return;
  let preparation;
  try {
    preparation = await api(
      `/api/commit-preparation?repo=${encodeURIComponent(repoPath)}&session=${encodeURIComponent(sessionId)}`,
      { method: "POST" },
    );
  } catch (error) {
    toast(error.message);
    return;
  }
  showCommitDialog(preparation, repoPath, sessionId);
}

function commitSubject(message) {
  return (
    String(message || "")
      .split("\n")
      .find((line) => line.trim()) || "Commit"
  );
}

function commitSize(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} kB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function showCommitDialog(preparation, repoPath, sessionId) {
  const backdrop = document.createElement("div");
  backdrop.className = "app-dialog-backdrop";
  backdrop.innerHTML = `
    <div class="app-dialog app-dialog-commit" role="dialog" aria-modal="true" aria-label="Commit changes">
      <p class="app-dialog-title">Commit changes</p>
      <p class="app-dialog-message commit-summary"></p>
      <p class="commit-redaction">The diffs below are redacted for display. The commit contains
        the real file content.</p>
      <div class="commit-file-list"></div>
      <p class="commit-secret-warning" hidden></p>
      <label class="commit-secret-override" hidden>
        <input type="checkbox" />
        <span>I understand these files contain the real content the scanner flagged, and I
          want to commit it.</span>
      </label>
      <label class="commit-message-label" for="commit-message-input">Commit message</label>
      <textarea id="commit-message-input" class="commit-message" rows="4"></textarea>
      <p class="commit-draft-notice" hidden></p>
      <div class="commit-hooks"></div>
      <div class="commit-verification"></div>
      <p class="commit-refusal" hidden></p>
      <p class="commit-error" hidden></p>
      <div class="app-dialog-actions">
        <button type="button" class="app-dialog-btn app-dialog-cancel">Cancel</button>
        <button type="button" class="app-dialog-btn commit-draft">Draft with model</button>
        <button type="button" class="app-dialog-btn app-dialog-confirm commit-submit" disabled>
          Commit
        </button>
      </div>
    </div>
  `;
  document.body.append(backdrop);

  const dialog = backdrop.querySelector(".app-dialog");
  const list = backdrop.querySelector(".commit-file-list");
  const summary = backdrop.querySelector(".commit-summary");
  const secretWarning = backdrop.querySelector(".commit-secret-warning");
  const override = backdrop.querySelector(".commit-secret-override");
  const overrideInput = override.querySelector('input[type="checkbox"]');
  const messageInput = backdrop.querySelector(".commit-message");
  const draftNotice = backdrop.querySelector(".commit-draft-notice");
  const hooks = backdrop.querySelector(".commit-hooks");
  const verification = backdrop.querySelector(".commit-verification");
  const refusal = backdrop.querySelector(".commit-refusal");
  const errorLine = backdrop.querySelector(".commit-error");
  const draftButton = backdrop.querySelector(".commit-draft");
  const submit = backdrop.querySelector(".commit-submit");

  const selected = new Set(
    preparation.candidates
      .filter((candidate) => candidate.selectedByDefault)
      .map((candidate) => candidate.path),
  );
  let message = preparation.message || "";
  let allowSecrets = false;
  let stale = false;
  let lastWarnings = "";
  messageInput.value = message;

  const selectedCandidates = () =>
    preparation.candidates.filter((candidate) => selected.has(candidate.path));
  // A rename is one change in two paths. The checkboxes move together; the
  // engine refuses a half-selection anyway, so this is the visible half of the
  // same rule.
  const syncCheckboxes = () => {
    list.querySelectorAll('input[type="checkbox"][data-path]').forEach((input) => {
      input.checked = selected.has(input.dataset.path);
    });
  };
  const warningsFor = (candidates) =>
    candidates.flatMap((candidate) =>
      (candidate.secretWarnings || []).map((warning) => ({ ...warning, path: candidate.path })),
    );

  const showError = (text) => {
    errorLine.textContent = text || "";
    errorLine.hidden = !text;
  };

  const refresh = () => {
    const chosen = selectedCandidates();
    const warnings = warningsFor(chosen);
    const warningKey = warnings
      .map((warning) => `${warning.path}:${warning.categories.join(",")}:${warning.count}`)
      .sort()
      .join("|");
    if (warningKey !== lastWarnings) {
      lastWarnings = warningKey;
      allowSecrets = false;
      overrideInput.checked = false;
    }
    summary.textContent = `${chosen.length} of ${preparation.candidates.length} files selected${
      preparation.branch
        ? ` · ${preparation.branch}`
        : preparation.head
          ? ` · ${preparation.head.slice(0, 8)}`
          : ""
    }`;
    secretWarning.hidden = warnings.length === 0;
    override.hidden = warnings.length === 0;
    if (warnings.length) {
      secretWarning.textContent =
        "The scanner flags the real content of: " +
        warnings
          .map((warning) => `${warning.path} (${warning.categories.join(", ")} × ${warning.count})`)
          .join("; ") +
        ". The displayed diff hides the matched text; the commit will not.";
    }
    const blockedBySecrets = warnings.length > 0 && !allowSecrets;
    submit.disabled =
      stale || chosen.length === 0 || preparation.refusals.length > 0 || blockedBySecrets;
    submit.textContent = chosen.length
      ? `Commit ${chosen.length} file${chosen.length === 1 ? "" : "s"}`
      : "Commit";
  };

  preparation.candidates.forEach((candidate) => {
    const row = document.createElement("div");
    row.className = "commit-file";
    const head = document.createElement("label");
    head.className = "commit-file-head";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = selected.has(candidate.path);
    checkbox.dataset.path = candidate.path;
    // A submodule or nested repository has no file bytes this flow can
    // compose; the server refuses it, and the checkbox says so by staying off.
    checkbox.disabled = candidate.source === "unsupported";
    checkbox.addEventListener("change", () => {
      if (checkbox.checked) selected.add(candidate.path);
      else selected.delete(candidate.path);
      const pair = candidate.renamedFrom || candidate.renamedTo;
      if (pair) {
        if (checkbox.checked) selected.add(pair);
        else selected.delete(pair);
      }
      syncCheckboxes();
      refresh();
    });
    const path = document.createElement("code");
    path.className = "commit-file-path";
    path.textContent = candidate.path;
    const label = document.createElement("span");
    label.className = "commit-file-label";
    label.textContent = `${candidate.status} · ${candidate.label}`;
    head.append(checkbox, path, label);
    row.append(head);
    const details = document.createElement("details");
    details.className = "commit-file-diff";
    const caption = document.createElement("summary");
    caption.textContent = "Diff (redacted)";
    const pre = document.createElement("pre");
    pre.textContent = candidate.diff || "No diff is shown for this file.";
    details.append(caption, pre);
    row.append(details);
    list.append(row);
  });

  const hookSource =
    preparation.hooks.source === "core.hooksPath"
      ? "core.hooksPath"
      : "the default hooks directory";
  const hookRows = preparation.hooks.hooks || [];
  const hookTitle = document.createElement("p");
  hookTitle.className = "commit-section-title";
  hookTitle.textContent = `Git hooks from ${hookSource} (${preparation.hooks.directory || "none"})`;
  hooks.append(hookTitle);
  if (!hookRows.length) {
    const none = document.createElement("p");
    none.className = "commit-note";
    none.textContent = "No hooks are present in the report.";
    hooks.append(none);
  }
  hookRows.forEach((hook) => {
    const row = document.createElement("p");
    row.className = "commit-hook";
    const detail = [
      hook.path,
      hook.exists ? commitSize(hook.size) : "not present",
      hook.interpreter || "no interpreter line",
    ].join(" · ");
    row.textContent = `${hook.name}: ${detail} — ${
      hook.runs ? "will run" : hook.skippedReason || "not present, so nothing runs"
    }`;
    hooks.append(row);
  });
  const never = document.createElement("p");
  never.className = "commit-note";
  never.textContent =
    "Damaian runs these hooks and never skips them. A failing hook aborts the commit.";
  hooks.append(never);

  const verdict = preparation.verification || {};
  const verifyTitle = document.createElement("p");
  verifyTitle.className = "commit-section-title";
  verifyTitle.textContent = "Checks";
  verification.append(verifyTitle);
  const verifyLine = document.createElement("p");
  verifyLine.className = "commit-note";
  verifyLine.textContent = verdict.summary || "No check evidence was recorded for this session.";
  verification.append(verifyLine);
  if (verdict.passed?.length) {
    const passed = document.createElement("p");
    passed.className = "commit-note";
    passed.textContent = `Passed: ${verdict.passed.join(", ")}`;
    verification.append(passed);
  }
  if (verdict.failed?.length) {
    const failed = document.createElement("p");
    failed.className = "commit-note commit-note-warn";
    failed.textContent = `Failed: ${verdict.failed.join(", ")}`;
    verification.append(failed);
  }
  if (verdict.stale?.length) {
    const staleChecks = document.createElement("p");
    staleChecks.className = "commit-note commit-note-warn";
    staleChecks.textContent = `Stale: ${verdict.stale.join(", ")}`;
    verification.append(staleChecks);
  }

  if (preparation.refusals.length) {
    refusal.hidden = false;
    refusal.textContent = preparation.refusals.join(" ");
  }

  overrideInput.addEventListener("change", () => {
    allowSecrets = overrideInput.checked;
    refresh();
  });
  messageInput.addEventListener("input", () => {
    message = messageInput.value;
  });

  draftButton.addEventListener("click", async () => {
    draftButton.disabled = true;
    draftNotice.hidden = true;
    try {
      const payload = await api(
        "/api/commit-message",
        form({
          repo: repoPath,
          session_id: sessionId,
          prepare_id: preparation.prepareId,
          paths: [...selected].join("\n"),
        }),
      );
      message = payload.message || message;
      messageInput.value = message;
      if (payload.notice) {
        draftNotice.textContent = payload.notice;
        draftNotice.hidden = false;
      }
    } catch (error) {
      draftNotice.textContent = error.message;
      draftNotice.hidden = false;
    } finally {
      draftButton.disabled = false;
    }
  });

  submit.addEventListener("click", async () => {
    submit.disabled = true;
    showError("");
    try {
      const payload = await api(
        "/api/commit",
        form({
          repo: repoPath,
          session_id: sessionId,
          prepare_id: preparation.prepareId,
          approval: "commit",
          paths: [...selected].join("\n"),
          message,
          allow_generated_secrets: allowSecrets ? "true" : "false",
        }),
      );
      cleanup();
      const warning = payload.warnings?.length ? ` ${payload.warnings.join(" ")}` : "";
      toast(
        `Committed ${payload.commitSha.slice(0, 8)} — ${commitSubject(payload.message)}.${warning}`,
        {
          duration: 9000,
        },
      );
      await refreshRepoBar();
    } catch (error) {
      const body = error.payload || {};
      if (body.code === "commit_message_rewritten") {
        message = body.message || message;
        messageInput.value = message;
        showError(
          "A commit-message hook rewrote your message. Review it above and commit again; " +
            "nothing was committed.",
        );
      } else {
        showError(error.message);
      }
      if (
        [
          "commit_head_moved",
          "commit_content_changed",
          "commit_hooks_changed",
          "commit_not_prepared",
          "commit_selection_invalid",
        ].includes(body.code)
      ) {
        stale = true;
        showError(`${error.message} Close this dialog and choose Commit… again.`);
      }
      refresh();
    }
  });

  const cleanup = () => {
    document.removeEventListener("keydown", onKeydown, true);
    backdrop.remove();
  };
  const onKeydown = (event) => {
    if (event.key === "Escape") cleanup();
  };
  backdrop.querySelector(".app-dialog-cancel").addEventListener("click", cleanup);
  document.addEventListener("keydown", onKeydown, true);
  refresh();
  dialog.querySelector(".commit-message").focus();
}

// ---------------------------------------------------------------------------
// Branch delivery (spec 36). The dialog reads the state once, then prepares
// one operation at a time. Creating, switching, and deleting are separate
// decisions: each sends back the exact prepared mutation with its own
// `approval=branch`. The merge and rebase panels are reports only — they have
// no control that could run either.

function branchOutcomeLabel(outcome) {
  switch (outcome?.mutation?.kind) {
    case "createBranch":
      return `Created ${outcome.branch}`;
    case "switchBranch":
      return `Switched to ${outcome.branch}`;
    case "deleteBranch":
      return `Deleted ${outcome.branch}`;
    default:
      return `Branch ready: ${outcome?.branch || "unknown"}`;
  }
}

function shortOid(oid) {
  return String(oid || "").slice(0, 8) || "the current commit";
}

function mergeReportText(report) {
  const lines = [
    `Merge ${report.base} into ${shortOid(report.headOid)}.`,
    `Merge base: ${report.mergeBase || "not found"}`,
    `Incoming commits (${report.incomingCommits.length}):`,
    ...report.incomingCommits.map((commit) => `  ${commit}`),
    `Changed files (${report.changedFiles.length}):`,
    ...report.changedFiles.map((path) => `  ${path}`),
    `Conflicted paths (${report.conflictedPaths.length}):`,
  ];
  if (report.conflictedPaths.length) {
    lines.push(...report.conflictedPaths.map((path) => `  ${path}`));
  } else {
    lines.push("  none");
  }
  lines.push(`Fast-forward: ${report.fastForward ? "yes" : "no"}`);
  if (report.problem) lines.push("", report.problem);
  if (!report.conflictPredictionAvailable && report.conflictPredictionUnavailableReason) {
    lines.push(
      "",
      `Conflict prediction is unavailable: ${report.conflictPredictionUnavailableReason}`,
    );
  }
  lines.push("", "Nothing was merged or changed. Damaian ran no merge and made no commit.");
  return lines.join("\n");
}

function rebaseReportText(report) {
  const onto = report.ontoOid ? `${report.onto} (${shortOid(report.ontoOid)})` : report.onto;
  const lines = [
    `Rebase ${shortOid(report.headOid)} onto ${onto}.`,
    `Merge base: ${report.mergeBase || "not found"}`,
    `Commits that would be replayed (${report.commitsToReplay.length}):`,
    ...report.commitsToReplay.map((commit) => `  ${commit}`),
  ];
  if (report.problem) lines.push("", report.problem);
  lines.push(
    "",
    "Conflict prediction for a rebase is unavailable because it would require replaying " +
      "the commits one by one. Nothing was replayed.",
  );
  return lines.join("\n");
}

async function openBranchDialog() {
  const repoPath = repo();
  const sessionId = currentSessionId;
  if (!repoPath || !sessionId) return;
  let info;
  try {
    info = await api(
      `/api/branch-info?repo=${encodeURIComponent(repoPath)}&session=${encodeURIComponent(sessionId)}`,
    );
  } catch (error) {
    toast(error.message);
    return;
  }
  let suggestion = null;
  try {
    suggestion = await api(
      "/api/branch-suggestion",
      form({ repo: repoPath, session_id: sessionId }),
    );
  } catch {
    // The name field stays editable when a suggestion cannot be produced.
  }
  showBranchDialog(info, suggestion, repoPath, sessionId);
}

// One card, one decision. Cancel takes focus: an approval surface never
// pre-selects its own confirmation (§5 of the UI style guide).
function renderBranchCard(card, { title, message, lines = [], confirmLabel, danger, onConfirm }) {
  card.replaceChildren();
  card.hidden = false;
  const titleEl = document.createElement("p");
  titleEl.className = "branch-card-title";
  titleEl.textContent = title;
  const messageEl = document.createElement("p");
  messageEl.className = "branch-card-message";
  messageEl.textContent = message;
  card.append(titleEl, messageEl);
  if (lines.length) {
    const list = document.createElement("ul");
    list.className = "branch-card-list";
    lines.forEach((line) => {
      const item = document.createElement("li");
      item.textContent = line;
      list.append(item);
    });
    card.append(list);
  }
  const actions = document.createElement("div");
  actions.className = "branch-actions";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "app-dialog-btn";
  cancel.textContent = "Cancel";
  const confirm = document.createElement("button");
  confirm.type = "button";
  confirm.className = `app-dialog-btn app-dialog-confirm${danger ? " app-dialog-btn-danger" : ""}`;
  confirm.textContent = confirmLabel;
  cancel.addEventListener("click", () => {
    card.hidden = true;
    card.replaceChildren();
  });
  confirm.addEventListener("click", async () => {
    confirm.disabled = true;
    try {
      await onConfirm();
    } finally {
      confirm.disabled = false;
    }
  });
  actions.append(cancel, confirm);
  card.append(actions);
  cancel.focus();
}

function showBranchDialog(info, suggestion, repoPath, sessionId) {
  const backdrop = document.createElement("div");
  backdrop.className = "app-dialog-backdrop";
  backdrop.innerHTML = `
    <div class="app-dialog app-dialog-branch" role="dialog" aria-modal="true" aria-label="Branches">
      <p class="app-dialog-title">Branches</p>
      <div class="branch-state"></div>
      <label class="branch-label" for="branch-name">Branch name</label>
      <input id="branch-name" class="app-dialog-input branch-name" type="text" spellcheck="false" />
      <p class="branch-note"></p>
      <button type="button" class="btn-sm branch-use-alternative" hidden></button>
      <label class="app-dialog-checkitem">
        <input type="checkbox" class="branch-switch-after" />
        <span>Switch to it after creating (a separate approval)</span>
      </label>
      <p class="branch-refusal" hidden></p>
      <div class="branch-actions">
        <button type="button" class="app-dialog-btn branch-create">Create branch…</button>
        <button type="button" class="app-dialog-btn branch-switch">Switch to branch…</button>
      </div>
      <div class="branch-card branch-approval" hidden></div>
      <p class="branch-section-title">Delete a branch</p>
      <p class="branch-note">The current branch is never offered for deletion.</p>
      <div class="branch-branch-list"></div>
      <div class="branch-card branch-delete" hidden></div>
      <div class="branch-card branch-confirm" hidden></div>
      <p class="branch-section-title">Merge or rebase report</p>
      <label class="branch-label" for="branch-base">Merge or onto branch</label>
      <input id="branch-base" class="app-dialog-input" type="text" spellcheck="false" />
      <div class="branch-actions">
        <button type="button" class="app-dialog-btn branch-merge">Preview merge</button>
        <button type="button" class="app-dialog-btn branch-rebase">Preview rebase</button>
      </div>
      <pre class="app-dialog-detail branch-report" tabindex="0" hidden></pre>
      <div class="app-dialog-actions">
        <button type="button" class="app-dialog-btn app-dialog-cancel">Close</button>
      </div>
    </div>
  `;
  document.body.append(backdrop);

  const dialog = backdrop.querySelector(".app-dialog");
  const state = dialog.querySelector(".branch-state");
  const nameInput = dialog.querySelector(".branch-name");
  const note = dialog.querySelector(".branch-note");
  const switchAfter = dialog.querySelector(".branch-switch-after");
  const alternativeButton = dialog.querySelector(".branch-use-alternative");
  const refusal = dialog.querySelector(".branch-refusal");
  const approvalCard = dialog.querySelector(".branch-approval");
  const deleteCard = dialog.querySelector(".branch-delete");
  const confirmCard = dialog.querySelector(".branch-confirm");
  const branchList = dialog.querySelector(".branch-branch-list");
  const baseInput = dialog.querySelector(".branch-base");
  const reportEl = dialog.querySelector(".branch-report");

  const cleanup = () => {
    document.removeEventListener("keydown", onKeydown, true);
    backdrop.remove();
  };
  const onKeydown = (event) => {
    if (event.key === "Escape") cleanup();
  };
  backdrop.querySelector(".app-dialog-cancel").addEventListener("click", cleanup);
  document.addEventListener("keydown", onKeydown, true);

  const showRefusal = (message) => {
    refusal.textContent = message || "";
    refusal.hidden = !message;
  };
  const hideDeleteCards = () => {
    deleteCard.hidden = true;
    deleteCard.replaceChildren();
    confirmCard.hidden = true;
    confirmCard.replaceChildren();
  };

  const addStateLine = (label, value) => {
    const line = document.createElement("p");
    line.className = "branch-state-line";
    const name = document.createElement("span");
    name.className = "branch-state-label";
    name.textContent = `${label}: `;
    const text = document.createElement("span");
    text.textContent = value;
    line.append(name, text);
    state.append(line);
  };
  addStateLine("Current branch", info.current || `detached HEAD at ${shortOid(info.headOid)}`);
  addStateLine("Upstream", info.upstream || "none configured");
  addStateLine("Ahead / behind", `${info.ahead ?? 0} / ${info.behind ?? 0}`);
  if (info.inProgressOperation) {
    addStateLine("Operation in progress", info.inProgressOperation);
  }
  const conflicted = info.conflictedPaths || [];
  addStateLine("Conflicted paths", conflicted.length ? conflicted.join(", ") : "none");

  let currentSuggestion = suggestion;
  nameInput.value = currentSuggestion?.name || "";
  const refreshNote = () => {
    const name = nameInput.value.trim();
    alternativeButton.hidden = true;
    note.classList.remove("branch-note-warn");
    if (!name) {
      note.textContent = "Enter the branch name to create or switch to.";
      return;
    }
    if (currentSuggestion && currentSuggestion.name === name) {
      if (!currentSuggestion.valid) {
        note.classList.add("branch-note-warn");
        note.textContent = `Git will not accept this name: ${
          currentSuggestion.refusal || "the name is invalid"
        }.`;
        return;
      }
      if (currentSuggestion.exists && currentSuggestion.alternative) {
        note.classList.add("branch-note-warn");
        note.textContent =
          `\`${name}\` already exists. A free alternative is ` +
          `\`${currentSuggestion.alternative}\`; the field was not changed.`;
        alternativeButton.textContent = `Use ${currentSuggestion.alternative}`;
        alternativeButton.hidden = false;
        return;
      }
    }
    note.textContent = "Create and switch are separate approvals; each asks before it runs.";
  };
  refreshNote();
  nameInput.addEventListener("input", refreshNote);
  alternativeButton.addEventListener("click", () => {
    nameInput.value = currentSuggestion?.alternative || nameInput.value;
    refreshNote();
    nameInput.focus();
  });

  const applyBranch = (preparation, confirmUnmerged) =>
    api(
      "/api/branch",
      form({
        repo: repoPath,
        session_id: sessionId,
        prepare_id: preparation.prepareId,
        mutation: JSON.stringify(preparation.mutation),
        approval: "branch",
        confirm_unmerged: confirmUnmerged ? "true" : "false",
      }),
    );

  const finishMutation = async (outcome) => {
    cleanup();
    toast(branchOutcomeLabel(outcome), { duration: 7000 });
    await refreshRepoBar();
    await openBranchDialog();
  };

  const applyPrepared = async (preparation, confirmUnmerged) => {
    try {
      await finishMutation(await applyBranch(preparation, confirmUnmerged));
    } catch (error) {
      showRefusal(error.message);
    }
  };

  const prepareBranch = async (operation) => {
    const name = nameInput.value.trim();
    if (!name) {
      showRefusal("Enter a branch name first.");
      nameInput.focus();
      return null;
    }
    showRefusal("");
    approvalCard.hidden = true;
    approvalCard.replaceChildren();
    hideDeleteCards();
    try {
      return await api(
        "/api/branch-prepare",
        form({ repo: repoPath, session_id: sessionId, operation, name }),
      );
    } catch (error) {
      const preparation = error.payload?.preparation || null;
      if (preparation?.suggestion) {
        currentSuggestion = preparation.suggestion;
        refreshNote();
      }
      showRefusal(error.message);
      return null;
    }
  };

  dialog.querySelector(".branch-create").addEventListener("click", async () => {
    const preparation = await prepareBranch("create");
    if (!preparation) return;
    renderBranchCard(approvalCard, {
      title: `Create ${preparation.mutation.name}?`,
      message:
        `This creates ${preparation.mutation.name} at ${shortOid(
          preparation.mutation.startPoint,
        )} and leaves ${info.current || "HEAD"} checked out. Nothing else changes; ` +
        "switching to it is a separate approval.",
      confirmLabel: "Create branch",
      onConfirm: async () => {
        try {
          const outcome = await applyBranch(preparation, false);
          toast(`Created ${outcome.branch}`, { duration: 7000 });
          if (!switchAfter.checked) {
            await finishMutation(outcome);
            return;
          }
          const switching = await prepareBranch("switch");
          if (!switching) return;
          renderBranchCard(approvalCard, {
            title: `Switch to ${switching.mutation.name}?`,
            message:
              "The branch was created and the current branch is still checked out. This is " +
              `the separate switch approval; it updates the index and working tree to ${shortOid(
                switching.startPointSha,
              )}.`,
            confirmLabel: "Switch branch",
            onConfirm: async () => {
              try {
                await finishMutation(await applyBranch(switching, false));
              } catch (error) {
                showRefusal(error.message);
              }
            },
          });
        } catch (error) {
          showRefusal(error.message);
        }
      },
    });
  });

  dialog.querySelector(".branch-switch").addEventListener("click", async () => {
    const preparation = await prepareBranch("switch");
    if (!preparation) return;
    renderBranchCard(approvalCard, {
      title: `Switch to ${preparation.mutation.name}?`,
      message: `This updates the index and working tree to ${shortOid(
        preparation.startPointSha,
      )}. Uncommitted changes that would be overwritten are refused before anything runs.`,
      confirmLabel: "Switch branch",
      onConfirm: () => applyPrepared(preparation, false),
    });
  });

  const showDeleteCard = (preparation) => {
    const name = preparation.mutation.name;
    const subjects = preparation.unmergedSubjects || [];
    const count = preparation.unmergedCommits;
    const reason = preparation.unmergedReason;
    if (count === 0) {
      renderBranchCard(deleteCard, {
        title: `Delete ${name}`,
        message:
          `${name} is fully merged: every commit is on the default branch or a remote, ` +
          "so deleting it loses no commits.",
        confirmLabel: `Delete ${name}`,
        danger: true,
        onConfirm: () => applyPrepared(preparation, false),
      });
      return;
    }
    const counted = count === null || count === undefined;
    const lost = counted
      ? "an unknown number of commits"
      : `${count} commit${count === 1 ? "" : "s"}`;
    renderBranchCard(deleteCard, {
      title: `${name} has unmerged commits`,
      message: reason
        ? `${reason}, so Git will not delete ${name} with a plain delete. ` +
          "Nothing has been deleted; Continue leads to a second confirmation."
        : counted
          ? `Damaian could not determine the default branch, so it cannot count what deleting ` +
            `${name} would lose. The commit subjects are below. Nothing has been deleted; ` +
            "Continue leads to a second confirmation."
          : `Deleting ${name} would lose ${lost} that are not on the default branch or any ` +
            "remote. Nothing has been deleted; Continue leads to a second confirmation.",
      lines: subjects,
      confirmLabel: "Continue…",
      onConfirm: () => {
        deleteCard.hidden = true;
        renderBranchCard(confirmCard, {
          title: reason || counted ? `Delete ${name} anyway?` : `Delete ${name} and lose ${lost}?`,
          message: reason
            ? `This is the second confirmation. ${reason}, and the commits below are not ` +
              `reachable from that reference. Damaian will run git branch -D.`
            : "This is the second confirmation. Damaian will run git branch -D; the commits " +
              "below will no longer be reachable from this branch.",
          lines: subjects,
          confirmLabel: counted ? "Delete anyway" : `Delete and lose ${lost}`,
          danger: true,
          onConfirm: () => applyPrepared(preparation, true),
        });
      },
    });
  };

  const deletable = (info.branches || []).filter((name) => name !== info.current);
  if (!deletable.length) {
    const empty = document.createElement("p");
    empty.className = "branch-note";
    empty.textContent = "No other local branches are available to delete.";
    branchList.append(empty);
  }
  deletable.forEach((name) => {
    const row = document.createElement("div");
    row.className = "branch-branch-row";
    const label = document.createElement("code");
    label.className = "branch-branch-name";
    label.textContent = name;
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn-sm btn-danger";
    button.textContent = "Delete…";
    button.addEventListener("click", async () => {
      showRefusal("");
      approvalCard.hidden = true;
      approvalCard.replaceChildren();
      hideDeleteCards();
      try {
        const preparation = await api(
          "/api/branch-prepare",
          form({ repo: repoPath, session_id: sessionId, operation: "delete", name }),
        );
        showDeleteCard(preparation);
      } catch (error) {
        showRefusal(error.message);
      }
    });
    row.append(label, button);
    branchList.append(row);
  });

  baseInput.value = info.upstream || "main";
  const runReport = async (mode) => {
    const base = baseInput.value.trim();
    if (!base) {
      showRefusal("Enter the branch to merge or rebase onto.");
      baseInput.focus();
      return;
    }
    showRefusal("");
    try {
      const report = await api(
        `/api/merge-preparation?repo=${encodeURIComponent(repoPath)}` +
          `&session=${encodeURIComponent(sessionId)}&mode=${mode}&base=${encodeURIComponent(base)}`,
      );
      reportEl.textContent = mode === "merge" ? mergeReportText(report) : rebaseReportText(report);
      reportEl.hidden = false;
    } catch (error) {
      reportEl.hidden = true;
      showRefusal(error.message);
    }
  };
  dialog.querySelector(".branch-merge").addEventListener("click", () => void runReport("merge"));
  dialog.querySelector(".branch-rebase").addEventListener("click", () => void runReport("rebase"));

  nameInput.focus();
}

function closeApprovalMenu() {
  approvalMenu.close();
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

// The repository-relative part of `target` when it lies inside `root`, and
// `""` otherwise. Both are absolute paths from the engine, so a proposal's
// working directory is rendered as the root it runs in (`context.md` §13).
function repositoryRelative(root, target) {
  const base = String(root || "").replace(/\/+$/, "");
  const full = String(target || "").replace(/\/+$/, "");
  if (!base || !full || full === base) return "";
  return full.startsWith(`${base}/`) ? full.slice(base.length + 1) : "";
}

// The nearest root that owns `path`, by longest path-segment prefix — the
// same rule as `RepositoryMap::root_for_path`. `""` (the repository root)
// owns everything, so a non-empty result is what gets shown.
function rootForPath(path) {
  const candidate = String(path || "").replace(/^\.\//, "");
  let best = "";
  for (const root of repositoryRoots) {
    if (!root || root.length <= best.length) continue;
    if (candidate === root || candidate.startsWith(`${root}/`)) best = root;
  }
  return best;
}

function rootTag(path) {
  const root = rootForPath(path);
  return root ? ` · ${root}` : "";
}

// Refreshes the roots cache and redraws the chips and the Roots section from
// a `/api/repository-map` or `/api/repository-roots` payload.
function applyRepositoryMap(payload) {
  repositoryRoots = (payload?.map?.roots || [])
    .filter((root) => root.userOverride !== "removed")
    .map((root) => root.path);
  renderRepositoryRoots(payload);
  renderPinnedContextFiles();
}

/// Fetches the selected repository's map and refreshes everything that names
/// a root. A missing repository clears both.
async function refreshRepositoryRoots(repoPath = repo()) {
  if (!repoPath) {
    applyRepositoryMap(null);
    return null;
  }
  const payload = await api(`/api/repository-map?repo=${encodeURIComponent(repoPath)}`);
  applyRepositoryMap(payload);
  return payload;
}

function rootDisplayPath(path) {
  return path === "" ? "." : path;
}

function rootEvidenceSentence(root) {
  const detected = root.detectedBy || {};
  switch (detected.kind) {
    case "repositoryRoot":
      return "Treated as a root because it is the repository root.";
    case "manifest":
      return `Treated as a root because ${detected.path} exists.`;
    case "userOverride":
      return "Added by you — it has no manifest of its own.";
    default:
      return "Treated as a root.";
  }
}

function rootStatusLabel(root) {
  if (root.userOverride === "added") return "added by you";
  if (root.userOverride === "removed") return "removed by you";
  return "detected";
}

function renderRepositoryRoots(payload) {
  const list = $("repository-roots-list");
  const detail = $("repository-root-detail");
  const excluded = $("repository-roots-excluded");
  const status = $("repository-roots-status");
  if (!list || !detail || !excluded || !status) return;
  list.innerHTML = "";
  detail.innerHTML = "";
  detail.hidden = true;
  excluded.innerHTML = "";
  excluded.hidden = true;

  if (!payload) {
    status.textContent = "No repository selected.";
    return;
  }
  const roots = payload.map?.roots || [];
  status.textContent = roots.length === 1 ? "1 root" : `${roots.length} roots`;

  roots.forEach((root) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "repository-root-row";
    row.dataset.rootPath = root.path;

    const path = document.createElement("code");
    path.className = "repository-root-path";
    path.textContent = rootDisplayPath(root.path);

    const meta = document.createElement("span");
    meta.className = "repository-root-meta";
    const languages = (root.languages || []).join(", ") || "no language detected";
    const commandCount = (root.commands || []).length;
    const commands = commandCount === 1 ? "1 command" : `${commandCount} commands`;
    meta.textContent = `${languages} · ${commands} · ${rootStatusLabel(root)}`;

    row.append(path, meta);
    row.addEventListener("click", () => renderRepositoryRootDetail(root));
    list.append(row);
  });

  const invalid = (payload.map?.excluded || []).filter(
    (entry) => entry.reason === "invalidOverride",
  );
  if (invalid.length) {
    const label = document.createElement("p");
    label.className = "settings-page-hint";
    label.textContent = "Overrides Damaian did not apply:";
    excluded.append(label);
    const items = document.createElement("ul");
    invalid.forEach((entry) => {
      const item = document.createElement("li");
      const path = document.createElement("code");
      path.textContent = entry.path;
      const reason = document.createElement("span");
      reason.textContent = " — not a directory inside this repository";
      item.append(path, reason);
      items.append(item);
    });
    excluded.append(items);
    excluded.hidden = false;
  }
}

function renderRepositoryRootDetail(root) {
  const detail = $("repository-root-detail");
  detail.innerHTML = "";

  const header = document.createElement("div");
  header.className = "repository-root-detail-header";
  const path = document.createElement("code");
  path.textContent = rootDisplayPath(root.path);
  header.append(path);

  // The repository root cannot be removed — the map is never empty — so it
  // offers no control. An override is undone rather than reversed.
  if (root.path !== "") {
    const override = root.userOverride === "added" || root.userOverride === "removed";
    const button = document.createElement("button");
    button.type = "button";
    button.className = override ? "btn-sm btn-quiet" : "btn-sm btn-danger";
    button.textContent = override ? "Undo" : "Remove root";
    button.addEventListener("click", async () => {
      try {
        button.disabled = true;
        const confirmed = await confirmDialog(
          override ? "Undo this override?" : "Stop treating this as a root?",
          override
            ? "This writes to .damaian/config.conf in the repository, which Git shows."
            : "This writes to .damaian/config.conf in the repository, which Git shows. " +
                "Damaian will detect the root again if a manifest still marks it.",
          { confirmLabel: override ? "Undo" : "Remove" },
        );
        if (!confirmed) {
          button.disabled = false;
          return;
        }
        await writeRootOverride(override ? "clear" : "remove", root.path);
      } catch (error) {
        button.disabled = false;
        toast(error.message);
      }
    });
    header.append(button);
  }
  detail.append(header);

  const evidence = document.createElement("p");
  evidence.className = "repository-root-evidence";
  evidence.textContent = rootEvidenceSentence(root);
  detail.append(evidence);

  if (root.path !== "" && (root.instruction_files || []).length) {
    detail.append(repositoryRootList("Instruction files", root.instruction_files));
  }
  if ((root.commands || []).length) {
    const commands = document.createElement("ul");
    commands.className = "repository-root-commands";
    root.commands.forEach((command) => {
      const item = document.createElement("li");
      const code = document.createElement("code");
      code.textContent = command.command;
      const runsIn = document.createElement("span");
      runsIn.textContent = ` — runs in ${rootDisplayPath(command.workingDirectory || root.path)}`;
      item.append(code, runsIn);
      commands.append(item);
    });
    const heading = document.createElement("p");
    heading.className = "repository-root-section-label";
    heading.textContent = "Commands";
    detail.append(heading, commands);
  } else if (root.userOverride !== "removed") {
    const empty = document.createElement("p");
    empty.className = "settings-page-hint";
    empty.textContent = "No commands detected in this root.";
    detail.append(empty);
  }

  detail.hidden = false;
}

function repositoryRootList(label, values) {
  const heading = document.createElement("p");
  heading.className = "repository-root-section-label";
  heading.textContent = label;
  const list = document.createElement("ul");
  list.className = "repository-root-files";
  values.forEach((value) => {
    const item = document.createElement("li");
    const code = document.createElement("code");
    code.textContent = value;
    item.append(code);
    list.append(item);
  });
  const wrapper = document.createElement("div");
  wrapper.append(heading, list);
  return wrapper;
}

async function writeRootOverride(action, path) {
  const payload = await api("/api/repository-roots", form({ repo: requireRepo(), action, path }));
  applyRepositoryMap(payload);
  if (action === "add") toast(`Added root ${path}`);
  else if (action === "remove") toast(`Removed root ${path}`);
  else toast(`Restored detection for ${path}`);
  return payload;
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
    // A chip whose file is in a sub-root reads `index.ts · packages/api`, so
    // two files named `index.ts` are distinguishable (`context.md` §4).
    label.textContent = `${fileBaseName(path)}${rootTag(path)}`;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${path} from context`);
    remove.textContent = "×";
    remove.addEventListener("click", () => removePinnedContextFile(path));
    chip.append(icon, label, remove);
    container.append(chip);
  });
}

const CONTEXT_CATEGORY_LABELS = {
  required: "Required",
  instructions: "Instructions",
  hook_context: "Hook context",
  pinned: "Pinned",
  plan_state: "Plan state",
  repository_map: "Repository map",
  symbols: "Symbols",
  search_results: "Search results",
  file_ranges: "File ranges",
  findings: "Findings",
  conversation_summary: "Summary",
  external_reference: "Reference pages",
  memory: "Memory",
};

const CONTEXT_REASON_LABELS = {
  restricted_path: "restricted path",
  secret_bearing: "contains a secret",
  category_budget_exhausted: "category share exhausted",
  total_budget_exhausted: "context budget exhausted",
  deduplicated_into_range: "merged into a wider range",
  truncated: "shortened to fit",
  too_large: "too large",
  binary: "binary file",
  user_removed: "removed by you",
  path_not_selected: "outside this session's paths",
  missing: "file missing",
  memory_no_longer_active: "memory was no longer active",
};

function formatContextTokens(value) {
  const tokens = Number(value) || 0;
  return tokens >= 10000 ? `${(tokens / 1000).toFixed(1)}k` : tokens.toLocaleString();
}

function contextRangeLabel(range) {
  return range ? `:${range.start}-${range.end}` : "";
}

// A path is a reference, never content: this is the one route that opens a
// file, and it is used for included and excluded paths alike.
function contextPathButton(path, range) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "context-path";
  button.textContent = `${path}${contextRangeLabel(range)}`;
  button.title = `${path} — open in Visual Studio Code`;
  button.addEventListener("click", async () => {
    try {
      const payload = await api(
        "/api/open-vscode-file",
        form({ repo: requireRepo(), path, line: range ? String(range.start) : "" }),
      );
      toast(`Opened ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
  });
  return button;
}

// The header chip shows how full this turn's context budget is.
function contextMeterText(totalTokens, limitTokens) {
  const total = Number(totalTokens) || 0;
  const limit = Number(limitTokens) || 0;
  if (limit <= 0) return { percent: null, label: `${formatContextTokens(total)} tokens` };
  const percent = Math.round((total / limit) * 100);
  return { percent, label: `${percent}% of budget` };
}

function renderContextButton() {
  const button = $("context-inspector-btn");
  if (!button) return;
  const divider = $("context-divider");
  if (!lastContextManifest) {
    button.hidden = true;
    if (divider) divider.hidden = true;
    button.setAttribute("aria-expanded", "false");
    return;
  }
  button.hidden = false;
  if (divider) divider.hidden = false;
  const totalTokens = lastContextManifest.totalTokens;
  const limitTokens = lastContextManifest.limit?.tokens ?? 0;
  const total = formatContextTokens(totalTokens);
  const limit = formatContextTokens(limitTokens);
  const { percent, label } = contextMeterText(totalTokens, limitTokens);
  $("context-inspector-label").textContent = label;
  $("context-meter-fill").style.width = `${Math.min(100, percent ?? 0)}%`;
  $("context-meter").hidden = percent === null;
  const window = lastContextManifest.limit?.window;
  button.title = `Context for this turn: ${total} of ${limit} tokens${
    window ? `; model window ${formatContextTokens(window)}` : ""
  }`;
}

function setContextManifest(manifest, target = "") {
  lastContextManifest = manifest || null;
  if (target) contextPanelTaskId = target;
  renderContextButton();
  if (!$("context-panel").hidden) renderContextPanel();
}

// Adopt a manifest the removal route returned. The card and the open review
// must both move to it, because removal is by index in the manifest the panel
// is showing.
function applyContextReviewManifest(proposalId, manifest) {
  const card = contextReviewCards.get(proposalId);
  if (card) {
    card.review = { ...card.review, manifest };
    card.refresh();
  }
  if (currentContextReview && currentContextReview.proposalId === proposalId) {
    currentContextReview = { ...currentContextReview, manifest };
  }
}

function openContextPanel(manifest, target = "") {
  if (manifest) setContextManifest(manifest, target);
  const panel = $("context-panel");
  panel.hidden = false;
  $("context-inspector-btn").setAttribute("aria-expanded", "true");
  renderContextPanel();
}

function closeContextPanel() {
  $("context-panel").hidden = true;
  $("context-inspector-btn").setAttribute("aria-expanded", "false");
}

function contextCanRemove() {
  return Boolean(
    currentContextReview && contextPanelTaskId === `review:${currentContextReview.proposalId}`,
  );
}

function memoryDisplayKey(manifest) {
  const references = manifest?.memory || [];
  if (!references.length) return "";
  return [
    currentSessionId,
    manifest.taskId || "",
    manifest.assembledAtMs || 0,
    references.map((reference) => reference.entryId).join(","),
  ].join("|");
}

function memoryDisplayFor(manifest) {
  const key = memoryDisplayKey(manifest);
  return key ? memoryDisplayCache.get(key) : undefined;
}

function ensureMemoryDisplay(manifest) {
  const key = memoryDisplayKey(manifest);
  if (!key || memoryDisplayCache.has(key) || memoryDisplayRequests.has(key)) return;
  memoryDisplayRequests.add(key);
  api(
    "/api/context-memory",
    form({
      repo: repo(),
      session_id: currentSessionId,
      memory: JSON.stringify(manifest.memory || []),
    }),
  )
    .then((payload) => {
      memoryDisplayCache.set(
        key,
        new Map((payload.entries || []).map((entry) => [entry.entryId, entry])),
      );
    })
    .catch((error) => {
      memoryDisplayCache.set(key, new Map([["error", error.message]]));
    })
    .finally(() => {
      memoryDisplayRequests.delete(key);
      if (lastContextManifest === manifest) renderContextPanel();
    });
}

function memoryVerificationLabel(verification) {
  if (verification === "evidence_stale") return "may be out of date";
  if (verification === "evidence_current") return "evidence current";
  return "user-confirmed";
}

function memoryProvenanceLabel(provenance) {
  if (!provenance) return "";
  if (provenance.type === "repositoryEvidence") {
    const paths = (provenance.paths || []).map((item) => item.path).filter(Boolean);
    return paths.length ? `derived from ${paths.join(", ")}` : "derived from repository evidence";
  }
  if (provenance.type === "suggestedAndConfirmed") return "confirmed after a task";
  if (provenance.type === "userRequest") return "you asked for it";
  return "";
}

function memoryDateLabel(ms) {
  if (!ms) return "";
  const date = new Date(Number(ms));
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString();
}

function memoryMatchReasonLabel(reason) {
  const terms = reason?.matchedTerms || [];
  const matched = terms.length ? `matched ${terms.join(", ")}` : "recalled";
  return reason?.categoryBoost ? `${matched}; category fitted the task` : matched;
}

function memoryEntryMeta(display) {
  return [
    display.scope,
    memoryProvenanceLabel(display.provenance),
    memoryDateLabel(display.createdAtMs),
    memoryVerificationLabel(display.verification),
    memoryMatchReasonLabel(display.matchReason),
  ]
    .filter(Boolean)
    .join(" · ");
}

function renderMemoryItems(list, manifest) {
  const references = manifest.memory || [];
  const indices = [];
  (manifest.included || []).forEach((item, index) => {
    if (item.category === "memory") indices.push(index);
  });
  const display = memoryDisplayFor(manifest);
  references.forEach((reference, position) => {
    const row = document.createElement("div");
    row.className = "context-memory-item";
    const head = document.createElement("div");
    head.className = "context-memory-head";
    const statement = document.createElement("span");
    statement.className = "context-memory-statement";
    const displayEntry = display instanceof Map ? display.get(reference.entryId) : undefined;
    const loadError = display instanceof Map ? display.get("error") : undefined;
    if (displayEntry?.deleted) {
      statement.textContent = "deleted";
    } else if (displayEntry?.statement) {
      statement.textContent = displayEntry.statement;
    } else if (loadError) {
      statement.textContent = "Memory details unavailable";
    } else {
      statement.textContent = "Resolving…";
    }
    head.append(statement);
    const index = indices[position];
    if (contextCanRemove() && index !== undefined) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "btn-icon context-item-action context-remove";
      remove.title = "Remove from this turn";
      remove.setAttribute("aria-label", `Remove ${reference.entryId} from this turn`);
      remove.textContent = "×";
      remove.addEventListener("click", () =>
        removeContextIndex(index).catch((error) => toast(error.message)),
      );
      head.append(remove);
    }
    row.append(head);
    const meta = document.createElement("span");
    meta.className = "context-memory-meta";
    if (displayEntry?.deleted) {
      meta.textContent = memoryMatchReasonLabel(reference.matchReason);
    } else if (displayEntry) {
      meta.textContent = memoryEntryMeta(displayEntry);
    } else if (loadError) {
      meta.textContent = loadError;
    }
    if (meta.textContent) row.append(meta);
    list.append(row);
  });
}

function contextCategoryRow(manifest, category) {
  const items = (manifest.included || []).filter((item) => item.category === category);
  const usage = (manifest.categories || []).find((entry) => entry.category === category);
  const details = document.createElement("details");
  details.className = "context-category";
  details.open = category === "required" || category === "pinned";
  const summary = document.createElement("summary");
  const label = document.createElement("span");
  label.className = "context-category-label";
  label.textContent = CONTEXT_CATEGORY_LABELS[category] || category;
  const tokens = document.createElement("span");
  tokens.className = "context-category-tokens";
  tokens.textContent = `${formatContextTokens(usage?.used ?? 0)} tokens`;
  const count = document.createElement("span");
  count.className = "context-category-count";
  // Memory is counted in entries, matching what the panel lists below it;
  // every other category is a list of context items.
  const countValue =
    category === "memory" ? (manifest.memory || []).length : (usage?.items ?? items.length);
  const countNoun = category === "memory" ? "entry" : "item";
  count.textContent = `${countValue} ${countNoun}${countValue === 1 ? "" : "s"}`;
  summary.append(label, tokens, count);
  if (category === "required") {
    const locked = document.createElement("span");
    locked.className = "context-locked";
    locked.textContent = "locked";
    summary.append(locked);
  }
  if (category === "symbols" && manifest.symbolProvenance) {
    const provenance = document.createElement("span");
    provenance.className = "context-provenance";
    provenance.textContent = `${manifest.symbolProvenance.total} symbols (${manifest.symbolProvenance.heuristic} heuristic)`;
    summary.append(provenance);
  }
  if (category === "memory") {
    // The stale count is the signal a user needs without expanding (proposal
    // §5.4): three memories, one of them questionable.
    const stale = (manifest.memory || []).filter(
      (reference) => reference.verification === "evidence_stale",
    ).length;
    if (stale) {
      const staleLabel = document.createElement("span");
      staleLabel.className = "context-category-stale";
      staleLabel.textContent = `${stale} stale`;
      summary.append(staleLabel);
    }
  }
  details.append(summary);

  const list = document.createElement("div");
  list.className = "context-items";
  if (category === "memory") {
    ensureMemoryDisplay(manifest);
    renderMemoryItems(list, manifest);
    details.append(list);
    return details;
  }
  items.forEach((item) => {
    const row = document.createElement("div");
    row.className = "context-item";
    if (item.path) {
      row.append(contextPathButton(item.path, item.lineRange));
    } else if (item.origin) {
      // A fetched page has no path; its URL and fetch time are its identity
      // (spec 51 §5.4), so the row names where it came from.
      const name = document.createElement("span");
      name.textContent = item.origin;
      row.append(name);
    } else {
      const name = document.createElement("span");
      name.textContent = item.kind === "user_prompt" ? "Your message" : item.kind || category;
      row.append(name);
    }
    const meta = document.createElement("span");
    meta.className = "context-item-tokens";
    meta.textContent = `${formatContextTokens(item.tokens)} tokens${
      item.provenance === "heuristic" ? " · heuristic" : ""
    }`;
    row.append(meta);
    if (item.path) {
      const pin = document.createElement("button");
      pin.type = "button";
      pin.className = "btn-icon context-item-action";
      pin.title = "Pin for later turns";
      pin.setAttribute("aria-label", `Pin ${item.path} for later turns`);
      pin.textContent = "📌";
      pin.addEventListener("click", () =>
        addContextPin(item.path, item.lineRange).catch((error) => toast(error.message)),
      );
      row.append(pin);
    }
    if (category !== "required" && contextCanRemove()) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "btn-icon context-item-action context-remove";
      remove.title = "Remove from this turn";
      remove.setAttribute("aria-label", `Remove ${item.path || category} from this turn`);
      remove.textContent = "×";
      remove.addEventListener("click", () =>
        removeContextItem(item).catch((error) => toast(error.message)),
      );
      row.append(remove);
    }
    list.append(row);
  });
  details.append(list);
  return details;
}

// The textarea is the *current* restriction as well as a form: populate it
// whenever the stored list changes (a session load, Apply, Clear), but never
// overwrite text the user has typed and not applied. Without this, re-applying
// a blank field after a reload silently replaced a real restriction with
// `Some([])`, which admits no file content.
function syncContextRestrictionField() {
  if (contextRestrictionDirty || !$("context-restriction-paths")) return;
  $("context-restriction-paths").value = (contextRestriction || []).join("\n");
}

function renderContextPanel() {
  const body = $("context-panel-body");
  const summary = $("context-panel-summary");
  body.innerHTML = "";
  syncContextRestrictionField();
  if (!lastContextManifest) {
    summary.textContent = "Nothing assembled yet.";
    return;
  }
  const manifest = lastContextManifest;
  const limit = manifest.limit?.tokens ?? 0;
  summary.textContent = `${formatContextTokens(manifest.totalTokens)} / ${formatContextTokens(
    limit,
  )} tokens · ${manifest.limit?.source === "observed" ? "observed" : "declared"} limit`;
  if (contextCanRemove()) {
    const hint = document.createElement("p");
    hint.className = "context-panel-hint";
    hint.textContent =
      "Nothing has been sent. Remove optional items here, then send or decline from the review card.";
    body.append(hint);
  }

  const order = (manifest.categories || []).map((entry) => entry.category);
  for (const category of order) {
    // The summary row is its own shape (tokens and a message count), not a
    // list of included refs.
    if (category === "conversation_summary") continue;
    body.append(contextCategoryRow(manifest, category));
  }

  if (manifest.conversationSummary) {
    const details = document.createElement("details");
    details.className = "context-category";
    details.open = true;
    const row = document.createElement("summary");
    const label = document.createElement("span");
    label.className = "context-category-label";
    label.textContent = "Summary";
    const tokens = document.createElement("span");
    tokens.className = "context-category-tokens";
    tokens.textContent = `${formatContextTokens(manifest.conversationSummary.tokens)} tokens`;
    const count = document.createElement("span");
    count.className = "context-category-count";
    count.textContent = `${manifest.conversationSummary.messages} messages${
      manifest.conversationSummary.factsOnly
        ? " · facts only"
        : manifest.conversationSummary.narrativeCarriedForward
          ? " · narrative carried forward"
          : ""
    }`;
    row.append(label, tokens, count);
    if (contextCanRemove()) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "btn-icon context-item-action context-remove";
      remove.title = "Remove the summary from this turn";
      remove.setAttribute("aria-label", "Remove the conversation summary from this turn");
      remove.textContent = "×";
      remove.addEventListener("click", () =>
        removeContextSummary().catch((error) => toast(error.message)),
      );
      row.append(remove);
    }
    details.append(row);
    body.append(details);
  }

  const pins = document.createElement("details");
  pins.className = "context-category";
  pins.open = contextPins.length > 0;
  const pinSummary = document.createElement("summary");
  const pinLabel = document.createElement("span");
  pinLabel.className = "context-category-label";
  pinLabel.textContent = "Pinned for this session";
  const pinCount = document.createElement("span");
  pinCount.className = "context-category-count";
  pinCount.textContent = `${contextPins.length}`;
  pinSummary.append(pinLabel, pinCount);
  pins.append(pinSummary);
  const pinList = document.createElement("div");
  pinList.className = "context-items";
  if (!contextPins.length) {
    const empty = document.createElement("p");
    empty.className = "context-panel-hint";
    empty.textContent = "No pins. A pin applies to later turns.";
    pinList.append(empty);
  }
  contextPins.forEach((pin) => {
    const row = document.createElement("div");
    row.className = "context-item";
    row.append(contextPathButton(pin.path, pin.range));
    const unpin = document.createElement("button");
    unpin.type = "button";
    unpin.className = "btn-icon context-item-action";
    unpin.title = "Unpin";
    unpin.setAttribute("aria-label", `Unpin ${pin.path}`);
    unpin.textContent = "×";
    unpin.addEventListener("click", () =>
      removeContextPin(pin.path, pin.range).catch((error) => toast(error.message)),
    );
    row.append(unpin);
    pinList.append(row);
  });
  pins.append(pinList);
  body.append(pins);

  if (contextRestriction) {
    const restriction = document.createElement("p");
    restriction.className = "context-panel-hint";
    restriction.textContent = contextRestriction.length
      ? `This session is restricted to ${contextRestriction.length} path(s). Only those paths' file content is assembled for a turn, and the file-content tools (read_file, list_directory, searches and read_git_diff) can use only those paths. A one-off shell command such as ls -R can still list names outside it, and the symbols row and repository map are still built from the whole repository.`
      : "This session is restricted to no paths, so no file content is assembled for a turn and the file-content tools cannot read or list any path. A one-off shell command such as ls -R can still list names outside it, and the symbols row and repository map are still built from the whole repository.";
    body.append(restriction);
  }

  const excluded = document.createElement("details");
  excluded.className = "context-excluded";
  const excludedSummary = document.createElement("summary");
  excludedSummary.textContent = `Excluded (${manifest.excluded?.length ?? 0})`;
  excluded.append(excludedSummary);
  (manifest.excluded || []).forEach((record) => {
    const row = document.createElement("div");
    row.className = "context-excluded-row";
    if (record.path) {
      row.append(contextPathButton(record.path, record.lineRange));
    }
    const reason = document.createElement("span");
    reason.className = "context-excluded-reason";
    reason.textContent = CONTEXT_REASON_LABELS[record.reason] || record.reason;
    row.append(reason);
    if (record.detail) {
      const detail = document.createElement("span");
      detail.className = "context-excluded-detail";
      detail.textContent = record.detail;
      row.append(detail);
    }
    if (record.path) {
      const pin = document.createElement("button");
      pin.type = "button";
      pin.className = "btn-icon context-item-action";
      pin.title = "Pin for later turns";
      pin.setAttribute("aria-label", `Pin ${record.path} for later turns`);
      pin.textContent = "📌";
      pin.addEventListener("click", () =>
        addContextPin(record.path, record.lineRange).catch((error) => toast(error.message)),
      );
      row.append(pin);
    }
    excluded.append(row);
  });
  body.append(excluded);
}

async function addContextPin(path, range) {
  const payload = await api(
    "/api/context-pin",
    form({
      session_id: currentSessionId,
      action: "add",
      path,
      start: range ? String(range.start) : "",
      end: range ? String(range.end) : "",
    }),
  );
  contextPins = payload.pins || [];
  renderContextPanel();
  toast(`Pinned ${path} for later turns`);
}

async function removeContextPin(path, range) {
  const payload = await api(
    "/api/context-pin",
    form({
      session_id: currentSessionId,
      action: "remove",
      path,
      start: range ? String(range.start) : "",
      end: range ? String(range.end) : "",
    }),
  );
  contextPins = payload.pins || [];
  renderContextPanel();
}

async function clearContextPins() {
  const payload = await api(
    "/api/context-pin",
    form({ session_id: currentSessionId, action: "clear" }),
  );
  contextPins = payload.pins || [];
  renderContextPanel();
  toast("Pins cleared");
}

async function applyContextRestriction() {
  const paths = $("context-restriction-paths").value.trim();
  if (!paths) {
    // `Some([])` is a restriction that admits no file content, not "no
    // restriction". Removing one is what Clear paths is for, so an empty Apply
    // is refused rather than silently blanking the session.
    throw new Error(
      "An empty list admits no file content. Use Clear paths to remove the restriction.",
    );
  }
  const payload = await api(
    "/api/context-restriction",
    form({ session_id: currentSessionId, paths }),
  );
  contextRestrictionDirty = false;
  contextRestriction = payload.restrictedPaths ?? null;
  syncContextRestrictionField();
  renderContextPanel();
  toast("Session paths applied to later turns");
}

async function clearContextRestriction() {
  const payload = await api(
    "/api/context-restriction",
    form({ session_id: currentSessionId, clear: "true" }),
  );
  contextRestrictionDirty = false;
  contextRestriction = payload.restrictedPaths ?? null;
  syncContextRestrictionField();
  renderContextPanel();
  toast("Session path restriction cleared");
}

function contextItemIndex(item) {
  return (lastContextManifest?.included || []).findIndex(
    (candidate) =>
      candidate.category === item.category &&
      candidate.kind === item.kind &&
      candidate.path === item.path &&
      JSON.stringify(candidate.lineRange || null) === JSON.stringify(item.lineRange || null),
  );
}

async function removeContextIndex(index) {
  if (!currentContextReview) throw new Error("No context review is open");
  if (!Number.isInteger(index) || index < 0) {
    throw new Error("That item is no longer in the review");
  }
  const proposalId = currentContextReview.proposalId;
  const payload = await api(
    "/api/context-remove",
    form({ proposal_id: proposalId, index: String(index) }),
  );
  applyContextReviewManifest(proposalId, payload.manifest);
  setContextManifest(payload.manifest, `review:${proposalId}`);
}

async function removeContextItem(item) {
  const index = contextItemIndex(item);
  if (index < 0) throw new Error("That item is no longer in the review");
  await removeContextIndex(index);
}

async function removeContextSummary() {
  if (!currentContextReview) throw new Error("No context review is open");
  const proposalId = currentContextReview.proposalId;
  const payload = await api("/api/context-remove-summary", form({ proposal_id: proposalId }));
  applyContextReviewManifest(proposalId, payload.manifest);
  setContextManifest(payload.manifest, `review:${proposalId}`);
}

function noteContextPayload(payload) {
  if (payload.context) {
    setContextManifest(payload.context, payload.taskId || "");
  }
  if (payload.contextReview) {
    const card = contextReviewCards.get(payload.contextReview.proposalId);
    if (card) {
      card.review = payload.contextReview;
      card.refresh();
    }
    currentContextReview = payload.contextReview;
    setContextManifest(
      payload.contextReview.manifest,
      `review:${payload.contextReview.proposalId}`,
    );
  }
}

function appendContextLink(body, manifest) {
  if (!manifest) return;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "context-link";
  button.textContent = `Context · ${formatContextTokens(manifest.totalTokens)} tokens`;
  button.title = "Show what this turn was sent";
  button.addEventListener("click", () => {
    currentContextReview = null;
    openContextPanel(manifest, manifest.taskId || "");
  });
  body.append(button);
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
  lastContextManifest = null;
  currentContextReview = null;
  contextReviewCards.clear();
  contextPins = [];
  contextRestriction = null;
  contextRestrictionDirty = false;
  syncContextRestrictionField();
  closeContextPanel();
  renderContextButton();
  void refreshRepoBar();
  // Refresh the roots cache so chips and patch files can qualify by root. A
  // failure here is not fatal: the UI simply shows unqualified names.
  void refreshRepositoryRoots(projectPath).catch(() => {});
  if (!$("settings-shell").hidden) {
    void refreshLanguageServers(projectPath).catch(() => {});
  }
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
  hideWorkersBoard();
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

// The footer exists only while there is an update to offer or a restart to ask
// for; the Settings button covers checking.
let appUpdateRestartPending = false;

function resetUpdateButton(title = "Check Updates") {
  if (appUpdateRestartPending) return;
  $("update-app-footer").hidden = true;
  const button = $("update-app-btn");
  button.hidden = true;
  button.disabled = false;
  button.textContent = "Check Updates";
  button.title = title;
  $("update-restart-notice").hidden = true;
}

function showRestartNotice() {
  appUpdateRestartPending = true;
  $("update-app-footer").hidden = false;
  $("update-app-btn").hidden = true;
  const notice = $("update-restart-notice");
  notice.hidden = false;
  notice.textContent = "Restart Damaian to finish the update";
}

async function checkForAppUpdate(showCurrent = true) {
  // A check mid-install would reset the footer the install is driving.
  if (appUpdateInstalling) return null;
  const updater = tauriUpdater();
  if (!updater?.check) {
    if (showCurrent) toast("Updater is not available in this build");
    return null;
  }
  const button = $("update-app-btn");
  const checkButton = $("check-updates-btn");
  try {
    if (checkButton) {
      checkButton.disabled = true;
      checkButton.textContent = "Checking...";
    }
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
    if (!appUpdateRestartPending) {
      $("update-app-footer").hidden = false;
      button.hidden = false;
    }
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
  } finally {
    if (checkButton) {
      checkButton.disabled = false;
      checkButton.textContent = "Check for updates";
    }
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
    showRestartNotice();
    toast("Update installed. Restart Damaian to finish.", { duration: 7000 });
  } catch (error) {
    appUpdateInstalling = false;
    button.disabled = false;
    button.textContent = appUpdateInfo?.available
      ? appUpdateInfo.version
        ? `Update ${appUpdateInfo.version}`
        : "Update"
      : "Check Updates";
    button.hidden = false;
    toast(`Update failed: ${updaterErrorMessage(error)}`, { duration: 7000 });
  }
}

function setSettingsPage(page) {
  const target = [
    "general",
    "shortcuts",
    "checkpoints",
    "mcp",
    "hooks",
    "providers",
    "models",
  ].includes(page)
    ? page
    : "providers";
  if (target === "mcp") renderMcpConfigSelect();
  if (target === "checkpoints") void renderCheckpointList();
  if (target === "hooks") void renderSettingsHooks();
  if (target === "general") void refreshLanguageServers().catch(() => {});
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
  void refreshRepositoryRoots().catch(() => {});
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

// The provider and model syncing parse the plain policy text; the table is
// drawn from the attributed policy, fetched separately (spec 31 Task 7).
function renderConfigPolicy(payload) {
  if (payload.effectiveError) {
    renderEffectivePolicyError(payload.effectiveError);
    return;
  }
  syncProviderCatalogFromPolicy(payload.effectivePolicy);
  currentPolicyModelOptions = modelOptionsFromPolicy(payload.effectivePolicy);
  syncChatModelControlsFromPolicy(payload.effectivePolicy);
  renderProviderConfigSelect();
  void loadEffectivePolicy().catch((error) => renderEffectivePolicyError(errorMessage(error)));
  void loadCustomPermissionProfiles().catch((error) => toast(errorMessage(error)));
}

// "local" is a name heuristic, and every place that names it says so
// (spec 31 context.md §7).
const LOCAL_COMMANDS_CAVEAT =
  "local means no command Damaian recognises as networked. That is a name heuristic, " +
  "not a sandbox: a command Damaian does not recognise, such as python -c with urllib, " +
  "can still reach the network. It also blocks every npm, pnpm and yarn command.";

const PERMISSION_PROFILE_DESCRIPTIONS = {
  full: "Today's behaviour. The profile narrows nothing; user, repository and admin config decide.",
  safe_local: `File edits need your approval. Commands are limited to local. ${LOCAL_COMMANDS_CAVEAT}`,
  read_only:
    "Reads and explanations only. No file edits, no commands, no browser diagnostics, " +
    "and only read-only MCP tools.",
  offline_private:
    `Commands are limited to local. ${LOCAL_COMMANDS_CAVEAT} No browser diagnostics, no MCP, ` +
    "and audit and checkpoint retention lowered to 7 days. It does not stop model traffic: " +
    "prompts still go to the model provider's base URL.",
};

const POLICY_CLASS_LABELS = {
  restrict_only: "restrict-only",
  forbidden: "forbidden",
  user_owned: "yours to set",
  unparsable: "could not be parsed",
};

// Why a request was refused, in words, for "Refused: … tried to change …".
const POLICY_REFUSAL_REASONS = {
  restrict_only: "It may only make this stricter.",
  forbidden: "It may never set this.",
  user_owned: "Only your own config may set this.",
  unparsable: "The line could not be read.",
};

const POLICY_REFUSED_BY_LABELS = {
  repository: "repository config",
  profile: "the permission profile",
};

let currentEffectivePolicy = null;

async function loadEffectivePolicy() {
  const params = new URLSearchParams({ repo: repo() });
  if (repo() && currentSessionId) params.set("session", currentSessionId);
  renderEffectivePolicy(await api(`/api/effective-policy?${params}`));
}

function renderEffectivePolicyError(message) {
  currentEffectivePolicy = null;
  $("policy-header").textContent = `Effective policy could not be loaded: ${message}`;
  $("policy-table").tBodies[0].replaceChildren();
  $("policy-other-refused").hidden = true;
  renderPermissionProfilePicker();
}

function policyCell(text, className = "") {
  const cell = document.createElement("td");
  cell.textContent = text;
  if (className) cell.className = className;
  return cell;
}

// The plain-language name leads; the config key stays underneath because
// refusal messages and the docs name the key (`allow_file_edits=false`).
function policyRuleCell(rule) {
  const cell = policyCell(rule.label, "policy-key");
  if (rule.label !== rule.key) {
    const key = document.createElement("span");
    key.className = "policy-key-name";
    key.textContent = rule.key;
    cell.append(key);
  }
  return cell;
}

// An empty list reads as "none", except where empty means unrestricted.
function policyEmptyListText(rule) {
  return rule.key === "allowed_roots" ? "any folder" : "none";
}

function policySourceCell(sources, adminWidened) {
  const cell = policyCell(sources.map((source) => source.label).join(", "));
  if (adminWidened) {
    const tag = document.createElement("span");
    tag.className = "policy-tag";
    tag.textContent = "widened by admin config";
    cell.append(" ", tag);
  }
  return cell;
}

function policyNoteRow(text, className) {
  const row = document.createElement("tr");
  row.className = className;
  const cell = policyCell(text);
  cell.colSpan = 3;
  row.append(cell);
  return row;
}

// Key and class only: the policy carries no refused value (context.md §8).
function policyRefusalText(refused) {
  const by = POLICY_REFUSED_BY_LABELS[refused.by] || refused.by;
  const kind = POLICY_REFUSAL_REASONS[refused.class] || `(${refused.class})`;
  const what = refused.label === refused.key ? refused.key : `“${refused.label}” (${refused.key})`;
  return `Refused: ${by} tried to change ${what}. ${kind}`;
}

function policyRuleRows(rule) {
  const rows = [];
  const entries = rule.entries;
  if (entries?.length) {
    // Consecutive entries from one source share a row, as in proposal §5.7's
    // example, so a long default list does not bury the entry that differs.
    const groups = [];
    entries.forEach((entry) => {
      const last = groups[groups.length - 1];
      if (last && last.source.label === entry.source.label) last.values.push(entry.value);
      else groups.push({ source: entry.source, values: [entry.value] });
    });
    groups.forEach((group, index) => {
      const row = document.createElement("tr");
      row.append(
        index === 0 ? policyRuleCell(rule) : policyCell("", "policy-key"),
        policyCell(group.values.join(", "), "policy-value"),
        policySourceCell([group.source], index === 0 && rule.adminWidened),
      );
      rows.push(row);
    });
  } else {
    const row = document.createElement("tr");
    row.append(
      policyRuleCell(rule),
      policyCell(entries ? policyEmptyListText(rule) : rule.value, "policy-value"),
      policySourceCell(rule.sources, rule.adminWidened),
    );
    rows.push(row);
  }
  if (rule.key === "command_access" && (rule.value === "local" || rule.value === "read_only")) {
    rows.push(
      policyNoteRow(
        rule.value === "local"
          ? LOCAL_COMMANDS_CAVEAT
          : "read_only allows only low-risk read-only commands that need no approval.",
        "policy-note",
      ),
    );
  }
  rule.refused.forEach((refused) => {
    rows.push(policyNoteRow(policyRefusalText(refused), "policy-refused"));
  });
  return rows;
}

function renderEffectivePolicy(policy) {
  currentEffectivePolicy = policy;
  $("policy-header").textContent = `Effective policy — ${policy.header}`;
  $("policy-table").tBodies[0].replaceChildren(...policy.rules.flatMap(policyRuleRows));
  const other = $("policy-other-refused");
  other.replaceChildren();
  if (policy.otherRefused.length) {
    const title = document.createElement("p");
    title.textContent = "Other refused requests";
    const list = document.createElement("ul");
    policy.otherRefused.forEach((refused) => {
      const item = document.createElement("li");
      item.textContent = policyRefusalText(refused);
      list.append(item);
    });
    other.append(title, list);
  }
  other.hidden = !policy.otherRefused.length;
  renderPermissionProfilePicker();
}

function isLoopbackUrl(value) {
  let host;
  try {
    host = new URL(value).hostname.toLowerCase();
  } catch {
    return false;
  }
  return (
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host === "[::1]" ||
    /^127(\.\d{1,3}){3}$/.test(host)
  );
}

// The provider the composer has selected is the one the next turn sends to,
// whatever the loaded config says (spec 31 Task 7, deviation 5). An unknown
// URL counts as not loopback, so the warning fails toward showing.
function activeModelBaseUrl(policy) {
  const configured = policy.rules.find((rule) => rule.key === "model_provider")?.value;
  const selected = selectedChatModelOptions().provider;
  if (!configured || normalizeChatProvider(configured) === selected) {
    return policy.rules.find((rule) => rule.key === "model_base_url")?.value || "";
  }
  return modelProviderPresets[selected]?.baseUrl || "";
}

let customPermissionProfiles = [];

async function loadCustomPermissionProfiles() {
  customPermissionProfiles = (await api("/api/permission-profiles")).custom;
  renderPermissionProfilePicker();
}

function renderPermissionProfilePicker() {
  const select = $("permission-profile-select");
  const policy = currentEffectivePolicy;
  for (const option of select.querySelectorAll("option[data-custom]")) option.remove();
  // Every custom file, plus the one in force even if its file has gone, so
  // the picker can still show what the load error is about.
  const custom = [...customPermissionProfiles];
  if (
    policy &&
    !PERMISSION_PROFILE_DESCRIPTIONS[policy.profile] &&
    !custom.includes(policy.profile)
  ) {
    custom.push(policy.profile);
  }
  custom.forEach((id) => {
    const option = document.createElement("option");
    option.value = id;
    option.textContent = `${id} (custom)`;
    option.dataset.custom = "true";
    select.append(option);
  });
  // Enabled even when the policy failed to load, so a checkout whose custom
  // profile file has gone missing can be switched away from it.
  select.disabled = !repo();
  if (policy) select.value = policy.profile;
  const description = $("permission-profile-description");
  if (!repo()) {
    description.textContent = "Open a repository to choose its permission profile.";
  } else if (policy) {
    description.textContent =
      PERMISSION_PROFILE_DESCRIPTIONS[policy.profile] ||
      "Your own profile file. A profile can only narrow: keys that would loosen are " +
        "ignored and listed below as refused.";
  } else {
    description.textContent = "";
  }
  const warning = $("permission-profile-warning");
  const baseUrl = policy?.profile === "offline_private" ? activeModelBaseUrl(policy) : "";
  warning.hidden = policy?.profile !== "offline_private" || isLoopbackUrl(baseUrl);
  if (!warning.hidden) {
    warning.textContent =
      "Offline private does not stop model traffic. Prompts and the code in context still go " +
      `to ${baseUrl || "the model provider"}, which is not a loopback address.`;
  }
}

async function selectPermissionProfile(profile) {
  const fields = { repo: repo(), profile };
  if (currentSessionId) fields.session = currentSessionId;
  renderEffectivePolicy(await api("/api/permission-profile", form(fields)));
  await carryProfileSelectionIntoEditor();
}

async function exportPermissionProfile() {
  const profile = $("permission-profile-select").value;
  const payload = await api(
    `/api/permission-profile-export?profile=${encodeURIComponent(profile)}`,
  );
  const output = $("profile-export-text");
  output.value = payload.text;
  output.hidden = false;
  output.select();
}

// What the review was made for. Import sends exactly this, so editing the
// fields after a review cannot import something nobody reviewed.
let pendingProfileImport = null;

function profileImportFields(preview) {
  const fields = {
    name: pendingProfileImport.name,
    text: pendingProfileImport.text,
    repo: repo(),
    replace: pendingProfileImport.replace ? "true" : "false",
  };
  if (preview) fields.preview = "true";
  return fields;
}

function profileImportList(title, keys, className) {
  const section = document.createElement("div");
  section.className = className;
  const heading = document.createElement("p");
  heading.textContent = `${title}: ${keys.length ? "" : "none"}`;
  section.append(heading);
  if (keys.length) {
    const list = document.createElement("ul");
    keys.forEach((refused) => {
      const item = document.createElement("li");
      item.textContent = `${refused.key} (${POLICY_CLASS_LABELS[refused.class] || refused.class})`;
      list.append(item);
    });
    section.append(list);
  }
  return section;
}

// Key and class only: the preview carries no refused value (context.md §8).
function renderProfileImportReview(review) {
  const carried = document.createElement("p");
  carried.textContent = review.carried.length
    ? `Will write to ${review.profile}: ${review.carried.join(", ")}`
    : "The file sets no key a permission profile can carry, so there is nothing to import.";
  const lists = [
    carried,
    profileImportList(
      "Not written, a profile cannot set these",
      review.notCarried,
      "profile-import-not-carried",
    ),
    profileImportList(
      "Written but no effect here, these would loosen your config and a profile only narrows",
      review.loosening,
      "profile-import-loosening",
    ),
  ];
  const blocked = review.exists && !pendingProfileImport.replace;
  if (blocked) {
    const note = document.createElement("p");
    note.className = "profile-import-not-carried";
    note.textContent = `A custom profile named ${review.profile} already exists. Tick Replace to overwrite it.`;
    lists.push(note);
  }
  $("profile-import-review-lists").replaceChildren(...lists);
  $("profile-import-confirm-btn").disabled = blocked || !review.carried.length;
  $("profile-import-review").hidden = false;
}

async function reviewPermissionProfileImport() {
  pendingProfileImport = {
    name: $("profile-import-name").value.trim(),
    text: $("profile-import-text").value,
    replace: $("profile-import-replace").checked,
  };
  renderProfileImportReview(
    await api("/api/permission-profile-import", form(profileImportFields(true))),
  );
}

function clearProfileImportReview() {
  pendingProfileImport = null;
  $("profile-import-review").hidden = true;
  $("profile-import-review-lists").replaceChildren();
}

async function confirmPermissionProfileImport() {
  if (!pendingProfileImport) return;
  const result = await api("/api/permission-profile-import", form(profileImportFields(false)));
  clearProfileImportReview();
  $("profile-import-text").value = "";
  $("profile-import-name").value = "";
  $("profile-import-replace").checked = false;
  await loadCustomPermissionProfiles();
  toast(
    `${result.replaced ? "Replaced" : "Imported"} ${result.profile}. Choose it in the picker to use it.`,
  );
}

// The selection is a line in user config, and the editor above saves user
// config whole. Without this, editing anything and pressing Save after a pick
// would silently write the old selection back. Only the selection lines are
// carried over, so unsaved edits in the editor survive.
async function carryProfileSelectionIntoEditor() {
  const saved = await api(
    `/api/config-file?scope=${encodeURIComponent(configScope())}&repo=${encodeURIComponent(
      configRepo(),
    )}`,
  );
  configEntries(saved.content)
    .filter(([key]) => key.startsWith("permission_profile."))
    .forEach(([key, value]) => {
      $("config-editor").value = upsertConfigValue($("config-editor").value, key, value);
    });
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
  if (provider === "open-router" || provider === "openrouter") return "openrouter";
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
      supportsNativeTools: provider.supportsNativeTools ?? existing.supportsNativeTools ?? true,
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

// The two composer triggers label themselves with their own current value.
// They were one combined string until spec 44, which is why a long model id
// used to truncate the effort label away.
function modelTriggerLabel(options = selectedChatModelOptions()) {
  return options.model || "Configured";
}

function effortTriggerLabel(options = selectedChatModelOptions()) {
  return reasoningLabels[options.reasoning] || "Default";
}

function modelOptionValues(provider) {
  const preset = modelProviderPresets[provider] || modelProviderPresets["openai-compatible"];
  const selected = selectedChatModelOptions().model;
  return [...new Set([...preset.models, selected].filter(Boolean))];
}

function renderChatModelMenu() {
  const options = selectedChatModelOptions();
  const modelLabel = modelTriggerLabel(options);
  $("model-menu-summary").textContent = modelLabel;
  // The trigger caps its width, so the full id has to stay reachable. The
  // popover's Model row below shows it in full too.
  $("chat-model-menu-btn").title = modelLabel;
  $("effort-menu-summary").textContent = effortTriggerLabel(options);
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

// Both composer triggers open this one popover, each on its own entry panel,
// so `aria-expanded` has to be written to every trigger rather than to one
// named id.
function modelMenuTriggers() {
  return document.querySelectorAll("#chat-model-menu [data-entry-panel]");
}

// Clicking the trigger whose panel is already showing closes the popover;
// clicking the other one switches panel instead, so the two do not fight.
function toggleModelMenu(panel = "root") {
  if ($("chat-model-popover").hidden) {
    openModelMenu(panel);
  } else if ($(`model-menu-${panel}`)?.hidden) {
    showModelMenuPanel(panel);
  } else {
    closeModelMenu();
  }
}

function openModelMenu(panel = "root") {
  renderChatModelMenu();
  $("chat-model-popover").hidden = false;
  for (const trigger of modelMenuTriggers()) {
    trigger.setAttribute("aria-expanded", "true");
  }
  showModelMenuPanel(panel);
}

function closeModelMenu() {
  $("chat-model-popover").hidden = true;
  for (const trigger of modelMenuTriggers()) {
    trigger.setAttribute("aria-expanded", "false");
  }
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
  $("provider-native-tools-input").checked = provider.supportsNativeTools !== false;
  $("provider-max-output-tokens-input").value = provider.maxOutputTokens || "";
  $("provider-context-budget-input").value = provider.contextTokenBudget || "";
  $("provider-remove-btn").disabled = !configuredProviderIds.has(id);
  void showProviderTokenDefaults();
}

function formatTokenCount(value) {
  return Number(value).toLocaleString("en-US");
}

// A blank *Max output tokens* or *Context budget* falls back to a per-model
// default that only the engine knows, so the placeholders ask it for the
// number rather than saying "Automatic". The first listed model is the one a
// new chat uses; the note lists every model when their defaults differ.
async function showProviderTokenDefaults() {
  const providerId = $("provider-id-input").value.trim();
  const models = splitModelList($("provider-models-input").value);
  const params = new URLSearchParams({ provider: providerId, models: models.join(",") });
  let defaults;
  try {
    defaults = (await api(`/api/provider-token-defaults?${params}`)).models || [];
  } catch {
    return;
  }
  // The form may have moved to another provider while the request ran.
  if ($("provider-id-input").value.trim() !== providerId) return;
  const describeOutput = (entry) =>
    entry.maxOutputTokens == null
      ? "none sent, the provider decides"
      : formatTokenCount(entry.maxOutputTokens);
  const first = defaults[0];
  $("provider-max-output-tokens-input").placeholder = first
    ? `Default: ${describeOutput(first)}`
    : "Automatic";
  $("provider-context-budget-input").placeholder = first
    ? `Default: ${formatTokenCount(first.contextTokenBudget)}`
    : "Automatic";
  const note = $("provider-token-defaults-note");
  const distinct = new Set(
    defaults.map((entry) => `${entry.maxOutputTokens}/${entry.contextTokenBudget}`),
  );
  if (distinct.size > 1) {
    note.textContent = `Defaults per model: ${defaults
      .map(
        (entry) =>
          `${entry.model} — output ${describeOutput(entry)}, context ${formatTokenCount(entry.contextTokenBudget)}`,
      )
      .join("; ")}.`;
    note.hidden = false;
  } else {
    note.textContent = "";
    note.hidden = true;
  }
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
  $("provider-native-tools-input").checked = true;
  $("provider-max-output-tokens-input").value = "";
  $("provider-context-budget-input").value = "";
  $("provider-remove-btn").disabled = true;
  void showProviderTokenDefaults();
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
    // The built-in presets carry environment-variable names for the CLI, but a
    // key pasted here can only be saved against a keychain: reference.
    $("provider-key-ref-input").value = preset.apiKeyEnv || `keychain:${provider.id}-api-key`;
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

const HOOK_ACTIVATION_LABELS = {
  active: "active",
  pending_review: "pending review",
  declined: "declined",
  changed_since_review: "changed since review",
  disabled: "disabled",
};

// Spec 32 Task 9: every configured hook, whether it can run, what it runs, and
// its fail direction. A repository hook is switched by review, never by the
// toggle: the repository owns its definition and the user owns the decision.
async function renderSettingsHooks() {
  const container = $("hook-list");
  if (!container) return;
  const repoPath = repo();
  const empty = (text) => {
    container.replaceChildren();
    const row = document.createElement("div");
    row.className = "provider-empty-row";
    row.textContent = text;
    container.append(row);
  };
  if (!repoPath) {
    empty("Open a folder to see the hooks that apply to it.");
    return;
  }
  let payload;
  try {
    payload = await api(`/api/hooks?repo=${encodeURIComponent(repoPath)}`);
  } catch (error) {
    empty(`Hooks could not be loaded: ${error.message}`);
    return;
  }
  if (repo() !== repoPath) return;
  const hooks = Array.isArray(payload.hooks) ? payload.hooks : [];
  if (!hooks.length) {
    empty(
      "No hooks are configured. Add hook.<id>.command and hook.<id>.events to your user configuration, or to this repository's .damaian/config.conf.",
    );
    return;
  }
  container.replaceChildren(...hooks.map((hook) => hookSettingEntry(repoPath, hook)));
}

function hookSettingEntry(repoPath, hook) {
  const entry = document.createElement("div");
  entry.className = "hook-entry";

  const row = document.createElement("div");
  row.className = "settings-row";
  const copy = document.createElement("div");
  copy.className = "hook-row-copy";

  const title = document.createElement("strong");
  title.textContent = hook.id;
  const meta = document.createElement("p");
  const failure = hook.mode === "mandatory" ? "If it fails: blocks" : "If it fails: warns";
  const activation = HOOK_ACTIVATION_LABELS[hook.activation] || hook.activation;
  meta.textContent = `${hook.source} · ${(hook.events || []).join(", ")} · ${failure} · ${activation}`;
  const command = document.createElement("p");
  command.className = "hook-command";
  command.textContent = hook.command + ((hook.args || []).length ? ` ${hook.args.join(" ")}` : "");
  copy.append(title, meta, command);

  const actions = document.createElement("div");
  actions.className = "provider-row-actions hook-row-actions";

  if (hook.source === "user") {
    const toggle = document.createElement("label");
    toggle.className = "field-checkbox hook-toggle";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = hook.enabled === true;
    box.setAttribute("aria-label", `Enable hook ${hook.id}`);
    box.addEventListener("change", async () => {
      box.disabled = true;
      try {
        await api(
          "/api/hook-enabled",
          form({ repo: repoPath, hook: hook.id, enabled: box.checked ? "true" : "false" }),
        );
        toast(`${box.checked ? "Enabled" : "Disabled"} hook ${hook.id}`);
        await renderSettingsHooks();
      } catch (error) {
        box.checked = !box.checked;
        box.disabled = false;
        toast(error.message);
      }
    });
    const label = document.createElement("span");
    label.textContent = "Enabled";
    toggle.append(box, label);
    actions.append(toggle);
  }

  if (
    hook.source === "repository" &&
    ["pending_review", "changed_since_review", "declined"].includes(hook.activation)
  ) {
    const approve = document.createElement("button");
    approve.type = "button";
    approve.className = "btn-sm";
    approve.textContent = "Approve";
    approve.disabled = !hook.hash;
    approve.addEventListener("click", () => decideHook(repoPath, hook, "approve"));
    actions.append(approve);
    // A declined hook is already declined; offering Decline again would be a
    // second, meaningless decision. Approve lets the user reconsider.
    if (hook.activation !== "declined") {
      const decline = document.createElement("button");
      decline.type = "button";
      decline.className = "btn-sm btn-quiet";
      decline.textContent = "Decline";
      decline.addEventListener("click", () => decideHook(repoPath, hook, "decline"));
      actions.append(decline);
    }
  }

  let testResult = null;
  if ((hook.events || []).length) {
    const eventSelect = document.createElement("select");
    eventSelect.className = "mode-select hook-event-select";
    eventSelect.setAttribute("aria-label", `Event to test for hook ${hook.id}`);
    hook.events.forEach((event) => {
      const option = document.createElement("option");
      option.value = event;
      option.textContent = event;
      eventSelect.append(option);
    });
    const test = document.createElement("button");
    test.type = "button";
    test.className = "btn-sm btn-quiet";
    test.textContent = "Test";
    actions.append(eventSelect, test);

    testResult = document.createElement("pre");
    testResult.className = "hook-test-result";
    testResult.hidden = true;
    test.addEventListener("click", () =>
      testHook(repoPath, hook, eventSelect.value, test, testResult),
    );
  }

  row.append(copy, actions);
  entry.append(row);
  if (testResult) entry.append(testResult);
  return entry;
}

async function decideHook(repoPath, hook, decision) {
  try {
    await api(
      "/api/hook-review",
      form({ repo: repoPath, hook: hook.id, decision, hash: hook.hash || "" }),
    );
    toast(`${decision === "approve" ? "Approved" : "Declined"} hook ${hook.id}`);
    await renderSettingsHooks();
  } catch (error) {
    toast(error.message);
  }
}

async function testHook(repoPath, hook, event, button, result) {
  button.disabled = true;
  result.hidden = false;
  result.dataset.state = "";
  result.textContent = "Running…";
  try {
    const run = await api("/api/hook-test", form({ repo: repoPath, hook: hook.id, event }));
    const lines = [`${event}: ${run.verdict}`];
    if (run.failure) lines.push(`failed: ${run.failure}`);
    if (run.stderr) lines.push(run.stderr.trimEnd());
    result.textContent = lines.join("\n");
    result.dataset.state = run.failure ? "error" : run.verdict === "allow" ? "ok" : "warn";
  } catch (error) {
    result.textContent = `${event}: ${error.message}`;
    result.dataset.state = "error";
  } finally {
    button.disabled = false;
  }
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
  return `Saved user key, but effective config still uses ${effectiveReference}. Save the key under that account, change the active provider's API key reference, or remove the model_api_key_env override in admin config.`;
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
  // The toggle writes through the editor's content, so it stays off until
  // that content is the file's: saving an empty editor would empty the file.
  $("worker-sessions-input").disabled = false;
  void refreshConversationAutoCompact();
  void refreshPauseForContextReview();
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
  // Any save can change it, the raw editor's included; the route reports the
  // effective value.
  await refreshWorkersEnabled();
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
  stopWorkerView();
  $("chat-log").innerHTML = "";
  sessionHasRecordedDiagnostics = false;
  // Every caller means "no conversation": a new, deleted or unselected
  // session. The findings belong to the conversation that just left.
  clearFindings();
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

// One disclosure implementation for every caller: builds the trigger, hides
// the panel, and wires `aria-expanded` to it. The caller places both. Always
// starts collapsed and is deliberately never remembered — see
// docs/UI_STYLE_GUIDE.md §7.
function createDisclosure(label, panel) {
  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "disclosure";
  trigger.setAttribute("aria-expanded", "false");
  const caret = document.createElement("span");
  caret.className = "disclosure-caret";
  caret.setAttribute("aria-hidden", "true");
  trigger.append(caret, document.createTextNode(label));
  panel.hidden = true;
  trigger.addEventListener("click", () => {
    const open = trigger.getAttribute("aria-expanded") === "true";
    trigger.setAttribute("aria-expanded", open ? "false" : "true");
    panel.hidden = open;
  });
  return trigger;
}

function appendChatMessage(role, content) {
  const message = document.createElement("article");
  message.className = `message ${role}`;

  // Screen-reader only: the bubble's alignment and background already carry
  // the role visually, and a printed label cost 22px on every message.
  const label = document.createElement("div");
  label.className = "visually-hidden";
  label.textContent =
    role === "assistant"
      ? "Assistant"
      : role === "user" || role === "answer"
        ? "You"
        : role === "question"
          ? "Assistant question"
          : "System";

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
// elapsed clock, and a Stop button when `onStop` is given. A path with no
// working cancel passes none, and gets no button that would do nothing.
//
// Owns its DOM, its interval, and its teardown so nothing outside has to track
// them. `finish` is idempotent, which lets error paths call it blindly.
function startTurnIndicator(target, onStop = null) {
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

  // Streamed command output (requirement 5). Its own element so a line of
  // command output does not replace the phase label it is not.
  const output = document.createElement("span");
  output.className = "turn-indicator-output";
  output.hidden = true;

  // Ticks every second. Inside the aria-live #chat-log it would otherwise be
  // read out once per second, which is unusable.
  const elapsed = document.createElement("span");
  elapsed.className = "turn-indicator-elapsed";
  elapsed.setAttribute("aria-hidden", "true");

  row.append(dot, label, output, elapsed);
  let stop = null;
  if (onStop) {
    stop = document.createElement("button");
    stop.type = "button";
    stop.className = "turn-indicator-stop";
    stop.textContent = "Stop";
    stop.addEventListener("click", () => onStop());
    row.append(stop);
  }
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
    // Read by the usage row, so the turn's duration is the clock the user
    // watched rather than a second timing that could disagree with it.
    startedAt,
    phase(payload) {
      if (done) return;
      if (payload.phase === "output") {
        output.hidden = false;
        output.textContent += payload.label;
        return;
      }
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
      // Spec 59 §5.5: the round label keeps meaning action rounds, and the
      // reads the allowance has covered are shown beside it when there are any.
      const reads =
        payload.phase !== "context" && payload.explorationRound > 0
          ? ` · ${payload.explorationRound} ${payload.explorationRound === 1 ? "read" : "reads"}`
          : "";
      // Spec 65 §5.8: the turn's notes size, beside the read count. A turn
      // that has recorded nothing shows nothing.
      const notes =
        payload.phase !== "context" && payload.notesChars > 0
          ? ` · notes ${
              payload.notesChars >= 1000
                ? `${(payload.notesChars / 1000).toFixed(1)}k`
                : payload.notesChars
            }`
          : "";
      label.textContent = `${text}${rounds}${reads}${notes}`;
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
        stop?.remove();
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
  // Read before the render request: the session may change while it is out,
  // and a block's parent is the session that holds the message.
  const sessionId = currentSessionId;
  try {
    const payload = await api("/api/render-markdown", form({ content, repo: chatRepo || "" }));
    target.body.innerHTML = payload.html;
  } catch (_error) {
    target.body.innerHTML = renderMarkdown(content);
  }
  appendWebDiagnosticArtifacts(target.body, content);
  wireFileReferences(target.body, chatRepo);
  // Only here, never in `updateChatMessage`: a block still streaming can
  // change under the button (spec 57 C8).
  attachWorkerPromptActions(target, sessionId);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
}

// Spec 12 `context.md` §3.3: a session with recorded diagnostics shows its
// screenshots in the diagnostic card, so the path-scraped thumbnails here are
// left to older sessions, which have nothing else. `sessionHasRecordedDiagnostics`
// is reset whenever a session renders, and set by a live `web_diagnostic` event.
function appendWebDiagnosticArtifacts(container, content) {
  if (sessionHasRecordedDiagnostics) return;
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

// The structural pair for an Activity row: an assistant summary immediately
// followed by the tool result it describes. The turn's last assistant is not
// grouped — the answer, plan panel, usage and stop marks belong on it.
function isGroupableAction(message, endsTheTurn) {
  return (
    message.role === "assistant" &&
    Boolean(message.taskId) &&
    message.kind !== "plan_reminder" &&
    !message.content.includes("worker-prompt") &&
    !endsTheTurn(message)
  );
}

// A command's `tool` result as `sandbox_command_context` writes it (spec 68
// §5.1), or null for anything else. STDOUT is greedy, so the split is at the
// last STDERR marker and stdout that quotes one keeps it. A trailing
// `\n\nHook `…` line is the after_tool hook's note, not the command's stderr.
const commandResultPattern =
  /^Command: (.*)\nWorking directory: (.*)\nExit code: (-?\d+|none \(timed out\)|none \(cancelled by user\))\n\nSTDOUT:\n([\s\S]*)\n\nSTDERR:\n([\s\S]*)$/;

function parseCommandResult(text) {
  const match = commandResultPattern.exec(text);
  if (!match) return null;
  const [, command, workingDirectory, exitText, stdout, capturedStderr] = match;
  let exit;
  if (exitText === "none (timed out)") exit = { state: "timed_out", code: null };
  else if (exitText === "none (cancelled by user)") exit = { state: "cancelled", code: null };
  else {
    const code = Number(exitText);
    exit = { state: code === 0 ? "ok" : "failed", code };
  }
  let stderr = capturedStderr;
  let hookNote = null;
  const hook = /\n\n(Hook `[^\n]*)$/.exec(capturedStderr);
  if (hook) {
    hookNote = hook[1];
    stderr = capturedStderr.slice(0, hook.index);
  }
  return { command, workingDirectory, exit, stdout, stderr, hookNote };
}

// The model's reason from `Ran \`<command>\` — <reason>`. The prefix is built
// from the known command rather than split on the first "` — ", because a
// command may itself contain one.
function commandReason(actionContent, command) {
  const prefix = `Ran \`${command}\` — `;
  return actionContent.startsWith(prefix) ? actionContent.slice(prefix.length).trim() : "";
}

// Spec 68 §5.2: `message` is an approval request whose command then ran.
// Both it and the "Ran" action are groupable, all three share a task, the
// result parses as a command result (a declined one never does), the action
// names that command, and the request contains it verbatim.
function approvalRequestFor(message, next, nextResult, endsTheTurn = () => false) {
  if (!requestShaped(message, next, nextResult, endsTheTurn)) return false;
  const parsed = parseCommandResult(nextResult.content);
  if (!parsed) return false;
  return (
    next.content.startsWith(`Ran \`${parsed.command}\``) && message.content.includes(parsed.command)
  );
}

// Every condition of `approvalRequestFor` but the parse: a request whose
// command was declined (or did not run). It stays a bubble, but its task still
// waited on the user.
function unparsedRequestFor(message, next, nextResult, endsTheTurn = () => false) {
  if (!requestShaped(message, next, nextResult, endsTheTurn)) return false;
  if (parseCommandResult(nextResult.content)) return false;
  // Without a parse the command is not known, so try each closing backtick:
  // the action is `Ran \`X\` — …`, and X may itself hold backticks.
  const action = next.content;
  for (let end = action.indexOf("`", 5); end !== -1; end = action.indexOf("`", end + 1)) {
    const command = action.slice(5, end);
    if (command && message.content.includes(command)) return true;
  }
  return false;
}

function requestShaped(message, next, nextResult, endsTheTurn) {
  return Boolean(
    message &&
      next &&
      nextResult &&
      isGroupableAction(message, endsTheTurn) &&
      isGroupableAction(next, endsTheTurn) &&
      next.taskId === message.taskId &&
      nextResult.role === "tool" &&
      nextResult.taskId === message.taskId &&
      next.content.startsWith("Ran `"),
  );
}

const exitMarkText = {
  ok: () => "✓ 0",
  failed: (code) => `exit ${code}`,
  timed_out: () => "timed out",
  cancelled: () => "cancelled",
};

// A parsed command pair as one row: `$ command`, exit mark, reason. The row is
// the trigger for its own collapsed output (spec 68 §3 req. 2–3). Every stored
// string goes in through `textContent`.
function renderCommandStep(step, action, parsed, request) {
  const panel = document.createElement("div");
  panel.className = "activity-command-panel";
  if (request) {
    const approval = document.createElement("div");
    approval.className = "activity-request";
    const label = document.createElement("div");
    label.className = "activity-stream-label";
    label.textContent = "Approval request";
    const text = document.createElement("div");
    text.className = "activity-request-text";
    text.textContent = request.content;
    approval.append(label, text);
    panel.append(approval);
  }
  if (parsed.workingDirectory !== repo()) {
    const cwd = document.createElement("div");
    cwd.className = "activity-command-cwd";
    cwd.textContent = parsed.workingDirectory;
    panel.append(cwd);
  }
  const streams = [
    ["stdout", parsed.stdout],
    ["stderr", parsed.stderr],
  ].filter(([, text]) => text !== "");
  streams.forEach(([name, text]) => {
    const label = document.createElement("div");
    label.className = "activity-stream-label";
    label.textContent = name;
    const output = document.createElement("pre");
    output.className = "activity-step-result";
    output.dataset.stream = name;
    output.textContent = text;
    panel.append(label, output);
  });
  if (!streams.length) {
    const empty = document.createElement("div");
    empty.className = "activity-no-output";
    empty.textContent = "No output";
    panel.append(empty);
  }
  if (parsed.hookNote) {
    const note = document.createElement("div");
    note.className = "activity-hook-note";
    note.textContent = parsed.hookNote;
    panel.append(note);
  }

  const trigger = createDisclosure("", panel);
  trigger.classList.add("activity-command");
  const text = document.createElement("span");
  text.className = "activity-command-text";
  text.textContent = `$ ${parsed.command}`;
  text.title = parsed.command;
  const mark = document.createElement("span");
  mark.className = "exit-mark";
  mark.dataset.state = parsed.exit.state;
  mark.textContent = exitMarkText[parsed.exit.state](parsed.exit.code);
  trigger.append(text, mark);
  if (request) {
    const approved = document.createElement("span");
    approved.className = "activity-approved";
    approved.textContent = "approved";
    trigger.append(approved);
  }
  const reason = commandReason(action.content, parsed.command);
  if (reason) {
    const why = document.createElement("span");
    why.className = "activity-command-reason";
    why.textContent = reason;
    trigger.append(why);
  }
  step.append(trigger, panel);
}

// One collapsed run of action/result pairs. The result is kept as preformatted
// text: tool output is not Markdown, and the exact bytes matter to a diff.
// A pair whose result parses as a command result is drawn as a command row
// instead; anything else keeps #67's title and `<pre>`.
function renderActivityGroup(pairs) {
  const article = document.createElement("article");
  article.className = "message activity";
  const label = document.createElement("div");
  label.className = "visually-hidden";
  label.textContent = "Activity";
  const body = document.createElement("div");
  body.className = "activity-body";
  const detail = document.createElement("div");
  detail.className = "activity-detail";
  detail.tabIndex = 0;
  detail.setAttribute("aria-label", "Activity detail");
  pairs.forEach(({ action, result, request }) => {
    const step = document.createElement("section");
    step.className = "activity-step";
    const parsed = parseCommandResult(result.content);
    if (parsed) {
      renderCommandStep(step, action, parsed, request);
      detail.append(step);
      return;
    }
    const title = document.createElement("div");
    title.className = "activity-step-title";
    title.innerHTML = renderMarkdown(action.content);
    wireFileReferences(title, repo());
    const output = document.createElement("pre");
    output.className = "activity-step-result";
    output.textContent = result.content;
    step.append(title, output);
    detail.append(step);
  });
  const count = pairs.length;
  body.append(
    createDisclosure(`Activity (${count} step${count === 1 ? "" : "s"})`, detail),
    detail,
  );
  article.append(label, body);
  $("chat-log").append(article);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
  return { message: article, body };
}

// Tasks that stopped for the user (spec 68 §5.3): they hold a question, an
// approval request absorbed into its step, or a request that matched every
// absorption condition but the parse (declined). A request still pending at
// reload has no "Ran" step yet; its task's status says it is waiting instead.
function pausedTasksOf(messages, endsTheTurn, tasks = []) {
  const paused = new Set(
    tasks.filter((task) => task.status === "waiting_for_approval").map((task) => task.id),
  );
  messages.forEach((message, index) => {
    if (!message.taskId) return;
    if (message.role === "question") {
      paused.add(message.taskId);
      return;
    }
    const next = messages[index + 1];
    const nextResult = messages[index + 2];
    if (
      approvalRequestFor(message, next, nextResult, endsTheTurn) ||
      unparsedRequestFor(message, next, nextResult, endsTheTurn)
    ) {
      paused.add(message.taskId);
      return;
    }
    // Fetch, MCP, plan and browser-diagnostic requests are not command steps,
    // but their resume paths also leave a non-final assistant message directly
    // followed by another of the same task, with no tool result between.
    if (
      isGroupableAction(message, endsTheTurn) &&
      next?.role === "assistant" &&
      next.taskId === message.taskId
    ) {
      paused.add(message.taskId);
    }
  });
  return paused;
}

// How long a turn took, from the user's message to its last assistant message.
// A paused turn includes the time spent waiting on the user, which is not work,
// so it gets no duration rather than a misleading one.
function turnDurationMs(taskId, messages, pausedTaskIds) {
  if (pausedTaskIds.has(taskId)) return null;
  const inTask = messages.filter((message) => message.taskId === taskId);
  const start = inTask.find((message) => message.role === "user")?.createdAtMs;
  const end = inTask.findLast((message) => message.role === "assistant")?.createdAtMs;
  if (typeof start !== "number" || typeof end !== "number" || end < start) return null;
  return end - start;
}

// The live counterpart of `turnDurationMs`: a turn that ended by asking the
// user (a pending command, fetch, plan or question) is paused, so it gets no
// duration live either, and live agrees with reload.
function liveTurnDurationMs(payload, indicator) {
  const paused =
    payload.commandProposal ||
    payload.fetchProposal ||
    payload.planProposal ||
    payload.questionProposal;
  return paused ? null : Date.now() - indicator.startedAt;
}

// `tasks` carries each task's final status so a stopped turn stays marked as
// one on reload. Without it a truncated answer renders as a complete short one.
// Returns each message with its bubble, in order, for entries placed between
// them afterwards.
function renderMessages(messages, tasks = [], compactions = [], conversation = null) {
  $("chat-log").innerHTML = "";
  // Spec 55 §5.9: the boundary marker says where the model's view begins. The
  // original messages stay below it — nothing was deleted, only not sent.
  const boundary = conversationBoundaryMarker(compactions, conversation);
  let boundaryDrawn = false;
  const drawBoundaryBefore = (message) => {
    if (boundaryDrawn || !boundary) return;
    if (!message.seq || !conversation || conversation.kind === "none") return;
    if (message.seq > conversation.throughSeq) {
      $("chat-log").append(boundary);
      boundaryDrawn = true;
    }
  };
  const cancelledTasks = new Set(
    tasks.filter((task) => task.status === "cancelled").map((task) => task.id),
  );
  const toolBudgetExhaustedTasks = new Set(
    tasks.filter((task) => task.status === "tool_budget_exhausted").map((task) => task.id),
  );
  const tokenBudgetExhaustedTasks = new Set(
    tasks.filter((task) => task.status === "token_budget_exhausted").map((task) => task.id),
  );
  // A task with no plan carries no `plan` field at all, which is how a trivial
  // turn stays panel-free rather than showing an empty plan.
  const planByTask = new Map(tasks.filter((task) => task.plan).map((task) => [task.id, task.plan]));
  // Likewise `webDiagnostics` is absent, not empty, for a task that ran none.
  const diagnosticsByTask = new Map(
    tasks.filter((task) => task.webDiagnostics).map((task) => [task.id, task.webDiagnostics]),
  );
  // A task with no notes carries no `notes` field, so a turn that recorded
  // none stays disclosure-free (spec 65 §5.8).
  const notesByTask = new Map(
    tasks.filter((task) => task.notes).map((task) => [task.id, task.notes]),
  );
  // The newest manifest per task (spec 27 §5.3). Absent for a turn that never
  // recorded an assembly, so an old session stays link-free.
  const contextByTask = new Map(
    tasks.filter((task) => task.context).map((task) => [task.id, task.context]),
  );
  // Set before any message renders: each one runs the legacy thumbnail scan.
  sessionHasRecordedDiagnostics = diagnosticsByTask.size > 0;
  // Whatever a turn appends *after* its answer — a stop row, the plan panel —
  // belongs to the turn, not to each of its messages. A turn that dispatched
  // tools wrote one assistant message per tool before its answer, and marking
  // every one of them put a duplicate "Tool budget exhausted" row under each.
  // The per-message guard inside those helpers could not catch it: each row
  // was the first under its own message.
  const lastAssistantByTask = new Map();
  messages.forEach((message) => {
    if (message.role === "assistant" && message.taskId) {
      lastAssistantByTask.set(message.taskId, message.id);
    }
  });
  const endsTheTurn = (message) => lastAssistantByTask.get(message.taskId) === message.id;
  const pausedTaskIds = pausedTasksOf(messages, endsTheTurn, tasks);
  // A task with no usage events carries no token fields at all, which is how
  // a session written before token accounting stays silent instead of
  // claiming a free turn.
  const usageByTask = new Map(
    tasks.filter((task) => typeof task.inputTokens === "number").map((task) => [task.id, task]),
  );
  // A turn's intermediate work is written as an assistant summary followed by
  // its tool result. Draw each consecutive run of those pairs as one collapsed
  // Activity instead of one full bubble per action. This is structural, so it
  // works on every log already written, and the last assistant of a task is
  // excluded: the answer, plan, usage and stop marks stay on their own message.
  const activityGroupByStart = new Map();
  const groupedMessageIds = new Set();
  for (let index = 0; index < messages.length; index += 1) {
    const first = messages[index];
    if (!isGroupableAction(first, endsTheTurn)) continue;
    const pairs = [];
    let cursor = index;
    while (cursor + 1 < messages.length) {
      // An approval request whose command then ran joins the run inside that
      // step, rather than as its own bubble (spec 68 §5.2).
      const request = messages[cursor];
      if (approvalRequestFor(request, messages[cursor + 1], messages[cursor + 2], endsTheTurn)) {
        pairs.push({ action: messages[cursor + 1], result: messages[cursor + 2], request });
        groupedMessageIds.add(request.id);
        groupedMessageIds.add(messages[cursor + 1].id);
        groupedMessageIds.add(messages[cursor + 2].id);
        cursor += 3;
        continue;
      }
      const action = messages[cursor];
      const result = messages[cursor + 1];
      if (!isGroupableAction(action, endsTheTurn)) break;
      if (result.role !== "tool" || result.taskId !== action.taskId) break;
      pairs.push({ action, result });
      groupedMessageIds.add(action.id);
      groupedMessageIds.add(result.id);
      cursor += 2;
    }
    if (pairs.length) {
      activityGroupByStart.set(first.id, pairs);
      index = cursor - 1;
    }
  }
  const rendered = messages
    .map((message, index) => {
      const activity = activityGroupByStart.get(message.id);
      if (activity) {
        drawBoundaryBefore(message);
        const bubble = renderActivityGroup(activity);
        if (message.seq) bubble.message.dataset.seq = message.seq;
        return { message, bubble };
      }
      if (groupedMessageIds.has(message.id)) return null;
      drawBoundaryBefore(message);
      // The engine's one plan reminder is a user-role message in the request but
      // not something the user typed. Render it as a system line, and not as an
      // answer, a rewind anchor, or a question (spec 66 `context.md` D10).
      // A memory refusal is the same kind of engine line: it explains why a
      // "remember that…" became no card (spec 29 §5.5).
      if (message.kind === "plan_reminder" || message.kind === "memory_refusal") {
        const bubble = appendChatMessage("system", message.content);
        if (message.seq) bubble.message.dataset.seq = message.seq;
        return { message, bubble };
      }
      if (message.role === "question") {
        // The answer event immediately follows its question in the log, so the
        // next message is what settles this card. Options ride on the question
        // message from `/api/session`; without them a reload could show the text
        // but could not answer it.
        const next = messages[index + 1];
        const answered =
          next && next.role === "answer" && next.taskId === message.taskId ? next.content : null;
        const article = document.createElement("article");
        article.className = "message question";
        if (message.seq) article.dataset.seq = message.seq;
        const label = document.createElement("div");
        label.className = "visually-hidden";
        label.textContent = "Assistant question";
        const card = createQuestionCard(
          {
            questionId: message.id,
            question: message.content,
            options: message.options || [],
            allowFreeText: message.allowFreeText === true,
          },
          repo(),
          { answered },
        );
        article.append(label, card);
        $("chat-log").append(article);
        return { message, bubble: { message: article, body: card } };
      }
      const bubble = appendChatMessage(message.role, message.content);
      if (message.seq) bubble.message.dataset.seq = message.seq;
      if (message.role === "user") {
        const checkpoint = sessionCheckpoints.find((entry) => entry.userMessageId === message.id);
        if (checkpoint) markMessageRewindable(bubble, checkpoint);
      }
      if (message.role === "assistant") {
        // The context link is added after the markdown render, because
        // `finalizeChatMessage` replaces the body's children.
        const finalized = finalizeChatMessage(bubble, message.content);
        if (message.taskId && endsTheTurn(message)) {
          if (cancelledTasks.has(message.taskId)) {
            markMessageStopped(bubble);
          } else if (toolBudgetExhaustedTasks.has(message.taskId)) {
            markMessageToolBudgetExhausted(bubble, message.sessionId, message.taskId);
          } else if (tokenBudgetExhaustedTasks.has(message.taskId)) {
            markMessageTokenBudgetExhausted(bubble);
          }
          if (planByTask.has(message.taskId)) {
            renderPlanPanel(bubble, planByTask.get(message.taskId));
          }
          (diagnosticsByTask.get(message.taskId) || []).forEach((record) => {
            renderWebDiagnosticCard(bubble, record);
          });
          appendTurnNotes(bubble.body, notesByTask.get(message.taskId));
          void finalized.then(() =>
            appendContextLink(bubble.body, contextByTask.get(message.taskId)),
          );
        }
        // Drawn once, under the turn's last assistant message, rather than under
        // each of the turn's messages (spec 68 §3 req. 1).
        if (message.taskId && endsTheTurn(message)) {
          markMessageUsage(
            bubble,
            usageByTask.get(message.taskId),
            turnDurationMs(message.taskId, messages, pausedTaskIds),
          );
        }
      }
      return { message, bubble };
    })
    .filter(Boolean);
  if (!boundaryDrawn && boundary) {
    // The summary covers everything (or the log predates `seq`): the boundary
    // is the top of the thread, not the bottom of it.
    $("chat-log").prepend(boundary);
  }
  return rendered;
}

// The marker is data from `/api/session`; it names how many messages the model
// no longer sees and, when a summary exists, lets the user read it.
function conversationBoundaryMarker(compactions, conversation) {
  if (!conversation) return null;
  if (conversation.kind === "none" && !conversation.lastFailure) return null;
  const article = document.createElement("article");
  article.className = "message system compaction-boundary";
  const body = document.createElement("div");
  body.className = "message-body";
  const details = document.createElement("details");
  const summaryLine = document.createElement("summary");
  const omitted = conversation.omittedMessages || 0;
  if (conversation.kind === "none" && conversation.lastFailure) {
    summaryLine.textContent = `Compaction could not run: ${conversation.lastFailure}`;
    details.append(summaryLine);
  } else if (conversation.kind === "elision") {
    summaryLine.textContent = `${omitted} earlier message${omitted === 1 ? "" : "s"} were not sent to the model and were not summarised. The full transcript is still here.`;
    details.append(summaryLine);
  } else {
    const summary = compactions[compactions.length - 1] || {};
    summaryLine.textContent = `${omitted} earlier message${omitted === 1 ? "" : "s"} are represented by a summary the model receives instead. The full transcript is below.`;
    details.append(summaryLine);
    details.append(summaryBody(summary));
  }
  body.append(details);
  if (conversation.lastFailure && conversation.kind !== "none") {
    const failure = document.createElement("p");
    failure.className = "compaction-failure";
    failure.textContent = `The last compaction could not run: ${conversation.lastFailure}`;
    body.append(failure);
  }
  if (conversation.canCompact) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn-quiet compaction-now";
    button.textContent = "Compact now";
    button.addEventListener("click", () => void compactConversationNow());
    body.append(button);
  }
  article.append(body);
  return article;
}

function summaryBody(summary) {
  const container = document.createElement("div");
  container.className = "compaction-summary";
  const section = (heading, lines) => {
    if (!lines || lines.length === 0) return;
    const title = document.createElement("p");
    title.className = "compaction-summary-heading";
    title.textContent = heading;
    container.append(title);
    const list = document.createElement("ul");
    lines.forEach((line) => {
      const item = document.createElement("li");
      item.textContent = line;
      list.append(item);
    });
    container.append(list);
  };
  if (summary.objective) section("Objective", [summary.objective]);
  section("Requirements and constraints", summary.constraints);
  section("Decisions", summary.decisions);
  section("Failed approaches", summary.failedApproaches);
  section("Remaining uncertainty", summary.uncertainty);
  if (summary.narrativeCarriedForward) {
    const note = document.createElement("p");
    note.className = "compaction-summary-note";
    note.textContent =
      "The last summarisation call failed, so these model-written fields are carried forward from the previous summary.";
    container.append(note);
  }
  section("Files changed", summary.filesChanged);
  section(
    "Plan steps",
    (summary.planSteps || []).map((step) => `${step.title} — ${step.status.replaceAll("_", " ")}`),
  );
  section(
    "Approvals",
    (summary.approvals || []).map(
      (approval) => `${approval.kind} ${approval.id}: ${approval.decision}`,
    ),
  );
  section(
    "Validation",
    (summary.validation || []).map((item) => `${item.label}: ${item.outcome}`),
  );
  return container;
}

async function compactConversationNow() {
  if (!currentSessionId || !repo()) return;
  try {
    const payload = await api("/api/compact", form({ repo: repo(), session_id: currentSessionId }));
    if (payload.kind === "compacted") {
      toast("Conversation compacted. The full transcript is still here.");
    } else if (payload.kind === "nothing") {
      toast("Nothing older than the conversation window to compact.");
    } else if (payload.kind === "cancelled") {
      toast("Compaction stopped.");
    } else {
      toast(`Compaction did not run: ${payload.reason || "unknown reason"}`);
    }
    await loadSession(currentSessionId);
  } catch (error) {
    toast(errorMessage(error));
  }
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

// Crash recovery, per docs/specs/45_crash_recovery_prompt.md.
//
// The sweep itself runs once per process on the server; this fetches its result
// once and keeps it, because the cards are rendered per session as sessions are
// opened. Nothing here decides what may be resumed: the card offers what the
// engine's classification said it may, and every decision is re-classified
// server-side before it is applied.
let recoveredTasks = [];
let reattachedApprovals = [];
let pendingVerifications = [];
let recoverySweepRequest = null;
// Spec 29: proposals still waiting for the user's decision, read from the
// session log so a reload shows exactly what was left undecided. They are not
// transcript messages and never enter the model's conversation.
let memoryProposals = [];
let memoryUserScopeEnabled = false;

function loadRecoverySweep() {
  if (!recoverySweepRequest) {
    // Every `/api/` route is token-gated and the token arrives with the Tauri
    // bootstrap, so a sweep fired at load time would otherwise race it and be
    // rejected — once, permanently, since the result is memoized.
    recoverySweepRequest = ensureDesktopApiReady()
      .then(() => api("/api/recovery"))
      .then((payload) => {
        recoveredTasks = Array.isArray(payload.tasks) ? payload.tasks : [];
        reattachedApprovals = Array.isArray(payload.approvals) ? payload.approvals : [];
        pendingVerifications = Array.isArray(payload.verifications) ? payload.verifications : [];
      })
      .catch(() => {
        // A sweep that cannot be read must not stop a session from opening.
        // The engine has already classified and audited either way. Cleared
        // rather than kept so opening the next session tries again: the
        // alternative is one bad moment at startup hiding every recovered task
        // for the rest of the run.
        recoveredTasks = [];
        reattachedApprovals = [];
        pendingVerifications = [];
        recoverySweepRequest = null;
      });
  }
  return recoverySweepRequest;
}

async function renderRecoveryPrompts(sessionId) {
  if (!sessionId) return;
  await loadRecoverySweep();
  if (sessionId !== currentSessionId) return;
  // C5: a worker's turn is never resumed in the shell, whose sweep cannot tell
  // a worker's live turn from a crashed one. Its strip offers *Restart worker*.
  const resumable = !(workersEnabled && isWorkerSession(sessionId));
  const cards = [
    ...recoveredTasks
      .filter((task) => resumable && task.sessionId === sessionId)
      .map(createRecoveryCard),
    ...reattachedApprovals
      .filter((approval) => approval.sessionId === sessionId)
      .map(createReattachedApprovalCard),
    ...pendingVerifications
      .filter((due) => due.sessionId === sessionId)
      .map(createVerificationOfferCard),
  ];
  // Prepended in reverse so the cards end up above the conversation in the
  // order they were built.
  cards.reverse().forEach((card) => {
    $("chat-log").prepend(card);
  });
}

// Spec 29 §5.7: the pending-proposal stack. It is rendered after the turn's
// messages, not inside them, and collapses to a count when more than one is
// waiting so the completion report is not buried under suggestion cards.
async function refreshMemoryProposals(sessionId = currentSessionId) {
  const log = $("chat-log");
  log.querySelector(".memory-proposal-stack")?.remove();
  if (!sessionId || !repo()) {
    memoryProposals = [];
    memoryUserScopeEnabled = false;
    return;
  }
  try {
    const payload = await api(`/api/memory-proposals?session_id=${encodeURIComponent(sessionId)}`);
    memoryProposals = Array.isArray(payload.proposals) ? payload.proposals : [];
    memoryUserScopeEnabled = payload.userScopeEnabled === true;
  } catch {
    // A proposal list that cannot be read must not stop the conversation from
    // rendering. It is simply absent until the next load.
    memoryProposals = [];
  }
  renderMemoryProposals();
}

function renderMemoryProposals() {
  const log = $("chat-log");
  log.querySelector(".memory-proposal-stack")?.remove();
  if (!memoryProposals.length) return;
  const stack = document.createElement("section");
  stack.className = "memory-proposal-stack";
  if (memoryProposals.length > 1) {
    // More than one suggestion would otherwise push the completion report up
    // the page. Collapse to a count, and let the user open the list.
    const details = document.createElement("details");
    details.className = "memory-proposal-details";
    const summary = document.createElement("summary");
    summary.className = "memory-proposal-heading";
    summary.textContent = `${memoryProposals.length} memories to confirm`;
    details.append(summary);
    memoryProposals.forEach((proposal) => {
      details.append(createMemoryProposalCard(proposal));
    });
    stack.append(details);
  } else {
    const heading = document.createElement("p");
    heading.className = "memory-proposal-heading";
    heading.textContent = "A memory to confirm";
    stack.append(heading, createMemoryProposalCard(memoryProposals[0]));
  }
  log.append(stack);
}

function createMemoryProposalCard(proposal) {
  const card = document.createElement("article");
  card.className = "memory-proposal-card";

  const statement = document.createElement("p");
  statement.className = "memory-proposal-statement";
  statement.textContent = proposal.statement || "";
  card.append(statement);

  const meta = document.createElement("p");
  meta.className = "memory-proposal-meta";
  const scope = proposal.scope || "project";
  meta.textContent = `${scope} memory · ${proposal.category || "fact"} · ${
    proposal.sourceLabel || proposal.originKind || "suggested"
  }`;
  card.append(meta);

  const controls = document.createElement("div");
  controls.className = "memory-proposal-controls";

  const scopeSelect = document.createElement("select");
  scopeSelect.className = "memory-proposal-scope";
  scopeSelect.setAttribute("aria-label", "Memory scope");
  // Only scopes this proposal may actually be confirmed as: a project fact
  // cannot silently become session- or user-scope, and a user-scope proposal
  // may be narrowed to project when the user scope is off.
  const allowedScopes =
    proposal.scope === "user"
      ? memoryUserScopeEnabled
        ? ["project", "user"]
        : ["project"]
      : proposal.scope === "session"
        ? ["session"]
        : ["project"];
  allowedScopes.forEach((value) => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    if (value === "user" && !memoryUserScopeEnabled) {
      option.disabled = true;
      option.textContent = "user (off)";
    }
    scopeSelect.append(option);
  });
  // A user-scope proposal while the scope is off offers project scope instead;
  // saving it must not flip the setting as a side effect.
  scopeSelect.value =
    proposal.scope === "user" && !memoryUserScopeEnabled ? "project" : proposal.scope;
  controls.append(scopeSelect);

  const confirm = document.createElement("button");
  confirm.type = "button";
  confirm.className = "btn-primary";
  confirm.textContent = "Remember";
  confirm.addEventListener("click", () => {
    // One decision per proposal. Disabling only this button let a quick second
    // click on Dismiss race the confirmation, and the store then had a
    // Remember and a Dismiss for one card.
    setCardDeciding(controls, true);
    decideMemoryProposal(proposal, "confirm", scopeSelect.value).catch((error) => {
      setCardDeciding(controls, false);
      toast(error.message);
      // A decision may have landed from a click that raced this one; re-read
      // the pending list so an already-decided card does not stay on screen.
      void refreshMemoryProposals(currentSessionId);
    });
  });
  controls.append(confirm);

  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.className = "btn-secondary";
  dismiss.textContent = "Dismiss";
  dismiss.addEventListener("click", () => {
    setCardDeciding(controls, true);
    decideMemoryProposal(proposal, "dismiss").catch((error) => {
      setCardDeciding(controls, false);
      toast(error.message);
      void refreshMemoryProposals(currentSessionId);
    });
  });
  controls.append(dismiss);

  card.append(controls);

  if (proposal.scope === "user" && !memoryUserScopeEnabled) {
    const note = document.createElement("p");
    note.className = "memory-proposal-note";
    note.textContent =
      "User scope is off. Remembering this saves it for this project; enable " +
      "user scope separately by setting memory_user_scope_enabled=true in your " +
      "user.conf (see the user guide). It then affects every repository.";
    card.append(note);
  }
  return card;
}

// One decision per card. While a decision is in flight both buttons and the
// scope picker stop accepting input, so a second click cannot race the first;
// a failed request re-enables them because nothing was decided.
function setCardDeciding(controls, deciding) {
  controls.querySelectorAll("button, select").forEach((control) => {
    control.disabled = deciding;
  });
}

async function decideMemoryProposal(proposal, decision, scope) {
  const body = {
    repo: requireRepo(),
    session_id: currentSessionId,
    proposal_id: proposal.id,
  };
  if (decision === "confirm") {
    body.scope = scope;
    await api("/api/memory-confirm", form(body));
    toast("Memory saved");
  } else {
    await api("/api/memory-dismiss", form(body));
    toast("Memory suggestion dismissed");
  }
  await refreshMemoryProposals(currentSessionId);
}

// The prompt for one recovered task. Two shapes, per §5.6: with `Resume` there
// is one filled action; without it there is none at all, because the surface is
// asking the user to decide something Damaian could not.
function createRecoveryCard(task) {
  const wrapper = document.createElement("section");
  wrapper.className = "recovery-card";
  wrapper.dataset.taskId = task.taskId;

  const eyebrow = document.createElement("p");
  eyebrow.className = "recovery-card-eyebrow";
  eyebrow.textContent = task.autoResumed ? "Picked up after a crash" : "After a crash";

  const headline = document.createElement("p");
  headline.className = "recovery-card-headline";
  headline.textContent = task.headline;

  const note = document.createElement("p");
  note.className = "recovery-card-note";
  if (!task.resumeAllowed) {
    note.textContent = task.resumeBlockedReason || "";
  } else if (task.autoResumed) {
    note.textContent =
      "Nothing that could have changed anything outside Damaian was in flight, so this turn " +
      "was marked safe to pick up again. It has not run yet — continuing sends the same " +
      "request to the model again.";
  } else {
    note.textContent = "Continuing sends the same request to the model again.";
  }

  const spend = recoverySpendLine(task);

  const footer = document.createElement("div");
  footer.className = "recovery-card-footer";
  const detail = createRecoveryDetail(task);
  const actions = document.createElement("div");
  actions.className = "inline-actions";

  const close = async (decision, label) => {
    try {
      await decideRecovery(task, decision);
      wrapper.remove();
      toast(label);
    } catch (error) {
      toast(error.message);
    }
  };

  const markFailed = async () => {
    const confirmed = await confirmDialog(
      "Mark this turn failed?",
      `${task.headline}. Marking it failed closes it out and records that its outcome was ` +
        "unknown. Nothing on disk is changed or undone.",
      { danger: true, confirmLabel: "Mark failed" },
    );
    if (confirmed) await close("mark_failed", "Turn marked failed");
  };

  const abandon = async () => {
    const confirmed = await confirmDialog(
      "Abandon this turn?",
      `${task.headline}. Abandoning closes the turn without retrying it. Nothing on disk is ` +
        "changed or undone.",
      { danger: true, confirmLabel: "Abandon" },
    );
    if (confirmed) await close("abandon", "Turn abandoned");
  };

  if (task.resumeAllowed) {
    const resume = document.createElement("button");
    resume.type = "button";
    resume.className = "btn-sm btn-primary";
    resume.textContent = task.autoResumed ? "Continue" : "Resume";
    resume.addEventListener("click", async () => {
      resume.disabled = true;
      try {
        const payload = await decideRecovery(task, "resume");
        wrapper.remove();
        // The prompt comes back from the server rather than from this card, so
        // what is re-sent is what the session log recorded.
        await sendChatPrompt({ prompt: payload.prompt, restorePrompt: false });
      } catch (error) {
        resume.disabled = false;
        toast(error.message);
      }
    });
    actions.append(resume);
  } else {
    // The only remaining answer stays quiet: filling it would make the loudest
    // control on the surface a terminal one.
    const failed = document.createElement("button");
    failed.type = "button";
    failed.className = "btn-sm btn-quiet";
    failed.textContent = "Mark failed";
    failed.addEventListener("click", () => void markFailed());
    actions.append(failed);
  }

  // Terminal choices live in the overflow: they outlive this interaction, and
  // the cost of a mis-click is not symmetric with resuming.
  const menuItems = [];
  if (task.resumeAllowed) {
    menuItems.push({
      label: "Mark failed",
      hint: "Closes the turn, recording that its outcome was unknown",
      onSelect: () => void markFailed(),
    });
  }
  menuItems.push({
    label: "Abandon",
    hint: "Closes the turn without retrying it",
    onSelect: () => void abandon(),
  });
  const overflow = document.createElement("button");
  overflow.type = "button";
  overflow.className = "btn-icon";
  overflow.dataset.menuId = `recovery-${task.taskId}`;
  overflow.setAttribute("aria-haspopup", "menu");
  overflow.setAttribute("aria-expanded", "false");
  overflow.setAttribute("aria-label", "More recovery options");
  overflow.title = "More recovery options";
  overflow.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
  overflow.addEventListener("click", (event) => {
    event.stopPropagation();
    toggleApprovalMenu(menuItems, overflow);
  });
  actions.append(overflow);

  footer.append(createDisclosure("Inspect", detail), actions);
  wrapper.append(eyebrow, headline);
  if (note.textContent) wrapper.append(note);
  if (spend) wrapper.append(spend);
  wrapper.append(detail, footer);
  return wrapper;
}

// What the interrupted turn had already spent, per
// docs/specs/19_token_and_cost_accounting/. A crash that cost money must not
// read as free, and the figure the sweep records for the call that was in
// flight is an estimate — so it is marked as one, the same way a turn's own
// usage line is.
//
// Returns null when nothing was recorded, which is not the same as zero: a
// session written before spec 19 has no figures, and a "0 tokens" line would
// claim the crash was free.
function recoverySpendLine(task) {
  if (typeof task.inputTokens !== "number") return null;
  const row = document.createElement("p");
  row.className = "recovery-card-note recovery-card-spend";
  const estimated = task.usageSource === "estimated";
  const total = task.inputTokens + task.outputTokens;
  const calls = task.runCount === 1 ? "1 model call" : `${task.runCount} model calls`;
  const parts = [
    `${estimated ? "~" : ""}${total.toLocaleString()} tokens${estimated ? " (estimated)" : ""}`,
    calls,
  ];
  if (typeof task.reportedCost === "number") {
    parts.push(formatCost(task.reportedCost));
  } else {
    const atYourRates = formatCostAtYourRates(task);
    if (atYourRates) parts.push(atYourRates);
  }
  const cached = formatCacheHitRate(task);
  if (cached) parts.push(cached);
  const cacheTitle = cacheUsageTitle(task);
  if (cacheTitle) row.title = cacheTitle;
  const spent = `Spent before it stopped: ${parts.join(" · ")}.`;
  row.textContent = task.includesLostCall
    ? `${spent} That includes the model call that was still in flight, which was billed even ` +
      "though its answer was lost."
    : spent;
  return row;
}

// What `Inspect` shows: the turn, the action that never finished, the files it
// may have touched, and its checkpoint. Restoring is spec 16's surface, so the
// checkpoint row opens the existing rewind dialog rather than restoring here.
function createRecoveryDetail(task) {
  const panel = document.createElement("dl");
  panel.className = "recovery-card-detail";

  const row = (label, value) => {
    const term = document.createElement("dt");
    term.textContent = label;
    const description = document.createElement("dd");
    if (typeof value === "string") {
      description.textContent = value;
    } else {
      description.append(value);
    }
    panel.append(term, description);
  };

  row("Your request", task.prompt || "Not recorded for this turn");
  row(
    "Action in flight",
    task.danglingAction
      ? `${task.danglingAction}${task.danglingRef ? ` — ${task.danglingRef}` : ""} (event ${task.danglingSeq})`
      : `None recorded. The turn's last recorded state was "${task.previousStatus}".`,
  );

  // A commit that was in flight is not a file-edit question: the record says
  // exactly which commit object may exist and where HEAD is now. The comparison
  // is shown, never applied (spec 35 §5.7).
  if (task.commit) {
    const short = (sha) => (sha ? sha.slice(0, 8) : "no commit");
    const outcome =
      task.commit.landed === true
        ? "It landed."
        : task.commit.landed === false
          ? "It did not land."
          : "HEAD is somewhere else, so whether it landed cannot be decided from here.";
    row(
      "Commit in flight",
      `Prepared ${short(task.commit.preparedCommit)} on top of ${short(task.commit.expectedOld)}. ` +
        `HEAD is now ${short(task.commit.headNow)}. ${outcome}`,
    );
  }

  const files = Array.isArray(task.files) ? task.files : [];
  if (files.length) {
    const list = document.createElement("ul");
    list.className = "recovery-card-files";
    files.forEach((path) => {
      const item = document.createElement("li");
      const code = document.createElement("code");
      code.textContent = path;
      item.append(code);
      list.append(item);
    });
    row("Files it may have touched", list);
  } else {
    row(
      "Files it may have touched",
      task.danglingAction === "run_command" || task.danglingAction === "mcp_call"
        ? "Not knowable — Damaian does not track what a command or a remote tool changed."
        : "None recorded.",
    );
  }

  const checkpoint = sessionCheckpoints.find((entry) => entry.checkpointId === task.checkpointId);
  if (checkpoint) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn-sm btn-quiet";
    button.textContent = `Rewind to before this turn (${checkpoint.fileCount} ${
      checkpoint.fileCount === 1 ? "file" : "files"
    })`;
    button.addEventListener("click", () => void rewindToCheckpoint(checkpoint));
    row("Checkpoint", button);
  } else {
    row("Checkpoint", "No checkpoint was taken for this turn.");
  }

  return panel;
}

// An approval that survived the crash with its proposal (spec 17 §5.5). The
// card is the same component the live turn uses, so re-presenting it runs
// nothing: the user still has to approve.
function createReattachedApprovalCard(approval) {
  const wrapper = document.createElement("section");
  wrapper.className = "recovery-card";
  wrapper.dataset.taskId = approval.taskId;

  const eyebrow = document.createElement("p");
  eyebrow.className = "recovery-card-eyebrow";
  eyebrow.textContent = "Still waiting for you";

  const headline = document.createElement("p");
  headline.className = "recovery-card-headline";
  const note = document.createElement("p");
  note.className = "recovery-card-note";

  if (approval.kind === "unavailable") {
    headline.textContent = "An approval from before the crash could not be restored";
    note.textContent =
      `${approval.unavailableReason || "The proposal is no longer readable."} The turn was ` +
      "closed rather than rebuilt from partial data — approving a command Damaian is " +
      "guessing at would be worse than losing the turn.";
    wrapper.append(eyebrow, headline, note);
    return wrapper;
  }

  // A question reattached after a crash is answered by the same card a live or
  // reloaded question uses. It carries no proposal id, so nothing here can
  // reach a command or patch endpoint; answering resumes the paused turn
  // through `/api/answer-question-stream`.
  if (approval.kind === "question") {
    const payload = approval.payload || {};
    headline.textContent = payload.headline || "A question was waiting for your answer";
    note.textContent = "Answering continues the turn that asked it. It authorises nothing.";
    wrapper.append(eyebrow, headline, note);
    wrapper.append(
      createQuestionCard(
        {
          questionId: payload.questionId,
          question: payload.question,
          options: payload.options,
          allowFreeText: payload.allowFreeText,
        },
        repo(),
        { answered: payload.answered ? (payload.answer?.answer ?? "") : null },
      ),
    );
    return wrapper;
  }

  const headlines = {
    command: "A command was waiting for your approval when Damaian stopped",
    patch: "A patch was waiting for your review when Damaian stopped",
    plan: "A plan was waiting for your review when Damaian stopped",
    provider_fallback: "An offer to switch providers was waiting when Damaian stopped",
    round_budget: "A turn was waiting for your decision when Damaian stopped",
    context_review: "A turn was waiting for you to review its context when Damaian stopped",
    fetch: "A page read was waiting for your approval when Damaian stopped",
    checks: "Checks were waiting for your approval when Damaian stopped",
  };
  const notes = {
    command:
      "Approving runs the command on its own: the conversation that proposed it ended with " +
      "the crash, so nothing is fed back to the model.",
    patch: "The patch is still on disk and still applies. Reviewing it here changes nothing else.",
    // The one reattached approval that genuinely continues: a paused plan
    // review keeps its turn on disk, so approving picks the work back up
    // rather than running something in isolation.
    plan:
      "The turn that raised this plan was paused waiting for you, and it was saved. Approving " +
      "the plan continues that turn from where it stopped.",
    // Saved like a plan review, so answering continues the same turn.
    provider_fallback:
      "A provider refused this turn, and the turn was saved while you decided. Keeping it " +
      "failed closes the turn; answering with the other provider continues it, for this turn " +
      "only.",
    // Saved like a plan review, so answering continues the same turn. Only the
    // rounds are known after a restart, not the full offer.
    round_budget:
      "This turn reached its round budget and was saved while you decided. Continue grants " +
      "it more rounds; Answer now ends it.",
    context_review:
      "This turn was assembled but not sent, and it was saved while you reviewed it. " +
      "Sending continues that turn; declining sends nothing.",
    // Saved like a plan review, so answering continues the same turn. Nothing
    // was requested before the crash: the dialog is the same question.
    fetch:
      "This turn was paused before it requested a page, and it was saved while you decided. " +
      "Allowing continues that turn and makes the request; Don't fetch sends nothing.",
    checks:
      "The verification of an applied patch stopped on this card. Deciding it continues the " +
      "same attempt; the report follows when every check is answered.",
  };
  headline.textContent = headlines[approval.kind];
  note.textContent = notes[approval.kind];

  wrapper.append(eyebrow, headline, note);
  if (approval.kind === "command") {
    wrapper.append(
      createCommandApprovalPreview(approval.payload, repo(), {
        detached: true,
        // A command is one decision, so its outcome is the decision.
        onResolved: (approved) =>
          resolveReattachedApproval(approval, approved ? "approved" : "rejected"),
      }),
    );
  } else if (approval.kind === "plan") {
    // No `onResolved`: resuming the plan resumes the turn that owns the task,
    // which reaches a terminal status on its own. Closing it out here would
    // cancel a task that is about to run.
    wrapper.append(createPlanReview(approval.payload, repo()));
  } else if (approval.kind === "provider_fallback") {
    // No `onResolved`, for the plan's reason: either answer ends the task.
    wrapper.append(createFallbackOffer(approval.payload, repo()));
  } else if (approval.kind === "round_budget") {
    // No `onResolved`, for the plan's reason: either answer ends the task.
    wrapper.append(createRoundBudgetOffer(approval.payload, repo()));
  } else if (approval.kind === "context_review") {
    // Saved and re-presented like a plan review; either answer ends this card.
    currentContextReview = approval.payload;
    wrapper.append(createContextReview(approval.payload, repo()));
  } else if (approval.kind === "fetch") {
    // Spec 51: the paused turn carries the verbatim URL, so the live dialog is
    // rebuilt and answers through the same fetch-decision route.
    wrapper.append(createFetchApproval(approval.payload, repo()));
  } else if (approval.kind === "checks") {
    wrapper.append(
      createCheckBatchCard(approval.payload, repo(), {
        reattached: true,
        onSubmitted: () => {
          reattachedApprovals = reattachedApprovals.filter((entry) => entry !== approval);
          wrapper.remove();
        },
      }),
    );
  } else {
    wrapper.append(
      createPatchPreview(approval.payload, repo(), {
        // A patch is answered per file and can end up part applied and part
        // rejected, so "resolved" is the only honest single word for it.
        onResolved: () => resolveReattachedApproval(approval, "resolved"),
      }),
    );
  }
  return wrapper;
}

// Closes out the task the approval belonged to. Without it the task stays
// `waiting_for_approval` in the log, and the next launch reattaches the same
// proposal and offers to run a command that has already run — the one way this
// surface could cause a side effect twice.
async function resolveReattachedApproval(approval, outcome) {
  reattachedApprovals = reattachedApprovals.filter(
    (entry) => entry.sessionId !== approval.sessionId || entry.taskId !== approval.taskId,
  );
  try {
    await api(
      "/api/recovery-approval-resolved",
      form({
        session_id: approval.sessionId,
        task_id: approval.taskId,
        outcome,
      }),
    );
  } catch (error) {
    // Surfaced rather than swallowed: the decision has already been carried
    // out, so a failure here means this approval can come back on the next
    // launch, and the user should know that before they act on it twice.
    toast(`Could not close out the recovered turn: ${error.message}`);
  }
}

async function decideRecovery(task, decision) {
  const payload = await api(
    "/api/recovery-decision",
    form({ session_id: task.sessionId, task_id: task.taskId, decision }),
  );
  recoveredTasks = recoveredTasks.filter(
    (entry) => entry.sessionId !== task.sessionId || entry.taskId !== task.taskId,
  );
  return payload;
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

// ---------------------------------------------------------------------------
// Worker sessions (docs/specs/57_worker_sessions/proposal.md §5.1, §5.7)
// ---------------------------------------------------------------------------

// `enable_worker_sessions`, as `GET /api/workers-enabled` reports it. While
// false nothing below draws anything (requirement 1).
let workersEnabled = false;

// `GET /api/workers` rows for the open project, and for the worktree's source
// when the open folder is a worker's worktree. They tell a worker session from
// any other, and a parent which workers it started.
let workerRows = [];

// The open worker's live state (§5.7 Worker session), or null when the open
// session is not a worker, the setting is off or the board is showing. Only
// while it is set do the control strip, the progress poll and the composer's
// worker commands exist.
let workerView = null;

async function refreshWorkersEnabled() {
  let enabled = false;
  try {
    enabled = Boolean((await api("/api/workers-enabled")).enabled);
  } catch {
    // Unknown reads as off: an entry point must not appear on a guess.
  }
  const changed = enabled !== workersEnabled;
  workersEnabled = enabled;
  $("worker-sessions-input").checked = enabled;
  if (changed) {
    // Off leaves nothing behind: no board, no rows, no poll (requirement 1).
    workerRowsByProject.clear();
    if (!enabled) hideWorkersBoard();
    renderProjectList();
  }
  // Redraw the open conversation so its blocks gain or lose the action. Not
  // during a turn: the next load redraws it anyway.
  if (changed && currentSessionId && !chatSubmitting) {
    void loadSession(currentSessionId).catch((error) => toast(error.message));
  }
}

// C9: written the way the MCP form writes, through the editor's content.
async function saveWorkerSessionsSetting(enabled) {
  $("config-editor").value = upsertConfigValue(
    $("config-editor").value,
    "enable_worker_sessions",
    enabled ? "true" : "false",
  );
  await saveConfigFile();
  if (workersEnabled !== enabled) {
    throw new Error("Saved, but another configuration file overrides Worker sessions.");
  }
}

// Spec 55 §5.5: read the *effective* value, so an absent key shows as checked
// (the default) and a rejected repository value never looks applied.
async function refreshConversationAutoCompact() {
  const input = $("conversation-auto-compact-input");
  try {
    const payload = await api(`/api/effective-policy?repo=${encodeURIComponent(repo() || "")}`);
    const rule = (payload.rules || []).find((entry) => entry.key === "conversation_auto_compact");
    input.checked = rule?.value !== "false";
    input.disabled = false;
  } catch {
    // Unknown reads as unchecked and disabled: a control must not appear to
    // hold a setting it could not read.
    input.checked = false;
    input.disabled = true;
  }
}

async function refreshPauseForContextReview() {
  const input = $("pause-for-context-review-input");
  try {
    const payload = await api(`/api/effective-policy?repo=${encodeURIComponent(repo() || "")}`);
    const rule = (payload.rules || []).find((entry) => entry.key === "pause_for_context_review");
    input.checked = rule?.value === "true";
    input.disabled = false;
  } catch {
    input.checked = false;
    input.disabled = true;
  }
}

// §5.1 Naming: a worktree lives at `<source>/.damaian/worktrees/<slug>`.
function workerSourceOf(repoPath) {
  const match = /^(.+)\/\.damaian\/worktrees\/[^/]+$/.exec(normalizeProjectPath(repoPath));
  return match ? match[1] : "";
}

async function refreshWorkerIndex() {
  workerRows = [];
  const repoPath = repo();
  if (!workersEnabled || !repoPath) return;
  const sources = [...new Set([repoPath, workerSourceOf(repoPath)].filter(Boolean))];
  const answers = await Promise.all(
    sources.map((source) =>
      api(`/api/workers?repo=${encodeURIComponent(source)}`).catch(() => null),
    ),
  );
  const seen = new Set();
  answers.forEach((answer, index) => {
    // A failed read keeps the sidebar's last rows rather than emptying them.
    if (!answer) return;
    setProjectWorkerRows(sources[index], answer.workers || []);
    (answer.workers || []).forEach((row) => {
      if (seen.has(row.sessionId)) return;
      seen.add(row.sessionId);
      workerRows.push(row);
    });
  });
}

function workerRowFor(sessionId) {
  return (sessionId && workerRows.find((row) => row.sessionId === sessionId)) || null;
}

// Workers do not start workers (§5.1 Entry points).
function isWorkerSession(sessionId) {
  return Boolean(workerRowFor(sessionId));
}

function workerChildrenOf(parentSessionId) {
  return workerRows
    .filter((row) => parentSessionId && row.parentSessionId === parentSessionId)
    .sort((a, b) => a.createdAtMs - b.createdAtMs);
}

// The shell's `worker_title`: the first non-blank line, at most 80 characters.
// How a block is matched to the worker it started once the page reloads.
function workerTitleOf(prompt) {
  const line = String(prompt || "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .find(Boolean);
  return line ? Array.from(line).slice(0, 80).join("") : "Worker";
}

// The block's text as written. Both renderers keep the content's own lines;
// the server's highlighter also keeps the newline that ended the last one.
function workerPromptText(code) {
  return code.textContent.replace(/\n$/, "");
}

// Opens a worker's conversation where the sidebar lists it: under its source
// project (§5.7 Sidebar), whose row is then the active one. When that project
// is not in the sidebar (only the worktree was added), it opens in place.
// `{ review: true }` scrolls to the approval card the worker is waiting on.
async function openWorkerSessionAt(sessionId, { source = "", review = false } = {}) {
  const row = workerRowFor(sessionId) || findProjectWorkerRow(sessionId);
  const home = normalizeProjectPath(
    source || (row && workerSourceOf(row.worktree)) || workerSourceOf(repo()) || repo(),
  );
  hideWorkersBoard();
  if (home && home !== repo() && projectPaths.includes(home)) {
    expandedProjectPaths.add(home);
    saveProjectState();
    await switchProject(home, { preferredSessionId: sessionId, reloadSelected: false });
  }
  await loadSession(sessionId);
  renderProjectList();
  if (review) {
    // The worker's approval card arrives with the view's first progress read.
    await workerViewCaughtUp();
    const cards = $("chat-log").querySelectorAll(
      ".command-approval, .patch-preview, .recovery-card",
    );
    cards[cards.length - 1]?.scrollIntoView({ block: "center" });
  }
}

function openWorkerSession(sessionId, options = {}) {
  void openWorkerSessionAt(sessionId, options).catch((error) => toast(error.message));
}

function createWorkerLink(row, text) {
  const link = document.createElement("button");
  link.type = "button";
  link.className = "worker-link";
  const title = document.createElement("em");
  title.textContent = row.title;
  link.append(document.createTextNode(`${text} `), title, document.createTextNode(" →"));
  link.addEventListener("click", () => openWorkerSession(row.sessionId));
  return link;
}

// C8: assistant messages only, and only a block whose class is exactly
// `language-worker-prompt`: the server writes the fence's info string into the
// class verbatim, so `worker-prompt extra` arrives as a different class.
function attachWorkerPromptActions(target, sessionId) {
  if (!workersEnabled || !sessionId || isWorkerSession(sessionId)) return;
  if (!target.message?.classList.contains("assistant")) return;
  target.body.querySelectorAll("pre > code").forEach((code) => {
    if (code.className !== "language-worker-prompt") return;
    const pre = code.parentElement;
    if (pre.nextElementSibling?.classList.contains("worker-prompt-footer")) return;
    const prompt = workerPromptText(code);
    const footer = document.createElement("div");
    footer.className = "worker-prompt-footer";
    const start = document.createElement("button");
    start.type = "button";
    start.className = "btn-sm";
    start.textContent = "Start worker";
    start.addEventListener("click", async () => {
      const started = await showNewWorkerDialog({ prompt, parentSessionId: sessionId });
      if (!started) return;
      footer.append(createWorkerLink(started, "Started"));
      appendSpawnedWorkerEntry(target.message, started);
    });
    footer.append(start);
    // A used block says so, so a second click is a deliberate second worker.
    const title = workerTitleOf(prompt);
    (startedWorkersByMessage.get(target.message) || [])
      .filter((row) => row.title === title)
      .forEach((row) => {
        footer.append(createWorkerLink(row, "Started"));
      });
    pre.after(footer);
  });
}

function spawnedWorkerEntry(row) {
  const entry = document.createElement("p");
  entry.className = "worker-spawned-entry";
  entry.append(createWorkerLink(row, "Spawned worker"));
  return entry;
}

// After the message, and after any entries already following it.
function appendSpawnedWorkerEntry(messageEl, row) {
  let anchor = messageEl;
  while (anchor.nextElementSibling?.classList.contains("worker-spawned-entry")) {
    anchor = anchor.nextElementSibling;
  }
  anchor.after(spawnedWorkerEntry(row));
}

// Which workers each rendered message started, for its blocks' "Started"
// links. Set on reload, before the messages' async finalize attaches them.
let startedWorkersByMessage = new WeakMap();

// The titles of a message's `worker-prompt` blocks. The client renderer keeps
// the exact tag and the block's lines as the server's does, and answers at once.
function workerPromptTitles(content) {
  const scratch = document.createElement("div");
  scratch.innerHTML = renderMarkdown(content);
  return [...scratch.querySelectorAll("pre > code")]
    .filter((code) => code.className === "language-worker-prompt")
    .map((code) => workerTitleOf(workerPromptText(code)));
}

// §5.7 Parent session, on reload. No route says which block started a worker:
// a row carries the worker's title and creation time, not the parent's event.
// So a worker belongs to the latest assistant message written before it that
// holds a block with its title, and failing that, to the last message written
// before it.
function renderSpawnedWorkerEntries(rendered, sessionId) {
  startedWorkersByMessage = new WeakMap();
  const children = workerChildrenOf(sessionId);
  if (!children.length) return;
  const titles = new Map(
    rendered
      .filter(({ message }) => message.role === "assistant")
      .map(({ message }) => [message, workerPromptTitles(message.content)]),
  );
  children.forEach((row) => {
    const before = rendered.filter(({ message }) => message.createdAtMs <= row.createdAtMs);
    const holder = before.filter(({ message }) => titles.get(message)?.includes(row.title)).pop();
    const anchor = holder || before[before.length - 1];
    if (!anchor) {
      $("chat-log").prepend(spawnedWorkerEntry(row));
      return;
    }
    if (holder) {
      const started = startedWorkersByMessage.get(holder.bubble.message) || [];
      startedWorkersByMessage.set(holder.bubble.message, [...started, row]);
    }
    appendSpawnedWorkerEntry(anchor.bubble.message, row);
  });
}

function describeUncommittedFiles(status) {
  const paths = Array.isArray(status.paths) ? status.paths : [];
  const mentioned = Array.isArray(status.mentioned) ? status.mentioned : [];
  // Mentioned first (§5.1): those are the files the worker is likely to need.
  const ordered = [...new Set([...mentioned, ...paths])];
  const shown = ordered.slice(0, 3);
  const more = ordered.length - shown.length;
  const message = document.createDocumentFragment();
  message.append(
    `⚠ ${paths.length} uncommitted ${paths.length === 1 ? "file" : "files"} in your tree, including `,
  );
  shown.forEach((path, index) => {
    if (index > 0) message.append(index === shown.length - 1 && !more ? " and " : ", ");
    const code = document.createElement("code");
    code.textContent = path;
    message.append(code);
  });
  if (more) message.append(` and ${more} more`);
  message.append(". The worker won't see them unless you include them.");
  return message;
}

// "New worker" (§5.1 Dialog), built the way `showRewindDialog` is. Resolves
// to the spawned worker's row, or null when cancelled.
function showNewWorkerDialog({ prompt = "", parentSessionId = "" } = {}) {
  const repoPath = repo();
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "app-dialog-backdrop";
    backdrop.innerHTML = `
      <div class="app-dialog app-dialog-wide app-dialog-worker" role="dialog" aria-modal="true">
        <p class="app-dialog-title">New worker</p>
        <textarea class="worker-dialog-prompt" rows="12" aria-label="Worker prompt"></textarea>
        <div class="worker-dialog-dirty" hidden>
          <p class="worker-dialog-dirty-message"></p>
          <label class="field-checkbox">
            <input type="radio" name="worker-dialog-base" value="include" checked />
            <span>Include uncommitted changes</span>
          </label>
          <label class="field-checkbox">
            <input type="radio" name="worker-dialog-base" value="head" />
            <span>Start from last commit</span>
          </label>
        </div>
        <p class="worker-dialog-status" role="status" hidden></p>
        <div class="app-dialog-actions">
          <button type="button" class="btn-quiet worker-dialog-cancel">Cancel</button>
          <button type="button" class="btn-primary worker-dialog-confirm" disabled>
            Start worker
          </button>
        </div>
      </div>
    `;
    const field = backdrop.querySelector(".worker-dialog-prompt");
    const dirty = backdrop.querySelector(".worker-dialog-dirty");
    const status = backdrop.querySelector(".worker-dialog-status");
    const cancelButton = backdrop.querySelector(".worker-dialog-cancel");
    const confirmButton = backdrop.querySelector(".worker-dialog-confirm");
    field.value = prompt;
    document.body.append(backdrop);

    const showStatus = (text) => {
      status.textContent = text;
      status.hidden = !text;
    };

    // Start waits for the first answer, so a dirty tree is never started
    // from without the user having seen the choice.
    let checks = 0;
    let checkTimer = null;
    const checkSource = async () => {
      const check = ++checks;
      try {
        const answer = await api(
          `/api/worker-source-status?repo=${encodeURIComponent(repoPath)}&prompt=${encodeURIComponent(field.value)}`,
        );
        if (check !== checks) return;
        const paths = Array.isArray(answer.paths) ? answer.paths : [];
        backdrop
          .querySelector(".worker-dialog-dirty-message")
          .replaceChildren(describeUncommittedFiles(answer));
        dirty.hidden = paths.length === 0;
        showStatus("");
      } catch (error) {
        if (check !== checks) return;
        dirty.hidden = true;
        showStatus(error.message);
      }
      confirmButton.disabled = false;
    };
    // The prompt decides which files are named first.
    const onInput = () => {
      clearTimeout(checkTimer);
      checkTimer = setTimeout(() => void checkSource(), 400);
    };

    const cleanup = (result) => {
      clearTimeout(checkTimer);
      checks += 1;
      document.removeEventListener("keydown", onKeydown, true);
      backdrop.remove();
      resolve(result);
    };
    const onKeydown = (event) => {
      if (event.key === "Escape") cleanup(null);
    };
    const onConfirm = async () => {
      const text = field.value;
      if (!text.trim()) {
        showStatus("Write the worker's prompt first.");
        return;
      }
      const include =
        !dirty.hidden &&
        backdrop.querySelector('input[name="worker-dialog-base"]:checked')?.value === "include";
      confirmButton.disabled = true;
      cancelButton.disabled = true;
      showStatus("Starting the worker…");
      try {
        const started = await api(
          "/api/workers",
          form({
            repo: repoPath,
            prompt: text,
            include_uncommitted: include ? "true" : "false",
            parent_session_id: parentSessionId,
          }),
        );
        const row = {
          sessionId: started.sessionId,
          title: started.title,
          parentSessionId: parentSessionId || null,
          branch: started.branch,
          worktree: started.worktree,
          createdAtMs: Date.now(),
        };
        workerRows.push(row);
        toast(`Started worker ${row.title} on ${row.branch}`);
        void loadProjectWorkers(repoPath);
        cleanup(row);
      } catch (error) {
        showStatus(error.message);
        confirmButton.disabled = false;
        cancelButton.disabled = false;
      }
    };

    field.addEventListener("input", onInput);
    confirmButton.addEventListener("click", () => void onConfirm());
    cancelButton.addEventListener("click", () => cleanup(null));
    document.addEventListener("keydown", onKeydown, true);
    field.focus();
    void checkSource();
  });
}

// ---------------------------------------------------------------------------
// Workers in the sidebar and on the board (spec 57 §5.7 Sidebar, Workers board)
// ---------------------------------------------------------------------------

// `GET /api/workers?repo=` rows by project path. The sidebar's nesting and
// dots, the Workers entry's badge and the board all read them. Empty while the
// setting is off, and nothing fills it then.
const workerRowsByProject = new Map();
const workerRowsLoading = new Set();

const WORKER_FAILED_STATUSES = new Set([
  "failed",
  "interrupted",
  "unknown_external_outcome",
  "tool_budget_exhausted",
  "token_budget_exhausted",
]);

const WORKER_DOT_LABELS = {
  running: "Running",
  "needs-you": "Needs you",
  paused: "Paused",
  finished: "Finished",
  failed: "Failed",
};

// Why a finished worker stopped, for the board's Doing column.
const WORKER_END_LABELS = {
  complete: "Done",
  cancelled: "Stopped",
  failed: "Failed",
  interrupted: "Interrupted: its process ended mid-turn",
  tool_budget_exhausted: "Tool budget used up",
  token_budget_exhausted: "Token budget used up",
  unknown_external_outcome: "Outcome unknown",
};

// The five dots of §5.7. A finished row that carries an error, or ended in a
// status that is not a clean end, is failed. A worker whose process died while
// paused or waiting (C5) is reported `interrupted` and lands here too.
function workerDotState(row) {
  if (row.group === "needs_you") return row.state === "paused" ? "paused" : "needs-you";
  if (row.group === "running") return "running";
  return WORKER_FAILED_STATUSES.has(row.taskStatus) || row.doing ? "failed" : "finished";
}

function createWorkerDot(row) {
  const state = workerDotState(row);
  const dot = document.createElement("span");
  dot.className = "worker-dot";
  dot.dataset.state = state;
  dot.setAttribute("role", "img");
  dot.setAttribute("aria-label", WORKER_DOT_LABELS[state]);
  dot.title = WORKER_DOT_LABELS[state];
  return dot;
}

// What the sidebar draws from a project's rows. The board replaces the rows
// every second; the sidebar is redrawn only when this changes.
function workerSidebarSignature(rows) {
  return JSON.stringify(
    (rows || []).map((row) => [row.sessionId, row.title, row.parentSessionId, workerDotState(row)]),
  );
}

function setProjectWorkerRows(projectPath, rows) {
  const path = normalizeProjectPath(projectPath);
  if (!path || !workersEnabled) return;
  const before = workerSidebarSignature(workerRowsByProject.get(path));
  workerRowsByProject.set(path, rows);
  undismissUnfinished(rows);
  if (before !== workerSidebarSignature(rows) && projectPaths.includes(path)) {
    renderProjectList();
  }
}

function findProjectWorkerRow(sessionId) {
  for (const rows of workerRowsByProject.values()) {
    const row = rows.find((entry) => entry.sessionId === sessionId);
    if (row) return row;
  }
  return null;
}

async function loadProjectWorkers(projectPath) {
  const path = normalizeProjectPath(projectPath);
  if (!workersEnabled || !path || workerRowsLoading.has(path)) return;
  workerRowsLoading.add(path);
  try {
    const answer = await api(`/api/workers?repo=${encodeURIComponent(path)}`);
    setProjectWorkerRows(path, answer.workers || []);
  } catch {
    // The sidebar keeps its last rows. A project that never answered counts
    // as having none, so the sidebar does not ask again on every redraw.
    if (workersEnabled && !workerRowsByProject.has(path)) workerRowsByProject.set(path, []);
  } finally {
    workerRowsLoading.delete(path);
  }
}

// The sidebar's rows refresh when a project is drawn, a session loads, the
// board polls, or the window comes back: there is no timed poll behind them.
window.addEventListener("focus", () => {
  if (!workersEnabled) return;
  expandedProjectPaths.forEach((path) => void loadProjectWorkers(path));
});

// A project's sessions with its workers placed (§5.7 Sidebar): each worker
// under its parent, oldest first, and a worker without a parent in this list
// (none, or one since deleted) among the sessions by its last activity, as the
// sessions themselves are ordered.
function sidebarEntries(sessions, rows) {
  const ids = new Set(sessions.map((session) => session.id));
  const byCreated = [...rows].sort((a, b) => a.createdAtMs - b.createdAtMs);
  const loose = byCreated
    .filter((row) => !row.parentSessionId || !ids.has(row.parentSessionId))
    .sort((a, b) => b.updatedAtMs - a.updatedAtMs);
  const entries = [];
  sessions.forEach((session) => {
    while (loose.length && loose[0].updatedAtMs > (session.updatedAtMs || 0)) {
      entries.push({ row: loose.shift(), nested: false });
    }
    entries.push({ session });
    byCreated
      .filter((row) => row.parentSessionId === session.id)
      .forEach((row) => {
        entries.push({ row, nested: true });
      });
  });
  loose.forEach((row) => {
    entries.push({ row, nested: false });
  });
  return entries;
}

// Most urgent first: a project's entry wears the dot of the worker that most
// needs a look.
const WORKER_DOT_URGENCY = ["needs-you", "running", "paused", "failed", "finished"];

// Null when there is nothing to say: no workers, or only dismissed finished
// ones. The board stays reachable from the project menu.
function renderWorkersEntry(projectPath, rows) {
  const shown = (rows || []).filter(
    (row) => !(row.group === "finished" && dismissedWorkers.has(row.sessionId)),
  );
  if (!shown.length) return null;
  const entry = document.createElement("button");
  entry.type = "button";
  entry.className = "project-workers-entry";
  entry.classList.toggle("active", workersBoardProject === projectPath);
  const states = shown.map(workerDotState);
  const state = WORKER_DOT_URGENCY.find((candidate) => states.includes(candidate));
  const dot = createWorkerDot({ ...shown[states.indexOf(state)] });
  const label = document.createElement("span");
  const running = shown.filter((row) => row.group === "running").length;
  label.textContent = running ? `Workers \u00b7 ${running} running` : "Workers";
  entry.append(dot, label);
  const needYou = shown.filter((row) => row.group === "needs_you").length;
  if (needYou) {
    const badge = document.createElement("span");
    badge.className = "workers-badge";
    badge.textContent = String(needYou);
    entry.append(badge);
  }
  const parts = [];
  if (needYou) parts.push(`${needYou} need you`);
  if (running) parts.push(`${running} running`);
  entry.setAttribute("aria-label", parts.length ? `Workers, ${parts.join(", ")}` : "Workers");
  entry.addEventListener("click", () => {
    void openWorkersBoard(projectPath).catch((error) => toast(error.message));
  });
  return entry;
}

function renderProjectWorker(projectPath, row, { nested }) {
  const item = document.createElement("div");
  item.className = "session-item project-session-item project-worker-item";
  item.classList.toggle("nested", nested);
  item.dataset.sessionId = row.sessionId;
  item.dataset.projectPath = projectPath;
  if (projectPath === repo() && row.sessionId === currentSessionId && !workersBoardProject) {
    item.classList.add("active");
  }
  const button = document.createElement("button");
  button.type = "button";
  button.className = "project-session-open project-worker-open";
  const title = document.createElement("span");
  title.className = "project-worker-title";
  title.textContent = row.title;
  button.append(createWorkerDot(row), title);
  button.title = `${row.title} (worker on ${row.branch})`;
  button.addEventListener("click", () => openWorkerSession(row.sessionId, { source: projectPath }));
  item.append(
    button,
    createWorkerMenuButton(row, projectPath, "project-menu-btn project-session-menu-btn"),
  );
  return item;
}

// One menu for a worker, from its sidebar row and its board row alike.
const workerMenu = createContextMenu();

function createWorkerMenuButton(row, projectPath, className, { board = false } = {}) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = className;
  button.title = "More worker options";
  button.setAttribute("aria-label", `More options for worker ${row.title}`);
  button.setAttribute("aria-haspopup", "menu");
  button.setAttribute("aria-expanded", "false");
  button.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    toggleWorkerMenu(row, projectPath, button, { board });
  });
  return button;
}

function toggleWorkerMenu(row, projectPath, anchorEl, { board }) {
  const finished = row.group === "finished";
  workerMenu.toggle(anchorEl, {
    key: `${board ? "board" : "sidebar"}\n${row.sessionId}`,
    sections: [
      [
        {
          label: "Reveal worktree in Finder",
          action: "reveal",
          onSelect: runSessionAction(() => revealWorkerWorktree(row)),
        },
      ],
      [
        {
          label: "Dismiss from board",
          action: "dismiss",
          hidden: !board || !finished,
          onSelect: () => dismissWorker(row),
        },
      ],
      [
        {
          label: "Stop",
          action: "stop",
          danger: true,
          hidden: !row.alive,
          onSelect: runSessionAction(() => stopWorker(row, projectPath)),
        },
        {
          label: "Remove worktree",
          action: "remove-worktree",
          danger: true,
          hidden: !finished,
          // The shell refuses while a live worker holds the worktree.
          disabled: Boolean(row.alive),
          hint: row.alive ? "Stop the worker first" : "",
          onSelect: runSessionAction(() => removeWorkerWorktree(row, projectPath)),
        },
      ],
    ],
  });
}

// The rows changed because the user acted: the board polls now, or the
// sidebar reads the project again.
function refreshWorkersAfterAction(projectPath) {
  if (workersBoardProject === normalizeProjectPath(projectPath)) {
    void pollWorkersBoard();
  } else {
    void loadProjectWorkers(projectPath);
  }
}

async function revealWorkerWorktree(row) {
  const payload = await api("/api/reveal-in-finder", form({ repo: row.worktree }));
  toast(`Revealed ${payload.path}`);
}

async function pauseWorker(row, projectPath) {
  await api("/api/worker-control", form({ session_id: row.sessionId, command: "pause" }));
  toast(`${row.title} pauses at its next round`);
  refreshWorkersAfterAction(projectPath);
}

// §5.3: `stop` ends a running, paused or waiting turn and escalates on its
// own. An idle worker has no turn to stop and refuses `stop`, so ending it is
// `shutdown`, the path a deleted parent takes.
async function stopWorker(row, projectPath) {
  const idle = row.state === "idle";
  const confirmed = await confirmDialog(
    "Stop worker?",
    idle
      ? `End the process of ${row.title}? Its conversation, worktree and branch are kept.`
      : `Stop ${row.title}? Its turn ends now, and its process is ended if the turn does not stop. Its conversation, worktree and branch are kept.`,
    { danger: true, confirmLabel: "Stop" },
  );
  if (!confirmed) return;
  await api(
    "/api/worker-control",
    form({ session_id: row.sessionId, command: idle ? "shutdown" : "stop" }),
  );
  toast(`Stopping ${row.title}`);
  refreshWorkersAfterAction(projectPath);
}

// §5.1 removal: a clean worktree goes at once; a dirty one is named path by
// path and goes only on a second, explicit confirmation.
async function removeWorkerWorktree(row, projectPath) {
  const confirmed = await confirmDialog(
    "Remove worktree?",
    `Remove the worktree at ${row.worktree}? The branch ${row.branch} and its commits are kept.`,
    { danger: true, confirmLabel: "Remove" },
  );
  if (!confirmed) return;
  let answer = await api("/api/worker-remove-worktree", form({ session_id: row.sessionId }));
  if (answer.status === "dirty") {
    const paths = Array.isArray(answer.paths) ? answer.paths : [];
    const shown = paths.slice(0, 8).join(", ");
    const more = paths.length > 8 ? ` and ${paths.length - 8} more` : "";
    const discard = await confirmDialog(
      "Discard uncommitted changes?",
      `The worktree has uncommitted changes that removing it discards: ${shown}${more}.`,
      { danger: true, confirmLabel: "Remove anyway" },
    );
    if (!discard) return;
    answer = await api(
      "/api/worker-remove-worktree",
      form({ session_id: row.sessionId, confirmed_dirty: "true" }),
    );
  }
  if (answer.status === "removed") toast(`Removed the worktree of ${row.title}`);
  refreshWorkersAfterAction(projectPath);
}

// Dismissed finished workers, by session id. Client-side, in this webview's
// storage: dismissing changes what the board lists and nothing on disk, and
// the session stays in the sidebar (§5.7).
const dismissedWorkersStorageKey = "damaian:dismissedWorkers";

function readDismissedWorkers() {
  try {
    return new Set(JSON.parse(localStorage.getItem(dismissedWorkersStorageKey) || "[]"));
  } catch {
    return new Set();
  }
}

const dismissedWorkers = readDismissedWorkers();

function saveDismissedWorkers() {
  localStorage.setItem(dismissedWorkersStorageKey, JSON.stringify([...dismissedWorkers]));
}

function dismissWorker(row) {
  dismissedWorkers.add(row.sessionId);
  saveDismissedWorkers();
  renderWorkersBoard();
  renderProjectList();
}

// A dismissed worker that runs again (Restart worker) is back on the board.
function undismissUnfinished(rows) {
  let changed = false;
  rows.forEach((row) => {
    if (row.group !== "finished" && dismissedWorkers.delete(row.sessionId)) changed = true;
  });
  if (changed) saveDismissedWorkers();
}

// The project whose board is showing, or "" while it is hidden.
let workersBoardProject = "";
let workersBoardRows = null; // null until the board's first answer
let workersBoardError = "";
let workersBoardSignature = "";
let workersBoardInFlight = false;
let workersBoardRequestStartedAt = 0;
let workersBoardAnsweredAt = 0;
let workersBoardPollTimer = null;
let workersBoardTicker = null;
// Bumped on every open and hide, so an answer to an earlier board is not drawn.
let workersBoardGeneration = 0;

const WORKER_GROUPS = [
  ["needs_you", "Needs you"],
  ["running", "Running"],
  ["finished", "Finished"],
];

async function openWorkersBoard(projectPath) {
  const path = normalizeProjectPath(projectPath);
  if (!workersEnabled || !path) return;
  if (path !== repo()) {
    expandedProjectPaths.add(path);
    saveProjectState();
    await switchProject(path, { reloadSelected: false });
    if (!currentSessionId) clearChat();
  }
  workersBoardGeneration += 1;
  workersBoardProject = path;
  workersBoardRows = workerRowsByProject.get(path) || null;
  workersBoardError = "";
  workersBoardSignature = "";
  workersBoardAnsweredAt = 0;
  document.body.classList.add("workers-board-open");
  $("workers-board").hidden = false;
  renderThreadHeader();
  renderProjectList();
  renderWorkersBoard();
  clearInterval(workersBoardTicker);
  workersBoardTicker = setInterval(tickWorkersBoard, 1000);
  void pollWorkersBoard();
}

function hideWorkersBoard() {
  if (!workersBoardProject) return;
  workersBoardProject = "";
  workersBoardGeneration += 1;
  clearTimeout(workersBoardPollTimer);
  workersBoardPollTimer = null;
  clearInterval(workersBoardTicker);
  workersBoardTicker = null;
  workerMenu.close();
  document.body.classList.remove("workers-board-open");
  $("workers-board").hidden = true;
  $("workers-board").replaceChildren();
  renderThreadHeader();
  renderProjectList();
}

// About once a second while the board is showing (§5.7), and C10: the shell
// serves one request at a time, so an answer can wait for the whole of a turn
// streaming in another session. One request is out at a time, and the next is
// scheduled only once it has answered, so a held shell gets one poll, not a
// pile. A slow answer is not an error: the board keeps its rows and says how
// old they are.
async function pollWorkersBoard() {
  clearTimeout(workersBoardPollTimer);
  workersBoardPollTimer = null;
  const project = workersBoardProject;
  if (!project || !workersEnabled || workersBoardInFlight) return;
  // A hidden window does not poll; `visibilitychange` starts it again.
  if (document.hidden) return;
  const generation = workersBoardGeneration;
  workersBoardInFlight = true;
  workersBoardRequestStartedAt = Date.now();
  try {
    const answer = await api(`/api/workers?repo=${encodeURIComponent(project)}`);
    if (generation === workersBoardGeneration) {
      workersBoardRows = answer.workers || [];
      workersBoardError = "";
      workersBoardAnsweredAt = Date.now();
    }
    setProjectWorkerRows(project, answer.workers || []);
  } catch (error) {
    if (generation === workersBoardGeneration) workersBoardError = error.message;
  } finally {
    workersBoardInFlight = false;
    workersBoardRequestStartedAt = 0;
  }
  if (!workersBoardProject) return;
  if (generation === workersBoardGeneration) renderWorkersBoard();
  // A board opened again while this request was out polls at once.
  workersBoardPollTimer = setTimeout(
    () => void pollWorkersBoard(),
    generation === workersBoardGeneration ? 1000 : 0,
  );
}

document.addEventListener("visibilitychange", () => {
  if (document.hidden || !workersBoardProject) return;
  if (!workersBoardInFlight && !workersBoardPollTimer) void pollWorkersBoard();
});

function formatElapsed(ms) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

// Elapsed runs live until a worker finishes. A row carries no end time: the
// session's `updatedAtMs` does not move while a worker's turns run, so a
// finished worker shows "—" rather than a duration that reads as zero.
function workerElapsedText(row) {
  if (row.group !== "finished") return formatElapsed(Date.now() - row.createdAtMs);
  return row.updatedAtMs > row.createdAtMs + 1000
    ? formatElapsed(row.updatedAtMs - row.createdAtMs)
    : "—";
}

function workerDoingText(row) {
  if (row.doing) return row.doing;
  if (row.group === "running") return row.state === "idle" ? "Starting" : "Working";
  if (row.group === "finished") {
    return WORKER_END_LABELS[row.taskStatus] || (row.alive ? "Idle" : "Ended");
  }
  return "";
}

function plural(count, one, many) {
  return `${count} ${count === 1 ? one : many}`;
}

// The one visible action of a row (§5.7).
function workerRowAction(row, projectPath) {
  if (row.group === "needs_you" && row.state === "waiting_for_approval") {
    return {
      label: "Review",
      run: () => openWorkerSession(row.sessionId, { source: projectPath, review: true }),
    };
  }
  if (row.group === "running" && row.state === "running") {
    return { label: "Pause", run: runSessionAction(() => pauseWorker(row, projectPath)) };
  }
  return { label: "Open", run: () => openWorkerSession(row.sessionId, { source: projectPath }) };
}

function renderWorkerBoardRow(row, projectPath) {
  const tr = document.createElement("tr");
  tr.className = "workers-row";
  tr.dataset.sessionId = row.sessionId;
  tr.dataset.state = workerDotState(row);
  const cell = (className) => {
    const td = document.createElement("td");
    td.className = className;
    tr.append(td);
    return td;
  };

  cell("workers-cell-dot").append(createWorkerDot(row));

  const worker = cell("workers-cell-worker");
  const title = document.createElement("span");
  title.className = "workers-title";
  title.textContent = row.title;
  title.title = row.title;
  const branch = document.createElement("span");
  branch.className = "workers-branch";
  branch.textContent = row.branch;
  worker.append(title, branch);

  const planner = cell("workers-cell-planner");
  planner.textContent = row.parentTitle || "—";
  planner.classList.toggle("is-empty", !row.parentTitle);

  const rounds = cell("workers-cell-rounds");
  if (Number.isFinite(row.round) && Number.isFinite(row.maxRounds) && row.maxRounds > 0) {
    const bar = document.createElement("span");
    bar.className = "workers-rounds-bar";
    bar.setAttribute("aria-hidden", "true");
    const fill = document.createElement("span");
    fill.style.width = `${Math.min(100, (row.round / row.maxRounds) * 100)}%`;
    bar.append(fill);
    const label = document.createElement("span");
    label.textContent = `${row.round}/${row.maxRounds}`;
    rounds.append(bar, label);
  } else {
    rounds.textContent = "—";
    rounds.classList.add("is-empty");
  }

  const doing = cell("workers-cell-doing");
  const doingText = document.createElement("span");
  doingText.className = "workers-doing";
  doingText.textContent = workerDoingText(row);
  doingText.title = doingText.textContent;
  doing.append(doingText);
  if (row.group === "finished" && (row.filesChanged != null || row.commitsAhead != null)) {
    const meta = document.createElement("span");
    meta.className = "workers-meta";
    meta.textContent = [
      row.filesChanged != null && plural(row.filesChanged, "file changed", "files changed"),
      row.commitsAhead != null && plural(row.commitsAhead, "commit ahead", "commits ahead"),
    ]
      .filter(Boolean)
      .join(" · ");
    doing.append(meta);
  }

  const elapsed = cell("workers-cell-elapsed");
  elapsed.dataset.from = String(row.createdAtMs);
  elapsed.textContent = workerElapsedText(row);
  if (row.group === "finished") elapsed.dataset.fixed = "true";

  const actions = cell("workers-cell-actions");
  const action = workerRowAction(row, projectPath);
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn-sm";
  button.textContent = action.label;
  button.addEventListener("click", action.run);
  actions.append(
    button,
    createWorkerMenuButton(row, projectPath, "btn-icon workers-menu-btn", { board: true }),
  );
  return tr;
}

function renderWorkersBoard() {
  const board = $("workers-board");
  const project = workersBoardProject;
  if (!project) return;
  const rows = (workersBoardRows || []).filter(
    (row) => !(row.group === "finished" && dismissedWorkers.has(row.sessionId)),
  );
  // Redrawn only when what it shows changed, so a menu or a focused button
  // is not replaced every second for nothing.
  const signature = JSON.stringify([workersBoardRows === null, rows]);
  if (signature === workersBoardSignature && board.childElementCount) {
    renderWorkersBoardStatus();
    return;
  }
  workersBoardSignature = signature;

  const head = document.createElement("div");
  head.className = "workers-board-head";
  const summary = document.createElement("p");
  summary.className = "workers-board-summary";
  const needYou = rows.filter((row) => row.group === "needs_you").length;
  summary.textContent =
    workersBoardRows === null
      ? "Loading workers…"
      : [plural(rows.length, "worker", "workers"), needYou && `${needYou} need you`]
          .filter(Boolean)
          .join(" · ");
  const status = document.createElement("p");
  status.className = "workers-board-status";
  status.setAttribute("role", "status");
  const newWorker = document.createElement("button");
  newWorker.type = "button";
  newWorker.className = "btn-sm";
  newWorker.textContent = "New worker";
  newWorker.addEventListener("click", async () => {
    const started = await showNewWorkerDialog({});
    if (started) refreshWorkersAfterAction(project);
  });
  head.append(summary, status, newWorker);
  board.replaceChildren(head);

  if (workersBoardRows !== null && !rows.length) {
    const empty = document.createElement("p");
    empty.className = "workers-board-empty";
    empty.textContent =
      "No workers in this project. Start one with New worker, or from a worker-prompt block in a conversation.";
    board.append(empty);
  } else if (rows.length) {
    const table = document.createElement("table");
    table.className = "workers-table";
    const thead = document.createElement("thead");
    const headRow = document.createElement("tr");
    ["Status", "Worker", "Planner", "Rounds", "Doing", "Elapsed", "Actions"].forEach((label) => {
      const th = document.createElement("th");
      th.scope = "col";
      // The dot and action columns keep their header for screen readers only;
      // the cell stays in the table so the columns line up.
      if (label === "Status" || label === "Actions") {
        const hidden = document.createElement("span");
        hidden.className = "visually-hidden";
        hidden.textContent = label;
        th.append(hidden);
      } else {
        th.textContent = label;
      }
      headRow.append(th);
    });
    thead.append(headRow);
    table.append(thead);
    WORKER_GROUPS.forEach(([group, label]) => {
      const groupRows = rows
        .filter((row) => row.group === group)
        .sort((a, b) => b.createdAtMs - a.createdAtMs);
      if (!groupRows.length) return;
      const body = document.createElement("tbody");
      body.className = "workers-group";
      body.dataset.group = group;
      const groupHead = document.createElement("tr");
      const groupLabel = document.createElement("th");
      groupLabel.scope = "rowgroup";
      groupLabel.colSpan = 7;
      groupLabel.textContent = `${label} · ${groupRows.length}`;
      groupHead.append(groupLabel);
      body.append(groupHead);
      groupRows.forEach((row) => {
        body.append(renderWorkerBoardRow(row, project));
      });
      table.append(body);
    });
    board.append(table);
  }
  renderWorkersBoardStatus();
}

// Quiet, never an error for slowness (C10): how old the rows are once an
// answer is late, and what went wrong when a request actually failed.
function renderWorkersBoardStatus() {
  const status = $("workers-board").querySelector(".workers-board-status");
  if (!status) return;
  const waiting = workersBoardInFlight && Date.now() - workersBoardRequestStartedAt > 3000;
  let text = "";
  if (workersBoardError) {
    text = `Could not refresh: ${workersBoardError}`;
  } else if (waiting) {
    const age = workersBoardAnsweredAt
      ? `Updated ${formatElapsed(Date.now() - workersBoardAnsweredAt)} ago. `
      : "";
    text = `${age}Waiting for the shell, which answers once a turn running in another session ends.`;
  }
  status.textContent = text;
  status.hidden = !text;
}

function tickWorkersBoard() {
  $("workers-board")
    .querySelectorAll(".workers-cell-elapsed:not([data-fixed])")
    .forEach((elapsed) => {
      elapsed.textContent = formatElapsed(Date.now() - Number(elapsed.dataset.from));
    });
  renderWorkersBoardStatus();
}

// ---------------------------------------------------------------------------
// The worker session (§5.7 Worker session): a control strip under the header,
// a composer that talks to the worker, and its live progress. History is the
// session log, drawn by `loadSession` as for any session. This adds only what
// the worker is doing now, read from its progress file.
// ---------------------------------------------------------------------------

// `status` states that end a job: the turn finished, or it waits for approval.
const WORKER_JOB_END_STATES = new Set(["idle", "waiting_for_approval"]);
const WORKER_TURN_STATES = new Set(["running", "paused", "waiting_for_approval"]);

const WORKER_VIEW_LABELS = {
  running: ["running", "Running"],
  paused: ["paused", "Paused"],
  waiting_for_approval: ["needs-you", "Waiting for approval"],
  idle: ["finished", "Idle"],
  shutting_down: ["finished", "Stopping"],
};

// What the composer's button sends in each state (§5.7), with its label and
// the field's placeholder.
const WORKER_COMPOSER_MODES = {
  running: ["note", "Send note", "Steer the worker: a note reaches it before its next round"],
  paused: ["resume", "Resume with note", "Add a note: the worker reads it as it resumes"],
  idle: ["message", "Send message", "Message the worker"],
};

// The composer as the page drew it, kept while a worker view borrows it.
let composerDefaults = null;

function workerViewActive(view) {
  return Boolean(
    view &&
      view === workerView &&
      workersEnabled &&
      !workersBoardProject &&
      view.sessionId === currentSessionId,
  );
}

// Called last in `loadSession`: starts, keeps or ends the view for the session
// now open. Reloading the same worker keeps its place in the progress file.
function syncWorkerView() {
  const sessionId = currentSessionId;
  if (!workersEnabled || !sessionId || workersBoardProject || !isWorkerSession(sessionId)) {
    stopWorkerView();
    return;
  }
  if (workerView?.sessionId !== sessionId) {
    stopWorkerView();
    const row = workerRowFor(sessionId);
    let resolveReady = () => {};
    const ready = new Promise((resolve) => {
      resolveReady = resolve;
    });
    workerView = {
      sessionId,
      after: 0,
      caughtUp: false,
      ready,
      resolveReady,
      alive: Boolean(row.alive),
      state: row.alive ? row.state : "exited",
      round: row.round ?? null,
      maxRounds: row.maxRounds ?? null,
      lastPhase: null,
      plan: null,
      approval: null,
      liveText: "",
      live: null,
      pendingNotes: [],
      turnError: "",
      pollError: "",
      sending: false,
      inFlight: false,
      timer: null,
    };
  } else {
    // `renderMessages` cleared the log, and the live bubble went with it.
    finishWorkerLive(workerView);
  }
  const view = workerView;
  document.body.classList.add("worker-view");
  if (view.caughtUp && view.alive && (view.state === "running" || view.state === "paused")) {
    showWorkerLive(view);
  }
  renderWorkerView(view);
  if (!view.inFlight && !view.timer) void pollWorkerProgress();
}

// Off, another session, a new one or the board: nothing of the view stays.
function stopWorkerView() {
  const view = workerView;
  if (!view) return;
  workerView = null;
  clearTimeout(view.timer);
  view.timer = null;
  finishWorkerLive(view);
  view.resolveReady();
  document.body.classList.remove("worker-view");
  document.querySelector(".worker-strip")?.remove();
  renderWorkerHeaderMeta();
  restoreComposerFromWorker();
}

// Resolves once the open worker's progress has been read, so *Review* finds
// its approval card. A held shell (C10) does not hold *Review* for long.
function workerViewCaughtUp() {
  const view = workerView;
  if (!view || view.caughtUp) return Promise.resolve();
  return Promise.race([view.ready, new Promise((resolve) => setTimeout(resolve, 3000))]);
}

// C10, as the board does it: one request out at a time, the next a second
// after the answer, no error for a slow answer, and nothing while the window
// is hidden or once the view is left.
async function pollWorkerProgress() {
  const view = workerView;
  if (!view) return;
  clearTimeout(view.timer);
  view.timer = null;
  if (!workerViewActive(view)) {
    stopWorkerView();
    return;
  }
  if (view.inFlight || document.hidden) return;
  view.inFlight = true;
  try {
    const answer = await api(
      `/api/worker-progress?session=${encodeURIComponent(view.sessionId)}&after=${view.after}`,
    );
    view.pollError = "";
    if (workerViewActive(view)) await applyWorkerProgress(view, answer);
  } catch (error) {
    view.pollError = error.message;
  } finally {
    view.inFlight = false;
    if (!view.caughtUp) view.resolveReady();
  }
  if (!workerViewActive(view)) {
    if (view === workerView) stopWorkerView();
    return;
  }
  renderWorkerView(view);
  // A restart starts the file again: read it at once from the start.
  const delay = view.readNow ? 0 : 1000;
  view.readNow = false;
  view.timer = setTimeout(() => void pollWorkerProgress(), delay);
}

function pollWorkerProgressNow(view) {
  if (view === workerView && !view.inFlight) void pollWorkerProgress();
}

document.addEventListener("visibilitychange", () => {
  const view = workerView;
  if (document.hidden || !view || view.inFlight || view.timer) return;
  void pollWorkerProgress();
});

async function applyWorkerProgress(view, answer) {
  // A restarted worker (§5.7) is a new process with a new progress file.
  if (view.caughtUp && !view.alive && answer.alive) {
    resetWorkerViewForRestart(view);
    return;
  }
  // The first read catches up silently: what finished is already in the log.
  const live = view.caughtUp;
  let jobEnded = false;
  for (const event of Array.isArray(answer.events) ? answer.events : []) {
    view.after = Math.max(view.after, Number(event.seq) || 0);
    const payload = event.payload || {};
    if (event.kind === "status") {
      const before = view.state;
      view.state = payload.state || view.state;
      if (view.state === "running" && before !== "paused" && before !== "running") {
        // A new job: a turn, or the rest of one after an approval.
        Object.assign(view, { liveText: "", plan: null, approval: null, lastPhase: null });
        view.turnError = "";
        if (live) {
          finishWorkerLive(view);
          showWorkerLive(view);
        }
      } else if (live && view.live && view.state !== before) {
        view.live.indicator.phase({
          phase: view.state,
          label: WORKER_VIEW_LABELS[view.state]?.[1],
        });
      }
      if (WORKER_JOB_END_STATES.has(view.state)) {
        view.turnError = payload.error || "";
        view.liveText = "";
        view.plan = null;
        if (live) {
          finishWorkerLive(view);
          jobEnded = true;
        }
      }
    } else if (event.kind === "phase") {
      view.lastPhase = payload;
      if (Number.isFinite(payload.round)) view.round = payload.round;
      if (Number.isFinite(payload.maxRounds)) view.maxRounds = payload.maxRounds;
      if (live) showWorkerLive(view).indicator.phase(payload);
    } else if (event.kind === "text") {
      view.liveText += payload.text || "";
      if (live) {
        const { target, indicator } = showWorkerLive(view);
        updateChatMessage(target, view.liveText);
        indicator.streaming();
      }
    } else if (event.kind === "plan") {
      view.plan = payload;
      if (live) renderPlanPanel(showWorkerLive(view).target, payload);
    } else if (event.kind === "approval") {
      view.approval = payload;
    } else if (event.kind === "note" && live) {
      await showWorkerNote(view, payload.messageId);
    }
  }
  const wasAlive = view.alive;
  view.alive = Boolean(answer.alive);
  if (!view.caughtUp) {
    view.caughtUp = true;
    if (view.alive && (view.state === "running" || view.state === "paused")) showWorkerLive(view);
    renderWorkerView(view);
    view.resolveReady();
    return;
  }
  // Its process ended: the log and the row (C5) say how it was left.
  if (wasAlive && !view.alive) {
    finishWorkerLive(view);
    jobEnded = true;
  }
  // The finished turn is in the session log now, so the log is drawn again,
  // as the shell does after a turn of its own.
  if (jobEnded) await loadSession(view.sessionId);
}

function resetWorkerViewForRestart(view) {
  finishWorkerLive(view);
  Object.assign(view, {
    after: 0,
    alive: true,
    state: "idle",
    round: null,
    maxRounds: null,
    lastPhase: null,
    plan: null,
    approval: null,
    liveText: "",
    turnError: "",
    pendingNotes: [],
    readNow: true,
  });
}

// The running turn's bubble: its text so far and the phase it is in.
function showWorkerLive(view) {
  if (view.live) return view.live;
  const target = appendChatMessage("assistant", "");
  const indicator = startTurnIndicator(target);
  view.live = { target, indicator };
  if (view.lastPhase) indicator.phase(view.lastPhase);
  if (view.liveText) {
    updateChatMessage(target, view.liveText);
    indicator.streaming();
  }
  if (view.state === "paused") indicator.phase({ phase: "paused", label: "Paused" });
  if (view.plan) renderPlanPanel(target, view.plan);
  return view.live;
}

function finishWorkerLive(view) {
  if (!view.live) return;
  view.live.indicator.finish("complete");
  view.live = null;
}

// §5.4: a note joins the conversation as a user message at the round boundary.
// The text streamed before it stays above it; the next round gets a new bubble.
async function showWorkerNote(view, messageId) {
  if (view.live && !view.liveText.trim()) view.live.target.message.remove();
  finishWorkerLive(view);
  view.liveText = "";
  let text = view.pendingNotes.shift();
  if (text === undefined) {
    try {
      const payload = await api(`/api/session?session_id=${encodeURIComponent(view.sessionId)}`);
      text = (payload.messages || []).find((message) => message.id === messageId)?.content;
    } catch {
      // The log has it; the next reload draws it.
    }
  }
  if (workerViewActive(view) && text) appendChatMessage("user", text);
}

function workerViewStatus(view) {
  if (!view.alive) {
    const unfinished =
      workerRowFor(view.sessionId)?.taskStatus === "interrupted" ||
      WORKER_TURN_STATES.has(view.state);
    return unfinished
      ? { dot: "failed", label: "Interrupted", ended: true }
      : { dot: "finished", label: "Ended", ended: true };
  }
  const [dot, label] = WORKER_VIEW_LABELS[view.state] || ["finished", "Idle"];
  return { dot, label, ended: false };
}

function workerViewRow(view) {
  const row = workerRowFor(view.sessionId) || findProjectWorkerRow(view.sessionId) || {};
  return {
    ...row,
    sessionId: view.sessionId,
    title: row.title || "this worker",
    state: view.state,
  };
}

function workerViewSource(view) {
  return workerSourceOf(workerViewRow(view).worktree || "") || repo();
}

function renderWorkerView(view) {
  if (!workerViewActive(view)) return;
  renderWorkerStrip(view);
  renderWorkerComposer(view);
  renderWorkerApproval(view);
  renderWorkerHeaderMeta();
}

// "from ‹parent›" and the branch, under the session title (§5.7).
function renderWorkerHeaderMeta() {
  const identity = document.querySelector(".thread-identity");
  if (!identity) return;
  let meta = identity.querySelector(".thread-worker-meta");
  const view = workerView;
  const row =
    view && !workersBoardProject && view.sessionId === currentSessionId
      ? workerRowFor(view.sessionId)
      : null;
  if (!row) {
    meta?.remove();
    return;
  }
  if (!meta) {
    meta = document.createElement("p");
    meta.className = "thread-worker-meta";
    identity.append(meta);
  }
  const parts = [];
  if (row.parentTitle) {
    const parent = document.createElement("em");
    parent.textContent = row.parentTitle;
    parts.push(document.createTextNode("from "), parent, document.createTextNode(" · "));
  }
  const branch = document.createElement("code");
  branch.textContent = row.branch;
  parts.push(branch);
  meta.replaceChildren(...parts);
  meta.title = row.parentTitle
    ? `Worker from ${row.parentTitle}, on ${row.branch}`
    : `Worker on ${row.branch}`;
}

function workerStripButton(label, className, run) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = className;
  button.textContent = label;
  button.addEventListener("click", runSessionAction(run));
  return button;
}

// Status pill, `Round r/max` with a bar, and the controls for the state:
// *Pause* and *Stop* while running, *Resume* and *Stop* while paused, *Stop*
// while waiting (§5.7). *Stop* also ends an idle worker, as on the board.
function renderWorkerStrip(view) {
  const top = document.querySelector(".thread-top");
  let strip = top.querySelector(".worker-strip");
  if (!strip) {
    strip = document.createElement("div");
    strip.className = "worker-strip";
    strip.setAttribute("role", "region");
    strip.setAttribute("aria-label", "Worker controls");
    top.querySelector(".thread-header").after(strip);
  }
  const status = workerViewStatus(view);
  const note = view.pollError
    ? `Could not refresh: ${view.pollError}`
    : view.turnError
      ? `Last turn failed: ${view.turnError}`
      : "";
  const signature = JSON.stringify([status, view.state, view.round, view.maxRounds, note]);
  // Redrawn only when it changed, so a focused button is not replaced each second.
  if (strip.dataset.signature === signature) return;
  strip.dataset.signature = signature;

  const pill = document.createElement("span");
  pill.className = "worker-pill";
  pill.dataset.state = status.dot;
  const dot = document.createElement("span");
  dot.className = "worker-dot";
  dot.dataset.state = status.dot;
  dot.setAttribute("aria-hidden", "true");
  pill.append(dot, document.createTextNode(status.label));

  const round = document.createElement("span");
  round.className = "worker-round";
  const bar = document.createElement("span");
  bar.className = "workers-rounds-bar";
  bar.setAttribute("aria-hidden", "true");
  const fill = document.createElement("span");
  const known =
    Number.isFinite(view.round) && Number.isFinite(view.maxRounds) && view.maxRounds > 0;
  fill.style.width = known ? `${Math.min(100, (view.round / view.maxRounds) * 100)}%` : "0%";
  bar.append(fill);
  round.append(
    bar,
    document.createTextNode(known ? `Round ${view.round}/${view.maxRounds}` : "Round —"),
  );

  const parts = [pill, round];
  if (note) {
    const text = document.createElement("span");
    text.className = "worker-strip-note";
    text.textContent = note;
    text.title = note;
    parts.push(text);
  }

  const actions = document.createElement("div");
  actions.className = "inline-actions worker-strip-actions";
  const row = () => workerViewRow(view);
  const after = () => pollWorkerProgressNow(view);
  if (status.ended) {
    actions.append(
      workerStripButton("Restart worker", "btn-sm", () => restartWorkerFromView(view)),
    );
  } else {
    if (view.state === "running") {
      actions.append(
        workerStripButton("Pause", "btn-sm", async () => {
          await pauseWorker(row(), workerViewSource(view));
          after();
        }),
      );
    }
    if (view.state === "paused") {
      actions.append(
        workerStripButton("Resume", "btn-sm btn-primary", async () => {
          await api("/api/worker-control", form({ session_id: view.sessionId, command: "resume" }));
          after();
        }),
      );
    }
    if (view.state !== "shutting_down") {
      actions.append(
        workerStripButton("Stop", "btn-sm btn-danger", async () => {
          await stopWorker(row(), workerViewSource(view));
          after();
        }),
      );
    }
  }
  parts.push(actions);
  strip.replaceChildren(...parts);
}

// *Restart worker* (§5.7): a new, idle process for the same session and
// worktree. A pending approval belonged to the old process and is not handed
// to the new one (Task 7), so the command it asked about did not run.
async function restartWorkerFromView(view) {
  const wasWaiting = view.state === "waiting_for_approval";
  await api("/api/worker-restart", form({ session_id: view.sessionId }));
  resetWorkerViewForRestart(view);
  toast(
    wasWaiting
      ? "Worker restarted. The approval it was waiting for ended with its old process, so that command did not run. Send a message to continue."
      : "Worker restarted. Send it a message to continue.",
  );
  refreshWorkersAfterAction(workerViewSource(view));
  renderWorkerView(view);
  pollWorkerProgressNow(view);
}

function workerComposerMode(view) {
  if (!view.alive || view.sending) return null;
  return WORKER_COMPOSER_MODES[view.state] || null;
}

// The composer sends a note while running, a resume with a note while paused
// and a message when idle, and says which on its button. It is disabled while
// the worker waits for approval (notes are refused then) and once it ended.
function renderWorkerComposer(view) {
  const button = $("ask-btn");
  const field = $("chat-prompt");
  if (!composerDefaults) {
    composerDefaults = { placeholder: field.placeholder, content: [...button.childNodes] };
  }
  const mode = workerComposerMode(view);
  let placeholder = mode?.[2];
  if (!mode && !view.sending) {
    if (workerViewStatus(view).ended) {
      placeholder = "This worker's process has ended. Restart it to continue.";
    } else if (view.state === "waiting_for_approval") {
      placeholder = "Approve or reject the proposal above first. A waiting worker takes no notes.";
    } else {
      placeholder = "The worker is stopping.";
    }
  }
  const label = mode?.[1] || "Send";
  button.classList.add("is-worker-command");
  if (button.textContent !== label) button.textContent = label;
  button.setAttribute("aria-label", label);
  button.disabled = !mode;
  field.disabled = !mode && !view.sending;
  if (placeholder) field.placeholder = placeholder;
}

function restoreComposerFromWorker() {
  if (!composerDefaults) return;
  const button = $("ask-btn");
  const field = $("chat-prompt");
  button.classList.remove("is-worker-command");
  button.replaceChildren(...composerDefaults.content);
  button.setAttribute("aria-label", "Send message");
  button.disabled = false;
  field.disabled = false;
  field.placeholder = composerDefaults.placeholder;
  composerDefaults = null;
}

// Requirement 4: every word to a worker goes through `/api/worker-control`.
async function sendWorkerComposer(override = "") {
  const view = workerView;
  const mode = view && workerComposerMode(view);
  if (!mode) return;
  const field = $("chat-prompt");
  const text = (override || field.value).trim();
  if (!text) {
    toast(mode[0] === "message" ? "Write a message first" : "Write a note first");
    return;
  }
  const fields = { session_id: view.sessionId, command: mode[0] };
  if (mode[0] === "resume") fields.note = text;
  else fields.text = text;
  view.sending = true;
  renderWorkerComposer(view);
  try {
    await api("/api/worker-control", form(fields));
    if (!override) {
      field.value = "";
      autoGrowPrompt();
    }
    if (mode[0] === "message") {
      // The worker writes it to the log as its turn starts; drawn now so it
      // does not wait for the next read.
      appendChatMessage("user", text);
      Object.assign(view, { state: "running", liveText: "", plan: null, approval: null });
      Object.assign(view, { lastPhase: null, turnError: "" });
      renderWorkerApproval(view);
      showWorkerLive(view);
    } else {
      view.pendingNotes.push(text);
      toast(
        mode[0] === "note"
          ? "Note queued. The worker reads it before its next round."
          : "Resuming. The worker reads your note before its next round.",
      );
    }
  } catch (error) {
    toast(error.message);
  } finally {
    view.sending = false;
    if (view === workerView) {
      renderWorkerView(view);
      pollWorkerProgressNow(view);
    }
  }
}

// Which proposal the open worker shows a card for. A command or plan waits in
// the live process, so it shows only while that process waits; a patch or a
// provider-fallback offer ended the turn and is answered through the shell's
// own routes with the worktree as `repo` (§5.6), until the next job starts.
function workerApprovalToShow(view) {
  const approval = view.approval;
  if (!view.caughtUp || !approval?.proposal) return null;
  if (approval.kind === "command" || approval.kind === "plan") {
    return view.alive && view.state === "waiting_for_approval" ? approval : null;
  }
  if (approval.kind === "patch" || approval.kind === "fallback") {
    return view.state === "idle" ? approval : null;
  }
  return null;
}

// The shell's own cards, consent content in full (style guide §5). Only where
// the decision goes differs: a command or plan to the worker.
function renderWorkerApproval(view) {
  const log = $("chat-log");
  const shown = log.querySelector("[data-worker-approval]");
  const approval = workerApprovalToShow(view);
  const proposal = approval?.proposal || {};
  const key = approval
    ? `${view.sessionId}\n${approval.kind}\n${proposal.proposalId || proposal.patchId || ""}`
    : "";
  if (shown && shown.dataset.workerApproval === key) return;
  shown?.remove();
  if (!approval) return;
  const worktree = workerViewRow(view).worktree || "";
  const decide = (approved) => decideWorkerApproval(view, proposal, approved);
  const card =
    approval.kind === "command"
      ? createCommandApprovalPreview(proposal, worktree, { decide })
      : approval.kind === "plan"
        ? createPlanReview(proposal, worktree, { decide })
        : approval.kind === "patch"
          ? createPatchPreview(proposal, worktree)
          : createFallbackOffer(proposal, worktree);
  card.dataset.workerApproval = key;
  // Beside the last answer's body, not in it: the body is re-rendered once
  // its markdown is finalized, which would take the card with it.
  const answers = log.querySelectorAll(".message.assistant");
  const host = answers[answers.length - 1];
  if (host) host.append(card);
  else log.append(card);
}

async function decideWorkerApproval(view, proposal, approved) {
  await api(
    "/api/worker-control",
    form({
      session_id: view.sessionId,
      command: approved ? "approve" : "reject",
      proposal_id: proposal.proposalId,
    }),
  );
  pollWorkerProgressNow(view);
  return approved
    ? "Approved. The worker continues its turn in its own process."
    : "Rejected. The worker continues its turn without it.";
}

// A cost that rounds to zero is shown as a bound, never as `$0.0000`: a real
// charge displayed as zero reads as "this turn was free", which is the one
// thing token accounting exists to stop saying. An actual zero is still zero.
function formatCost(cost) {
  if (cost > 0 && cost < 0.0001) return "<$0.0001";
  return `$${cost.toFixed(4)}`;
}

// Per docs/specs/49_prompt_cache_accounting_and_reuse/ §5.3. A figure computed
// at the full input rate while the provider reported cached tokens can only be
// too high, so it is shown rather than suppressed — but never as though it were
// exact. Composes with `formatCost`, so a sub-$0.0001 bound still reads
// `at most <$0.0001`.
function formatCostAtYourRates(usage) {
  if (typeof usage.estimatedCost !== "number") return null;
  const figure = formatCost(usage.estimatedCost);
  return usage.estimatedCostIsUpperBound
    ? `at most ${figure} at your rates`
    : `${figure} at your rates`;
}

// §5.6. A hit rate is only ever shown when the provider actually reported one:
// an unreported split must never render as 0%, which reads as "caching is
// broken" when the truth is "we cannot see it". That case is explained in the
// row's tooltip instead of taking a permanent slot in a one-line summary.
//
// `cacheHitRate` is a rate over the runs that reported, not over the task, so
// where some runs stayed silent the text says how many it covers rather than
// implying it covers everything.
function formatCacheHitRate(usage) {
  if (typeof usage.cacheHitRate !== "number") return null;
  const percent = `${Math.round(usage.cacheHitRate * 100)}% cached`;
  const silent = usage.runsWithoutCacheReport ?? 0;
  if (silent === 0) return percent;
  const covered = usage.runCount - silent;
  return `${percent} (of ${covered} of ${usage.runCount} calls)`;
}

// The sentence §5.6 requires for a provider that reports nothing. Lives in the
// tooltip rather than the visible row: stated on every turn it would be
// permanent chrome saying the same thing forever, which is what specs 41–44
// were written to remove.
function cacheUsageTitle(usage) {
  if (typeof usage.cacheHitRate === "number") return null;
  if (typeof usage.cachedInputTokens === "number") {
    return "This provider reported a cache split for this turn, but no input tokens to measure it against.";
  }
  return "Cache usage not reported by this provider. This is not 0% — it means the split cannot be seen from here.";
}

// Per docs/specs/19_token_and_cost_accounting/. An estimated figure is always
// marked as one: a total is only as trustworthy as its weakest term, so a
// single estimated call makes the whole turn's number an approximation.
//
// `reportedCost` is what the provider charged. It is absent for almost every
// provider, and absent is rendered as nothing rather than as a zero, which
// would read as "this turn was free".
function markMessageUsage(target, usage, durationMs = null) {
  if (!usage || typeof usage.inputTokens !== "number") return;
  if (target.body.nextElementSibling?.dataset?.state === "usage") return;
  const row = document.createElement("p");
  row.className = "turn-indicator";
  row.dataset.state = "usage";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  const estimated = usage.usageSource === "estimated";
  const total = usage.inputTokens + usage.outputTokens;
  const calls = usage.runCount === 1 ? "1 model call" : `${usage.runCount} model calls`;
  const parts = [
    `${estimated ? "~" : ""}${total.toLocaleString()} tokens${estimated ? " (estimated)" : ""}`,
    calls,
  ];
  if (typeof durationMs === "number") parts.push(formatElapsed(durationMs));
  // What the provider charged wins over what the user's own rates compute:
  // one is a fact about the bill, the other is arithmetic, and the label says
  // which is which rather than letting them look alike.
  if (typeof usage.reportedCost === "number") {
    parts.push(formatCost(usage.reportedCost));
  } else {
    const atYourRates = formatCostAtYourRates(usage);
    if (atYourRates) parts.push(atYourRates);
  }
  const cached = formatCacheHitRate(usage);
  if (cached) parts.push(cached);
  const cacheTitle = cacheUsageTitle(usage);
  if (cacheTitle) row.title = cacheTitle;
  label.textContent = parts.join(" · ");
  row.append(label);
  target.body.after(row);
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

function markMessageToolBudgetExhausted(target, sessionId = currentSessionId, taskId = null) {
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
  // *Ask again* (spec 61 §5.7): starts a new turn whose prompt the engine
  // builds from the stopped task's own request, not a fixed "continue
  // debugging" string. Without a task id there is nothing to re-ask, so the
  // button is only offered when one is known.
  button.textContent = "Ask again";
  button.addEventListener("click", async () => {
    try {
      button.disabled = true;
      await askAgainFromExhausted(sessionId, taskId);
    } catch (error) {
      button.disabled = false;
      toast(error.message);
    }
  });
  if (taskId) row.append(label, button);
  else row.append(label);
  target.body.after(row);
}

// A turn stopped at its token ceiling (spec 21 §5.4). Deliberately not folded
// into the tool-budget row: one means the work needed more rounds, the other
// that it needed more money, and the remedies are different. There is no
// "continue" button here on purpose — continuing means raising
// `agent_max_task_tokens` first, and a one-click button beside the number that
// stopped the spending would be the wrong shape of control.
function markMessageTokenBudgetExhausted(target) {
  if (target.body.nextElementSibling?.dataset?.state === "incomplete") return;
  const row = document.createElement("div");
  row.className = "turn-indicator";
  row.dataset.state = "incomplete";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.textContent = "Token budget exhausted";
  const hint = document.createElement("span");
  hint.className = "turn-indicator-hint";
  hint.textContent = "Raise agent_max_task_tokens to go further.";
  row.append(label, hint);
  target.body.after(row);
}

// The one place that turns a stopped turn's status into a row under the
// answer. Both stops used to be handled at each call site, which is how a
// token stop rendered as an ordinary completed turn everywhere except the one
// site that knew about it.
function markMessageTurnStop(target, payload, sessionId = currentSessionId) {
  if (payload.taskStatus === "tool_budget_exhausted") {
    markMessageToolBudgetExhausted(target, payload.sessionId || sessionId, payload.taskId || null);
  } else if (payload.taskStatus === "token_budget_exhausted") {
    markMessageTokenBudgetExhausted(target);
  } else if (payload.incomplete) {
    markMessageIncomplete(target);
  }
}

// A turn whose model run ended early for a reason no budget names (spec 58
// §3.2). Quiet on purpose: there is nothing to press, only something to know.
//
// Live-only. The run's `incomplete` flag is not stored on the task, so a reload
// cannot tell this turn from a complete one; and the engine sets it only on
// cancellation, whose task is stored as `cancelled` and reloads as "Stopped by
// you" instead.
function markMessageIncomplete(target) {
  if (target.body.nextElementSibling?.dataset?.state === "incomplete") return;
  const row = document.createElement("p");
  row.className = "turn-indicator";
  row.dataset.state = "incomplete";
  const label = document.createElement("span");
  label.className = "turn-indicator-label";
  label.textContent = "Stopped before finishing";
  row.append(label);
  target.body.after(row);
}

// How a finished turn's indicator ends: "incomplete" for every end that
// `markMessageTurnStop` marks, "complete" otherwise.
function chatCompletionState(payload) {
  const exhausted =
    payload.taskStatus === "tool_budget_exhausted" ||
    payload.taskStatus === "token_budget_exhausted";
  return exhausted || payload.incomplete ? "incomplete" : "complete";
}

// The files a turn read, folded into that turn rather than a strip docked above
// the composer. Reference information, so the paths are quiet links rather than
// buttons.
//
// Live-turn only: stored messages carry no `contextFiles`, so this does not
// survive a session reload. See the spec's context.md §3 — making it durable is
// a server-side change.
function appendContextDisclosure(body, files) {
  if (!files.length) return;

  const panel = document.createElement("div");
  panel.className = "context-file-list";
  files.forEach((path) => {
    const link = document.createElement("button");
    link.type = "button";
    link.className = "context-file";
    link.textContent = path;
    link.title = `${path} — open in Visual Studio Code`;
    link.addEventListener("click", async () => {
      try {
        const payload = await api("/api/open-vscode-file", form({ repo: requireRepo(), path }));
        toast(`Opened ${payload.path}`);
      } catch (error) {
        toast(error.message);
      }
    });
    panel.append(link);
  });

  const label = files.length === 1 ? "Read 1 file" : `Read ${files.length} files`;
  body.append(createDisclosure(label, panel), panel);
}

// The notes a turn kept, folded into that turn (spec 65 §5.8). The text comes
// from the shell, so it survives a reload; a turn with no notes shows nothing.
function appendTurnNotes(body, notes) {
  if (!notes?.text) return;

  const panel = document.createElement("pre");
  panel.className = "turn-notes";
  panel.textContent = notes.text;
  const label = notes.carried ? "Notes (carried from the previous turn)" : "Notes";
  body.append(createDisclosure(label, panel), panel);
}

// The plan panel, per docs/specs/21_task_plan_progress_and_budget §5.6.
//
// One panel per assistant message, replaced in place as `plan` events arrive.
// The engine sends the whole plan each time rather than a delta, so a rebuild
// is the correct update: there is no accumulated state here that a missed
// event could corrupt.
const STEP_OUTCOME_LABELS = {
  verified: "confirmed",
  // Said out loud rather than shown as a plain tick. Requirement 6's whole
  // point is that "the model said it was done" is not confirmation, and a
  // panel that rendered the two identically would undo the distinction the
  // engine works to preserve.
  unverified: "completed, unverified",
  blocked: "blocked",
  skipped: "skipped",
  outstanding: "",
};

function renderPlanPanel(message, plan) {
  if (!plan || !Array.isArray(plan.steps) || !plan.steps.length) return;

  const panel = document.createElement("section");
  panel.className = "plan-panel";
  panel.dataset.planPanel = "true";

  const header = document.createElement("div");
  header.className = "plan-panel-header";
  const title = document.createElement("strong");
  title.textContent = "Plan";
  const summary = document.createElement("span");
  summary.className = "plan-panel-summary";
  summary.dataset.complete = plan.isComplete ? "true" : "false";
  summary.textContent = plan.summary || "";
  header.append(title, summary);
  panel.append(header);

  // Requirement 2 says one step runs at a time. If the log ever says otherwise
  // the panel says so too: rendering the first of two in-progress steps and
  // moving on would hide the exact bug the invariant exists to catch.
  if (plan.violatesSingleInProgress) {
    const warning = document.createElement("p");
    warning.className = "plan-panel-warning";
    warning.textContent =
      "More than one step is marked in progress. That should not happen — the plan below is shown as recorded.";
    panel.append(warning);
  }

  const list = document.createElement("ol");
  list.className = "plan-step-list";
  plan.steps.forEach((step) => {
    const item = document.createElement("li");
    item.className = "plan-step";
    item.dataset.status = step.status || "pending";
    item.dataset.outcome = step.outcome || "outstanding";

    const label = document.createElement("span");
    label.className = "plan-step-title";
    label.textContent = step.title || "";
    item.append(label);

    const outcome = STEP_OUTCOME_LABELS[step.outcome] || "";
    if (outcome) {
      const note = document.createElement("span");
      note.className = "plan-step-outcome";
      note.textContent = outcome;
      item.append(note);
    }
    if (step.status === "in_progress") {
      // "running" only while a turn is going. Once it has ended, an open step
      // is still open — say what Damaian knows rather than implying work is in
      // flight (spec 66 §5.4). An older payload without `active` reads as live.
      const live = plan.active !== false;
      const state = document.createElement("span");
      state.className = "plan-step-outcome";
      state.textContent = live ? "running" : "not finished";
      item.append(state);
    }

    const evidence = Array.isArray(step.evidence) ? step.evidence : [];
    if (evidence.length) {
      const detail = document.createElement("ul");
      detail.className = "plan-step-evidence";
      evidence.forEach((entry) => {
        const line = document.createElement("li");
        line.textContent = describeEvidence(entry);
        detail.append(line);
      });
      item.append(createDisclosure(`What confirms this (${evidence.length})`, detail), detail);
    }
    list.append(item);
  });
  const steps = createDisclosure(`Steps (${plan.steps.length})`, list);
  panel.append(steps, list);

  const existing = message.body.parentElement?.querySelector('[data-plan-panel="true"]');
  if (existing) {
    existing.replaceWith(panel);
  } else {
    message.body.after(panel);
  }
}

// Evidence in the user's words. The engine's `Evidence` is a tagged union and
// this is the only place that maps it to prose, so a variant added later shows
// up as "recorded" rather than as `[object Object]`.
function describeEvidence(entry) {
  if (!entry || typeof entry !== "object") return "recorded";
  if (entry.kind === "commandExit") {
    return entry.exitCode === 0
      ? "a command ran and exited 0"
      : entry.exitCode === null || entry.exitCode === undefined
        ? "a command ran but reported no exit code"
        : `a command ran and exited ${entry.exitCode}`;
  }
  if (entry.kind === "patchApplied") {
    const files = Array.isArray(entry.files) ? entry.files : [];
    const names = files.map((file) => file.path).join(", ");
    return files.length ? `a patch was applied to ${names}` : "a patch was applied";
  }
  if (entry.kind === "fileRead") {
    return `${entry.path} was read`;
  }
  if (entry.kind === "findings") {
    const failing = Number.isInteger(entry.failing) ? entry.failing : 0;
    const total = Array.isArray(entry.refs) ? entry.refs.length : 0;
    return failing > 0
      ? `the check reported ${failing} failing finding${failing === 1 ? "" : "s"}`
      : `the check reported ${total} finding${total === 1 ? "" : "s"}, none failing`;
  }
  return "recorded";
}

// The browser diagnostic card (spec 12 §5.3). Every string in `record.report`
// came from a web page, so all of it goes in through `textContent`.
//
// The header mirrors `WebDiagnosticReport::render_for_model` in
// workspace-engine's `web_diagnostics.rs` — sections, order, plurals and the
// 500-character cap on a tool error. That Rust is the source of truth: if the
// two ever read differently, this is the one to fix.
const WEB_DIAGNOSTIC_ITEM_CHARS = 500;
const WEB_DIAGNOSTIC_VISIBLE_ITEMS = 5;
const WEB_DIAGNOSTIC_IMAGE = /\.(?:png|jpe?g|webp|gif)$/i;

// Characters, not UTF-16 units, to match Rust's `chars().count()`.
function truncateWebDiagnosticText(text, limit) {
  const chars = Array.from(String(text ?? ""));
  return chars.length <= limit ? chars.join("") : `${chars.slice(0, limit).join("")}…`;
}

function webDiagnosticConsoleIs(entry, levels) {
  return levels.includes(String(entry.level || "").toLowerCase());
}

// `problem_sections`, in its order. `items` are only counted here; the lists
// below format their own lines.
function webDiagnosticSections(details) {
  const entries = Array.isArray(details.console) ? details.console : [];
  const steps = Array.isArray(details.steps) ? details.steps : [];
  return [
    { noun: "page error", items: details.page_errors || [] },
    {
      noun: "console error",
      items: entries.filter((entry) => webDiagnosticConsoleIs(entry, ["error", "assert"])),
    },
    {
      noun: "console warning",
      items: entries.filter((entry) => webDiagnosticConsoleIs(entry, ["warning", "warn"])),
    },
    { noun: "failed request", items: details.failed_requests || [] },
    { noun: "failed step", items: steps.filter((step) => !step.success) },
  ];
}

function webDiagnosticHeader(report) {
  const details = report.details;
  if (!details) {
    return report.is_error ? "Browser diagnostic failed" : "Browser diagnostic result";
  }
  if (typeof details.tool_error === "string") {
    return `Browser diagnostic failed: ${truncateWebDiagnosticText(
      details.tool_error,
      WEB_DIAGNOSTIC_ITEM_CHARS,
    )}`;
  }
  const parts = webDiagnosticSections(details)
    .filter((section) => section.items.length)
    .map(
      (section) =>
        `${section.items.length} ${section.noun}${section.items.length === 1 ? "" : "s"}`,
    );
  return parts.length
    ? `Browser diagnostic found ${parts.join(", ")}.`
    : "Browser diagnostic found no page errors, console problems, or failed requests.";
}

function webDiagnosticLocation(location) {
  if (!location?.url) return "";
  return [location.url, location.line, location.column]
    .filter((part) => part !== null && part !== undefined)
    .join(":");
}

function webDiagnosticRequestLine(request) {
  const target = request.method ? `${request.method} ${request.url}` : request.url;
  const outcome = [request.status, request.failure]
    .filter((part) => part !== null && part !== undefined && part !== "")
    .join(" ");
  return `${target} → ${outcome || "failed"}`;
}

// One labelled list, its first five items shown and the rest behind a toggle.
// `line` returns the item's text and, optionally, a source location.
function webDiagnosticList(label, items, line) {
  const section = document.createElement("div");
  section.className = "web-diagnostic-card-section";
  const heading = document.createElement("span");
  heading.className = "web-diagnostic-card-label";
  heading.textContent = label;
  const list = document.createElement("ul");
  list.className = "web-diagnostic-card-list";
  items.forEach((item, index) => {
    const { text, location } = line(item);
    const entry = document.createElement("li");
    entry.title = text;
    entry.textContent = truncateWebDiagnosticText(text, WEB_DIAGNOSTIC_ITEM_CHARS);
    if (location) {
      const where = document.createElement("span");
      where.className = "web-diagnostic-card-location";
      where.textContent = location;
      entry.append(" ", where);
    }
    entry.hidden = index >= WEB_DIAGNOSTIC_VISIBLE_ITEMS;
    list.append(entry);
  });
  section.append(heading, list);

  if (items.length > WEB_DIAGNOSTIC_VISIBLE_ITEMS) {
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "btn-sm btn-quiet web-diagnostic-card-more";
    toggle.textContent = `Show all ${items.length}`;
    toggle.setAttribute("aria-expanded", "false");
    toggle.addEventListener("click", () => {
      const expand = toggle.getAttribute("aria-expanded") !== "true";
      toggle.setAttribute("aria-expanded", expand ? "true" : "false");
      toggle.textContent = expand ? "Show fewer" : `Show all ${items.length}`;
      [...list.children].forEach((entry, index) => {
        entry.hidden = !expand && index >= WEB_DIAGNOSTIC_VISIBLE_ITEMS;
      });
    });
    section.append(toggle);
  }
  return section;
}

function webDiagnosticDomPanel(dom) {
  const buttons = Array.isArray(dom.buttons) ? dom.buttons : [];
  const rows = [
    ["Buttons", buttons.join(", ")],
    ["Status text", dom.status_text || ""],
    ["Visible text", dom.visible_text_excerpt || ""],
  ].filter(([, value]) => value);
  if (!rows.length) return null;
  const panel = document.createElement("dl");
  panel.className = "web-diagnostic-card-dom";
  rows.forEach(([label, value]) => {
    const term = document.createElement("dt");
    term.textContent = label;
    const description = document.createElement("dd");
    description.textContent = value;
    panel.append(term, description);
  });
  return panel;
}

function webDiagnosticScreenshot(artifact) {
  const path = artifact.path;
  const name = path.split("/").pop() || "screenshot";
  const item = document.createElement("figure");
  item.className = "web-diagnostic-artifact";

  const image = document.createElement("img");
  image.alt = name;
  image.loading = "lazy";

  const caption = document.createElement("figcaption");
  caption.title = path;
  caption.textContent =
    artifact.width && artifact.height ? `${name} · ${artifact.width}×${artifact.height}` : name;

  const actions = document.createElement("div");
  actions.className = "web-diagnostic-artifact-actions";
  const reveal = document.createElement("button");
  reveal.type = "button";
  reveal.className = "btn-sm btn-quiet";
  reveal.textContent = "Reveal in Finder";
  const failure = document.createElement("p");
  failure.className = "web-diagnostic-card-error";
  failure.hidden = true;
  reveal.addEventListener("click", async () => {
    failure.hidden = true;
    try {
      await api("/api/reveal-web-diagnostic-artifact", form({ repo: repo(), path }));
    } catch (error) {
      failure.textContent = error.message;
      failure.hidden = false;
    }
  });
  actions.append(reveal, failure);

  item.append(image, caption, actions);
  void loadWebDiagnosticArtifact(image, path);
  return item;
}

function renderWebDiagnosticCard(message, record) {
  const report = record?.report;
  if (!report) return;
  const host = message.body.parentElement;
  if (!host || (record.id && host.querySelector(`[data-record-id="${CSS.escape(record.id)}"]`))) {
    return;
  }
  const details = report.details || null;

  const card = document.createElement("section");
  card.className = "web-diagnostic-card";
  if (record.id) card.dataset.recordId = record.id;

  const header = document.createElement("div");
  header.className = "web-diagnostic-card-header";
  const title = document.createElement("strong");
  title.className = "web-diagnostic-card-title";
  title.textContent = webDiagnosticHeader(report);
  // Another runner's result carries no counts, so it claims neither state.
  title.dataset.state =
    report.is_error || typeof details?.tool_error === "string"
      ? "failed"
      : !details
        ? "unknown"
        : webDiagnosticSections(details).some((section) => section.items.length)
          ? "problems"
          : "clean";
  const target = document.createElement("span");
  target.className = "web-diagnostic-card-target";
  target.textContent = [record.tool === "run_web_scenario" ? "Scenario" : "Inspected", record.url]
    .filter(Boolean)
    .join(" · ");
  header.append(title, target);
  card.append(header);

  const footer = document.createElement("div");
  footer.className = "web-diagnostic-card-footer";
  // Disclosure panels go above the footer, next to what they explain
  // (docs/UI_STYLE_GUIDE.md §7), so they are collected and placed last.
  const panels = [];

  if (details) {
    const meta = [
      details.final_url,
      details.status ? `HTTP ${details.status}` : "",
      details.title,
    ].filter(Boolean);
    if (meta.length) {
      const row = document.createElement("p");
      row.className = "web-diagnostic-card-meta";
      meta.forEach((value) => {
        const part = document.createElement("span");
        part.textContent = value;
        row.append(part);
      });
      card.append(row);
    }

    const [pageErrors, consoleErrors, consoleWarnings, failedRequests, failedSteps] =
      webDiagnosticSections(details).map((section) => section.items);
    // Errors and warnings are counted apart in the header but read as one
    // list, in the order the page logged them.
    const consoleProblems = (details.console || []).filter(
      (entry) => consoleErrors.includes(entry) || consoleWarnings.includes(entry),
    );
    const lists = [
      ["Page errors", pageErrors, (error) => ({ text: error })],
      [
        "Console",
        consoleProblems,
        (entry) => ({
          text: `${entry.level}: ${entry.text}`,
          location: webDiagnosticLocation(entry.location),
        }),
      ],
      [
        "Failed requests",
        failedRequests,
        (request) => ({ text: webDiagnosticRequestLine(request) }),
      ],
      [
        "Failed steps",
        failedSteps,
        (step) => ({
          text: `step ${step.step} ${step.action}${step.error ? `: ${step.error}` : ""}`,
        }),
      ],
    ];
    lists.forEach(([label, items, line]) => {
      if (items.length) card.append(webDiagnosticList(label, items, line));
    });

    const dom = details.dom_summary ? webDiagnosticDomPanel(details.dom_summary) : null;
    if (dom) {
      footer.append(createDisclosure("Page summary", dom));
      panels.push(dom);
    }
  } else if (report.text) {
    const raw = document.createElement("pre");
    raw.className = "web-diagnostic-card-raw";
    raw.textContent = report.text;
    footer.append(createDisclosure("Diagnostic output", raw));
    panels.push(raw);
  }

  const screenshots = (report.artifacts || []).filter(
    (artifact) =>
      typeof artifact.path === "string" &&
      artifact.path.startsWith("web-diagnostics/") &&
      WEB_DIAGNOSTIC_IMAGE.test(artifact.path),
  );
  if (screenshots.length) {
    const grid = document.createElement("div");
    grid.className = "web-diagnostic-artifacts";
    screenshots.forEach((artifact) => {
      grid.append(webDiagnosticScreenshot(artifact));
    });
    card.append(grid);
  }

  card.append(...panels);
  if (footer.childElementCount) card.append(footer);

  // In run order, directly under the answer and ahead of the plan panel.
  const cards = host.querySelectorAll(".web-diagnostic-card");
  if (cards.length) {
    cards[cards.length - 1].after(card);
  } else {
    message.body.after(card);
  }
}

// The findings panel (spec 22 Task 11). It is session-scoped: what the
// session's checks found, for the user to open, dismiss, or hand back to the
// agent as a scoped repair (spec 22 §5.5, `context.md` §17). Every string came
// from tool output, so all of it goes in through `textContent`.
const FINDING_SOURCE_LABELS = {
  compiler: "Compiler",
  test: "Tests",
  lint: "Lint",
  security: "Security",
  browser_console: "Browser console",
  browser_network: "Network",
  browser_scenario: "Browser scenario",
  code_review: "Code review",
  language_server: "Language server",
  hook: "Hook",
  command: "Unparsed checks",
};

const FINDING_STATUS_NOTES = {
  stale: "Its file changed after the check ran. Re-run the check to repair it.",
  dismissed: "Dismissed.",
  fixed: "Marked fixed.",
};

let sessionFindings = [];
const findingsSelection = new Set();
let findingsPanelOpen = false;

async function refreshFindings() {
  const sessionId = currentSessionId;
  // A worker opened under its source project keeps its findings in its
  // worktree, the repository its session belongs to (spec 57 §5.7).
  const repoPath = (workersEnabled && workerRowFor(sessionId)?.worktree) || repo();
  if (!sessionId || !repoPath) {
    sessionFindings = [];
  } else {
    try {
      const payload = await api(
        `/api/findings?repo=${encodeURIComponent(repoPath)}&session_id=${encodeURIComponent(sessionId)}`,
      );
      // A session switched while this was in flight owns the panel now.
      if (sessionId !== currentSessionId) return;
      sessionFindings = payload.findings || [];
    } catch (error) {
      sessionFindings = [];
      toast(`Findings could not be loaded: ${error.message}`);
    }
  }
  for (const id of [...findingsSelection]) {
    const stillOpen = sessionFindings.some(
      (finding) => finding.id === id && finding.status === "open",
    );
    if (!stillOpen) findingsSelection.delete(id);
  }
  renderFindingsPanel();
}

function clearFindings() {
  sessionFindings = [];
  findingsSelection.clear();
  renderFindingsPanel();
}

function findingVisible(finding) {
  const severity = $("findings-severity-filter").value;
  const status = $("findings-status-filter").value;
  const severityShown =
    severity === "all" ||
    (severity === "warning" ? finding.severity !== "info" : finding.severity === "error");
  const statusShown =
    status === "all" ||
    (status === "open_stale"
      ? finding.status === "open" || finding.status === "stale"
      : finding.status === "open");
  return severityShown && statusShown;
}

/** Groups in first-appearance order, so session order survives grouping. */
function groupFindings(findings, key) {
  const groups = new Map();
  for (const finding of findings) {
    const value = key(finding);
    if (!groups.has(value)) groups.set(value, []);
    groups.get(value).push(finding);
  }
  return groups;
}

// The findings list is a popover off the repo bar's action chevron (spec 58
// §1 req 17), so it is reachable only while that action is showing.
function toggleFindingsPopover(anchorEl, severity) {
  if (findingsPanelOpen) {
    closeFindingsPopover({ restoreFocus: true });
    return;
  }
  closeAllContextMenus();
  // Opened from "Fix N warnings", the default Errors filter would show none of
  // the findings the action counts.
  const severityFilter = $("findings-severity-filter");
  if (severity === "warning" && severityFilter.value === "error") severityFilter.value = "warning";
  findingsPanelOpen = true;
  renderFindingsPanel();
  anchorEl.setAttribute("aria-expanded", "true");
  $("findings-severity-filter").focus();
}

function closeFindingsPopover({ restoreFocus = false } = {}) {
  if (!findingsPanelOpen) return;
  findingsPanelOpen = false;
  renderFindingsPanel();
  if (restoreFocus) $("repo-action-more").focus();
}

function renderFindingsPanel() {
  renderRepoAction();
  const panel = $("findings-panel");
  // Its anchor went away (no open errors or warnings left, or no folder).
  if (findingsPanelOpen && !repoActionCurrent?.key.startsWith("fix-")) findingsPanelOpen = false;
  panel.hidden = !findingsPanelOpen || sessionFindings.length === 0;
  $("repo-action-more").setAttribute("aria-expanded", String(!panel.hidden));
  if (panel.hidden) return;

  const visible = sessionFindings.filter(findingVisible);
  $("findings-count").textContent = `Showing ${visible.length} of ${sessionFindings.length}`;
  const list = $("findings-list");
  list.replaceChildren();
  if (!visible.length) {
    const empty = document.createElement("p");
    empty.className = "findings-empty";
    empty.textContent = "Nothing matches these filters.";
    list.append(empty);
  }
  for (const [source, bySource] of groupFindings(visible, (finding) => finding.source)) {
    const group = document.createElement("section");
    group.className = "findings-group";
    const title = document.createElement("h3");
    title.className = "findings-group-title";
    title.textContent = `${FINDING_SOURCE_LABELS[source] || source} (${bySource.length})`;
    group.append(title);
    for (const [path, byFile] of groupFindings(bySource, (finding) => finding.range?.path || "")) {
      const file = document.createElement("div");
      file.className = "findings-file";
      const label = document.createElement("div");
      label.className = "findings-file-path";
      label.textContent = path || "No location";
      if (path) label.title = path;
      file.append(label, ...byFile.map(findingRow));
      group.append(file);
    }
    list.append(group);
  }
  wireFileReferences(list, repo());

  const fix = $("findings-fix-btn");
  fix.disabled = findingsSelection.size === 0 || chatSubmitting;
  fix.textContent = findingsSelection.size
    ? `Fix selected (${findingsSelection.size})`
    : "Fix selected";
}

function findingRow(finding) {
  const row = document.createElement("div");
  row.className = "finding-row";
  row.dataset.findingId = finding.id;
  row.dataset.status = finding.status;

  const select = document.createElement("input");
  select.type = "checkbox";
  select.className = "finding-select";
  select.checked = findingsSelection.has(finding.id);
  // Only an open finding can be repaired (spec 22 `context.md` §15).
  select.disabled = finding.status !== "open";
  select.setAttribute("aria-label", `Select: ${finding.summary}`);
  select.addEventListener("change", () => {
    if (select.checked) findingsSelection.add(finding.id);
    else findingsSelection.delete(finding.id);
    renderFindingsPanel();
  });

  const body = document.createElement("div");
  body.className = "finding-body";
  const headline = document.createElement("div");
  headline.className = "finding-headline";
  const severity = document.createElement("span");
  severity.className = `finding-severity finding-severity-${finding.severity}`;
  severity.textContent = finding.severity;
  const summary = document.createElement("span");
  summary.className = "finding-summary";
  summary.textContent = finding.summary;
  headline.append(severity, summary);
  if (finding.code) {
    const code = document.createElement("code");
    code.className = "finding-code";
    code.textContent = finding.code;
    headline.append(code);
  }
  body.append(headline);

  if (finding.range) {
    // The element spec 05's `/api/render-markdown` emits, wired by
    // `wireFileReferences`: no second navigation path (§5.5).
    const { path, startLine, startColumn } = finding.range;
    const location = document.createElement("button");
    location.type = "button";
    location.className = "file-reference finding-location";
    location.dataset.path = path;
    location.dataset.line = String(startLine);
    if (startColumn) location.dataset.col = String(startColumn);
    location.textContent = `${path}:${startLine}${startColumn ? `:${startColumn}` : ""}`;
    location.title = location.textContent;
    body.append(location);
  }

  const note = FINDING_STATUS_NOTES[finding.status];
  if (note) {
    const text = document.createElement("p");
    text.className = "finding-note";
    text.textContent = note;
    body.append(text);
  }

  const footer = document.createElement("div");
  footer.className = "finding-footer";
  let details = null;
  if (finding.details) {
    details = document.createElement("pre");
    details.className = "finding-details";
    details.textContent = finding.details;
    footer.append(createDisclosure("Details", details));
  }
  const action =
    finding.status === "dismissed"
      ? { label: "Restore", status: "open" }
      : finding.status === "fixed"
        ? null
        : { label: "Dismiss", status: "dismissed" };
  if (action) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn-sm btn-quiet";
    button.textContent = action.label;
    button.addEventListener("click", () => setFindingStatus(finding.id, action.status));
    footer.append(button);
  }
  // §7: the expanded content sits above the footer it is paired with.
  if (details) body.append(details);
  if (footer.childElementCount) body.append(footer);

  row.append(select, body);
  return row;
}

async function setFindingStatus(findingId, status) {
  try {
    const payload = await api(
      "/api/finding-status",
      form({ repo: requireRepo(), session_id: currentSessionId, finding_id: findingId, status }),
    );
    sessionFindings = payload.findings || [];
    findingsSelection.delete(findingId);
    renderFindingsPanel();
  } catch (error) {
    toast(error.message);
  }
}

async function fixSelectedFindings() {
  await repairFindings([...findingsSelection]);
}

// One repair turn for exactly `ids`, through spec 22's path: the server builds
// the prompt, and the agentic chat turn runs it. Shared by Fix selected and the
// repo bar's Fix N errors / warnings.
async function repairFindings(ids) {
  if (chatSubmitting || findingsRepairPending) {
    toast("A turn is already running");
    return;
  }
  if (!ids.length) return;
  findingsRepairPending = true;
  renderFindingsPanel();
  try {
    const payload = await api(
      "/api/findings-repair",
      form({ repo: requireRepo(), session_id: currentSessionId, finding_ids: ids.join(",") }),
    );
    const excluded = payload.request?.excluded || [];
    if (excluded.length) {
      toast(`Left out ${excluded.length}: ${excluded.map((entry) => entry.reason).join(", ")}`);
    }
    // Nothing left to repair: the server built no prompt (§15). Redraw, so
    // the panel shows why.
    if (!payload.prompt) {
      findingsRepairPending = false;
      await refreshFindings();
      return;
    }
    // The exact prompt is the thing being authorized. Show it before sending:
    // a generic command finding is often an environment/PATH failure, and
    // "Fix 1 error" must not turn that into a code change the user never saw.
    const kept = payload.request?.findings || [];
    const environmentOnly =
      kept.length > 0 && kept.every((finding) => finding.source === "command" && !finding.range);
    const send = await showAppDialog({
      title: `Fix ${kept.length} finding${kept.length === 1 ? "" : "s"}?`,
      message: environmentOnly
        ? "This is a command that could not run, which is often an environment or PATH problem rather than a code defect. Review the prompt; Cancel if the fix belongs outside the repository."
        : "This sends the prompt below as a new turn. Review it before sending.",
      detail: payload.prompt,
      confirmLabel: "Send",
    });
    if (!send) return;
    findingsSelection.clear();
    // `sendChatPrompt` sets `chatSubmitting` before its first await, which
    // keeps the buttons disabled from here.
    findingsRepairPending = false;
    closeFindingsPopover();
    await sendChatPrompt({ prompt: payload.prompt, restorePrompt: false, agentic: true });
  } catch (error) {
    toast(error.message);
  } finally {
    findingsRepairPending = false;
    renderFindingsPanel();
  }
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
    empty.textContent = "Add a working folder to begin";
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
      <span class="project-chevron" aria-hidden="true"></span>
      <span class="project-name"></span>
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
    menuButton.setAttribute("aria-expanded", "false");
    menuButton.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
    menuButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleProjectMenu(projectPath, menuButton);
    });

    const addSessionButton = document.createElement("button");
    addSessionButton.type = "button";
    addSessionButton.className = "project-session-add-btn";
    addSessionButton.append(icon("compose"));
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
      // Workers come by source repository, never from `/api/sessions`
      // (§5.7 Sidebar), and nothing of them is drawn while the setting is off.
      const workers = workersEnabled ? workerRowsByProject.get(projectPath) : null;
      if (workersEnabled) {
        if (!workers) void loadProjectWorkers(projectPath);
        const workersEntry = renderWorkersEntry(projectPath, workers || []);
        if (workersEntry) sessionsList.append(workersEntry);
      }
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
      } else {
        const entries = sidebarEntries(sessions, workers || []);
        if (!entries.length) {
          const empty = document.createElement("p");
          empty.className = "project-session-empty";
          empty.textContent = "No sessions yet";
          sessionsList.append(empty);
        }
        entries.forEach((entry) => {
          sessionsList.append(
            entry.session
              ? renderProjectSession(projectPath, entry.session)
              : renderProjectWorker(projectPath, entry.row, { nested: entry.nested }),
          );
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
  if (projectPath === repo() && session.id === currentSessionId && !workersBoardProject) {
    row.classList.add("active");
  }
  const button = document.createElement("button");
  button.type = "button";
  button.className = "project-session-open";
  button.textContent = session.title;
  button.title = `${session.title} - double-click to rename`;
  const when = document.createElement("span");
  when.className = "session-when";
  when.textContent = formatRelativeTime(session.updatedAtMs);
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

  const menuButton = document.createElement("button");
  menuButton.type = "button";
  menuButton.className = "project-menu-btn project-session-menu-btn";
  menuButton.title = "More options";
  menuButton.setAttribute("aria-label", `More options for ${session.title}`);
  menuButton.setAttribute("aria-haspopup", "menu");
  menuButton.setAttribute("aria-expanded", "false");
  menuButton.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
  menuButton.addEventListener("click", (event) => {
    event.stopPropagation();
    toggleSessionMenu(projectPath, session, menuButton);
  });

  row.append(button, when, menuButton);
  return row;
}

// "now", minutes, hours, a weekday within the last 6 days, else a short date.
function formatRelativeTime(ms, now = Date.now()) {
  const age = now - ms;
  if (!Number.isFinite(age)) return "";
  if (age < 60_000) return "now";
  if (age < 3_600_000) return `${Math.floor(age / 60_000)}m`;
  const startOfToday = new Date(now).setHours(0, 0, 0, 0);
  if (ms >= startOfToday) return `${Math.floor(age / 3_600_000)}h`;
  if (ms >= startOfToday - 6 * 86_400_000) {
    return new Date(ms).toLocaleDateString("en-US", { weekday: "short" });
  }
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// The session row menu. Rows are re-rendered by `renderProjectList()`, so the
// session id, not the button, keys the menu.
const sessionMenu = createContextMenu();

// Runs a session action and toasts its error, as the old row buttons did.
function runSessionAction(task) {
  return async () => {
    try {
      await task();
    } catch (error) {
      toast(error.message);
    }
  };
}

function toggleSessionMenu(projectPath, session, anchorEl) {
  sessionMenu.toggle(anchorEl, {
    key: `${projectPath}\n${session.id}`,
    sections: [
      [
        {
          label: "Rename",
          action: "rename",
          onSelect: runSessionAction(() => renameSessionForProject(projectPath, session)),
        },
        {
          label: "Export as Markdown",
          action: "export",
          onSelect: runSessionAction(() => exportSessionDownload(session)),
        },
      ],
      [
        {
          label: "Delete",
          action: "delete",
          danger: true,
          onSelect: runSessionAction(() => deleteSessionForProject(projectPath, session)),
        },
      ],
    ],
  });
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
  hideWorkersBoard();
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
  await api("/api/session-delete", form({ session_id: session.id, repo: projectPath }));
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
  renderThreadHeader();
  document.querySelectorAll(".session-item").forEach((button) => {
    button.classList.toggle(
      "active",
      button.dataset.projectPath === repo() &&
        button.dataset.sessionId === currentSessionId &&
        !workersBoardProject,
    );
  });
}

// Downloads a session's Markdown export through the browser's save dialog, which
// is the "path the user chose" the export spec means: the shell writes nowhere
// and makes no network request.
async function exportSessionDownload(session) {
  const path = `/api/session-export?session_id=${encodeURIComponent(session.id)}&format=markdown`;
  await ensureDesktopApiReady();
  const response = await fetch(apiUrl(path), withApiToken(path, {}));
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || response.statusText);
  }
  const filename =
    exportFilenameFromDisposition(response.headers.get("content-disposition")) ||
    `${session.title || session.id}.md`;
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

function exportFilenameFromDisposition(disposition) {
  if (!disposition) return null;
  const match = /filename="?([^";]+)"?/.exec(disposition);
  return match ? match[1] : null;
}

function closeSessionSearch() {
  $("session-search-panel").hidden = true;
  $("session-search-btn").setAttribute("aria-expanded", "false");
}

function closeSymbolSearch() {
  $("symbol-search-panel").hidden = true;
  $("symbol-search-btn").setAttribute("aria-expanded", "false");
}

function toggleSessionSearch() {
  const panel = $("session-search-panel");
  const button = $("session-search-btn");
  const open = panel.hidden;
  closeSymbolSearch();
  panel.hidden = !open;
  button.setAttribute("aria-expanded", String(open));
  if (open) {
    $("session-search-input").value = "";
    $("session-search-results").innerHTML = "";
    $("session-search-input").focus();
  }
}

async function runSessionSearch(query) {
  const results = $("session-search-results");
  if (!query.trim()) {
    results.innerHTML = "";
    return;
  }
  const repoPath = repo();
  if (!repoPath) {
    results.innerHTML = '<p class="project-session-empty">Select a repository first</p>';
    return;
  }
  try {
    const payload = await api(
      `/api/session-search?repo=${encodeURIComponent(repoPath)}&query=${encodeURIComponent(query.trim())}`,
    );
    renderSessionSearchResults(payload);
  } catch (error) {
    results.innerHTML = "";
    toast(error.message);
  }
}

function renderSessionSearchResults(payload) {
  const results = $("session-search-results");
  results.innerHTML = "";
  const hits = payload.hits || [];
  if (!hits.length) {
    const empty = document.createElement("p");
    empty.className = "project-session-empty";
    empty.textContent = "No matches";
    results.append(empty);
    return;
  }
  hits.forEach((hit) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "session-search-hit";
    const title = document.createElement("span");
    title.className = "session-search-hit-title";
    title.textContent = hit.sessionTitle;
    const snippet = document.createElement("span");
    snippet.className = "session-search-hit-snippet";
    snippet.textContent = hit.snippet;
    row.append(title, snippet);
    row.addEventListener("click", () => {
      void openSearchHit(hit);
    });
    results.append(row);
  });
  if (payload.capped) {
    const note = document.createElement("p");
    note.className = "project-session-empty";
    note.textContent = "More results were cut off — narrow the search";
    results.append(note);
  }
}

async function openSearchHit(hit) {
  closeSessionSearch();
  await switchProject(repo(), { preferredSessionId: hit.sessionId, reloadSelected: false });
  await loadSession(hit.sessionId);
  const target = document.querySelector(`.message[data-seq="${hit.seq}"]`);
  if (target) target.scrollIntoView({ behavior: "smooth", block: "start" });
}

// Symbol search (spec 25 Task 10). A result is reference information, so the
// row stays quiet and its path:line is the clickable part. A heuristic symbol
// carries the "likely" marker in the payload's `provenance`; nothing here
// infers it.
function toggleSymbolSearch() {
  const panel = $("symbol-search-panel");
  const button = $("symbol-search-btn");
  const open = panel.hidden;
  closeSessionSearch();
  panel.hidden = !open;
  button.setAttribute("aria-expanded", String(open));
  if (open) {
    $("symbol-search-input").value = "";
    $("symbol-search-results").innerHTML = "";
    setSymbolSearchStatus("");
    $("symbol-search-input").focus();
  }
}

function setSymbolSearchStatus(message) {
  const status = $("symbol-search-status");
  status.textContent = message || "";
  status.hidden = !message;
}

async function runSymbolSearch(query) {
  const results = $("symbol-search-results");
  if (!query.trim()) {
    results.innerHTML = "";
    setSymbolSearchStatus("");
    return;
  }
  const repoPath = repo();
  if (!repoPath) {
    results.innerHTML = "";
    setSymbolSearchStatus("Select a repository first");
    return;
  }
  try {
    const payload = await api(
      `/api/symbols?repo=${encodeURIComponent(repoPath)}&q=${encodeURIComponent(query.trim())}`,
    );
    renderSymbolSearchResults(payload);
  } catch (error) {
    results.innerHTML = "";
    setSymbolSearchStatus(error.message);
  }
}

function symbolKindLabel(kind) {
  return String(kind || "symbol").replace(/_/g, " ");
}

function renderSymbolSearchResults(payload) {
  const results = $("symbol-search-results");
  results.innerHTML = "";
  const matches = payload.matches || [];
  if (!matches.length) {
    setSymbolSearchStatus("No matching symbols");
    return;
  }
  setSymbolSearchStatus(matches.length === 1 ? "1 symbol" : `${matches.length} symbols`);
  matches.forEach((match) => {
    results.append(symbolResultRow(match));
  });
}

function symbolResultRow(match) {
  const symbol = match.symbol || {};
  const row = document.createElement("div");
  row.className = "symbol-search-hit";
  row.dataset.provenance = symbol.provenance || "";

  const header = document.createElement("div");
  header.className = "symbol-search-hit-header";
  const name = document.createElement("strong");
  name.className = "symbol-search-hit-name";
  name.textContent = symbol.name || "";
  const kind = document.createElement("span");
  kind.className = "symbol-kind-chip";
  kind.textContent = symbolKindLabel(symbol.kind);
  header.append(name, kind);
  if (symbol.provenance === "heuristic") {
    const likely = document.createElement("span");
    likely.className = "symbol-provenance";
    likely.textContent = "likely";
    likely.title = "Derived from source text rather than a language server; may be inaccurate.";
    header.append(likely);
  }

  const meta = document.createElement("div");
  meta.className = "symbol-search-hit-meta";
  meta.append(symbolPathButton(symbol));
  if (symbol.rootPath) {
    const root = document.createElement("span");
    root.className = "symbol-root";
    root.textContent = `· ${symbol.rootPath}`;
    meta.append(root);
  }

  const actions = document.createElement("div");
  actions.className = "inline-actions";
  const find = document.createElement("button");
  find.type = "button";
  find.className = "btn-sm symbol-find-references";
  find.textContent = "Find references";
  actions.append(find);

  const references = document.createElement("div");
  references.className = "symbol-references";
  references.hidden = true;
  find.addEventListener("click", () => void loadSymbolReferences(find, references, symbol));

  row.append(header, meta, actions, references);
  return row;
}

function symbolPathButton(symbol) {
  const path = document.createElement("button");
  path.type = "button";
  path.className = "symbol-path";
  path.textContent = `${symbol.path}:${symbol.line}`;
  path.title = `Open ${symbol.path}:${symbol.line} in Visual Studio Code`;
  path.addEventListener("click", async () => {
    try {
      const fields = { repo: requireRepo(), path: symbol.path, line: symbol.line };
      if (symbol.column) fields.col = symbol.column;
      const payload = await api("/api/open-vscode-file", form(fields));
      toast(`Opened ${payload.path}`);
    } catch (error) {
      toast(error.message);
    }
  });
  return path;
}

async function loadSymbolReferences(button, container, symbol) {
  button.disabled = true;
  container.hidden = false;
  container.textContent = "Looking up references…";
  try {
    const payload = await api(
      `/api/symbol-references?repo=${encodeURIComponent(requireRepo())}&path=${encodeURIComponent(
        symbol.path,
      )}&line=${symbol.line}&name=${encodeURIComponent(symbol.name)}`,
    );
    renderSymbolReferences(container, payload);
  } catch (error) {
    container.textContent = error.message;
  } finally {
    button.disabled = false;
  }
}

function renderSymbolReferences(container, payload) {
  container.innerHTML = "";
  if (!payload.available) {
    container.textContent = unavailableReferencesMessage(payload);
    return;
  }
  const relations = payload.relations || [];
  if (!relations.length) {
    container.textContent = "No references found.";
    return;
  }
  relations.forEach((relation) => {
    const target = relation.to || {};
    const row = document.createElement("div");
    row.className = "symbol-reference-row";
    if (target.type === "symbol") {
      row.append(symbolPathButton(target));
      const name = document.createElement("span");
      name.className = "symbol-reference-name";
      name.textContent = target.name || "";
      row.append(name);
    } else if (target.type === "file") {
      const path = document.createElement("code");
      path.textContent = target.path || "";
      row.append(path);
    } else {
      const external = document.createElement("span");
      external.textContent = target.specifier || "unknown target";
      row.append(external);
    }
    container.append(row);
  });
}

function unavailableReferencesMessage(payload) {
  const reason = String(payload.reason || "");
  const language = String(payload.language || "");
  if (/^no language server for /i.test(reason)) {
    return language ? `No language server for ${language}` : "No language server for this language";
  }
  if (reason) return reason;
  return language ? `No language server for ${language}` : "No language server is configured";
}

// Settings › Language servers. Every configured language is listed, whether a
// lookup has started it or not; a hub entry replaces the idle row once it
// exists, so an unavailable or crashed server keeps its reason.
const languageServerStateLabels = {
  idle: "not started",
  starting: "starting",
  ready: "running",
  unavailable: "unavailable",
  crashed: "crashed",
};

async function refreshLanguageServers(repoPath = repo()) {
  const list = $("language-server-list");
  const status = $("language-servers-status");
  list.innerHTML = "";
  if (!repoPath) {
    status.textContent = "No repository selected.";
    return;
  }
  try {
    const payload = await api(`/api/language-servers?repo=${encodeURIComponent(repoPath)}`);
    renderLanguageServers(payload);
  } catch (error) {
    status.textContent = error.message;
  }
}

function renderLanguageServers(payload) {
  const list = $("language-server-list");
  const status = $("language-servers-status");
  list.innerHTML = "";
  const servers = payload.servers || [];
  if (!servers.length) {
    status.textContent =
      "No language server is configured. Symbol results are heuristic and marked likely.";
    return;
  }
  status.textContent = servers.length === 1 ? "1 configured" : `${servers.length} configured`;
  servers.forEach((server) => {
    list.append(languageServerRow(server));
  });
}

function languageServerRow(server) {
  const state = server.state || "idle";
  const row = document.createElement("div");
  row.className = "language-server-row";
  row.dataset.state = state;

  const heading = document.createElement("div");
  heading.className = "language-server-heading";
  const language = document.createElement("strong");
  language.className = "language-server-language";
  language.textContent = server.language || "";
  const label = document.createElement("span");
  label.className = "language-server-state";
  label.textContent = languageServerStateLabels[state] || state;
  if (state === "ready" && server.pid) label.textContent = `running — pid ${server.pid}`;
  heading.append(language, label);

  const command = document.createElement("code");
  command.className = "language-server-command";
  command.textContent = server.command || "";

  row.append(heading, command);
  const reason = languageServerReason(server);
  if (reason) {
    const detail = document.createElement("p");
    detail.className = "language-server-reason";
    detail.textContent = reason;
    row.append(detail);
  }
  return row;
}

function languageServerReason(server) {
  if (server.state === "unavailable") return server.reason || "The server could not start.";
  if (server.state === "crashed") {
    return Number(server.restarts) > 0
      ? "Crashed again after its one restart; it stays down until this repository's server entry expires."
      : "Crashed; the next symbol lookup will restart it once.";
  }
  return "";
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

// After a patch apply or rollback. The same answer feeds the repo bar, which
// replaced the "N changed" suffix this used to write into the sidebar's
// `#repo-state` (spec 58 §3.3); that line now always names the folder alone.
async function appendGitStatusAfterChange(repoPath) {
  const payload = await api(`/api/git-status?repo=${encodeURIComponent(repoPath)}`);
  // The commit candidate count is a session question, not a git-status one;
  // ask it again rather than showing the previous count beside a new status.
  let commitStatus = null;
  if (currentSessionId) {
    try {
      commitStatus = await api(
        `/api/commit-status?repo=${encodeURIComponent(repoPath)}&session=${encodeURIComponent(currentSessionId)}`,
      );
    } catch {
      // The bar still works; the action hides rather than claiming a count.
    }
  }
  applyRepoBarStatus(repoPath, payload, commitStatus);
  appendChatMessage("system", renderGitStatusText(payload));
}

// Spec 50's question card. It deliberately shares no class with
// `.command-approval`: no header, no risk badge, no footer and no primary
// action. Every option is `.btn-sm` at the same weight because no answer is
// the safe one, and nothing is focused or pre-selected.
//
// `answered` is the recorded answer when the card is rebuilt from the
// transcript or a recovered turn; `null` means it is still open.
function createQuestionCard(question, questionRepo, { answered = null } = {}) {
  const wrapper = document.createElement("section");
  wrapper.className = "question-card";
  wrapper.dataset.questionId = question.questionId || "";
  wrapper.dataset.answered = answered === null ? "false" : "true";

  const eyebrow = document.createElement("p");
  eyebrow.className = "question-card-eyebrow";
  eyebrow.textContent = "Question";

  const text = document.createElement("p");
  text.className = "question-card-question";
  text.textContent = question.question || "";

  const options = document.createElement("div");
  options.className = "question-card-options";

  const answer = document.createElement("p");
  answer.className = "question-card-answer";
  answer.hidden = answered === null;
  answer.textContent = answered === null ? "" : `You answered: ${answered}`;

  const status = document.createElement("p");
  status.className = "question-card-status";
  status.hidden = true;

  const controls = [];
  const setDisabled = (disabled) => {
    controls.forEach((control) => {
      control.disabled = disabled;
    });
  };
  const settle = (value) => {
    wrapper.dataset.answered = "true";
    answer.hidden = false;
    answer.textContent = `You answered: ${value}`;
    status.hidden = true;
    status.textContent = "";
    setDisabled(true);
  };

  wrapper.append(eyebrow, text);

  if (Array.isArray(question.options)) {
    question.options.forEach((label, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn-sm";
      button.textContent = label;
      button.addEventListener("click", () => void submit(label, index));
      controls.push(button);
      options.append(button);
    });
  }
  wrapper.append(options);

  if (question.allowFreeText) {
    const row = document.createElement("div");
    row.className = "question-card-free-text";
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = "Type an answer…";
    input.setAttribute("aria-label", "Your answer");
    const send = document.createElement("button");
    send.type = "button";
    send.className = "btn-sm";
    send.textContent = "Send";
    send.addEventListener("click", () => {
      const value = input.value.trim();
      if (!value) {
        input.focus();
        return;
      }
      void submit(value, null);
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        send.click();
      }
    });
    controls.push(input, send);
    row.append(input, send);
    wrapper.append(row);
  }

  wrapper.append(answer, status);
  if (answered !== null) setDisabled(true);

  async function submit(value, selectedOption) {
    if (chatSubmitting) {
      toast("A turn is already running");
      return;
    }
    setDisabled(true);
    status.hidden = false;
    status.textContent = "Answering…";
    // The engine records the answer before it resumes, so show it the way a
    // reload will: as a user-side reply after the question.
    appendChatMessage("answer", value);
    const assistantMessage = appendChatMessage("assistant", "");
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    chatSubmitting = true;
    setComposerBusy(true);
    try {
      await streamResumeAnswerRequest(
        {
          repo: questionRepo,
          question_id: question.questionId,
          answer: value,
          selected_option: selectedOption === null ? "" : String(selectedOption),
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
          },
          done(payload) {
            if (payload.response && payload.response !== assistantText) {
              assistantText = payload.response;
              updateChatMessage(assistantMessage, assistantText);
            }
            appendProposals(assistantMessage, payload, questionRepo);
            if (payload.sessionId) {
              currentSessionId = payload.sessionId;
              localStorage.setItem(lastSessionStorageKey(), currentSessionId);
            }
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = new Error(payload.error || "Answer failed");
          },
        },
      );
    } catch (error) {
      streamError = error;
    } finally {
      indicator.finish("failed");
      chatSubmitting = false;
      setComposerBusy(false);
    }
    if (streamError) {
      // The card stays open: the engine puts the pause back on a refusal, so a
      // second attempt is still the same question rather than a new one.
      renderChatMessageError(assistantMessage, streamError);
      status.textContent = streamError.message;
      setDisabled(false);
      return;
    }
    settle(value);
    await loadSessions(currentSessionId, false);
    await refreshFindings();
    void refreshRepoBar();
  }

  return wrapper;
}

// Spec 51 §5.3: the first fetch of a host asks before anything leaves the
// machine. The host is the headline and the URL is shown exactly as the model
// wrote it — wrapped, never shortened — because a user approving a request has
// to be able to read all of it. An over-long query or fragment is broken out
// and marked, since a long opaque parameter is the shape exfiltration takes and
// nobody spots it inside a long line.
//
// "Always allow" persists beyond this turn, so it goes in the overflow rather
// than beside the one-shot action (UI_STYLE_GUIDE §4).
function createFetchApproval(proposal, proposalRepo) {
  const wrapper = document.createElement("section");
  wrapper.className = "fetch-approval";
  wrapper.dataset.fetchProposalId = proposal.proposalId || "";

  const eyebrow = document.createElement("p");
  eyebrow.className = "fetch-approval-eyebrow";
  eyebrow.textContent = "Read an external page";

  const host = document.createElement("p");
  host.className = "fetch-approval-host";
  host.textContent = proposal.host || "";

  const lead = document.createElement("p");
  lead.className = "fetch-approval-lead";
  lead.textContent =
    "The agent wants to request this address. Nothing has been sent, and the request " +
    "carries no credentials or cookies.";

  const url = document.createElement("code");
  url.className = "fetch-approval-url";
  url.textContent = proposal.url || "";

  wrapper.append(eyebrow, host, lead, url);

  const flags = Array.isArray(proposal.longComponents) ? proposal.longComponents : [];
  for (const flag of flags) {
    const component = flag.component === "fragment" ? "fragment" : "query";
    const value = component === "fragment" ? proposal.fragment : proposal.query;
    const box = document.createElement("div");
    box.className = "fetch-approval-flag";
    const label = document.createElement("p");
    label.className = "fetch-approval-flag-label";
    label.textContent = `Long ${component} — ${flag.bytes} bytes. Read it before allowing this.`;
    const code = document.createElement("code");
    code.className = "fetch-approval-flag-value";
    code.textContent = value || "";
    box.append(label, code);
    wrapper.append(box);
  }

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const once = document.createElement("button");
  once.type = "button";
  once.className = "btn-sm btn-primary";
  once.textContent = "Allow once";
  const deny = document.createElement("button");
  deny.type = "button";
  deny.className = "btn-sm btn-quiet";
  deny.textContent = "Don't fetch";
  actions.append(once, deny);

  let overflowButton = null;
  if (proposal.approvalKey) {
    overflowButton = document.createElement("button");
    overflowButton.type = "button";
    overflowButton.className = "btn-icon";
    overflowButton.dataset.menuId = `fetch-approval-${proposal.proposalId}`;
    overflowButton.setAttribute("aria-haspopup", "menu");
    overflowButton.setAttribute("aria-expanded", "false");
    overflowButton.setAttribute("aria-label", "More fetch options");
    overflowButton.title = "More fetch options";
    overflowButton.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
    overflowButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleApprovalMenu(
        [
          {
            label: "Always allow this host in this project",
            hint: "Writes web_domain_allowlist to user config — persists after this turn",
            onSelect: () => void decide("always", "Allowed for this project"),
          },
        ],
        overflowButton,
      );
    });
    actions.append(overflowButton);
  }

  const status = document.createElement("p");
  status.className = "fetch-approval-status";
  status.hidden = true;

  wrapper.append(actions, status);

  const controls = [once, deny, ...(overflowButton ? [overflowButton] : [])];
  const setDisabled = (disabled) => {
    controls.forEach((control) => {
      control.disabled = disabled;
    });
  };

  once.addEventListener("click", () => void decide("once", "Allowed once"));
  deny.addEventListener("click", () => void decide("deny", "Not fetched"));

  async function decide(decision, settledText) {
    if (chatSubmitting) {
      toast("A turn is already running");
      return;
    }
    setDisabled(true);
    closeApprovalMenu();
    status.hidden = false;
    status.textContent = decision === "deny" ? "Refusing…" : "Reading…";

    const assistantMessage = appendChatMessage("assistant", "");
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    chatSubmitting = true;
    setComposerBusy(true);
    try {
      await streamResumeFetchRequest(
        {
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          decision,
          session_id: currentSessionId || "",
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
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
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = new Error(payload.error || "The fetch decision failed");
          },
        },
      );
    } catch (error) {
      streamError = error;
    } finally {
      indicator.finish("failed");
      chatSubmitting = false;
      setComposerBusy(false);
    }
    if (streamError) {
      // The engine puts the pause back on a refusal, so a second attempt is
      // still the same decision rather than a new one.
      renderChatMessageError(assistantMessage, streamError);
      status.textContent = streamError.message;
      setDisabled(false);
      return;
    }
    status.textContent = settledText;
    await loadSessions(currentSessionId, false);
    await refreshFindings();
    void refreshRepoBar();
  }

  return wrapper;
}

// One verification continuation, streamed into a fresh assistant bubble. Used
// by the patch card's settle trigger, the recovery offer, and the check card,
// so all three render the report and any repair patch the same way.
async function runVerificationStream(streamCall, targetRepo) {
  const { message, body } = appendChatMessage("assistant", "");
  const target = { message, body };
  const indicator = startTurnIndicator(target);
  let text = "";
  let streamError = null;
  try {
    await streamCall({
      phase(payload) {
        indicator.phase(payload);
      },
      plan(planPayload) {
        renderPlanPanel(target, planPayload);
      },
      token(token) {
        text += token;
        updateChatMessage(target, text);
        indicator.streaming();
      },
      done(payload) {
        if (payload.response && payload.response !== text) {
          text = payload.response;
          updateChatMessage(target, text);
        }
        appendProposals(target, payload, targetRepo);
        appendContextDisclosure(target.body, payload.contextFiles || []);
        markMessageUsage(target, payload.usage, liveTurnDurationMs(payload, indicator));
        markMessageTurnStop(target, payload);
        indicator.finish(chatCompletionState(payload));
      },
      error(payload) {
        streamError = new Error(payload.error || "Verification failed");
      },
    });
  } catch (error) {
    streamError = error;
  } finally {
    indicator.finish("failed");
  }
  if (streamError) {
    renderChatMessageError(target, streamError);
    toast(streamError.message);
  }
  await loadSessions(currentSessionId, false);
  await refreshFindings();
}

// A settled patch makes verification due in Code mode only. The engine is
// asked rather than guessed at: nothing due is a no-op, and an Ask/Plan/Review
// session has nothing applied to verify.
async function startVerification(repoName) {
  if (!currentSessionId) return;
  if ((currentSessionMode?.mode || "code") !== "code") return;
  await runVerificationStream(
    (handlers) => streamVerifyRequest({ repo: repoName, session_id: currentSessionId }, handlers),
    repoName,
  );
}

function checkLabel(check) {
  return `${check.command} (${check.workingDirectory || "repository root"})`;
}

// One card for every check that needs approval (spec 23 §5.3). Unchecked
// checks are declined and reported as skipped; the batch still resolves, so
// declining everything is a valid answer rather than a stall.
function createCheckBatchCard(batch, batchRepo, { reattached = false, onSubmitted = null } = {}) {
  const card = document.createElement("section");
  card.className = "check-batch";

  const header = document.createElement("div");
  header.className = "check-batch-header";
  const title = document.createElement("strong");
  title.textContent = "Checks need approval";
  const count = document.createElement("span");
  count.className = "check-batch-count";
  count.textContent = `${batch.checks.length} check${batch.checks.length === 1 ? "" : "s"}`;
  header.append(title, count);

  const note = document.createElement("p");
  note.className = "check-batch-note";
  note.textContent =
    "Verification is waiting for these checks. Unchecked checks are skipped and reported as " +
    "declined; the report follows once this card is submitted.";

  const list = document.createElement("div");
  list.className = "check-batch-list";
  const rows = batch.checks.map((check) => {
    const row = document.createElement("div");
    row.className = "check-batch-item";

    const run = document.createElement("input");
    run.type = "checkbox";
    run.className = "check-batch-run";
    run.checked = reattached;
    run.setAttribute("aria-label", `Run ${check.command}`);

    const command = document.createElement("code");
    command.className = "check-batch-command";
    command.textContent = check.command;

    const risk = document.createElement("span");
    risk.className = "check-batch-risk";
    risk.dataset.blocked = check.blocked ? "true" : "false";
    risk.textContent = check.blocked ? "blocked" : check.risk || "review";

    const cause = document.createElement("span");
    cause.className = "check-batch-cause";
    cause.textContent = check.cause || "";

    row.append(run, command, risk, cause);

    let always = null;
    if (check.allowAlways) {
      const grant = document.createElement("label");
      grant.className = "check-batch-allow";
      always = document.createElement("input");
      always.type = "checkbox";
      always.disabled = true;
      run.addEventListener("change", () => {
        always.disabled = !run.checked;
      });
      grant.append(always, document.createTextNode(" Always allow"));
      row.append(grant);
    }
    list.append(row);
    return { check, run, always };
  });

  const actions = document.createElement("div");
  actions.className = "check-batch-actions";
  const submit = document.createElement("button");
  submit.type = "button";
  submit.className = "btn-sm btn-primary";
  submit.textContent = "Submit checks";
  actions.append(submit);

  submit.addEventListener("click", async () => {
    submit.disabled = true;
    const decisions = rows.map(({ check, run }) => ({
      proposalId: check.proposalId,
      approved: run.checked,
    }));
    const always = rows
      .filter(({ run, always }) => run.checked && always?.checked)
      .map(({ check }) => check.proposalId)
      .join("\n");
    card.classList.add("submitted");
    try {
      if (onSubmitted) onSubmitted();
      await runVerificationStream(
        (handlers) =>
          streamResumeChecksRequest(
            {
              repo: batchRepo,
              proposal_id: batch.proposalId,
              decisions: JSON.stringify(decisions),
              always,
            },
            handlers,
          ),
        batchRepo,
      );
    } catch (error) {
      submit.disabled = false;
      card.classList.remove("submitted");
      toast(error.message);
    }
  });

  card.append(header, note, list, actions);
  return card;
}

// The structured half of proposal §5.7. The text copy is the assistant
// message; this card is what makes the sections scannable and the changed
// paths clickable.
function createVerificationReportCard(report, reportRepo) {
  const card = document.createElement("section");
  card.className = "verification-report";

  const header = document.createElement("div");
  header.className = "verification-report-header";
  const title = document.createElement("strong");
  title.textContent = "Verification report";
  const outcome = document.createElement("span");
  outcome.className = "verification-report-outcome";
  outcome.dataset.outcome = report.due ? "due" : report.outcome || "running";
  outcome.textContent = report.due ? "due" : (report.outcome || "").replaceAll("_", " ");
  header.append(title, outcome);
  card.append(header);

  const section = (label, items) => {
    const row = document.createElement("div");
    row.className = "verification-report-row";
    const name = document.createElement("span");
    name.className = "verification-report-label";
    name.textContent = label;
    const value = document.createElement("div");
    value.className = "verification-report-value";
    if (!items.length) {
      value.textContent = "none";
    } else {
      items.forEach((item, index) => {
        if (index) value.append(document.createTextNode(", "));
        value.append(item);
      });
    }
    row.append(name, value);
    card.append(row);
    return value;
  };

  const files = (report.filesChanged || []).map((path) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "file-reference";
    button.dataset.path = path;
    button.textContent = path;
    return button;
  });
  section("Files changed", files);
  section(
    "Checks passed",
    (report.passed || []).map((check) => document.createTextNode(checkLabel(check))),
  );
  section(
    "Checks failed",
    (report.failed || []).map((item) =>
      document.createTextNode(`${checkLabel(item.check)} — ${item.reason}`),
    ),
  );
  section(
    "Checks skipped",
    (report.skipped || []).map((item) => {
      const reason = item.reason || {};
      const detail =
        reason.reason === "declined_by_user"
          ? "declined by you"
          : reason.reason === "no_running_target"
            ? "no running target"
            : reason.policyReason || "blocked";
      return document.createTextNode(`${checkLabel(item.check)} — ${detail}`);
    }),
  );
  const stale = section(
    "Checks stale",
    (report.stale || []).map((check) => {
      const badge = document.createElement("span");
      badge.className = "verification-stale";
      badge.textContent = `${checkLabel(check)} — stale`;
      return badge;
    }),
  );
  if (report.stale?.length) stale.classList.add("has-stale");
  section(
    "Behaviour verified",
    (report.behaviourVerified || []).map((item) => document.createTextNode(item)),
  );
  section(
    "Unverified steps",
    (report.unverifiedSteps || []).map((step) => document.createTextNode(`"${step}"`)),
  );
  section(
    "Regressions",
    (report.regressions || []).map((check) => document.createTextNode(checkLabel(check))),
  );
  section("Outcome", [
    document.createTextNode(
      report.due ? "verification is due" : (report.outcome || "not finished").replaceAll("_", " "),
    ),
  ]);

  wireFileReferences(card, reportRepo);
  return card;
}

// A due verification from the recovery sweep. Nothing has run: the button is
// the only thing that starts it, matching spec 17's rule for commands.
function createVerificationOfferCard(due) {
  const wrapper = document.createElement("section");
  wrapper.className = "recovery-card verification-offer";
  wrapper.dataset.taskId = due.taskId;

  const eyebrow = document.createElement("p");
  eyebrow.className = "recovery-card-eyebrow";
  eyebrow.textContent = "Still waiting for you";
  const headline = document.createElement("p");
  headline.className = "recovery-card-headline";
  headline.textContent = "A verification was due when Damaian stopped";
  const note = document.createElement("p");
  note.className = "recovery-card-note";
  note.textContent =
    "The patch was applied and the checks have not run. Verifying now runs the checks the " +
    "project detects; nothing has been assumed to pass.";

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const verify = document.createElement("button");
  verify.type = "button";
  verify.className = "btn-sm btn-primary";
  verify.textContent = "Verify now";
  actions.append(verify);
  verify.addEventListener("click", async () => {
    verify.disabled = true;
    pendingVerifications = pendingVerifications.filter((entry) => entry !== due);
    wrapper.remove();
    await startVerification(repo());
  });

  wrapper.append(eyebrow, headline, note, actions);
  return wrapper;
}

// Appends whatever proposals a finished chat turn carries. Shared by every
// `done` handler: a turn can end with a command awaiting approval, a patch
// awaiting review, or neither, and each handler must treat all of them the
// same. Keeping this in one place is deliberate — the resume-after-approval
// handler previously rendered only `commandProposal`, so a patch proposed
// after the user approved a command was silently dropped and could never be
// reviewed.
function appendProposals(message, payload, repo) {
  noteContextPayload(payload);
  if (payload.commandProposal) {
    message.body.append(createCommandApprovalPreview(payload.commandProposal, repo));
  }
  if (payload.patchProposal) {
    message.body.append(
      createPatchPreview(payload.patchProposal, repo, {
        // A patch is verified once every file of it is settled. The engine
        // decides whether anything is due, so an unrelated or all-rejected
        // patch is a no-op rather than a second run.
        onResolved: () => startVerification(repo),
      }),
    );
  }
  if (payload.checkBatch) {
    message.body.append(createCheckBatchCard(payload.checkBatch, repo));
  }
  if (payload.verificationReport) {
    message.body.append(createVerificationReportCard(payload.verificationReport, repo));
  }
  if (payload.planProposal) {
    message.body.append(createPlanReview(payload.planProposal, repo));
  }
  if (payload.fallbackProposal) {
    message.body.append(createFallbackOffer(payload.fallbackProposal, repo));
  }
  if (payload.roundBudgetProposal) {
    message.body.append(createRoundBudgetOffer(payload.roundBudgetProposal, repo));
  }
  if (payload.questionProposal) {
    message.body.append(createQuestionCard(payload.questionProposal, repo));
  }
  if (payload.fetchProposal) {
    message.body.append(createFetchApproval(payload.fetchProposal, repo));
  }
  if (payload.contextReview) {
    message.body.append(createContextReview(payload.contextReview, repo));
  }
}

// What a refusal code means, for the fallback offer's heading. An unknown code
// is shown as it stands rather than hidden.
const REFUSAL_LABELS = {
  provider_rate_limited: "rate limited",
  provider_overloaded: "overloaded",
  provider_quota_exhausted: "out of quota",
};

function providerWithModel(provider, model) {
  return model ? `${provider} (${model})` : provider;
}

// Spec 56: a provider refused this turn and another configured provider could
// answer it. The offer is one-shot and refused by default, so "Keep it failed"
// is the primary, focused action, and there is deliberately no "always": the
// next turn goes back to the configured provider whatever is chosen here.
//
// A reattached offer (after a restart) knows only the fallback's name, so the
// refusal fields are optional.
function createFallbackOffer(proposal, proposalRepo) {
  const fallback = providerWithModel(proposal.fallbackProvider, proposal.fallbackModel);

  const wrapper = document.createElement("div");
  wrapper.className = "command-approval fallback-offer";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = `Answer this turn with ${proposal.fallbackProvider}?`;
  header.append(title);
  if (proposal.refusalCode) {
    const meta = document.createElement("span");
    meta.className = "command-approval-risk";
    meta.dataset.blocked = "true";
    meta.textContent = REFUSAL_LABELS[proposal.refusalCode] || proposal.refusalCode;
    header.append(meta);
  }

  const note = document.createElement("p");
  note.className = "plan-review-note";
  // Says nothing about the refusal itself: a live offer sits under the
  // engine's account of it, and a reattached one under the recovery card's.
  note.textContent =
    `Answering sends the same request to ${fallback}, for this turn only. Your next turn ` +
    "uses the configured provider again.";

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const keepButton = document.createElement("button");
  keepButton.type = "button";
  keepButton.className = "btn-sm btn-primary";
  keepButton.textContent = "Keep it failed";
  const switchButton = document.createElement("button");
  switchButton.type = "button";
  switchButton.className = "btn-sm btn-quiet";
  switchButton.textContent = `Answer with ${proposal.fallbackProvider}`;
  actions.append(keepButton, switchButton);

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  async function decide(approved) {
    keepButton.disabled = true;
    switchButton.disabled = true;
    output.hidden = false;
    output.textContent = approved ? `Asking ${proposal.fallbackProvider}…` : "Keeping it failed…";

    // The engine writes the marker into the transcript, which is what a reload
    // renders; this shows it now, above the answer it introduces. A reattached
    // offer does not know who refused, so its version is shorter.
    if (approved) {
      const after = proposal.refusedProvider
        ? ` after ${proposal.refusedProvider} refused it (${proposal.refusalCode})`
        : "";
      appendChatMessage("system", `Switched to ${fallback} for this turn${after}.`);
    }
    const assistantMessage = approved ? appendChatMessage("assistant", "") : null;
    // No Stop: this stream is sent without an abort signal, so nothing on this
    // path can cancel it.
    const indicator = assistantMessage ? startTurnIndicator(assistantMessage) : null;
    let assistantText = "";
    let streamError = null;
    try {
      await streamResumeFallbackRequest(
        {
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          approved: approved ? "true" : "false",
        },
        {
          phase(payload) {
            if (indicator) indicator.phase(payload);
          },
          plan(planPayload) {
            if (assistantMessage) renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            if (assistantMessage) renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            if (!assistantMessage) return;
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            if (indicator) indicator.streaming();
          },
          done(payload) {
            if (!assistantMessage) return;
            if (payload.response && payload.response !== assistantText) {
              assistantText = payload.response;
              updateChatMessage(assistantMessage, assistantText);
            }
            appendProposals(assistantMessage, payload, proposalRepo);
            if (payload.sessionId) {
              currentSessionId = payload.sessionId;
              localStorage.setItem(lastSessionStorageKey(), currentSessionId);
            }
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            if (indicator) indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = payload.error || "The provider decision failed";
          },
        },
      );
    } catch (error) {
      // The request itself failed, not the provider: record it in the bubble
      // and let the caller's toast say it too.
      renderChatMessageError(assistantMessage, error);
      throw error;
    } finally {
      if (indicator) indicator.finish("failed");
    }
    if (approved) {
      // The fallback refusing too is an ordinary failure, not a second offer.
      // The error text in the bubble is that failure's mark.
      if (streamError) updateChatMessage(assistantMessage, streamError);
      output.textContent = streamError
        ? `${proposal.fallbackProvider} could not answer either. The turn has failed.`
        : `Answered by ${fallback} — see the answer below.`;
    } else {
      // A decline fails the task exactly as the refusal would have, and the
      // engine reports it as that refusal: an error here is the expected outcome.
      // This card's own line is the only record a decline needs.
      output.textContent = streamError ? `Kept failed: ${streamError}` : "Kept failed.";
    }
    await loadSessions(currentSessionId, false);
    void refreshRepoBar();
  }

  keepButton.addEventListener("click", () => decide(false).catch((error) => toast(error.message)));
  switchButton.addEventListener("click", () => decide(true).catch((error) => toast(error.message)));

  wrapper.append(header, note, actions, output);
  // Focus once the caller has put the card in the document: the default
  // answer is the one a stray Enter should give.
  requestAnimationFrame(() => {
    if (wrapper.isConnected && !keepButton.disabled) keepButton.focus();
  });
  return wrapper;
}

// Spec 61: a turn paused at its round budget. The transcript already names the
// counts ("Used 8 of 8 action rounds and 12 read-only rounds." is the pause
// response); the two buttons here carry the decision, and neither says
// anything about debugging (requirement 3).
//
// Two shapes: a live pause carries the full proposal (roundsUsed, roundsTotal,
// roundsGranted, roundsMore, atHardCap), while a reattached pause (after a
// restart) carries only roundsUsed and roundsGranted — all `PausedTurns`
// exposes. For the reattached shape the hard cap is derived from roundsUsed,
// duplicating `ABSOLUTE_TURN_ROUND_CAP` (64) from chat.rs, so a reattached
// pause at the cap is never offered a Continue the engine would refuse.

// Spec 27 §5.1: a turn assembled and not sent. The card is the decision; the
// panel is the detail. Removal happens in the panel, so Send reads the amended
// manifest the panel last received.
function createContextReview(review, reviewRepo) {
  const wrapper = document.createElement("div");
  wrapper.className = "command-approval context-review";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = "Review the context before sending";
  const meta = document.createElement("span");
  meta.className = "command-approval-risk";
  meta.dataset.blocked = "false";
  header.append(title, meta);

  const note = document.createElement("p");
  note.className = "plan-review-note";

  // The card owns the live review: Send reads the manifest the panel last
  // showed, so this is the same object the panel's removals amend.
  const card = {
    review,
    refresh: () => renderReviewState(),
  };
  contextReviewCards.set(review.proposalId, card);
  renderReviewState();

  function renderReviewState() {
    const manifest = card.review.manifest || {};
    meta.textContent = `${formatContextTokens(manifest.totalTokens || 0)} / ${formatContextTokens(
      manifest.limit?.tokens || 0,
    )} tokens`;
    const excluded = manifest.excluded?.length || 0;
    note.textContent = `Nothing has been sent yet. ${excluded} item${
      excluded === 1 ? "" : "s"
    } were left out; open the panel to see why or to pin something for later turns.`;
  }

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";

  const reviewButton = document.createElement("button");
  reviewButton.type = "button";
  reviewButton.className = "btn-sm btn-quiet";
  reviewButton.textContent = "Review context";
  reviewButton.addEventListener("click", () => {
    currentContextReview = card.review;
    openContextPanel(card.review.manifest, `review:${card.review.proposalId}`);
  });

  const sendButton = document.createElement("button");
  sendButton.type = "button";
  sendButton.className = "btn-sm btn-primary";
  sendButton.textContent = "Send";
  const declineButton = document.createElement("button");
  declineButton.type = "button";
  declineButton.className = "btn-sm btn-quiet";
  declineButton.textContent = "Decline";
  actions.append(reviewButton, sendButton, declineButton);

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  async function decide(decision) {
    reviewButton.disabled = true;
    sendButton.disabled = true;
    declineButton.disabled = true;
    output.hidden = false;
    output.textContent = decision === "send" ? "Sending…" : "Declining…";
    const assistantMessage = appendChatMessage("assistant", "");
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    try {
      await streamResumeContextRequest(
        {
          repo: reviewRepo,
          proposal_id: review.proposalId,
          decision,
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
          },
          done(payload) {
            if (payload.response && payload.response !== assistantText) {
              assistantText = payload.response;
              updateChatMessage(assistantMessage, assistantText);
            }
            appendProposals(assistantMessage, payload, reviewRepo);
            if (payload.sessionId) {
              currentSessionId = payload.sessionId;
              localStorage.setItem(lastSessionStorageKey(), currentSessionId);
            }
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = new Error(payload.error || "Context decision failed");
          },
        },
      );
    } catch (error) {
      streamError = error;
    } finally {
      indicator.finish("failed");
    }
    if (streamError) {
      renderChatMessageError(assistantMessage, streamError);
      // The review is put back by the engine on a refusal, so the card stays
      // answerable.
      reviewButton.disabled = false;
      sendButton.disabled = false;
      declineButton.disabled = false;
      output.textContent = streamError.message;
      return;
    }
    currentContextReview = null;
    contextReviewCards.delete(review.proposalId);
    closeContextPanel();
    output.textContent =
      decision === "send" ? "Sent the reviewed context." : "No request was sent.";
    await loadSessions(currentSessionId, false);
    await refreshFindings();
  }

  sendButton.addEventListener("click", () => {
    decide("send").catch((error) => toast(error.message));
  });
  declineButton.addEventListener("click", () => {
    decide("decline").catch((error) => toast(error.message));
  });

  wrapper.append(header, note, actions, output);
  return wrapper;
}

function createRoundBudgetOffer(proposal, proposalRepo) {
  const more = proposal.roundsMore;
  const atHardCap = proposal.atHardCap === true || (proposal.roundsUsed ?? 0) >= 64;

  const wrapper = document.createElement("div");
  wrapper.className = "command-approval round-budget-offer";

  const note = document.createElement("p");
  note.className = "plan-review-note";
  note.textContent = atHardCap
    ? "This turn has reached its hard cap: it can be answered now, but not continued."
    : "Continue grants this same turn another segment of tool rounds; Answer now makes the final call with tools dropped.";

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";

  const answerButton = document.createElement("button");
  answerButton.type = "button";
  answerButton.className = "btn-sm btn-primary";
  answerButton.textContent = "Answer now";
  actions.append(answerButton);

  const continueButton = document.createElement("button");
  if (!atHardCap) {
    continueButton.type = "button";
    continueButton.className = "btn-sm btn-quiet";
    continueButton.textContent = typeof more === "number" ? `Continue — ${more} more` : "Continue";
    actions.append(continueButton);
  }

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  async function decide(decision) {
    answerButton.disabled = true;
    if (!atHardCap) continueButton.disabled = true;
    output.hidden = false;
    output.textContent = decision === "continue" ? "Continuing…" : "Answering now…";

    const assistantMessage = appendChatMessage("assistant", "");
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    try {
      await streamResumeRoundBudgetRequest(
        {
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          decision,
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
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
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = payload.error || "The round-budget decision failed";
          },
        },
      );
    } catch (error) {
      renderChatMessageError(assistantMessage, error);
      throw error;
    } finally {
      indicator.finish("failed");
    }
    if (streamError) {
      updateChatMessage(assistantMessage, streamError);
      output.textContent = streamError;
      answerButton.disabled = false;
      if (!atHardCap) continueButton.disabled = false;
    } else {
      output.textContent = decision === "continue" ? "Continued." : "Answered.";
    }
    await loadSessions(currentSessionId, false);
    void refreshRepoBar();
  }

  answerButton.addEventListener("click", () =>
    decide("answer_now").catch((error) => toast(error.message)),
  );
  if (!atHardCap) {
    continueButton.addEventListener("click", () =>
      decide("continue").catch((error) => toast(error.message)),
    );
  }

  wrapper.append(note, actions, output);
  requestAnimationFrame(() => {
    if (wrapper.isConnected && !answerButton.disabled) answerButton.focus();
  });
  return wrapper;
}

// The plan review gate, per spec 21 §5.5: the turn has stopped before its
// first mutating step and the user may reorder, retitle, delete or approve.
//
// Steps that already reached an outcome are shown but not editable — their
// evidence is tied to a state of the repository, and the engine refuses to
// delete one, so offering the control would be offering something that does
// not work.
//
// `decide` is for a worker's plan (spec 57 §5.6), as on the command card. The
// worker's `approve` takes no revision, so its steps are shown, not edited.
function createPlanReview(proposal, proposalRepo, { decide: decideInWorker = null } = {}) {
  const plan = proposal.plan || { steps: [] };
  const editable = decideInWorker
    ? []
    : (plan.steps || []).filter((step) => step.outcome === "outstanding");
  const settled = decideInWorker
    ? plan.steps || []
    : (plan.steps || []).filter((step) => step.outcome !== "outstanding");
  let steps = editable.map((step) => ({ id: step.id, title: step.title }));

  const wrapper = document.createElement("div");
  wrapper.className = "command-approval";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = "Review the plan";
  const meta = document.createElement("span");
  meta.className = "command-approval-risk";
  meta.textContent = proposal.deferredAction || "review";
  meta.dataset.blocked = "false";
  header.append(title, meta);

  const note = document.createElement("p");
  note.className = "plan-review-note";
  note.textContent = decideInWorker
    ? "Nothing has changed yet. Approve the steps as they stand, or decline them."
    : settled.length
      ? `Nothing has changed yet. ${settled.length === 1 ? "One step has" : `${settled.length} steps have`} already finished and cannot be removed.`
      : "Nothing has changed yet. Adjust the steps or approve them as they stand.";

  const list = document.createElement("ol");
  list.className = "plan-review-list";

  function renderRows() {
    list.innerHTML = "";
    settled.forEach((step) => {
      const row = document.createElement("li");
      row.className = "plan-review-row";
      row.dataset.settled = "true";
      const text = document.createElement("span");
      text.className = "plan-step-title";
      text.textContent = step.title;
      const outcome = document.createElement("span");
      outcome.className = "plan-step-outcome";
      outcome.textContent = STEP_OUTCOME_LABELS[step.outcome] || "";
      row.append(text, outcome);
      list.append(row);
    });
    steps.forEach((step, index) => {
      const row = document.createElement("li");
      row.className = "plan-review-row";
      const input = document.createElement("input");
      input.type = "text";
      input.className = "plan-review-title";
      input.value = step.title;
      input.setAttribute("aria-label", `Step ${index + 1} title`);
      input.addEventListener("input", () => {
        steps[index].title = input.value;
      });
      const up = document.createElement("button");
      up.type = "button";
      up.className = "btn-icon";
      up.title = "Move up";
      up.setAttribute("aria-label", `Move step ${index + 1} up`);
      up.textContent = "↑";
      up.disabled = index === 0;
      up.addEventListener("click", () => {
        [steps[index - 1], steps[index]] = [steps[index], steps[index - 1]];
        renderRows();
      });
      const down = document.createElement("button");
      down.type = "button";
      down.className = "btn-icon";
      down.title = "Move down";
      down.setAttribute("aria-label", `Move step ${index + 1} down`);
      down.textContent = "↓";
      down.disabled = index === steps.length - 1;
      down.addEventListener("click", () => {
        [steps[index], steps[index + 1]] = [steps[index + 1], steps[index]];
        renderRows();
      });
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "btn-icon";
      remove.title = "Remove this step";
      remove.setAttribute("aria-label", `Remove step ${index + 1}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        steps = steps.filter((_, position) => position !== index);
        renderRows();
      });
      row.append(input, up, down, remove);
      list.append(row);
    });
  }
  renderRows();

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const approveButton = document.createElement("button");
  approveButton.type = "button";
  approveButton.className = "btn-sm btn-primary";
  approveButton.textContent = "Approve";
  const declineButton = document.createElement("button");
  declineButton.type = "button";
  declineButton.className = "btn-sm btn-quiet";
  declineButton.textContent = "Decline";
  actions.append(approveButton, declineButton);

  const output = document.createElement("pre");
  output.className = "command-approval-output";
  output.hidden = true;

  // Whether this counts as a revision is decided by comparing against what was
  // proposed, not by whether the user touched a control: a reorder followed by
  // a reorder back is not a revision, and recording one would put a
  // `plan_revised` event in the log describing no change.
  function revisionOrNull() {
    const same =
      steps.length === editable.length &&
      steps.every(
        (step, index) => step.id === editable[index].id && step.title === editable[index].title,
      );
    return same ? null : steps;
  }

  async function decide(approved) {
    approveButton.disabled = true;
    declineButton.disabled = true;
    list.querySelectorAll("input, button").forEach((control) => {
      control.disabled = true;
    });
    output.hidden = false;
    output.textContent = approved ? "Continuing…" : "Declining…";

    if (decideInWorker) {
      try {
        output.textContent = await decideInWorker(approved);
      } catch (error) {
        approveButton.disabled = false;
        declineButton.disabled = false;
        output.textContent = error.message;
        throw error;
      }
      return;
    }

    const revision = approved ? revisionOrNull() : null;
    const assistantMessage = appendChatMessage("assistant", "");
    // No Stop: this stream is sent without an abort signal, so nothing on this
    // path can cancel it.
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    try {
      await streamResumePlanRequest(
        {
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          approved: approved ? "true" : "false",
          steps: revision ? JSON.stringify(revision) : "",
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
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
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = new Error(payload.error || "Plan decision failed");
          },
        },
      );
    } catch (error) {
      streamError = error;
    } finally {
      indicator.finish("failed");
    }
    if (streamError) {
      // The bubble is the failure's record; the caller's toast goes away.
      renderChatMessageError(assistantMessage, streamError);
      throw streamError;
    }
    output.textContent = approved
      ? revision
        ? "Plan revised and approved — see the assistant's answer above."
        : "Plan approved — see the assistant's answer above."
      : "Plan declined — see the assistant's answer above.";
    await loadSessions(currentSessionId, false);
    await refreshFindings();
    void refreshRepoBar();
  }

  approveButton.addEventListener("click", async () => {
    try {
      await decide(true);
    } catch (error) {
      toast(error.message);
    }
  });
  declineButton.addEventListener("click", async () => {
    try {
      await decide(false);
    } catch (error) {
      toast(error.message);
    }
  });

  wrapper.append(header, note, list, actions, output);
  return wrapper;
}

// `onResolved` is called once no file is left pending, for a patch reattached
// after a crash: the task it belonged to has to be closed out, or the next
// launch offers the same patch again. See
// docs/specs/45_crash_recovery_prompt.md §5.7.
function createPatchPreview(payload, patchRepo, { onResolved = null } = {}) {
  const state = {
    patchId: payload.patchId,
    initialFileCount: (payload.files || []).length,
    // Warnings a hook returned on an apply (spec 32 Task 9). They are not a
    // reason to roll back, so they stay beside the applied files.
    hookWarnings: [],
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

  const headerText = document.createElement("div");
  headerText.className = "patch-preview-heading";
  const title = document.createElement("strong");
  title.textContent = payload.summary || "Patch preview";
  const meta = document.createElement("span");
  headerText.append(title, meta);

  const actions = document.createElement("div");
  actions.className = "patch-actions";
  const applyButton = document.createElement("button");
  applyButton.type = "button";
  applyButton.className = "btn-sm btn-primary";
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.className = "btn-sm btn-quiet";
  rejectButton.textContent = "Reject";
  actions.append(applyButton, rejectButton);
  header.append(headerText, actions);

  // Label and metadata both track the current selection, so the header states
  // what will actually happen rather than the indefinite "Selected". The patch
  // id it used to show is still on `state.patchId`.
  function updateHeaderCounts() {
    const selected = state.files.filter((file) => file.selected);
    const additions = selected.reduce((sum, file) => sum + file.additions, 0);
    const deletions = selected.reduce((sum, file) => sum + file.deletions, 0);
    applyButton.textContent = `Apply ${selected.length}`;
    applyButton.disabled = selected.length === 0;
    meta.textContent =
      selected.length === 1
        ? `1 file · +${additions} −${deletions}`
        : `${selected.length} files · +${additions} −${deletions}`;
  }

  updateHeaderCounts();

  // Shown when the secret scanner flags a selected file. The apply is not
  // refused outright: the user sees what was found and decides.
  const secretNotice = document.createElement("div");
  secretNotice.className = "patch-secret-notice";
  secretNotice.hidden = true;

  const list = document.createElement("div");
  list.className = "diff-list";
  wrapper.append(header, secretNotice, list);

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

  // Every file answered, one way or the other.
  async function reportResolvedIfSettled() {
    if (!onResolved) return;
    if (state.files.some((file) => file.state === "pending")) return;
    await onResolved();
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
    updateHeaderCounts();
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
        // Not `render()`: this handler deliberately does not rebuild the list,
        // which would discard focus mid-selection. Only the header counts
        // depend on which files are ticked.
        updateHeaderCounts();
      });
      const name = document.createElement("span");
      name.textContent = file.path;
      label.append(checkbox, name);
      // Each file header names its root when it is not the repository root,
      // so a patch that spans two roots shows both (`context.md` §13).
      const fileRoot = rootForPath(file.path);
      if (fileRoot) {
        const rootLabel = document.createElement("span");
        rootLabel.className = "diff-root";
        rootLabel.textContent = fileRoot;
        label.append(rootLabel);
      }

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
    if (state.hookWarnings.length) {
      const notice = document.createElement("div");
      notice.className = "patch-hook-warnings";
      const heading = document.createElement("strong");
      heading.textContent =
        state.hookWarnings.length === 1
          ? "A hook reported on this apply"
          : "Hooks reported on this apply";
      notice.append(heading);
      state.hookWarnings.forEach((warning) => {
        const line = document.createElement("p");
        line.textContent = warning;
        notice.append(line);
      });
      list.append(notice);
    }
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

      state.hookWarnings = Array.isArray(result.hookWarnings) ? result.hookWarnings : [];
      const applied = result.appliedFiles || [];
      markFiles(applied, "applied");
      await reportResolvedIfSettled();
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
      await reportResolvedIfSettled();
    } catch (error) {
      toast(error.message);
      render();
    }
  });

  render();
  return wrapper;
}

// `detached` is for a proposal reattached after a crash: the same card, but
// the turn that raised it is gone, so approving runs the stored proposal on its
// own rather than resuming a chat turn. See docs/specs/45_crash_recovery_prompt.md §5.7.
//
// `decide` is for a worker's proposal (spec 57 §5.6): the turn waits in the
// worker's process, so the decision goes to it and the card only reports what
// was sent. It resolves to the line the card shows.
function createCommandApprovalPreview(
  proposal,
  proposalRepo,
  { detached = false, onResolved = null, decide = null } = {},
) {
  const isBrowserDiagnostic =
    proposal.allowBrowserDiagnosticsForSession || (proposal.risk || "").startsWith("browser");
  const wrapper = document.createElement("div");
  wrapper.className = "command-approval";

  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = isBrowserDiagnostic ? "Run browser diagnostic" : "Run command";
  const meta = document.createElement("span");
  meta.className = "command-approval-risk";
  meta.textContent = proposal.blocked ? "blocked" : proposal.risk || "review";
  meta.dataset.blocked = proposal.blocked ? "true" : "false";
  header.append(title, meta);

  // The policy's deciding cause and the agent's own reason sit outside the
  // rationale disclosure (spec 62 §5.2). They are what tells the user whether
  // a prompt is routine before they spend attention opening "Why this
  // command"; that disclosure still carries the full prompt, every cause.
  // MCP and browser-diagnostic proposals have neither field set, so both lines
  // stay hidden rather than showing an empty label.
  const cause = document.createElement("p");
  cause.className = "command-approval-cause";
  cause.textContent = proposal.cause || "";
  cause.hidden = !proposal.cause;

  const command = document.createElement("code");
  command.className = "command-approval-command";
  command.textContent = proposal.command || "";

  const agentReason = document.createElement("p");
  agentReason.className = "command-approval-reason";
  if (proposal.reason) {
    agentReason.textContent = `The agent says: ${proposal.reason}`;
  } else {
    agentReason.hidden = true;
  }

  // The card payload comes from `chat.rs`'s `AgentCommandProposal`, which
  // carries no working directory (spec 24 Task 8), so the shell reads it from
  // the stored proposal. The card says where the command runs only when that
  // is a sub-root (`context.md` §13).
  const location = document.createElement("p");
  location.className = "command-approval-location";
  location.hidden = true;
  if (proposal.proposalId && proposalRepo) {
    void api(
      `/api/command-proposal?repo=${encodeURIComponent(proposalRepo)}&proposal_id=${encodeURIComponent(proposal.proposalId)}`,
    )
      .then((stored) => {
        const relative = repositoryRelative(stored.repositoryRoot, stored.workingDirectory);
        if (!relative) return;
        location.textContent = "Runs in ";
        const code = document.createElement("code");
        code.textContent = relative;
        location.append(code);
        location.hidden = false;
      })
      .catch(() => {});
  }

  // The rationale sits behind a disclosure because the common case is a short
  // command approved without reading it.
  const details = document.createElement("pre");
  details.className = "command-approval-details";
  details.textContent = proposal.prompt || "";

  const disclosure = createDisclosure("Why this command", details);

  // Disclosure and actions share one footer row: the disclosure is secondary
  // chrome, and giving it a row of its own cost 34px on every card for a
  // control most approvals never use.
  const footer = document.createElement("div");
  footer.className = "command-approval-footer";

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const runButton = document.createElement("button");
  runButton.type = "button";
  runButton.className = "btn-sm btn-primary";
  runButton.textContent = proposal.blocked ? "Blocked" : "Approve";
  runButton.disabled = Boolean(proposal.blocked);
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.className = "btn-sm btn-quiet";
  rejectButton.textContent = "Reject";
  actions.append(runButton, rejectButton);

  // Grants that outlive this turn go behind the overflow rather than sitting at
  // the same weight as Approve: the cost of a mis-click is not symmetric.
  // `Allow Always` writes the user's allowlist for this repository (spec 10);
  // the session grant widens browser diagnostics for the whole session
  // (spec 12). The server decides eligibility — blocked commands, shell-control
  // syntax and `require_approval_for_all_commands` all rule out the allowlist
  // path — so an item is offered only when the proposal says it may be.
  // A worker's `approve` carries no grant, so a worker's card offers none.
  const menuItems = [];
  if (proposal.allowAlways && !decide) {
    menuItems.push({
      label: "Always allow in this project",
      hint: "Adds to the allowlist — persists after this turn",
      onSelect: () => runWithGrant({ always: true }, "Command allowed for this project"),
    });
  }
  if (proposal.allowBrowserDiagnosticsForSession && !decide) {
    menuItems.push({
      label: "Allow for this session",
      hint: "Browser diagnostics only, until this session ends",
      onSelect: () =>
        runWithGrant(
          { allowBrowserDiagnosticsForSession: true },
          "Browser diagnostics allowed for this session",
        ),
    });
  }

  let overflowButton = null;
  if (menuItems.length && !proposal.blocked) {
    overflowButton = document.createElement("button");
    overflowButton.type = "button";
    overflowButton.className = "btn-icon";
    overflowButton.dataset.menuId = `approval-${proposal.proposalId}`;
    overflowButton.setAttribute("aria-haspopup", "menu");
    overflowButton.setAttribute("aria-expanded", "false");
    overflowButton.setAttribute("aria-label", "More approval options");
    overflowButton.title = "More approval options";
    overflowButton.innerHTML = '<span class="dots-icon" aria-hidden="true"></span>';
    // Without `stopPropagation` the document click handler that dismisses the
    // menu fires in the same tick and closes it as it opens.
    overflowButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleApprovalMenu(menuItems, overflowButton);
    });
    actions.append(overflowButton);
  }

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
    rejectButton.disabled = true;
    if (overflowButton) overflowButton.disabled = true;
    closeApprovalMenu();
    output.hidden = false;
    output.textContent = approved
      ? isBrowserDiagnostic
        ? "Running diagnostic…"
        : "Running…"
      : "Rejecting…";

    // A proposal reattached after a crash has no chat turn left to resume —
    // the turn died with the process, and `/api/resume-command-stream`
    // rightly refuses a proposal it holds no pending state for. The stored
    // proposal still runs, so the decision goes to the non-streaming
    // endpoints and its output lands in this card instead of in an answer
    // from a model that was never asked anything.
    if (detached) {
      const payload = await api(
        approved ? "/api/run-command" : "/api/reject-command",
        form({
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          always: always ? "true" : "false",
        }),
      );
      output.textContent = approved
        ? `exit ${payload.exitCode}\n${payload.stdout || ""}${payload.stderr || ""}`.trimEnd()
        : "Command rejected. The turn that proposed it ended with the crash, so nothing continues.";
      if (onResolved) await onResolved(approved);
      await loadSessions(currentSessionId, false);
      await refreshFindings();
      void refreshRepoBar();
      return;
    }

    if (decide) {
      output.textContent = await decide(approved);
      return;
    }

    const assistantMessage = appendChatMessage("assistant", "");
    // No Stop: this stream is sent without an abort signal, so nothing on this
    // path can cancel it.
    const indicator = startTurnIndicator(assistantMessage);
    let assistantText = "";
    let streamError = null;
    try {
      await streamResumeCommandRequest(
        {
          repo: proposalRepo,
          proposal_id: proposal.proposalId,
          approved: approved ? "true" : "false",
          always: always ? "true" : "false",
          allow_browser_diagnostics_for_session: allowBrowserDiagnosticsForSession
            ? "true"
            : "false",
        },
        {
          phase(payload) {
            indicator.phase(payload);
          },
          plan(planPayload) {
            renderPlanPanel(assistantMessage, planPayload);
          },
          webDiagnostic(record) {
            sessionHasRecordedDiagnostics = true;
            renderWebDiagnosticCard(assistantMessage, record);
          },
          token(token) {
            assistantText += token;
            updateChatMessage(assistantMessage, assistantText);
            indicator.streaming();
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
            appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
            appendTurnNotes(assistantMessage.body, payload.turnNotes);
            markMessageUsage(assistantMessage, payload.usage, null);
            indicator.finish(chatCompletionState(payload));
            markMessageTurnStop(assistantMessage, payload);
          },
          error(payload) {
            streamError = new Error(payload.error || "Command resume failed");
          },
        },
      );
    } catch (error) {
      streamError = error;
    } finally {
      indicator.finish("failed");
    }
    if (streamError) {
      // The bubble is the failure's record; the caller's toast goes away.
      renderChatMessageError(assistantMessage, streamError);
      throw streamError;
    }
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
    await refreshFindings();
    void refreshRepoBar();
  }

  // Re-enable after a failure so the user can retry or pick a different
  // action; a blocked proposal's run button stays disabled regardless.
  function restoreActions() {
    runButton.disabled = Boolean(proposal.blocked);
    rejectButton.disabled = false;
    if (overflowButton) overflowButton.disabled = false;
  }

  // Shared by both overflow grants. Their handlers were previously identical
  // apart from the options and the toast.
  async function runWithGrant(options, successToast) {
    try {
      await resolveCommandProposal(true, options);
      toast(successToast);
    } catch (error) {
      restoreActions();
      output.textContent = error.message;
      toast(error.message);
    }
  }

  runButton.addEventListener("click", async () => {
    try {
      await resolveCommandProposal(true);
      toast(decide ? "Command approved" : "Command completed");
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

  footer.append(disclosure, actions);
  wrapper.append(header, cause, command, agentReason, location, details, footer, output);
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
  // The list just changed under the header: a rename of the open session, from
  // either ⋮ or a double-click, lands here and nowhere else.
  renderThreadHeader();
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
  // Opening a conversation leaves the Workers board.
  hideWorkersBoard();
  if (!sessionId) {
    currentSessionId = "";
    localStorage.removeItem(lastSessionStorageKey());
    loadPinnedContextFiles("");
    sessionCheckpoints = [];
    clearChat();
    void refreshRepoBar();
    return;
  }
  // A rewind reloads the session through here, so it refreshes the bar too.
  void refreshRepoBar();
  const payload = await api(`/api/session?session_id=${encodeURIComponent(sessionId)}`);
  currentSessionId = payload.session.id;
  currentSessionMode = { sessionId: payload.session.id, mode: payload.session.mode || "code" };
  localStorage.setItem(lastSessionStorageKey(), currentSessionId);
  $("session-select").value = currentSessionId;
  syncSessionListActive();
  loadPinnedContextFiles(currentSessionId);
  await loadSessionCheckpoints(currentSessionId);
  // Before rendering: the messages' *Start worker* actions depend on it.
  await refreshWorkerIndex();
  renderThreadHeader();
  const rendered = renderMessages(
    payload.messages,
    payload.tasks || [],
    payload.conversationView?.compactions || [],
    payload.conversationView?.conversation || null,
  );
  renderSpawnedWorkerEntries(rendered, currentSessionId);
  contextPins = payload.contextState?.pins || [];
  contextRestrictionDirty = false;
  contextRestriction = payload.contextState?.restrictedPaths ?? null;
  syncContextRestrictionField();
  const manifests = (payload.tasks || []).map((task) => task.context).filter(Boolean);
  lastContextManifest = manifests.reduce(
    (newest, manifest) =>
      !newest || (manifest.assembledAtMs || 0) >= (newest.assembledAtMs || 0) ? manifest : newest,
    null,
  );
  contextPanelTaskId = lastContextManifest?.taskId || "";
  currentContextReview = null;
  renderContextButton();
  if (!$("context-panel").hidden) renderContextPanel();
  await refreshFindings();
  // After the conversation, because `renderMessages` clears the log and the
  // cards sit above what it renders.
  await renderRecoveryPrompts(currentSessionId);
  // The memory stack sits below the conversation and above nothing else, so it
  // is drawn after the messages it follows.
  await refreshMemoryProposals(currentSessionId);
  // Last: the strip and a worker's approval card follow what was just drawn.
  syncWorkerView();
}

async function streamChatRequest(data, handlers, signal) {
  return streamRequest("/api/ask-stream", "/api/ask", data, handlers, signal);
}

async function streamResumeCommandRequest(data, handlers) {
  const fallbackPath = data.approved === "true" ? "/api/run-command" : "/api/reject-command";
  return streamRequest("/api/resume-command-stream", fallbackPath, data, handlers);
}

// A fetch decision resumes the turn, so the non-streaming fallback carries the
// same decision and answers with the whole turn result.
async function streamResumeFetchRequest(data, handlers) {
  return streamRequest("/api/resume-fetch-stream", "/api/fetch-decision", data, handlers);
}

// No non-streaming fallback: resuming a plan decision *is* a turn, and there is
// no one-shot endpoint that could stand in for one. `streamRequest` raises
// rather than posting the decision somewhere that would not carry it.
async function streamResumePlanRequest(data, handlers) {
  return streamRequest("/api/resume-plan-stream", null, data, handlers);
}

// Like a plan decision, answering a question *is* a turn, so there is no
// one-shot endpoint to fall back to.
async function streamResumeAnswerRequest(data, handlers) {
  return streamRequest("/api/answer-question-stream", null, data, handlers);
}

// Like a plan decision, answering a fallback offer *is* a turn, so there is no
// one-shot endpoint to fall back to.
async function streamResumeFallbackRequest(data, handlers) {
  return streamRequest("/api/resume-fallback-stream", null, data, handlers);
}

// Like a fallback decision, answering a round-budget pause *is* a turn, so
// there is no one-shot endpoint to fall back to.
async function streamResumeRoundBudgetRequest(data, handlers) {
  return streamRequest("/api/resume-round-budget-stream", null, data, handlers);
}

async function streamResumeContextRequest(data, handlers) {
  return streamRequest("/api/resume-context-stream", null, data, handlers);
}

// Verification is a continuation turn the engine starts, not a user prompt, so
// there is no one-shot endpoint behind it.
async function streamVerifyRequest(data, handlers) {
  return streamRequest("/api/verify-stream", null, data, handlers);
}

async function streamResumeChecksRequest(data, handlers) {
  return streamRequest("/api/resume-checks-stream", null, data, handlers);
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
    if (!fallbackPath) {
      throw new Error("This request needs a streaming response and the browser gave none.");
    }
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
  if (event === "plan" && handlers.plan) handlers.plan(payload);
  if (event === "web_diagnostic" && handlers.webDiagnostic) handlers.webDiagnostic(payload);
  if (event === "context") {
    // Handled here rather than per call site: every stream that assembles a
    // turn emits this, including a plain prompt and the verification repair.
    // A per-call handler left the default (no-pause) pre-send view invisible
    // until a reload read it back from /api/session (spec 27 C9).
    setContextManifest(payload, payload.taskId || "");
    if (handlers.context) handlers.context(payload);
  }
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

$("pick-folder-btn").addEventListener("click", () => void pickWorkingFolder());

// The sidebar's "Add working folder" and the repo bar's "Change folder…".
async function pickWorkingFolder() {
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
}

$("context-inspector-btn").addEventListener("click", () => {
  if ($("context-panel").hidden) openContextPanel();
  else closeContextPanel();
});

$("context-panel-close").addEventListener("click", closeContextPanel);

$("context-pin-btn").addEventListener("click", async () => {
  try {
    const path = $("context-pin-path").value.trim();
    if (!path) throw new Error("A path is required");
    const raw = $("context-pin-range").value.trim();
    let range = null;
    if (raw) {
      const match = /^(\d+)\s*-\s*(\d+)$/.exec(raw);
      if (!match) throw new Error("A range must look like 40-120");
      range = { start: Number(match[1]), end: Number(match[2]) };
    }
    await addContextPin(path, range);
    $("context-pin-path").value = "";
    $("context-pin-range").value = "";
  } catch (error) {
    toast(error.message);
  }
});

$("context-pins-clear").addEventListener("click", () => {
  clearContextPins().catch((error) => toast(error.message));
});

$("context-restriction-paths").addEventListener("input", () => {
  contextRestrictionDirty = true;
});

$("context-restriction-apply").addEventListener("click", () => {
  applyContextRestriction().catch((error) => toast(error.message));
});

$("context-restriction-clear").addEventListener("click", () => {
  clearContextRestriction().catch((error) => toast(error.message));
});

$("pause-for-context-review-input").addEventListener("change", async (event) => {
  const enabled = event.target.checked;
  try {
    await api(
      "/api/config-set",
      form({ key: "pause_for_context_review", value: enabled ? "true" : "false" }),
    );
    await loadConfigFile();
    await refreshPauseForContextReview();
    toast(enabled ? "Turns will pause before sending" : "Turns send without pausing");
  } catch (error) {
    event.target.checked = !enabled;
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
  const searchPanel = $("session-search-panel");
  if (
    !searchPanel.hidden &&
    !searchPanel.contains(event.target) &&
    !$("session-search-btn").contains(event.target)
  ) {
    closeSessionSearch();
  }
  const symbolPanel = $("symbol-search-panel");
  if (
    !symbolPanel.hidden &&
    !symbolPanel.contains(event.target) &&
    !$("symbol-search-btn").contains(event.target)
  ) {
    closeSymbolSearch();
  }
});

// The header ⋮ (spec 58 §1 req 6): Open in first, then the open session's own
// actions, which call exactly what its sidebar row ⋮ calls. Built on every
// open, so the session items follow whichever session is open now.
const threadMenu = createContextMenu();

function toggleThreadMenu(anchorEl) {
  const projectPath = repo();
  const session = currentSession();
  const noRepo = !projectPath;
  const noSession = !session;
  threadMenu.toggle(anchorEl, {
    sections: [
      [
        {
          label: "Open in",
          action: "open-in",
          disabled: noRepo,
          items: [
            [
              {
                label: "VS Code",
                action: "open-vscode",
                onSelect: () => void handleProjectMenuAction("open-vscode", projectPath),
              },
              {
                label: "Finder",
                action: "open-finder",
                onSelect: () => void handleProjectMenuAction("open-finder", projectPath),
              },
              {
                label: "Terminal",
                action: "open-terminal",
                onSelect: () => setTerminalOpen(true),
              },
            ],
          ],
        },
      ],
      [
        {
          label: "Rename session",
          action: "rename",
          disabled: noSession,
          onSelect: runSessionAction(() => renameSessionForProject(projectPath, session)),
        },
        {
          label: "Export as Markdown",
          action: "export",
          disabled: noSession,
          onSelect: runSessionAction(() => exportSessionDownload(session)),
        },
      ],
      [
        {
          label: "Delete session",
          action: "delete",
          danger: true,
          disabled: noSession,
          onSelect: runSessionAction(() => deleteSessionForProject(projectPath, session)),
        },
      ],
    ],
  });
}

$("thread-menu-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleThreadMenu(event.currentTarget);
});

$("terminal-toggle-btn").addEventListener("click", () => {
  setTerminalOpen(!terminalOpen);
});

$("repo-folder-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleRepoFolderMenu(event.currentTarget);
});

$("repo-action-main").addEventListener("click", () => {
  if (repoActionCurrent) void repoActionCurrent.run();
});

$("repo-action-more").addEventListener("click", (event) => {
  event.stopPropagation();
  repoActionCurrent?.more?.toggle(event.currentTarget);
});

$("session-mode-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleSessionModeMenu(event.currentTarget);
});

$("findings-close-btn").addEventListener("click", () => {
  closeFindingsPopover({ restoreFocus: true });
});
// `composedPath`, not `contains`: Dismiss and the checkboxes redraw the list,
// so by the time this runs the clicked element may no longer be in the panel.
document.addEventListener("click", (event) => {
  if (!findingsPanelOpen) return;
  const path = event.composedPath();
  if (path.includes($("findings-panel")) || path.includes($("repo-action-more"))) return;
  closeFindingsPopover();
});
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || !findingsPanelOpen) return;
  closeFindingsPopover({ restoreFocus: true });
});
$("findings-severity-filter").addEventListener("change", renderFindingsPanel);
$("findings-status-filter").addEventListener("change", renderFindingsPanel);
$("findings-fix-btn").addEventListener("click", () => void fixSelectedFindings());

$("session-search-btn").addEventListener("click", () => {
  toggleSessionSearch();
});

$("session-search-input").addEventListener("input", (event) => {
  void runSessionSearch(event.target.value);
});

$("session-search-input").addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeSessionSearch();
});

$("symbol-search-btn").addEventListener("click", () => {
  toggleSymbolSearch();
});

$("symbol-search-input").addEventListener("input", (event) => {
  void runSymbolSearch(event.target.value);
});

$("symbol-search-input").addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeSymbolSearch();
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

// Like the mode, the selection is the explicit user action, so there is no
// confirmation step. It applies from the next turn, resume or command.
$("permission-profile-select").addEventListener("change", async () => {
  try {
    await selectPermissionProfile($("permission-profile-select").value);
  } catch (error) {
    toast(errorMessage(error));
    renderPermissionProfilePicker();
  }
});

$("profile-export-btn").addEventListener("click", () => {
  exportPermissionProfile().catch((error) => toast(errorMessage(error)));
});
$("profile-import-review-btn").addEventListener("click", () => {
  reviewPermissionProfileImport().catch((error) => {
    clearProfileImportReview();
    toast(errorMessage(error));
  });
});
$("profile-import-confirm-btn").addEventListener("click", () => {
  confirmPermissionProfileImport().catch((error) => toast(errorMessage(error)));
});
$("profile-import-cancel-btn").addEventListener("click", clearProfileImportReview);
for (const id of ["profile-import-text", "profile-import-name", "profile-import-replace"]) {
  $(id).addEventListener("input", clearProfileImportReview);
}

// A request to plan, split or hand off work. With Worker sessions on, it goes
// to the chat turn, whose system prompt carries the handoff paragraph; the
// one-shot patch flow can only answer with a diff (spec 57 §7 item 11). With
// the setting off, routing is exactly what it was.
const PLANNING_REQUEST =
  /\b(split|plan|planning|tasks?|hand\s*off|handoffs?|parallel|workers?|sessions?)\b|\bbreak\b.*\b(down|up|into)\b/;

// Whether a change request may take the one-shot patch flow. Only a provider
// without native tool calling needs it: its text envelope can request a
// command but cannot propose a patch. With native tools the agentic turn
// proposes patches itself (`propose_patch`, `edit_file`) and can also read the
// repository, run checks and keep a plan, which the one-shot flow cannot, so
// every request goes to the turn (spec 72).
function changeRequestsUseOneShotPatch() {
  const { provider } = selectedChatModelOptions();
  return modelProviderPresets[provider]?.supportsNativeTools === false;
}

function looksLikeEditRequest(prompt) {
  const text = prompt.trim().toLowerCase();
  if (!text) return false;
  if (/^(how\s+(do|can|would|should)\s+i|how\s+to|what\s+is|why\b|where\b)/.test(text)) {
    return false;
  }
  if (workersEnabled && PLANNING_REQUEST.test(text)) return false;
  const editVerb =
    /\b(add|create|write|generate|implement|modify|update|change|fix|refactor|remove|delete|make)\b/;
  const codeTarget =
    /\b(file|test|code|function|component|class|module|endpoint|api|route|ui|layout|style|css|html|javascript|typescript|rust|readme|doc|docs|config|script|bug|issue|error)\b/;
  return editVerb.test(text) && codeTarget.test(text);
}

async function proposePatchFromChat(prompt, assistantMessage) {
  setChatMessagePlaceholder(assistantMessage, "Generating a patch preview...");
  // No Stop: this is a plain request with no abort signal, so `currentTurn` is
  // never set and nothing could cancel it. No phase events either, so the
  // label stays put while the clock runs.
  const indicator = startTurnIndicator(assistantMessage);
  indicator.phase({ phase: "tool", label: "Generating patch" });
  const patchRepo = requireRepo();
  let payload;
  try {
    payload = await api(
      "/api/propose-edit",
      form({
        repo: patchRepo,
        prompt,
        // The open session's mode governs this flow too; the server refuses the
        // proposal, before any model call, when that mode forbids patches.
        session_id: currentSessionId,
        context_files: pinnedContextFiles.join("\n"),
        ...chatModelFormFields(),
      }),
    );
  } finally {
    // Removes itself either way: the patch card below says "ready", and the
    // caller writes a failure into the bubble.
    indicator.finish(payload ? "complete" : "failed");
  }
  updateChatMessage(
    assistantMessage,
    `Prepared a patch preview for \`${payload.patchId}\`. Review the diff and apply selected files when ready.`,
  );
  assistantMessage.body.append(createPatchPreview(payload, patchRepo));
  appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
  appendTurnNotes(assistantMessage.body, payload.turnNotes);
  markMessageUsage(assistantMessage, payload.usage, liveTurnDurationMs(payload, indicator));
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
  // "Fix selected" and the repo bar's action follow `chatSubmitting`, which
  // callers set before this.
  renderFindingsPanel();
}

// Sizes the composer to its content. The floor and the cap both live in CSS
// (`.prompt-box textarea`), so the field stays usable and bounded even if this
// never runs; all this does is pick the height in between.
//
// Call it after any programmatic change to `value` — those do not fire `input`.
function autoGrowPrompt() {
  const field = $("chat-prompt");
  field.style.height = "auto";
  field.style.height = `${field.scrollHeight}px`;
  field.style.overflowY = field.scrollHeight > field.clientHeight ? "auto" : "hidden";
}

// Puts a prompt back after a stop or a failure, unless the user has started
// typing something new. Either way the original is in the session history.
function restorePrompt(text) {
  const field = $("chat-prompt");
  if (field.value.trim()) return;
  field.value = text;
  autoGrowPrompt();
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
  // A worker's turns run in its own process (requirement 4): its composer
  // talks to the worker, never to the shell's turn routes.
  if (workerView) {
    await sendWorkerComposer(promptOverride);
    return;
  }
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
    autoGrowPrompt();
    // A scoped repair from the findings panel needs the agentic turn, which
    // reads, edits and re-runs. Its text ("Fix the 2 findings… error…") would
    // otherwise match the edit heuristic and go to the one-shot patch flow
    // (spec 22 `context.md` §17). With native tools nothing takes that flow
    // (spec 72).
    if (!options.agentic && changeRequestsUseOneShotPatch() && looksLikeEditRequest(prompt)) {
      await proposePatchFromChat(prompt, assistantMessage);
      dismissContextChips();
      return;
    }

    const controller = new AbortController();
    currentTurn = { controller, stopped: false };
    indicator = startTurnIndicator(assistantMessage, stopCurrentTurn);

    let assistantText = "";

    await streamChatRequest(
      {
        repo: chatRepo,
        prompt,
        session_id: currentSessionId,
        context_files: pinnedContextFiles.join("\n"),
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
        plan(payload) {
          renderPlanPanel(assistantMessage, payload);
        },
        webDiagnostic(record) {
          sessionHasRecordedDiagnostics = true;
          renderWebDiagnosticCard(assistantMessage, record);
        },
        token(token) {
          assistantText += token;
          updateChatMessage(assistantMessage, assistantText);
          if (indicator) indicator.streaming();
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
          appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
          appendTurnNotes(assistantMessage.body, payload.turnNotes);
          markMessageUsage(assistantMessage, payload.usage, liveTurnDurationMs(payload, indicator));
          void refreshMemoryProposals(currentSessionId);
          if (payload.cancelled) {
            if (indicator) indicator.finish("stopped");
            return;
          }
          if (indicator) indicator.finish(chatCompletionState(payload));
          markMessageTurnStop(assistantMessage, payload);
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
    await refreshFindings();
  } catch (error) {
    // A stop is not a failure. Without this every Stop would toast "Failed"
    // and write an error into the bubble the user just chose to keep.
    if (currentTurn?.stopped || error.name === "AbortError") {
      if (indicator) indicator.finish("stopped");
      if (restoreSubmittedPrompt) restorePrompt(prompt);
      await loadSessions(currentSessionId, false);
      await refreshFindings();
    } else {
      if (indicator) indicator.finish("failed");
      toast(error.message);
      renderChatMessageError(assistantMessage, error);
      if (restoreSubmittedPrompt) restorePrompt(prompt);
    }
  } finally {
    chatSubmitting = false;
    currentTurn = null;
    setComposerBusy(false);
    // Whatever the outcome, an agentic turn may have changed files.
    void refreshRepoBar();
  }
}

async function askAgainFromExhausted(sessionId, taskId) {
  if (chatSubmitting) throw new Error("A turn is already running");
  if (!sessionId && !currentSessionId) throw new Error("No session selected");
  if (!taskId) throw new Error("This turn has nothing to re-ask");
  if (sessionId && sessionId !== currentSessionId) {
    await loadSession(sessionId);
  }
  chatSubmitting = true;
  setComposerBusy(true);
  await ensureDesktopApiReady();
  const chatRepo = requireRepo();
  const assistantMessage = appendChatMessage("assistant", "");
  const indicator = startTurnIndicator(assistantMessage, stopCurrentTurn);
  const controller = new AbortController();
  currentTurn = { controller, stopped: false };
  let assistantText = "";
  let streamError = null;
  try {
    await streamChatRequest(
      {
        repo: chatRepo,
        session_id: sessionId || currentSessionId,
        ask_again_task_id: taskId,
        context_files: pinnedContextFiles.join("\n"),
        ...chatModelFormFields(),
      },
      {
        session(newSessionId) {
          if (newSessionId) currentSessionId = newSessionId;
        },
        phase(payload) {
          indicator.phase(payload);
        },
        plan(payload) {
          renderPlanPanel(assistantMessage, payload);
        },
        webDiagnostic(record) {
          sessionHasRecordedDiagnostics = true;
          renderWebDiagnosticCard(assistantMessage, record);
        },
        token(token) {
          assistantText += token;
          updateChatMessage(assistantMessage, assistantText);
          indicator.streaming();
        },
        async done(payload) {
          currentSessionId = payload.sessionId;
          localStorage.setItem(lastSessionStorageKey(), currentSessionId);
          persistPinnedContextForSession(currentSessionId);
          if (payload.response && payload.response !== assistantText) {
            assistantText = payload.response;
          }
          await finalizeChatMessage(assistantMessage, assistantText);
          appendProposals(assistantMessage, payload, chatRepo);
          appendContextDisclosure(assistantMessage.body, payload.contextFiles || []);
          appendTurnNotes(assistantMessage.body, payload.turnNotes);
          markMessageUsage(assistantMessage, payload.usage, liveTurnDurationMs(payload, indicator));
          void refreshMemoryProposals(currentSessionId);
          if (payload.cancelled) {
            indicator.finish("stopped");
            return;
          }
          indicator.finish(chatCompletionState(payload));
          markMessageTurnStop(assistantMessage, payload);
        },
        error(payload) {
          streamError = new Error(payload.error || "Model request failed");
        },
      },
      controller.signal,
    );
    if (streamError) throw streamError;
    dismissContextChips();
    await loadSessionCheckpoints(currentSessionId);
    await loadSessions(currentSessionId, false);
    await refreshFindings();
  } catch (error) {
    if (currentTurn?.stopped || error.name === "AbortError") {
      indicator.finish("stopped");
      await loadSessions(currentSessionId, false);
      await refreshFindings();
    } else {
      indicator.finish("failed");
      toast(error.message);
      renderChatMessageError(assistantMessage, error);
    }
  } finally {
    chatSubmitting = false;
    currentTurn = null;
    setComposerBusy(false);
    void refreshRepoBar();
  }
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

$("chat-prompt").addEventListener("input", autoGrowPrompt);
autoGrowPrompt();

$("chat-prompt").addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  void sendChatPrompt();
});

for (const trigger of modelMenuTriggers()) {
  trigger.addEventListener("click", (event) => {
    event.stopPropagation();
    toggleModelMenu(trigger.dataset.entryPanel);
  });
}

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

// The defaults depend on the provider id and its models, so they follow edits
// to either.
$("provider-models-input").addEventListener("change", () => void showProviderTokenDefaults());
$("provider-id-input").addEventListener("change", () => void showProviderTokenDefaults());
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

$("roots-add-btn").addEventListener("click", async () => {
  try {
    if (!repo()) {
      toast("Select a repository first");
      return;
    }
    const path = await promptDialog("Add a repository root (writes to .damaian/config.conf)", "");
    const trimmed = String(path || "").trim();
    if (!trimmed) return;
    await writeRootOverride("add", trimmed);
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

$("conversation-auto-compact-input").addEventListener("change", async (event) => {
  const enabled = event.target.checked;
  try {
    await api(
      "/api/config-set",
      form({ key: "conversation_auto_compact", value: enabled ? "true" : "false" }),
    );
    // The editor is authoritative for everything else; reload it so its next
    // Save cannot write the old value back.
    await loadConfigFile();
    await refreshConversationAutoCompact();
  } catch (error) {
    toast(errorMessage(error));
    await refreshConversationAutoCompact();
  }
});

$("worker-sessions-input").addEventListener("change", async (event) => {
  const enabled = event.target.checked;
  try {
    await saveWorkerSessionsSetting(enabled);
    toast(enabled ? "Worker sessions on" : "Worker sessions off");
  } catch (error) {
    event.target.checked = workersEnabled;
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

$("check-updates-btn").addEventListener("click", () => {
  void checkForAppUpdate(true);
});

$("update-app-btn").addEventListener("click", () => {
  void installAppUpdate();
});

$("symbol-search-btn").replaceChildren(icon("code"));
$("terminal-toggle-btn").replaceChildren(icon("terminal"));
$("composer-attach-btn").replaceChildren(icon("paperclip"));
$("pick-folder-btn").replaceChildren(icon("folderPlus"));

$("ask-btn").disabled = true;
syncProviderCatalogFromPolicy("");
applyChatModelOptions(modelOptionsFromPolicy(""));
renderProviderConfigSelect();
renderPinnedContextFiles();

bootstrapPromise = startBootstrap();
// Classify what the last run left behind now rather than when a session
// happens to be opened: spec 17 requirement 4 is a launch-time sweep, and it
// is what authorizes the tasks that were safe to pick up again.
void loadRecoverySweep();
void setupContextFileDragDrop();
