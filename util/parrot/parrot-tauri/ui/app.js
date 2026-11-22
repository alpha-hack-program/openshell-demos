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
//
// Three mutually exclusive main-column views: chat (sandbox-exec'd agent
// CLI, unchanged architecture), playground (direct tools/call against a
// schema-generated form), and brief (src/meeting_brief.rs's deterministic,
// no-LLM report for the next meeting). The evidence panel's content
// depends on which one is active.

import { renderToolForm } from "./schema-form.js?v=4";

const { invoke, Channel } = window.__TAURI__.core;

const SPLASH_DURATION_MS = 1300; // matches parrot-tui/parrot-gpui's SPLASH_DURATION

let turnRunning = false;
let splashDismissed = false;
let activeTool = null;
let activeForm = null;
let cachedBrief = null;
let briefRequested = false; // true once "Generate brief" has been clicked at least once

// The chat transcript is a merge of two independent sources: the backend's
// own cumulative `SessionState.log` (real sandbox-exec'd turns) and
// locally-synthesized entries: "brief" (src/meeting_brief.rs's output) and
// "snapshot" (src/client_snapshot.rs's output) -- neither is ever sent
// through the sandbox at all, see generateBrief()/generateClientSnapshot()
// below. Each synthetic entry records how many backend log entries existed
// when it was created, so re-merging after a later real turn still places
// it correctly instead of always jumping to the end.
let lastBackendLog = [];
const syntheticEntries = []; // { kind, afterCount, prompt, brief|snapshot, error, loading }

// Escapes quotes too, not just &/</> -- this gets used inside HTML
// attribute values (e.g. the suitability verdict's title="..."), where an
// unescaped literal " in real tool-returned text would close the
// attribute early and corrupt everything rendered after it.
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
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
      return { cls: "user-prompt", text: `${data}` };
    case "AssistantText":
      return { cls: "assistant-text", text: `${data}`, markdown: true };
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

// Interleaves `syntheticEntries` into `lastBackendLog` at each one's
// recorded `afterCount` position -- see the module-level doc comment on
// `syntheticEntries` for why this is needed instead of just appending.
function mergedTimeline() {
  const merged = [];
  const synthByPosition = [...syntheticEntries].sort((a, b) => a.afterCount - b.afterCount);
  let si = 0;
  for (let i = 0; i <= lastBackendLog.length; i++) {
    while (si < synthByPosition.length && synthByPosition[si].afterCount === i) {
      merged.push({ kind: synthByPosition[si].kind, data: synthByPosition[si] });
      si++;
    }
    if (i < lastBackendLog.length) {
      merged.push({ kind: "agent", data: lastBackendLog[i] });
    }
  }
  return merged;
}

// Agent event types that collapse into one "N steps" disclosure between
// the real content (UserPrompt/AssistantText/Error) around them -- mirrors
// Claude's own UI, where tool calls/thinking are a small expandable
// summary rather than permanent inline noise. UserPrompt/AssistantText/
// Error always get their own visible bubble.
const COLLAPSIBLE_AGENT_EVENT_TYPES = new Set([
  "ToolCall",
  "ToolResult",
  "Thinking",
  "ThinkingTokens",
  "Result",
  "Info",
  "Unknown",
  "Unparseable",
]);

// Turns the flat merged timeline into render units: agent/brief/snapshot
// entries pass through, but runs of collapsible agent events between two
// pieces of real content become one "agent-group" unit. Each unit also
// gets its own `evidence` rows precomputed here, since that's exactly the
// scope the IntersectionObserver-driven evidence panel (see
// observeTimelineUnits() below) should show once that unit scrolls into
// view -- an assistant-text bubble's evidence is the tool calls that
// immediately preceded it, not the whole session's.
function buildTimelineUnits(timeline) {
  const units = [];
  let pendingGroup = [];

  function flushGroup() {
    if (pendingGroup.length === 0) return [];
    const evidence = evidenceRowsFromLog(pendingGroup);
    units.push({ kind: "agent-group", items: pendingGroup, evidence });
    pendingGroup = [];
    return evidence;
  }

  for (const item of timeline) {
    if (item.kind !== "agent") {
      flushGroup();
      units.push({ kind: item.kind, data: item.data, evidence: evidenceForEntry(item) });
      continue;
    }
    const type = item.data.event.type;
    if (type === "UserPrompt") {
      flushGroup();
      units.push({ kind: "user-prompt", data: item.data, evidence: [] });
    } else if (type === "AssistantText") {
      const evidence = flushGroup();
      units.push({ kind: "assistant-text", data: item.data, evidence });
    } else if (type === "Error") {
      flushGroup();
      units.push({ kind: "agent-error", data: item.data, evidence: [] });
    } else if (COLLAPSIBLE_AGENT_EVENT_TYPES.has(type)) {
      pendingGroup.push(item.data);
    } else {
      flushGroup();
      units.push({ kind: "agent-error", data: item.data, evidence: [] });
    }
  }
  flushGroup();
  return units;
}

