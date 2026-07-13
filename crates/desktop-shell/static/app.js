const $ = (id) => document.getElementById(id);

let currentCommandProposalId = "";
let currentSessionId = "";
let apiToken = "";
let bootstrapPromise = null;
let bootstrapError = null;
let chatSubmitting = false;
let pinnedContextFiles = [];

const localApiOrigin = "http://127.0.0.1:4765";
const localApiHostnames = new Set(["127.0.0.1", "localhost"]);
const apiTokenHeader = "x-damaian-api-token";
const lastRepoStorageKey = "damaian:lastRepository";
const inspectorCollapsedStorageKey = "damaian:inspectorCollapsed";
const pinnedContextStoragePrefix = "damaian:pinnedContextFiles";

function repo() {
  return $("repo").value.trim();
}

function toast(message) {
  const el = $("toast");
  el.textContent = message;
  el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 2600);
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
  return path.startsWith("/api/") && path !== "/api/bootstrap";
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
  return api("/api/bootstrap")
    .then((payload) => {
      if (!payload.apiToken) throw new Error("Desktop API token missing from bootstrap");
      apiToken = payload.apiToken;
      if (!chatSubmitting) $("ask-btn").disabled = false;
      setInspectorCollapsed(localStorage.getItem(inspectorCollapsedStorageKey) === "true");
      const lastRepo = localStorage.getItem(lastRepoStorageKey);
      if (lastRepo) {
        setRepository(lastRepo, false);
      } else if (payload.defaultRepo) {
        setRepository(payload.defaultRepo, false);
      } else {
        loadPinnedContextFiles("");
        clearSessionList();
        clearChat();
      }
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

function setRepository(value, persist = true) {
  $("repo").value = value;
  setRepoState(value ? "Repository selected" : "No repository selected");
  if (persist && value) {
    localStorage.setItem(lastRepoStorageKey, value);
  }
  currentSessionId = "";
  loadPinnedContextFiles("");
  if (value) {
    void loadSessions("", true).catch(() => {});
  } else {
    clearSessionList();
    clearChat();
  }
}

function tauriDialogOpen() {
  return window.__TAURI__?.dialog?.open;
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

function setInspectorCollapsed(collapsed) {
  document.body.classList.toggle("inspector-collapsed", collapsed);
  localStorage.setItem(inspectorCollapsedStorageKey, collapsed ? "true" : "false");
  const button = $("inspector-toggle-btn");
  button.classList.toggle("is-collapsed", collapsed);
  const label = collapsed ? "Show tools" : "Hide tools";
  button.title = label;
  button.setAttribute("aria-label", label);
  button.setAttribute("aria-expanded", collapsed ? "false" : "true");
}

function ensureInspectorVisible() {
  if (document.body.classList.contains("inspector-collapsed")) {
    setInspectorCollapsed(false);
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
}

function configValue(content, key) {
  const prefix = `${key}=`;
  const line = String(content || "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .find((item) => item.startsWith(prefix));
  return line ? line.slice(prefix.length).trim() : "";
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
  const payload = await api(`/api/model-key-status?repo=${encodeURIComponent(repo())}`);
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
  syncModelKeyAccountFromConfig(payload.content);
  renderConfigPolicy(payload);
  $("config-path").textContent = payload.exists ? payload.path : `${payload.path} (new)`;
  void refreshModelKeyStatus().catch((error) => setModelKeyStatus(error.message, "error"));
  return payload;
}

async function saveConfigFile() {
  const payload = await api(
    "/api/config-file",
    form({
      scope: configScope(),
      repo: configRepo(),
      content: $("config-editor").value,
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
  $("session-list").innerHTML = "";
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

function renderSessionList(sessions = []) {
  const list = $("session-list");
  list.innerHTML = "";
  if (!sessions.length) {
    const empty = document.createElement("p");
    empty.className = "sidebar-empty";
    empty.textContent = "No sessions yet";
    list.append(empty);
    return;
  }
  sessions.forEach((session) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "session-item";
    button.dataset.sessionId = session.id;
    if (session.id === currentSessionId) {
      button.classList.add("active");
    }
    button.textContent = session.title;
    button.title = session.title;
    button.addEventListener("click", async () => {
      try {
        $("session-select").value = session.id;
        await loadSession(session.id);
        renderSessionList(sessions);
      } catch (error) {
        toast(error.message);
      }
    });
    list.append(button);
  });
}

function syncSessionListActive() {
  document.querySelectorAll(".session-item").forEach((button) => {
    button.classList.toggle("active", button.dataset.sessionId === currentSessionId);
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

function createPatchPreview(payload, patchRepo) {
  const state = {
    patchId: payload.patchId,
    files: (payload.files || []).map((file) => {
      const stats = diffStats(file.diff);
      return {
        path: file.path,
        status: file.status,
        diff: file.diff,
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

      cardHeader.append(label, meta);
      card.append(cardHeader, renderColoredDiff(file.diff));
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
      const result = await api(
        "/api/apply-patch",
        form({ repo: patchRepo, patch_id: state.patchId, paths: paths.join("\n") }),
      );
      const applied = result.appliedFiles || [];
      markFiles(applied, "applied");
      toast(`Applied ${applied.length} file(s)`);
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

async function loadSessions(preferredSessionId = "", reloadSelected = false) {
  const repoPath = repo();
  if (!repoPath) {
    clearSessionList();
    return;
  }
  const payload = await api(`/api/sessions?repo=${encodeURIComponent(repoPath)}`);
  const select = $("session-select");
  const storedSessionId = localStorage.getItem(lastSessionStorageKey(repoPath)) || "";
  const selectedSessionId = preferredSessionId || currentSessionId || storedSessionId;
  select.innerHTML = '<option value="">New session</option>';
  payload.sessions.forEach((session) => {
    const option = document.createElement("option");
    option.value = session.id;
    option.textContent = session.title;
    select.append(option);
  });
  currentSessionId = payload.sessions.some((session) => session.id === selectedSessionId)
    ? selectedSessionId
    : "";
  select.value = currentSessionId;
  renderSessionList(payload.sessions);
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

document.querySelectorAll(".tab").forEach((button) => {
  button.addEventListener("click", () => {
    ensureInspectorVisible();
    document.querySelectorAll(".tab").forEach((tab) => tab.classList.remove("active"));
    document.querySelectorAll(".view").forEach((view) => view.classList.remove("active"));
    button.classList.add("active");
    document.getElementById(button.dataset.tab).classList.add("active");
  });
});

$("repo").addEventListener("change", () => {
  const value = repo();
  if (value) {
    setRepository(value);
  }
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

$("status-btn").addEventListener("click", async () => {
  try {
    const payload = await api(`/api/git-status?repo=${encodeURIComponent(requireRepo())}`);
    setRepoState(payload.clean ? "Clean workspace" : `${payload.files.length} changed paths`);
    appendChatMessage("system", `\`\`\`json\n${JSON.stringify(payload, null, 2)}\n\`\`\``);
  } catch (error) {
    toast(error.message);
  }
});

$("config-btn").addEventListener("click", async () => {
  try {
    ensureInspectorVisible();
    document.querySelector('[data-tab="settings"]').click();
    await loadConfigFile();
  } catch (error) {
    toast(error.message);
  }
});

$("open-vscode-btn").addEventListener("click", async () => {
  try {
    const payload = await api("/api/open-vscode", form({ repo: requireRepo() }));
    toast(`Opened ${payload.path}`);
  } catch (error) {
    toast(error.message);
  }
});

$("session-select").addEventListener("change", async () => {
  try {
    await loadSession($("session-select").value);
  } catch (error) {
    toast(error.message);
  }
});

$("new-session-btn").addEventListener("click", () => {
  currentSessionId = "";
  $("session-select").value = "";
  localStorage.removeItem(lastSessionStorageKey());
  syncSessionListActive();
  loadPinnedContextFiles("");
  clearChat();
  $("chat-prompt").focus();
});

$("rename-session-btn").addEventListener("click", async () => {
  try {
    if (!currentSessionId) throw new Error("No session selected");
    const currentTitle = $("session-select").selectedOptions[0]?.textContent || "";
    const title = window.prompt("Session name", currentTitle);
    if (!title || !title.trim()) return;
    const payload = await api(
      "/api/session-rename",
      form({ session_id: currentSessionId, title: title.trim() }),
    );
    await loadSessions(payload.session.id, false);
    toast("Session renamed");
  } catch (error) {
    toast(error.message);
  }
});

$("delete-session-btn").addEventListener("click", async () => {
  try {
    if (!currentSessionId) throw new Error("No session selected");
    if (!window.confirm("Delete this session?")) return;
    await api("/api/session-delete", form({ session_id: currentSessionId }));
    localStorage.removeItem(lastSessionStorageKey());
    currentSessionId = "";
    clearChat();
    await loadSessions("", false);
    toast("Session deleted");
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
        repo: requireRepo(),
        prompt,
        session_id: currentSessionId,
        context_files: pinnedContextFiles.join("\n"),
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

$("propose-command-btn").addEventListener("click", async () => {
  try {
    const payload = await api(
      "/api/propose-command",
      form({ repo: requireRepo(), command: $("command-input").value }),
    );
    currentCommandProposalId = payload.proposalId;
    $("command-output").textContent = payload.prompt;
  } catch (error) {
    toast(error.message);
  }
});

$("run-command-btn").addEventListener("click", async () => {
  try {
    if (!currentCommandProposalId) throw new Error("No command proposal selected");
    const payload = await api(
      "/api/run-command",
      form({ repo: requireRepo(), proposal_id: currentCommandProposalId }),
    );
    $("command-output").textContent = JSON.stringify(payload, null, 2);
  } catch (error) {
    toast(error.message);
  }
});

$("reject-command-btn").addEventListener("click", async () => {
  try {
    if (!currentCommandProposalId) throw new Error("No command proposal selected");
    const payload = await api(
      "/api/reject-command",
      form({ repo: requireRepo(), proposal_id: currentCommandProposalId }),
    );
    $("command-output").textContent = JSON.stringify(payload, null, 2);
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

$("inspector-toggle-btn").addEventListener("click", () => {
  setInspectorCollapsed(!document.body.classList.contains("inspector-collapsed"));
});

$("ask-btn").disabled = true;
setChatStatus("Starting", "running");
renderPinnedContextFiles();

bootstrapPromise = startBootstrap();
