const $ = (id) => document.getElementById(id);

let currentPatchId = "";
let currentCommandProposalId = "";

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
  const response = await fetch(path, options);
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

function renderResults(results) {
  $("search-results").innerHTML = results
    .map(
      (item) => `
        <div class="result">
          <strong>${escapeHtml(item.path)}</strong>
          <span>${escapeHtml(item.language)} · ${item.score}</span>
          <p>${escapeHtml(item.snippet.slice(0, 180))}</p>
        </div>
      `,
    )
    .join("");
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function setRepoState(message) {
  $("repo-state").textContent = message;
}

function tauriInvoke() {
  return window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke;
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

$("pick-folder-btn").addEventListener("click", async () => {
  try {
    const invoke = tauriInvoke();
    if (!invoke) throw new Error("Folder picker is available in the desktop app");
    const selected = await invoke("pick_working_folder");
    if (selected) {
      $("repo").value = selected;
      setRepoState("Repository selected");
      $("search-results").innerHTML = "";
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

$("search-btn").addEventListener("click", async () => {
  try {
    const q = $("search-query").value.trim();
    const payload = await api(
      `/api/search?repo=${encodeURIComponent(requireRepo())}&q=${encodeURIComponent(q)}`,
    );
    renderResults(payload.results);
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
    if (payload.defaultRepo) {
      $("repo").value = payload.defaultRepo;
      setRepoState("Repository selected");
    }
  })
  .catch(() => {});