function evidenceForEntry(item) {
  if (item.kind === "brief") return item.data.brief ? evidenceRowsFromBrief(item.data.brief) : [];
  if (item.kind === "snapshot") return item.data.snapshot ? item.data.snapshot.evidence || [] : [];
  return [];
}

function renderAgentGroupHtml(items, openByDefault) {
  const toolCalls = items.filter((i) => i.event.type === "ToolCall").length;
  const isThinking = items.some((i) => i.event.type === "Thinking" || i.event.type === "ThinkingTokens");
  const hasError = items.some((i) => i.event.type === "ToolResult" && i.event.data.is_error);
  const parts = [];
  if (isThinking) parts.push("Thinking");
  if (toolCalls) parts.push(`${toolCalls} tool call${toolCalls === 1 ? "" : "s"}`);
  const summaryText = parts.length ? parts.join(" · ") : `${items.length} step${items.length === 1 ? "" : "s"}`;
  const bodyHtml = items
    .map((entry) => {
      const { cls, text } = classifyEntry(entry);
      return `<div class="log-entry ${cls}">${escapeHtml(text)}</div>`;
    })
    .join("");
  return `<details class="turn-details${hasError ? " turn-details-error" : ""}"${openByDefault ? " open" : ""}>
    <summary>${escapeHtml(summaryText)}</summary>
    ${bodyHtml}
  </details>`;
}

function renderUnitHtml(unit, isLast) {
  switch (unit.kind) {
    case "user-prompt":
    case "agent-error": {
      const { cls, text } = classifyEntry(unit.data);
      return `<div class="log-entry ${cls}">${escapeHtml(text)}</div>`;
    }
    case "assistant-text": {
      const { cls, text, markdown } = classifyEntry(unit.data);
      const html = markdown ? renderMarkdown(escapeHtml(text)) : escapeHtml(text);
      return `<div class="log-entry ${cls}">${html}</div>`;
    }
    case "agent-group":
      // Open while it's the newest thing in the log and the turn is still
      // running, so progress is visible live -- collapses on the next
      // render once the turn finishes (see requirement: "otherwise you
      // don't know if something is going on").
      return renderAgentGroupHtml(unit.items, isLast && turnRunning);
    case "snapshot":
      return renderSnapshotEntryHtml(unit.data);
    case "brief":
      return renderBriefEntryHtml(unit.data);
    default:
      return "";
  }
}

let timelineUnitsEvidence = [];
let timelineObserver = null;
const visibleUnitRatios = new Map();

function updateEvidenceForVisibleUnit() {
  if (visibleUnitRatios.size === 0) return;
  let bestIndex = null;
  let bestRatio = -1;
  for (const [index, ratio] of visibleUnitRatios) {
    // Ties go to the most recent (highest-index) unit, not whichever the
    // Map happened to report first -- matters when a short thread fits
    // entirely in the viewport and everything is simultaneously at
    // ratio 1, where "most visible" alone can't distinguish them and
    // defaulting to the newest message is the more useful behavior.
    if (ratio > bestRatio || (ratio === bestRatio && index > bestIndex)) {
      bestRatio = ratio;
      bestIndex = index;
    }
  }
  if (bestIndex === null) return;
  renderEvidenceRows(timelineUnitsEvidence[bestIndex] || []);
}

// Re-attached after every renderChatLog() rebuild, since replacing
// #log-entries' innerHTML destroys the previously observed elements.
// IntersectionObserver fires with each element's *current* state as soon
// as observe() is called, so the evidence panel stays in sync with
// scroll position across re-renders too, not just live scrolling.
function observeTimelineUnits() {
  if (timelineObserver) timelineObserver.disconnect();
  visibleUnitRatios.clear();
  // The actual scrolling container is #chat-view (`.view`'s
  // `overflow-y: auto`), not #log itself -- #log just grows to fit its
  // content (see its own CSS comment on why it has no min-height/overflow
  // of its own).
  const root = document.getElementById("chat-view");
  timelineObserver = new IntersectionObserver(
    (observerEntries) => {
      for (const e of observerEntries) {
        const index = Number(e.target.dataset.unitIndex);
        if (e.isIntersecting) {
          visibleUnitRatios.set(index, e.intersectionRatio);
        } else {
          visibleUnitRatios.delete(index);
        }
      }
      updateEvidenceForVisibleUnit();
    },
    { root, threshold: [0, 0.1, 0.25, 0.5, 0.75, 1] },
  );
  document.querySelectorAll(".timeline-unit").forEach((el) => timelineObserver.observe(el));
}

