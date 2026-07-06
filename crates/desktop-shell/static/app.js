const $ = (id) => document.getElementById(id);

let currentPatchId = "";
let currentCommandProposalId = "";

const localApiOrigin = "http://127.0.0.1:4765";
const lastRepoStorageKey = "damaian:lastRepository";

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
  const response = await fetch(apiUrl(path), options);
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

function setRepository(value, persist = true) {
  $("repo").value = value;
  setRepoState(value ? "Repository selected" : "No repository selected");
  if (persist && value) {
    localStorage.setItem(lastRepoStorageKey, value);
  }
}

function tauriDialogOpen() {
  return window.__TAURI__?.dialog?.open;
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

document.querySelectorAll(".tab").forEach((button) => {
  button.addEventListener("click", () => {
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
    $("chat-output").textContent = JSON.stringify(payload, null, 2);
  } catch (error) {
    toast(error.message);
  }
});

$("config-btn").addEventListener("click", async () => {
  try {
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

$("ask-btn").addEventListener("click", async () => {
  try {
    const payload = await api(
      "/api/ask",
      form({
        repo: requireRepo(),
        prompt: $("chat-prompt").value,
        mock_response: $("chat-mock").value,
      }),
    );
    $("chat-output").textContent = `${payload.response}\n\nContext:\n${payload.contextFiles.join("\n")}`;
  } catch (error) {
    toast(error.message);
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
    $("diff-output").textContent = payload.diff;
    toast(`Patch ${payload.patchId}`);
  } catch (error) {
    toast(error.message);
  }
});

$("apply-patch-btn").addEventListener("click", async () => {
  try {
    if (!currentPatchId) throw new Error("No patch selected");
    const payload = await api(
      "/api/apply-patch",
      form({ repo: requireRepo(), patch_id: currentPatchId }),
    );
    $("diff-output").textContent = JSON.stringify(payload, null, 2);
  } catch (error) {
    toast(error.message);
  }
});

$("reject-patch-btn").addEventListener("click", async () => {
  try {
    if (!currentPatchId) throw new Error("No patch selected");
    const payload = await api(
      "/api/reject-patch",
      form({ repo: requireRepo(), patch_id: currentPatchId }),
    );
    $("diff-output").textContent = JSON.stringify(payload, null, 2);
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

api("/api/bootstrap")
  .then((payload) => {
    const lastRepo = localStorage.getItem(lastRepoStorageKey);
    if (lastRepo) {
      setRepository(lastRepo, false);
    } else if (payload.defaultRepo) {
      setRepository(payload.defaultRepo, false);
    }
  })
  .catch(() => {});
