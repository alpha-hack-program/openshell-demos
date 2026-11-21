// SPDX-License-Identifier: Apache-2.0
//
// parrot-tauri's whole frontend: no build step, no framework. The backend
// (src/main.rs) does all JSONL classification via parrot-core and ships
// already-classified `AgentEvent`s as JSON (see parrot-core's jsonl.rs doc
// comment) -- this file only maps those onto CSS classes/markup, mirroring
// parrot-gpui's `render_log_entry`/`unknown_label`, plus a small
// reimplementation of parrot-core's markdown.rs rules (same restricted
// subset: headers, bullets, **bold**/__bold__, `code`) since that pass
// isn't shipped over IPC.

const { invoke, Channel } = window.__TAURI__.core;

const SPLASH_DURATION_MS = 1300; // matches parrot-tui/parrot-gpui's SPLASH_DURATION

let turnRunning = false;
let splashDismissed = false;

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function scanInline(line) {
  let out = "";
  let i = 0;
  while (i < line.length) {
    if (line.startsWith("**", i) || line.startsWith("__", i)) {
      const marker = line.slice(i, i + 2);
      const close = line.indexOf(marker, i + 2);
      if (close !== -1) {
        out += `<strong>${line.slice(i + 2, close)}</strong>`;
        i = close + 2;
        continue;
      }
    } else if (line[i] === "`") {
      const close = line.indexOf("`", i + 1);
      if (close !== -1) {
        out += `<code>${line.slice(i + 1, close)}</code>`;
        i = close + 1;
        continue;
      }
    }
    out += line[i];
    i += 1;
  }
  return out;
}