function renderChatLog() {
  const scroller = document.getElementById("chat-view");
  const entries = document.getElementById("log-entries");
  // Sticky auto-scroll: only follow new content if the user was already
  // at (or very near) the bottom before this render. Otherwise force-
  // scrolling on every update (this fires on every streamed line) would
  // yank anyone trying to scroll up and read an earlier message/evidence
  // straight back down.
  const wasNearBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 48;

  const timeline = mergedTimeline();
  document.getElementById("log-empty-state").classList.toggle("hidden", timeline.length > 0);

  const units = buildTimelineUnits(timeline);
  timelineUnitsEvidence = units.map((u) => u.evidence);
  entries.innerHTML = units
    .map(
      (unit, i) =>
        `<div class="timeline-unit" data-unit-index="${i}">${renderUnitHtml(unit, i === units.length - 1)}</div>`,
    )
    .join("");
  observeTimelineUnits();

  if (wasNearBottom) {
    scroller.scrollTop = scroller.scrollHeight;
  }
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
    document.body.classList.toggle("theme-dark", mode === "dark");
    document.body.classList.toggle("theme-light", mode !== "dark");
  };
  if (theme === "system") {
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    apply(query.matches ? "dark" : "light");
    query.addEventListener("change", (e) => apply(e.matches ? "dark" : "light"));
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

function initialsFor(name) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  return (parts[0][0] + (parts[1]?.[0] || "")).toUpperCase();
}

// --- Evidence panel -----------------------------------------------------
//
// Generic renderer: both the chat view (derived from its own ToolCall/
// ToolResult AgentEvents) and the brief view (src/meeting_brief.rs's own
// `evidence` array, already numbered/sourced) feed it the same row shape.

function renderEvidenceRows(rows) {
  document.getElementById("evidence-count").textContent =
    rows.length === 1 ? "1 call" : `${rows.length} calls`;
  document.getElementById("evidence-list").innerHTML = rows
    .map(
      (row) => `<div class="evidence-row">
        <div class="evidence-n">${row.n}</div>
        <div class="evidence-body">
          <div class="evidence-top-row">
            <span class="evidence-tool">${escapeHtml(stripMcpPrefix(row.tool))}</span>
            ${row.time ? `<span class="evidence-time">${escapeHtml(row.time)}</span>` : ""}
          </div>
          <div class="evidence-detail">${escapeHtml(row.detail)}</div>
          <span class="evidence-source">${escapeHtml(row.source)}</span>
        </div>
      </div>`,
    )
    .join("");
}

function stripMcpPrefix(name) {
  return name.replace(/^mcp_/, "");
}

// Builds evidence rows from the chat transcript's own ToolCall/ToolResult
// events -- paired FIFO by order, since AgentEvent carries no call id (see
// mcp_client.rs's doc comment for the same reasoning applied there).
function evidenceRowsFromLog(log) {
  const rows = [];
  const pending = [];
  for (const entry of log) {
    const event = entry.event;
    if (event.type === "ToolCall") {
      pending.push({ name: event.data.name, input: event.data.input });
    } else if (event.type === "ToolResult") {
      const call = pending.shift();
      const name = call ? call.name : "(unknown tool)";
      const args = call && call.input && Object.keys(call.input).length ? JSON.stringify(call.input) : "";
      rows.push({
        n: rows.length + 1,
        tool: name,
        detail: event.data.is_error ? `error: ${toolResultText(event.data.content)}` : args || "ok",
        source: "Sandbox agent",
      });
    }
  }
  for (const call of pending) {
    rows.push({ n: rows.length + 1, tool: call.name, detail: "running…", source: "Sandbox agent" });
  }
  return rows;
}

function evidenceRowsFromBrief(brief) {
  return brief.evidence.map((c) => ({ n: c.n, tool: c.tool, detail: c.detail, source: c.source }));
}

// --- View switching -------------------------------------------------------

function clearNavActive() {
  document.getElementById("nav-chat").classList.remove("nav-item-active");
  document.querySelectorAll(".tool-item").forEach((el) => el.classList.remove("nav-item-active"));
}

function showChatView() {
  activeTool = null;
  activeForm = null;
  document.getElementById("playground-view").classList.add("hidden");
  document.getElementById("chat-view").classList.remove("hidden");
  document.getElementById("regulatory-card").classList.toggle("hidden", !cachedBrief?.regulatory);
  clearNavActive();
  document.getElementById("nav-chat").classList.add("nav-item-active");
  if (lastBackendLog.length || syntheticEntries.length) {
    renderEvidenceRows(cachedBrief ? evidenceRowsFromBrief(cachedBrief) : evidenceRowsFromLog(lastBackendLog));
  }
}

function showPlaygroundView(tool, toolButton) {
  activeTool = tool;
  document.getElementById("chat-view").classList.add("hidden");
  document.getElementById("playground-view").classList.remove("hidden");
  document.getElementById("regulatory-card").classList.add("hidden");
  clearNavActive();
  if (toolButton) toolButton.classList.add("nav-item-active");

  document.getElementById("playground-tool-name").textContent = tool.title || tool.name;
  document.getElementById("playground-tool-description").textContent = tool.description || "";
  document.getElementById("playground-result").innerHTML = "";
  activeForm = renderToolForm(document.getElementById("playground-form"), tool.inputSchema);
  renderEvidenceRows([]);
}


// --- MCP Gateway toolbar/playground ----------------------------------

