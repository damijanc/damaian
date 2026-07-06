const $ = (id) => document.getElementById(id);

let currentPatchId = "";
let currentPatchFiles = [];
let currentCommandProposalId = "";
let currentSessionId = "";
let apiToken = "";

const localApiOrigin = "http://127.0.0.1:4765";
const apiTokenHeader = "x-damaian-api-token";
const lastRepoStorageKey = "damaian:lastRepository";
const inspectorCollapsedStorageKey = "damaian:inspectorCollapsed";

function repo() {
  return $("repo").value.trim();
}

function toast(message) {
  const el = $("toast");
  el.textContent = message;
  el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 2600);
}

async function api(path, options = {}) {
  const response = await fetch(apiUrl(path), withApiToken(path, options));
  const text = await response.text();
  let payload;
  try {
    payload = JSON.parse(text);
  } catch {
    payload = { error: text };
  }
  if (!response.ok || payload.error) {
    throw new Error(payload.error || response.statusText);
  }
  return payload;
}

function withApiToken(path, options = {}) {
  const next = { ...options };
  const headers = new Headers(next.headers || {});
  if (path.startsWith("/api/") && path !== "/api/bootstrap") {
    if (!apiToken) throw new Error("Desktop API is not initialized");
    headers.set(apiTokenHeader, apiToken);
  }
  next.headers = headers;
  return next;
}

function apiUrl(path) {
  if (!path.startsWith("/api/")) return path;
  if (window.location.origin === localApiOrigin) return path;
  return `${localApiOrigin}${path}`;
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

function setRepository(value, persist = true) {
  $("repo").value = value;
  setRepoState(value ? "Repository selected" : "No repository selected");
  if (persist && value) {
    localStorage.setItem(lastRepoStorageKey, value);
  }
  currentSessionId = "";
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
  return $("config-scope").value;
}

function configRepo() {
  return configScope() === "repo" ? requireRepo() : repo();
}

async function loadConfigFile() {
  const payload = await api(
    `/api/config-file?scope=${encodeURIComponent(configScope())}&repo=${encodeURIComponent(
      configRepo(),
    )}`,
  );
  $("config-editor").value = payload.content;
  $("config-output").textContent = payload.effectivePolicy;
  $("config-path").textContent = payload.exists ? payload.path : `${payload.path} (new)`;
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
  $("config-output").textContent = payload.effectivePolicy;
  $("config-path").textContent = payload.path;
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

function renderPatchFiles() {
  const container = $("diff-output");
  container.innerHTML = "";
  if (!currentPatchFiles.length) {
    const empty = document.createElement("p");
    empty.className = "empty-state";
    empty.textContent = "No patch preview loaded.";
    container.append(empty);
    return;
  }

  currentPatchFiles.forEach((file) => {
    const card = document.createElement("article");
    card.className = `diff-card ${file.state}`;

    const header = document.createElement("div");
    header.className = "diff-card-header";

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

    const meta = document.createElement("span");
    meta.className = "diff-state";
    meta.textContent = file.state === "pending" ? file.status : file.state;

    const pre = document.createElement("pre");
    pre.className = "diff-pre";
    pre.textContent = file.diff;

    header.append(label, meta);
    card.append(header, pre);
    container.append(card);
  });
}

function setPatchFiles(files = []) {
  currentPatchFiles = files.map((file) => ({
    path: file.path,
    status: file.status,
    diff: file.diff,
    selected: true,
    state: "pending",
  }));
  renderPatchFiles();
}

function selectedPendingPatchPaths() {
  return currentPatchFiles
    .filter((file) => file.state === "pending" && file.selected)
    .map((file) => file.path);
}

function markPatchFiles(paths, state) {
  currentPatchFiles.forEach((file) => {
    if (paths.includes(file.path)) {
      file.state = state;
      file.selected = false;
    }
  });
  renderPatchFiles();
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
    clearChat();
    return;
  }
  const payload = await api(`/api/session?session_id=${encodeURIComponent(sessionId)}`);
  currentSessionId = payload.session.id;
  localStorage.setItem(lastSessionStorageKey(), currentSessionId);
  $("session-select").value = currentSessionId;
  syncSessionListActive();
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

$("ask-btn").addEventListener("click", async () => {
  const button = $("ask-btn");
  let streamError = null;
  try {
    const prompt = $("chat-prompt").value.trim();
    if (!prompt) throw new Error("Prompt is required");
    appendChatMessage("user", prompt);
    const assistantMessage = appendChatMessage("assistant", "");
    let assistantText = "";
    button.disabled = true;
    setChatStatus("Thinking", "running");

    await streamChatRequest(
      {
        repo: requireRepo(),
        prompt,
        session_id: currentSessionId,
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
    button.disabled = false;
  }
});

$("propose-edit-btn").addEventListener("click", async () => {
  try {
    const payload = await api(
      "/api/propose-edit",
      form({
        repo: requireRepo(),
        prompt: $("edit-prompt").value,
        model_output: $("edit-model-output").value,
      }),
    );
    currentPatchId = payload.patchId;
    setPatchFiles(payload.files || []);
    toast(`Patch ${payload.patchId}`);
  } catch (error) {
    toast(error.message);
  }
});

$("apply-patch-btn").addEventListener("click", async () => {
  try {
    if (!currentPatchId) throw new Error("No patch selected");
    const paths = selectedPendingPatchPaths();
    if (!paths.length) throw new Error("No pending patch files selected");
    const payload = await api(
      "/api/apply-patch",
      form({ repo: requireRepo(), patch_id: currentPatchId, paths: paths.join("\n") }),
    );
    markPatchFiles(payload.appliedFiles || [], "applied");
    toast(`Applied ${payload.appliedFiles.length} file(s)`);
  } catch (error) {
    toast(error.message);
  }
});

$("reject-patch-btn").addEventListener("click", async () => {
  try {
    if (!currentPatchId) throw new Error("No patch selected");
    const paths = selectedPendingPatchPaths();
    if (!paths.length) throw new Error("No pending patch files selected");
    const payload = await api(
      "/api/reject-patch-files",
      form({ repo: requireRepo(), patch_id: currentPatchId, paths: paths.join("\n") }),
    );
    markPatchFiles(payload.rejectedFiles || [], "rejected");
    toast(`Rejected ${payload.rejectedFiles.length} file(s)`);
  } catch (error) {
    toast(error.message);
  }
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

$("inspector-toggle-btn").addEventListener("click", () => {
  setInspectorCollapsed(!document.body.classList.contains("inspector-collapsed"));
});

api("/api/bootstrap")
  .then((payload) => {
    if (!payload.apiToken) throw new Error("Desktop API token missing from bootstrap");
    apiToken = payload.apiToken;
    setInspectorCollapsed(localStorage.getItem(inspectorCollapsedStorageKey) === "true");
    const lastRepo = localStorage.getItem(lastRepoStorageKey);
    if (lastRepo) {
      setRepository(lastRepo, false);
    } else if (payload.defaultRepo) {
      setRepository(payload.defaultRepo, false);
    } else {
      clearSessionList();
      clearChat();
    }
  })
  .catch(() => {});