function renderMarkdownLine(line) {
  const header = /^(\s*)(#{1,6}) (.*)$/.exec(line);
  if (header) {
    const [, indent, , rest] = header;
    return `${indent}<span class="header-run">${scanInline(rest)}</span>`;
  }
  const bullet = /^(\s*)[-*+] (.*)$/.exec(line);
  if (bullet) {
    const [, indent, rest] = bullet;
    return `${indent}• ${scanInline(rest)}`;
  }
  return scanInline(line);
}

function renderMarkdown(escapedText) {
  return escapedText.split("\n").map(renderMarkdownLine).join("\n");
}

const MAX_TOOL_RESULT_TEXT_LEN = 2000; // mirrors jsonl.rs

function textBlocks(blocks) {
  const texts = blocks
    .map((b) => (b && typeof b.text === "string" ? b.text : null))
    .filter((t) => t !== null);
  return texts.length ? texts.join("\n") : null;
}

// Mirrors parrot-core's jsonl.rs::tool_result_text.
function toolResultText(value) {
  let text;
  if (typeof value === "string") {
    text = value;
  } else if (Array.isArray(value)) {
    text = textBlocks(value) ?? JSON.stringify(value);
  } else if (value && typeof value === "object") {
    if (typeof value.message === "string") {
      text = value.message;
    } else if (Array.isArray(value.content)) {
      text = textBlocks(value.content) ?? JSON.stringify(value);
    } else {
      text = JSON.stringify(value);
    }
  } else if (value === null || value === undefined) {
    text = "";
  } else {
    text = String(value);
  }

  const chars = [...text];
  if (chars.length > MAX_TOOL_RESULT_TEXT_LEN) {
    return `${chars.slice(0, MAX_TOOL_RESULT_TEXT_LEN).join("")}… (truncated)`;
  }
  return text;
}

// Mirrors parrot-gpui's app.rs::unknown_label.
function unknownLabel(value) {
  const ty = (value && value.type) || "?";
  const nested = value && (value.subtype || (value.item && value.item.type));
  return nested ? `${ty}/${nested}` : ty;
}

// Mirrors parrot-gpui's app.rs::render_log_entry.
function classifyEntry(entry) {
  const event = entry.event;
  const data = event.data;
  switch (event.type) {
    case "UserPrompt":
      return { cls: "user-prompt", text: `> ${data}` };
    case "AssistantText":
      return { cls: "assistant-text", text: `» ${data}`, markdown: true };
    case "ToolCall":
      return { cls: "tool-call", text: `→ tool call: ${data.name}` };
    case "ToolResult": {
      if (data.is_error) {
        const text = toolResultText(data.content);
        return {
          cls: "tool-result-error",
          text: text ? `✗ tool result: error: ${text}` : "✗ tool result: error",
        };
      }
      return { cls: "tool-result-ok", text: "✓ tool result" };
    }
    case "Thinking":
      return { cls: "thinking", text: `… ${data}` };
    case "ThinkingTokens":
      return { cls: "thinking", text: `… thinking… (${data} tokens)` };
    case "Result":
      return { cls: "result", text: "● final result" };
    case "Error":
      return { cls: "error", text: `! ${data}` };
    case "Info":
      return { cls: "info", text: `ℹ ${data}` };
    case "Unknown":
      return { cls: "unknown", text: `· unrecognized event: ${unknownLabel(data)}` };
    case "Unparseable":
      return { cls: "info", text: `  ${data}` };
    default:
      return { cls: "unknown", text: JSON.stringify(event) };
  }
}

function renderLog(log) {
  const container = document.getElementById("log");
  container.innerHTML = log
    .map((entry) => {
      const { cls, text, markdown } = classifyEntry(entry);
      const html = markdown ? renderMarkdown(escapeHtml(text)) : escapeHtml(text);
      return `<div class="log-entry ${cls}">${html}</div>`;
    })
    .join("");
  container.scrollTop = container.scrollHeight;
}

function renderStatus(status) {
  const el = document.getElementById("header-status");
  switch (status.type) {
    case "Running":
      el.textContent = "running";
      el.className = "status status-running";
      break;
    case "Completed":
      if (status.data.exit_code === 0) {
        el.textContent = "completed";
        el.className = "status status-completed";
      } else {
        el.textContent = `completed (exit ${status.data.exit_code})`;
        el.className = "status status-completed-error";
      }
      break;
    case "Crashed":
      el.textContent = "crashed";
      el.className = "status status-crashed";
      break;
    default:
      el.textContent = "";
      el.className = "status";
  }
}

// Maps parrot-tauri's `sandbox_phase_str` (src/main.rs) onto this file's
// existing turn-status color classes (style.css) -- reused rather than
// adding a parallel set of phase-specific colors.
function sandboxPhaseClass(phase) {
  switch (phase) {
    case "ready":
    case "completed":
      return "status-completed";
    case "error":
    case "stopped":
    case "deleting":
      return "status-crashed";
    case "provisioning":
    case "starting":
    case "stopping":
    case "suspending":
      return "status-running";
    default:
      return "";
  }
}

function applyTheme(theme) {
  const apply = (mode) => {
    document.body.classList.toggle("theme-light", mode === "light");
    document.body.classList.toggle("theme-dark", mode !== "light");
  };
  if (theme === "system") {
    const query = window.matchMedia("(prefers-color-scheme: light)");
    apply(query.matches ? "light" : "dark");
    query.addEventListener("change", (e) => apply(e.matches ? "light" : "dark"));
  } else {
    apply(theme);
  }
}

function dismissSplash() {
  if (splashDismissed) return;
  splashDismissed = true;
  document.getElementById("splash").classList.add("hidden");
  document.getElementById("app").classList.remove("hidden");
}

async function submit(promptText) {
  const prompt = promptText.trim();
  if (!prompt || turnRunning) return;

  const input = document.getElementById("composer-input");
  input.value = "";
  input.style.height = "auto";
  turnRunning = true;
  document.getElementById("composer-hint").textContent = "running…";

  const channel = new Channel();
  channel.onmessage = (snapshot) => {
    renderLog(snapshot.log);
    renderStatus(snapshot.status);
  };

  try {
    await invoke("submit_turn", { prompt, onEvent: channel });
  } catch (err) {
    console.error("parrot-tauri: submit_turn failed", err);
  } finally {
    turnRunning = false;
    document.getElementById("composer-hint").textContent = "";
  }
}

async function main() {
  const greeting = await invoke("greeting");
  applyTheme(greeting.theme);

  document.getElementById("splash-greeting").textContent = greeting.identity_greeting;
  document.getElementById("splash-gateway").textContent =
    `Gateway: ${greeting.gateway_name} (${greeting.gateway_endpoint})`;
  document.getElementById("splash-sandbox").textContent =
    `Sandbox: ${greeting.sandbox} (${greeting.sandbox_phase})`;

  document.getElementById("header-sandbox").textContent =
    `parrot — sandbox ${greeting.sandbox} — `;
  const phaseEl = document.getElementById("header-sandbox-phase");
  phaseEl.textContent = `${greeting.sandbox_phase} — `;
  phaseEl.className = `status ${sandboxPhaseClass(greeting.sandbox_phase)}`;

  const input = document.getElementById("composer-input");
  input.addEventListener("keydown", (event) => {
    dismissSplash();
    if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      submit(input.value);
    }
  });
  input.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = `${input.scrollHeight}px`;
  });

  if (greeting.no_splash) {
    dismissSplash();
  } else {
    setTimeout(dismissSplash, SPLASH_DURATION_MS);
  }

  // Runs concurrently with the splash timer, same as parrot-gpui's
  // `--prompt` handling -- the turn is already streaming by the time the
  // splash dismisses.
  if (greeting.prompt) {
    submit(greeting.prompt);
  }

  input.focus();
}

main().catch((err) => {
  console.error("parrot-tauri: failed to initialize", err);
  const greetingEl = document.getElementById("splash-greeting");
  greetingEl.textContent = `Error: ${err}`;
});