function renderToolGroups(groups) {
  document.getElementById("tools-disclosure").classList.remove("hidden");
  const container = document.getElementById("tool-groups");
  container.innerHTML = "";
  for (const group of groups) {
    if (group.tools.length === 0) continue;
    const details = document.createElement("details");
    details.open = true;
    const summary = document.createElement("summary");
    summary.textContent = `${group.server || "(other)"} (${group.tools.length})`;
    details.appendChild(summary);
    for (const tool of group.tools) {
      const button = document.createElement("button");
      button.className = "nav-item tool-item";
      button.textContent = tool.title || tool.name;
      button.title = tool.description || "";
      button.addEventListener("click", () => showPlaygroundView(tool, button));
      details.appendChild(button);
    }
    container.appendChild(details);
  }
}

function renderToolResult(container, result) {
  container.innerHTML = "";
  if (result.isError) {
    const badge = document.createElement("div");
    badge.className = "playground-error";
    badge.textContent = "Tool reported an error:";
    container.appendChild(badge);
  }
  for (const item of result.content || []) {
    const block = document.createElement("pre");
    block.className = "playground-content-block";
    block.textContent = item.type === "text" ? item.text : JSON.stringify(item, null, 2);
    container.appendChild(block);
  }
  if (result.structuredContent) {
    const block = document.createElement("pre");
    block.className = "playground-content-block";
    block.textContent = JSON.stringify(result.structuredContent, null, 2);
    container.appendChild(block);
  }
  if (!result.content?.length && !result.structuredContent) {
    const block = document.createElement("div");
    block.className = "field-hint";
    block.textContent = "(empty result)";
    container.appendChild(block);
  }
}

async function runActiveTool() {
  if (!activeTool || !activeForm) return;
  const resultEl = document.getElementById("playground-result");
  const collected = activeForm.collect();
  if (!collected.ok) {
    resultEl.innerHTML = `<div class="playground-error">${escapeHtml(collected.error)}</div>`;
    return;
  }
  resultEl.innerHTML = '<div class="field-hint">Running…</div>';
  try {
    const result = await invoke("mcp_call_tool", {
      toolName: activeTool.name,
      arguments: collected.values,
    });
    renderToolResult(resultEl, result);
  } catch (err) {
    resultEl.innerHTML = `<div class="playground-error">${escapeHtml(String(err))}</div>`;
  }
}

// --- Today rail -----------------------------------------------------------

function riskBadgeClass(risk) {
  switch (risk) {
    case "low":
      return "badge-green";
    case "moderate":
      return "badge-amber";
    case "high":
      return "badge-red";
    default:
      return "badge-neutral";
  }
}

function renderToday(today) {
  document.getElementById("today-panel").classList.remove("hidden");
  document.getElementById("today-date").textContent =
    `Today · ${new Date().toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" })}`;
  refreshSavedBriefs();

  const nextMeeting = today.meetings[0];
  const card = document.getElementById("next-meeting-card");
  if (nextMeeting) {
    card.classList.remove("hidden");
    const name = nextMeeting.client_name || nextMeeting.client_id;
    const when = new Date(nextMeeting.datetime);
    const etaMs = when.getTime() - Date.now();
    const etaHours = Math.max(0, Math.round(etaMs / 3_600_000));
    document.getElementById("next-meeting-eta").textContent = `in ${etaHours} h`;
    document.getElementById("next-meeting-name").textContent = name;
    document.getElementById("next-meeting-when").textContent =
      `${when.toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short" })} · ${when.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })} · ${nextMeeting.id}`;
    const client = today.clients.find((c) => c.id === nextMeeting.client_id);
    const pills = [];
    if (client?.risk_profile) pills.push(`${client.risk_profile[0].toUpperCase()}${client.risk_profile.slice(1)} risk`);
    if (client?.sector_focus) pills.push(`${client.sector_focus[0].toUpperCase()}${client.sector_focus.slice(1)} focus`);
    document.getElementById("next-meeting-pills").innerHTML = pills
      .map((p) => `<span class="pill-navy">${escapeHtml(p)}</span>`)
      .join("");
  } else {
    card.classList.add("hidden");
  }

  document.getElementById("book-count").textContent =
    today.clients.length === 1 ? "1 client" : `${today.clients.length} clients`;
  document.getElementById("book-clients").innerHTML = today.clients
    .map((c) => {
      const badges = [
        `<span class="badge ${riskBadgeClass(c.risk_profile)}">${escapeHtml(c.risk_profile || "unknown")}</span>`,
        `<span class="badge ${c.kyc_status === "complete" ? "badge-green" : "badge-red"}">${escapeHtml(c.kyc_status || "unknown")}</span>`,
      ];
      badges.push(`<span class="badge ${c.pep_flag ? "badge-red" : "badge-neutral"}">${c.pep_flag ? "PEP" : "No PEP"}</span>`);
      return `<button class="book-client" type="button" data-client-name="${escapeHtml(c.name)}">
        <div class="avatar-circle">${escapeHtml(initialsFor(c.name))}</div>
        <div style="flex:1; min-width:0;">
          <div class="book-client-name">${escapeHtml(c.name)}</div>
          <div class="badge-row">${badges.join("")}</div>
        </div>
      </button>`;
    })
    .join("");
  document.querySelectorAll(".book-client").forEach((el) => {
    el.addEventListener("click", () => generateClientSnapshot(el.dataset.clientName));
  });
}

function renderAttention(brief) {
  const card = document.getElementById("attention-card");
  const items = [];
  if (brief.over_limit_sectors.length > 0) {
    items.push({
      warn: true,
      text: `${brief.client_name}'s ${brief.over_limit_sectors.join(", ")} weight is above the 25% sector limit. New products in that sector will fail suitability.`,
    });
  }
  if (brief.news.length > 0) {
    items.push({
      warn: false,
      text: `${brief.news.length} news item${brief.news.length === 1 ? "" : "s"} touch ${brief.client_name}'s holdings in the last 48 h.`,
    });
  }
  if (items.length === 0) {
    card.classList.add("hidden");
    return;
  }
  card.classList.remove("hidden");
  document.getElementById("attention-items").innerHTML = items
    .map((item) => `<div class="attention-item${item.warn ? " warn" : ""}">${escapeHtml(item.text)}</div>`)
    .join("");
}

// --- Meeting Brief --------------------------------------------------------

function formatPct(value) {
  if (value === null || value === undefined) return "—";
  const pct = value * 100;
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(1)}%`;
}

const ALLOCATION_COLORS = ["#B07A1E", "#0E1726", "#4C6A92", "#8FA4BF", "#D5CFC2", "#7A9E8F"];

// Builds the brief <article> as an HTML string for embedding inline in the
// chat log (see renderChatLog/renderBriefEntryHtml) -- everything here is
// pure string templating over `brief`'s already-computed fields, no DOM
// lookups, since the whole chat log gets rebuilt via innerHTML on every
// update and per-element ids wouldn't survive that.
function buildBriefArticleHtml(brief) {
  const when = new Date(brief.datetime);
  const isTomorrow = when.toDateString() === new Date(Date.now() + 86_400_000).toDateString();
  const dayLabel = isTomorrow ? "tomorrow" : when.toLocaleDateString(undefined, { weekday: "long" });
  const title = `${brief.client_name}, ${dayLabel} at ${when.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`;

  const metrics = brief.performance.map((p) => ({
    label: { MTD: "Month to date", QTD: "Quarter to date", YTD: "Year to date" }[p.period] || p.period,
    value: formatPct(p.twr),
    sub: p.benchmark_twr === null ? "no benchmark data" : `vs benchmark ${formatPct(p.benchmark_twr)}`,
    warn: false,
  }));
  if (brief.over_limit_sectors.length > 0) {
    const sector = brief.over_limit_sectors[0];
    const pct = brief.allocation.find((a) => a.sector === sector)?.pct ?? 0;
    metrics.push({
      label: `${sector[0].toUpperCase()}${sector.slice(1)} weight`,
      value: `${pct.toFixed(0)}%`,
      sub: "limit 25%",
      warn: true,
    });
  }

  const metricsHtml = metrics
    .map(
      (m) => `<div class="brief-metric${m.warn ? " brief-metric--warn" : ""}">
        <p class="brief-metric-label">${escapeHtml(m.label)}</p>
        <p class="brief-metric-value">${escapeHtml(m.value)}</p>
        <p class="brief-metric-sub">${escapeHtml(m.sub)}</p>
      </div>`,
    )
    .join("");

  const notesHtml = brief.notes
    ? `<ul><li>${escapeHtml(brief.notes)}</li></ul>`
    : '<p class="field-hint">No notes recorded for this meeting.</p>';

  const allocationBarHtml = brief.allocation
    .map((a, i) => `<div style="width:${a.pct}%; background:${ALLOCATION_COLORS[i % ALLOCATION_COLORS.length]};"></div>`)
    .join("");
  const allocationLegendHtml = brief.allocation
    .map(
      (a, i) => `<div class="brief-allocation-legend-row">
        <span><span class="brief-allocation-swatch" style="background:${ALLOCATION_COLORS[i % ALLOCATION_COLORS.length]};"></span>${escapeHtml(a.sector)}</span>
        <span>${a.pct.toFixed(0)}%</span>
      </div>`,
    )
    .join("");

  const newsHtml = brief.news.length
    ? brief.news
        .map(
          (n) => `<div class="brief-news-item">
            <span class="brief-news-tag">${escapeHtml(n.ticker || n.sector || "news")}</span>
            <span>${escapeHtml(n.headline)}</span>
          </div>`,
        )
        .join("")
    : '<p class="field-hint">No recent news for this client’s holdings.</p>';

  const suitabilityHtml = brief.suitability
    .map(
      (s) => `<div class="brief-suitability-row ${s.potentially_suitable ? "ok" : "no"}">
        <span>${escapeHtml(s.product_name)}</span>
        <span class="brief-suitability-verdict" title="${escapeHtml(s.explanation)}">${s.potentially_suitable ? "Suitable" : "Not suitable"}</span>
      </div>`,
    )
    .join("");

  const talkingPointsHtml = brief.talking_points
    .map(
      (p, i) => `<li class="brief-point">
        <span class="brief-point-n">${i + 1}</span>
        <p class="brief-point-title">${escapeHtml(p.title)}</p>
        <p class="brief-point-detail">${escapeHtml(p.detail)}</p>
      </li>`,
    )
    .join("");

  return `<article class="brief">
    <header class="brief-head">
      <p class="brief-eyebrow">Meeting brief</p>
      <div class="brief-title-row">
        <h1 class="brief-title">${escapeHtml(title)}</h1>
        <div class="brief-actions">
          <button class="brief-action brief-action--ghost brief-share-btn" type="button">Share</button>
          <button class="brief-action brief-action--solid brief-export-btn" type="button">Export</button>
        </div>
      </div>
      <p class="brief-summary">${escapeHtml(brief.headline)}</p>
    </header>
    <div class="brief-metrics">${metricsHtml}</div>
    <div class="brief-panels">
      <section class="brief-panel">
        <h2 class="brief-panel-title">Since you last met</h2>
        ${notesHtml}
      </section>
      <section class="brief-panel">
        <h2 class="brief-panel-title">Allocation</h2>
        <div class="brief-allocation-bar" role="img">${allocationBarHtml}</div>
        <div class="brief-allocation-legend">${allocationLegendHtml}</div>
      </section>
      <section class="brief-panel">
        <h2 class="brief-panel-title">Market context &middot; last 48h</h2>
        ${newsHtml}
      </section>
      <section class="brief-panel">
        <h2 class="brief-panel-title">Suitability</h2>
        ${suitabilityHtml}
      </section>
    </div>
    <section class="brief-points-section">
      <h2 class="brief-panel-title">Talking points</h2>
      <ol class="brief-points">${talkingPointsHtml}</ol>
    </section>
  </article>`;
}

// Renders one synthetic timeline entry: the predefined user-prompt bubble
// plus whatever state the brief fetch is in (loading/error/ready).
function renderBriefEntryHtml(entry) {
  const promptHtml = `<div class="log-entry user-prompt">${escapeHtml(entry.prompt)}</div>`;
  if (entry.loading) {
    return `${promptHtml}<div class="field-hint">Preparing the brief…</div>`;
  }
  if (entry.error) {
    return `${promptHtml}<div class="playground-error">${escapeHtml(entry.error)}</div>`;
  }
  return `${promptHtml}${buildBriefArticleHtml(entry.brief)}
    <div class="chip-row">
      <button class="chip" data-prompt="Model the rebalance">Model the rebalance</button>
      <button class="chip" data-prompt="Draft a pre-meeting note">Draft a pre-meeting note</button>
      <button class="chip" data-prompt="How is my best client doing MTD?">How is my best client doing MTD?</button>
    </div>`;
}

// Builds the client-snapshot <article> as an HTML string -- same card
// language as buildBriefArticleHtml (reusing its CSS classes) but for
// src/client_snapshot.rs's smaller, LLM-filled-in shape: a summary, a
// handful of facts, and a few talking points. See that file's doc comment
// for why this one lets the model use tools instead of pre-fetching facts.
function buildClientSnapshotArticleHtml(snapshot) {
  const factsHtml = snapshot.facts
    .map(
      (f) => `<div class="snapshot-fact">
        <p class="snapshot-fact-label">${escapeHtml(f.label)}</p>
        <p class="snapshot-fact-value">${escapeHtml(f.value)}</p>
      </div>`,
    )
    .join("");

  const talkingPointsHtml = snapshot.talking_points
    .map(
      (p, i) => `<li class="brief-point">
        <span class="brief-point-n">${i + 1}</span>
        <p class="brief-point-title">${escapeHtml(p.title)}</p>
        <p class="brief-point-detail">${escapeHtml(p.detail)}</p>
      </li>`,
    )
    .join("");

  return `<article class="brief">
    <header class="brief-head">
      <p class="brief-eyebrow">Client snapshot</p>
      <div class="brief-title-row">
        <h1 class="brief-title">${escapeHtml(snapshot.client_name)}</h1>
      </div>
      <p class="brief-summary">${escapeHtml(snapshot.summary)}</p>
    </header>
    ${factsHtml ? `<div class="snapshot-facts">${factsHtml}</div>` : ""}
    ${
      snapshot.talking_points.length
        ? `<section class="brief-points-section">
      <h2 class="brief-panel-title">Talking points</h2>
      <ol class="brief-points">${talkingPointsHtml}</ol>
    </section>`
        : ""
    }
  </article>`;
}

// Renders one synthetic "snapshot" timeline entry -- same loading/error/
// ready states as renderBriefEntryHtml.
function renderSnapshotEntryHtml(entry) {
  const promptHtml = `<div class="log-entry user-prompt">${escapeHtml(entry.prompt)}</div>`;
  if (entry.loading) {
    return `${promptHtml}<div class="field-hint">Looking into ${escapeHtml(entry.query)}…</div>`;
  }
  if (entry.error) {
    return `${promptHtml}<div class="playground-error">${escapeHtml(entry.error)}</div>`;
  }
  return `${promptHtml}${buildClientSnapshotArticleHtml(entry.snapshot)}`;
}

// Triggered by clicking a client in the sidebar's book (see renderToday())
// instead of the old free-text `submit("Tell me about {name}")`, which ran
// the full interactive chat and rendered plain markdown. This asks for a
// client snapshot instead: a one-shot, schema-constrained LLM call
// (mcp_client_snapshot) rendered through the same card template as the
// Meeting Brief, inline in the chat log like generateBrief() does.
async function generateClientSnapshot(clientName) {
  showChatView();

  const entry = {
    kind: "snapshot",
    afterCount: lastBackendLog.length,
    prompt: `Tell me about ${clientName}`,
    query: clientName,
    snapshot: null,
    error: null,
    loading: true,
  };
  syntheticEntries.push(entry);
  renderChatLog();

  try {
    const snapshot = await invoke("mcp_client_snapshot", { query: clientName });
    entry.loading = false;
    entry.snapshot = snapshot;
  } catch (err) {
    entry.loading = false;
    entry.error = String(err);
  }
  renderChatLog();
}

// Dummy per the spec ("not implemented") -- present so the template looks
// right, deliberately inert. Event delegation since brief cards are
// rebuilt via innerHTML on every renderChatLog() call, which would drop
// any per-element listener attached the usual way.
document.addEventListener("click", (event) => {
  if (event.target.closest(".brief-share-btn, .brief-export-btn")) {
    event.preventDefault();
  }
});

async function generateBrief() {
  const btn = document.getElementById("brief-open-btn");
  if (briefRequested) {
    showChatView();
    const view = document.getElementById("chat-view");
    view.scrollTo({ top: view.scrollHeight, behavior: "smooth" });
    return;
  }
  briefRequested = true;
  btn.disabled = true;
  btn.textContent = "Generating…";
  showChatView();

  const entry = {
    kind: "brief",
    afterCount: lastBackendLog.length,
    prompt: "I have an upcoming meeting with one of my clients. Catch me up.",
    brief: null,
    error: null,
    loading: true,
  };
  syntheticEntries.push(entry);
  renderChatLog();

  try {
    const brief = await invoke("mcp_meeting_brief");
    entry.loading = false;
    entry.brief = brief;
    applyBriefToSidebar(brief);
    btn.textContent = "Brief ready · open";
    btn.disabled = false;
    refreshSavedBriefs(); // the backend auto-saves on generate; pick up the new entry
  } catch (err) {
    entry.loading = false;
    entry.error = String(err);
    briefRequested = false;
    btn.textContent = "Generate brief";
    btn.disabled = false;
  }
  renderChatLog();
}

// Shared by generateBrief() and loadSavedBrief() -- everything outside the
// chat log itself (cachedBrief, evidence panel default, regulatory card,
// attention card) that a freshly generated or recovered brief updates.
function applyBriefToSidebar(brief) {
  cachedBrief = brief;
  renderEvidenceRows(evidenceRowsFromBrief(brief));
  const reg = brief.regulatory;
  document.getElementById("regulatory-card").classList.toggle("hidden", !reg);
  if (reg) {
    document.getElementById("regulatory-text").textContent = reg.text;
    // Just the source filename -- "fictional corpus" was the mockup's own
    // flavor text for its hand-written illustrative example; this is a
    // real search_regulatory_guidance result, not fabricated, and
    // labeling it otherwise would be actively misleading.
    document.getElementById("regulatory-source").textContent = reg.source;
  }
  renderAttention(brief);
}

// --- Saved briefs -------------------------------------------------------

async function refreshSavedBriefs() {
  const container = document.getElementById("saved-briefs-list");
  if (!container) return;
  try {
    const briefs = await invoke("list_saved_briefs");
    if (briefs.length === 0) {
      container.innerHTML = '<p class="field-hint">No saved briefs yet.</p>';
      return;
    }
    container.innerHTML = briefs
      .map(
        (b) =>
          `<a href="#" data-client-id="${escapeHtml(b.client_id)}" data-date="${escapeHtml(b.date)}">${escapeHtml(b.client_name)} &middot; ${escapeHtml(b.date)}</a>`,
      )
      .join("");
    container.querySelectorAll("a").forEach((el) => {
      el.addEventListener("click", (event) => {
        event.preventDefault();
        loadSavedBrief(el.dataset.clientId, el.dataset.date);
      });
    });
  } catch (err) {
    console.error("parrot-tauri: list_saved_briefs failed", err);
  }
}

// Recovers a previously generated brief from disk (src/meeting_brief.rs's
// `load_saved_brief`) -- same inline-in-chat rendering as a freshly
// generated one, just sourced from the saved-briefs sidebar instead of
// "Generate brief".
async function loadSavedBrief(clientId, date) {
  showChatView();

  const entry = {
    kind: "brief",
    afterCount: lastBackendLog.length,
    prompt: `Show me the saved brief for ${clientId} on ${date}.`,
    brief: null,
    error: null,
    loading: true,
  };
  syntheticEntries.push(entry);
  renderChatLog();

  try {
    const brief = await invoke("load_saved_brief", { clientId, date });
    entry.loading = false;
    entry.brief = brief;
    applyBriefToSidebar(brief);
  } catch (err) {
    entry.loading = false;
    entry.error = String(err);
  }
  renderChatLog();
}

// --- Connect / login --------------------------------------------------

async function connectMcpGateway() {
  const statusEl = document.getElementById("mcp-status");
  const connectBtn = document.getElementById("mcp-connect-btn");
  connectBtn.disabled = true;
  statusEl.textContent = "Opening browser for Keycloak login…";
  try {
    const identity = await invoke("mcp_login");
    // The MCP Gateway's own identity -- "your book only" refers to this
    // login, which is the one that actually determines which tools/data
    // the user sees, so the header should reflect it once known (it may
    // differ from the OpenShell-sandbox identity main() set initially).
    if (identity.display_name) {
      document.getElementById("header-avatar").textContent = initialsFor(identity.display_name);
      document.getElementById("header-identity-name").textContent = identity.display_name;
    }
    document.getElementById("header-readonly-text").textContent =
      `SSO · ${identity.display_name || "connected"} · read-only`;
  } catch (err) {
    statusEl.textContent = `Error: ${err}`;
    connectBtn.disabled = false;
    return;
  }

  statusEl.textContent = "Loading…";
  let toolCount = 0;
  try {
    const groups = await invoke("mcp_list_tools");
    renderToolGroups(groups);
    toolCount = groups.reduce((n, g) => n + g.tools.length, 0);
  } catch (err) {
    console.error("parrot-tauri: mcp_list_tools failed", err);
  }
  try {
    const today = await invoke("mcp_today");
    renderToday(today);
  } catch (err) {
    console.error("parrot-tauri: mcp_today failed", err);
    statusEl.textContent += " (today's meetings/book failed to load — see console)";
  }

  statusEl.textContent = `Connected — ${toolCount} tools`;
  connectBtn.classList.add("hidden");
}

async function submit(promptText) {
  const prompt = promptText.trim();
  if (!prompt || turnRunning) return;

  showChatView();
  const input = document.getElementById("composer-input");
  input.value = "";
  input.style.height = "auto";
  turnRunning = true;
  document.getElementById("composer-hint").textContent = "running…";

  const channel = new Channel();
  channel.onmessage = (snapshot) => {
    lastBackendLog = snapshot.log;
    renderChatLog();
    if (!document.getElementById("chat-view").classList.contains("hidden")) {
      renderEvidenceRows(evidenceRowsFromLog(lastBackendLog));
    }
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

  // Initial header identity comes from the OpenShell-sandbox session
  // (whatever `greeting` resolved); `connectMcpGateway` overwrites this
  // with the MCP Gateway's own identity once that login succeeds, since
  // that's the one "your book only" actually refers to.
  const displayName = greeting.display_name || greeting.workspace || "Banker";
  document.getElementById("header-avatar").textContent = initialsFor(displayName);
  document.getElementById("header-identity-name").textContent = displayName;
  document.getElementById("header-readonly-text").textContent =
    `SSO · sandbox ${greeting.sandbox} · read-only`;

  document.getElementById("splash-greeting").textContent = greeting.identity_greeting;
  document.getElementById("splash-gateway").textContent =
    `Gateway: ${greeting.gateway_name} (${greeting.gateway_endpoint})`;
  document.getElementById("splash-sandbox").textContent =
    `Sandbox: ${greeting.sandbox} (${greeting.sandbox_phase})`;

  document.getElementById("header-sandbox").textContent =
    `sandbox ${greeting.sandbox} — `;
  const phaseEl = document.getElementById("header-sandbox-phase");
  phaseEl.textContent = `${greeting.sandbox_phase} — `;
  phaseEl.className = `status ${sandboxPhaseClass(greeting.sandbox_phase)}`;

  if (greeting.mcp_available) {
    document.getElementById("mcp-connect-btn").addEventListener("click", connectMcpGateway);
  } else {
    document.getElementById("mcp-panel").classList.add("hidden");
  }
  document.getElementById("nav-chat").addEventListener("click", showChatView);
  document.getElementById("playground-run-btn").addEventListener("click", runActiveTool);
  document.getElementById("brief-open-btn").addEventListener("click", generateBrief);
  document.getElementById("composer-send-btn").addEventListener("click", () => {
    submit(document.getElementById("composer-input").value);
  });
  // Delegated (not attached per-element): suggestion/follow-up chips
  // inside a generated brief card don't exist in the DOM until
  // renderChatLog() builds that card's innerHTML, so a one-time
  // querySelectorAll at startup would miss them.
  document.addEventListener("click", (event) => {
    const chip = event.target.closest(".suggestion-chip, .chip");
    if (chip?.dataset.prompt) submit(chip.dataset.prompt);
  });

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
