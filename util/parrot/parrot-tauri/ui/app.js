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
let turnsCompleted = 0; // drives the composer placeholder ("Ask a question" -> "Reply")

// Tracks the current turn's thinking phase so the live "agent-group"
// summary can show "Thinking for Ns…" while it's ongoing and freeze into
// "Thought for Ns" once real content (a tool call/assistant text/result)
// follows -- set/reset in submit()'s channel.onmessage, not derivable from
// a pure re-render since the backend doesn't timestamp events itself.
let turnThinkingStartedAt = null;
let turnThinkingEndedAt = null;

// --- Connection heartbeat ---------------------------------------------
//
// Neither the sandbox exec channel nor the MCP Gateway session is ever
// re-checked once connected (sandbox_phase/mcp_login are both one-shot) --
// this polls the sandbox on an interval and piggybacks on every successful
// real call (a finished turn, a tool call) to mark both as freshly alive,
// since those already prove liveness without a redundant round-trip.
const SANDBOX_POLL_MS = 30000;
let sandboxName = null;
let mcpAvailable = false;
let mcpConnected = false;
let lastSandboxOkAt = null;
let lastMcpOkAt = null;
let sandboxPollStarted = false;
let pendingPrompt = null; // --prompt, queued until connect_sandbox succeeds

function noteSandboxOk() {
  lastSandboxOkAt = Date.now();
  updateConnectionHeader();
}

function noteMcpOk() {
  lastMcpOkAt = Date.now();
  updateConnectionHeader();
}

function timeAgo(ms) {
  const seconds = Math.round((Date.now() - ms) / 1000);
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  return `${minutes}m ago`;
}

function updateConnectionHeader() {
  const readonlyEl = document.getElementById("header-readonly-text");
  const pillEl = document.getElementById("header-status-pill");
  if (!readonlyEl || !pillEl) return;

  if (!sandboxName) {
    readonlyEl.textContent = "SSO · not connected";
    pillEl.textContent = "Offline";
    pillEl.className = "header-status-pill pill-offline";
    return;
  }

  const staleAfter = SANDBOX_POLL_MS * 2;
  const sandboxAge = lastSandboxOkAt ? Date.now() - lastSandboxOkAt : Infinity;
  const mcpAge = lastMcpOkAt ? Date.now() - lastMcpOkAt : Infinity;
  const mcpOk = !mcpConnected || mcpAge < staleAfter;

  let label;
  let cls;
  if (lastSandboxOkAt && sandboxAge < staleAfter && mcpOk) {
    label = "online";
    cls = "pill-online";
  } else if (lastSandboxOkAt) {
    label = "poor";
    cls = "pill-poor";
  } else {
    label = "offline";
    cls = "pill-offline";
  }

  const suffix = lastSandboxOkAt ? ` · ${timeAgo(lastSandboxOkAt)}` : "";
  readonlyEl.textContent = `SSO · sandbox ${sandboxName} · ${label}${suffix}`;
  pillEl.textContent = label === "online" ? "Online" : label === "poor" ? "Poor" : "Offline";
  pillEl.className = `header-status-pill ${cls}`;
}

async function pollSandboxStatus() {
  if (!sandboxName) return;
  try {
    await invoke("sandbox_status");
    noteSandboxOk();
  } catch (err) {
    console.error("parrot-tauri: sandbox_status failed", err);
    updateConnectionHeader();
  }
}

// Shared by both attach paths -- an explicit --sandbox at launch, or a
// successful connect_sandbox -- so the header/splash render identically
// either way.
function applySandboxInfo(sandbox, phase) {
  sandboxName = sandbox;
  lastSandboxOkAt = Date.now();

  document.getElementById("splash-sandbox").textContent = `Sandbox: ${sandbox} (${phase})`;

  updateConnectionHeader();
  if (!sandboxPollStarted) {
    sandboxPollStarted = true;
    setInterval(pollSandboxStatus, SANDBOX_POLL_MS);
  }
}

// The splash screen's Connect button. When MCP is configured for this
// launch, identity comes from *there*: log into the MCP Gateway first (the
// same interactive Keycloak flow as before), extract its
// `preferred_username` (the stable login name -- NOT `display_name`, which
// prefers "Alice Meridian" over "alice" and would produce a malformed
// sandbox name), and pass that to `connect_sandbox` so it computes
// `{sandbox_prefix}{agent}-{preferred_username}`. Only falls back to the
// primary gateway's own `state.workspace` (server-side) when MCP isn't
// configured at all -- there's no MCP identity to derive from in that
// case. `connect_sandbox` only ever *reads*/verifies the sandbox (never
// creates one); a specific failure (e.g. "ask your admin to provision
// ...") surfaces inline.
async function connectSandbox() {
  const btn = document.getElementById("splash-connect-btn");
  const errorEl = document.getElementById("splash-error");
  btn.disabled = true;
  errorEl.textContent = "";

  let user = null;
  if (mcpAvailable) {
    try {
      const identity = await mcpLogin();
      user = identity.preferred_username || null;
    } catch (err) {
      errorEl.textContent = `Couldn't log in to the MCP Gateway: ${err}`;
      btn.disabled = false;
      return;
    }
  }

  try {
    const result = await invoke("connect_sandbox", { user });
    applySandboxInfo(result.sandbox, result.sandbox_phase);
    dismissSplash();
    if (pendingPrompt) {
      const prompt = pendingPrompt;
      pendingPrompt = null;
      submit(prompt);
    }
  } catch (err) {
    errorEl.textContent = String(err);
    btn.disabled = false;
    return;
  }

  if (mcpAvailable) {
    loadMcpToolsAndToday();
  }
}

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
      return { cls: "tool-call", text: `→ ${capitalize(humanizeToolName(data.name))}` };
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
// A model that narrates while it works ("I'll check X, then Y...") sends
// several AssistantText events per turn, not just its one real final
// answer -- only the *last* AssistantText before the next UserPrompt (or
// end of log) is that real answer; every earlier one in the same turn is
// "thinking out loud" and should fold into the turn's single collapsible
// alongside its tool calls, not fragment it into several back-to-back
// disclosures (mirrors Claude's own UI: one collapsible per turn).
function markFinalAssistantTexts(timeline) {
  const isFinal = new Array(timeline.length).fill(false);
  let lastIdxInTurn = -1;
  timeline.forEach((item, i) => {
    if (item.kind !== "agent") return;
    const type = item.data.event.type;
    if (type === "UserPrompt") {
      lastIdxInTurn = -1;
    } else if (type === "AssistantText") {
      if (lastIdxInTurn !== -1) isFinal[lastIdxInTurn] = false;
      isFinal[i] = true;
      lastIdxInTurn = i;
    }
  });
  return isFinal;
}

function buildTimelineUnits(timeline) {
  const units = [];
  let pendingGroup = [];
  const isFinalAssistantText = markFinalAssistantTexts(timeline);
  // Once a turn's `Result` event has been seen, any further collapsible
  // events are trailing noise (e.g. Codex's own "fail to delete session:
  // ... HTTP 403" line emitted to stderr after the turn is already done) --
  // not real content, so they're dropped rather than appended. Reset on the
  // next real-content boundary (a new turn/response).
  let suppressAfterResult = false;

  function flushGroup() {
    suppressAfterResult = false;
    if (pendingGroup.length === 0) return [];
    // `Result` almost always arrives right after the final AssistantText
    // (which already flushed the real tool-call/thinking group), landing
    // alone in a fresh, otherwise-empty group of its own -- rather than
    // show that as a content-free "1 step" disclosure, drop it: the turn's
    // actual answer is already the AssistantText bubble right above it.
    const meaningful = pendingGroup.filter((i) => i.event.type !== "Result");
    pendingGroup = [];
    if (meaningful.length === 0) return [];
    const evidence = evidenceRowsFromLog(meaningful);
    units.push({ kind: "agent-group", items: meaningful, evidence });
    return evidence;
  }

  timeline.forEach((item, i) => {
    if (item.kind !== "agent") {
      flushGroup();
      units.push({ kind: item.kind, data: item.data, evidence: evidenceForEntry(item) });
      return;
    }
    const type = item.data.event.type;
    if (type === "UserPrompt") {
      flushGroup();
      units.push({ kind: "user-prompt", data: item.data, evidence: [] });
    } else if (type === "AssistantText" && isFinalAssistantText[i]) {
      const evidence = flushGroup();
      units.push({ kind: "assistant-text", data: item.data, evidence });
    } else if (type === "Error") {
      // Silenced entirely, not even folded into the collapsible -- in
      // practice these are transient infra/model warnings (a model-
      // metadata fallback notice, a stream-reconnect retry), not content
      // the user needs to act on; a genuine fatal failure still surfaces
      // via the turn's own RunStatus ("crashed").
      return;
    } else if (COLLAPSIBLE_AGENT_EVENT_TYPES.has(type) || type === "AssistantText") {
      if (suppressAfterResult) return;
      pendingGroup.push(item.data);
      if (type === "Result") suppressAfterResult = true;
    } else {
      flushGroup();
      units.push({ kind: "agent-error", data: item.data, evidence: [] });
    }
  });
  flushGroup();

  // Mark which agent-group units belong to the most recent turn (occur
  // after the last UserPrompt) -- deliberately not just "is this the last
  // unit overall", since an AssistantText bubble commonly renders *after*
  // its own turn's tool-call group, which shouldn't make that group's
  // thinking-duration revert to a generic, non-timed label.
  let lastUserPromptIndex = -1;
  units.forEach((u, i) => {
    if (u.kind === "user-prompt") lastUserPromptIndex = i;
  });
  units.forEach((u, i) => {
    if (u.kind === "agent-group") u.isLatestTurn = i > lastUserPromptIndex;
  });

  return units;
}

function evidenceForEntry(item) {
  if (item.kind === "brief") return item.data.brief ? evidenceRowsFromBrief(item.data.brief) : [];
  if (item.kind === "snapshot") return item.data.snapshot ? item.data.snapshot.evidence || [] : [];
  return [];
}

// `openByDefault` (the newest group, turn still running) gets a *live*
// summary naming whatever's happening right now -- the most recent tool
// call, or a running thinking-duration counter -- instead of a static
// count, so the summary line visibly updates as the turn progresses
// (mirrors Claude's own collapsed tool-use summary). Once the turn/group
// is done, it freezes into a short recap instead.
// Shared by the chat log's agent-group recap and the client-snapshot
// progress feed's own collapsed summary (see renderSnapshotEntryHtml) --
// both describe "which tools were called" the same way: name them if
// there are only a couple, otherwise just a count.
function summarizeToolNames(names) {
  const unique = [...new Set(names.map((n) => capitalize(humanizeToolName(n))))];
  // The count must match `names.length` (one row per call, e.g. the same
  // "get_performance" called separately for MTD/QTD/YTD), not
  // `unique.length` -- counting distinct tool names undercounts whenever a
  // tool is called more than once, showing "Ran 6 tools" for what the
  // table actually lists as 8 rows.
  return unique.length > 2 ? `Ran ${names.length} tool call${names.length === 1 ? "" : "s"}` : unique.join(", ");
}

// Trailing chevron (not a leading triangle) -- rotates 90° via the
// `.turn-details[open]` CSS rule, pointing right collapsed / down open.
const CHEVRON_SVG =
  '<svg class="turn-details-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
  'stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="m9 18 6-6-6-6"/></svg>';

// One row in the activity table per tool call -- deliberately just
// "Calling {name}", not the raw arrow/checkmark log lines or a tool's
// actual error text (a failed attempt and a retried one render
// identically; the point is "what was attempted", not a verbose success/
// failure transcript -- `hasError`, computed by the caller, still drives
// the disclosure's own border color for an at-a-glance signal).
function toolCallRowHtml(name) {
  return `<div class="tool-call-row">Calling ${escapeHtml(capitalize(humanizeToolName(name)))}</div>`;
}

// Shared by the chat log's agent-group and the client-snapshot progress
// feed: collapsed by default always (never auto-opened, even while live --
// the one-line summary is what updates in place), a trailing chevron
// instead of a leading triangle, and a single rounded-corner table of rows
// for the body instead of a stack of separate bubbles.
function activityDisclosureHtml(summaryText, rowsHtml, hasError) {
  const tableHtml = rowsHtml ? `<div class="tool-call-table">${rowsHtml}</div>` : "";
  return `<details class="turn-details${hasError ? " turn-details-error" : ""}">
    <summary><span>${escapeHtml(summaryText)}</span>${CHEVRON_SVG}</summary>
    ${tableHtml}
  </details>`;
}

function renderAgentGroupHtml(items, isLive, isMostRecentTurnGroup) {
  const toolCalls = items.filter((i) => i.event.type === "ToolCall");
  const isThinking = items.some((i) => i.event.type === "Thinking" || i.event.type === "ThinkingTokens");
  const hasError = items.some((i) => i.event.type === "ToolResult" && i.event.data.is_error);

  const parts = [];
  if (isThinking) {
    // turnThinkingStartedAt/EndedAt only ever describe the most recent
    // turn -- an older, already-closed group from an earlier turn in this
    // session falls back to a plain label instead of that stale timing.
    if (!isMostRecentTurnGroup || turnThinkingStartedAt === null) {
      parts.push("Thinking");
    } else if (turnThinkingEndedAt !== null) {
      const secs = Math.max(1, Math.round((turnThinkingEndedAt - turnThinkingStartedAt) / 1000));
      parts.push(`Thought for ${secs}s`);
    } else {
      const secs = Math.max(1, Math.round((Date.now() - turnThinkingStartedAt) / 1000));
      parts.push(`Thinking for ${secs}s…`);
    }
  }
  if (toolCalls.length) {
    if (isLive) {
      const current = capitalize(humanizeToolName(toolCalls[toolCalls.length - 1].event.data.name));
      parts.push(`Running ${current}…`);
    } else {
      parts.push(summarizeToolNames(toolCalls.map((c) => c.event.data.name)));
    }
  }
  const summaryText = parts.length ? parts.join(" · ") : `${items.length} step${items.length === 1 ? "" : "s"}`;
  // Body only ever shows tool-call rows and the model's own "thinking out
  // loud" narration (non-final AssistantText, folded in here by
  // buildTimelineUnits) -- ToolResult/Thinking/Info/Unknown/Unparseable/
  // Result carry no useful standalone text to a banker (raw Rust/MCP error
  // chains, token-heartbeat spam, protocol bookkeeping), so they're
  // dropped rather than rendered line-by-line.
  const rowsHtml = items
    .map((entry) => {
      if (entry.event.type === "ToolCall") return toolCallRowHtml(entry.event.data.name);
      if (entry.event.type === "AssistantText") {
        return `<div class="tool-call-row">${renderMarkdown(escapeHtml(entry.event.data))}</div>`;
      }
      return "";
    })
    .join("");
  return activityDisclosureHtml(summaryText, rowsHtml, hasError);
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
      // Always collapsed by default (never auto-opened) -- `isLast &&
      // turnRunning` only picks the summary's tense ("Running X…" live vs.
      // a frozen recap once done), not whether the disclosure is open.
      // `unit.isLatestTurn` (not `isLast`) separately gates the thinking-
      // duration display, since the group belonging to the current turn
      // commonly isn't the very last unit (an AssistantText bubble renders
      // after it) -- older groups from earlier turns fall back to a plain
      // "Thinking".
      return renderAgentGroupHtml(unit.items, isLast && turnRunning, unit.isLatestTurn);
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

function updateComposerPlaceholder() {
  const input = document.getElementById("composer-input");
  if (input) input.placeholder = turnsCompleted > 0 ? "Reply" : "How can I help you today?";
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
            <span class="evidence-tool">${escapeHtml(humanizeToolName(row.tool))}</span>
            ${row.time ? `<span class="evidence-time">${escapeHtml(row.time)}</span>` : ""}
          </div>
          <div class="evidence-detail">${escapeHtml(row.detail)}</div>
          <span class="evidence-source">${escapeHtml(row.source)}</span>
        </div>
      </div>`,
    )
    .join("");
}

// Populated from `mcp_list_tools`' grouped response (see
// `loadMcpToolsAndToday`), longest-prefix-first -- mirrors parrot-core's
// own `group_by_server` longest-prefix-wins rule, needed because some
// server names are themselves multi-word (e.g. "mcp-crm-calendar"), so a
// single generic `mcp_<word>_` strip isn't enough to fully remove it.
let knownServerPrefixes = [];

// Turns a gateway tool name into a short, readable label. Two shapes seen
// in practice:
//   - Claude Code's flat, gateway-prefixed name, e.g.
//     "mcp_portfolio_list_my_clients".
//   - Codex's own `{server}::{tool}` item naming (parrot-core's
//     `codex_mcp_tool_call_event`), e.g.
//     "gateway::mcp_kyc_compliance_check_suitability" or
//     "codex::list_mcp_resources" for its *built-in* MCP introspection
//     tools -- the "{server}::" part is always just "gateway"/"codex" and
//     adds nothing, so it's dropped unconditionally.
//
// `dropServer` additionally strips the gateway server segment itself
// (`knownServerPrefixes`) -- used by the Tools list, where the server name
// is already the card's own heading, so repeating it in every row's label
// is redundant. Left `false` (default) for standalone text with no such
// heading (chat log, evidence rows, live progress), where keeping it
// (e.g. "kyc compliance check suitability") is the only thing that says
// which service was actually called.
function humanizeToolName(name, { dropServer = false } = {}) {
  const withoutItemServer = name.includes("::") ? name.split("::").pop() : name;
  if (dropServer) {
    const prefix = knownServerPrefixes.find((p) => withoutItemServer.startsWith(p));
    const stripped = prefix ? withoutItemServer.slice(prefix.length) : withoutItemServer.replace(/^mcp_/, "");
    return stripped.replace(/_/g, " ");
  }
  return withoutItemServer.replace(/^mcp_/, "").replace(/_/g, " ");
}

function capitalize(s) {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
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

function clearToolActive() {
  document.querySelectorAll(".tool-client").forEach((el) => el.classList.remove("tool-client-active"));
}

function setHeaderViewToggleLabel(label) {
  const toggle = document.getElementById("header-view-toggle");
  if (toggle) toggle.textContent = label;
}

// Tools is a separate full-width takeover of the play area (its own
// `#tools-layout`, not a third pane squeezed in alongside chat's sidebar/
// evidence panel) -- these three functions toggle which of
// `#chat-layout`/`#tools-layout` is visible, not individual panes within
// one shared layout.
function showChatView() {
  activeTool = null;
  activeForm = null;
  document.getElementById("tools-layout").classList.add("hidden");
  document.getElementById("chat-layout").classList.remove("hidden");
  document.getElementById("regulatory-card").classList.toggle("hidden", !cachedBrief?.regulatory);
  setHeaderViewToggleLabel("Tools");
  if (lastBackendLog.length || syntheticEntries.length) {
    renderEvidenceRows(cachedBrief ? evidenceRowsFromBrief(cachedBrief) : evidenceRowsFromLog(lastBackendLog));
  }
}

function showPlaygroundView(tool, toolButton) {
  activeTool = tool;
  document.getElementById("chat-layout").classList.add("hidden");
  document.getElementById("tools-layout").classList.remove("hidden");
  document.getElementById("tools-empty-state").classList.add("hidden");
  document.getElementById("tools-active-panel").classList.remove("hidden");
  clearToolActive();
  if (toolButton) toolButton.classList.add("tool-client-active");
  setHeaderViewToggleLabel("Chat");

  document.getElementById("playground-tool-name").textContent = tool.title || tool.name;
  document.getElementById("playground-tool-description").textContent = tool.description || "";
  document.getElementById("playground-result").innerHTML = "";
  activeForm = renderToolForm(document.getElementById("playground-form"), tool.inputSchema);
}

// Landing state for the header's "Tools" toggle when no specific tool has
// been picked yet from the list -- showPlaygroundView() always expects a
// tool object, which doesn't exist on a fresh toggle click.
function showPlaygroundLanding() {
  activeTool = null;
  activeForm = null;
  document.getElementById("chat-layout").classList.add("hidden");
  document.getElementById("tools-layout").classList.remove("hidden");
  document.getElementById("tools-empty-state").classList.remove("hidden");
  document.getElementById("tools-active-panel").classList.add("hidden");
  clearToolActive();
  setHeaderViewToggleLabel("Chat");
}


// --- MCP Gateway toolbar/playground ----------------------------------

// One `.card` per MCP server, "Your book"-styled (see `renderToday`'s
// `.book-client` buttons) -- each tool is a clickable row with an avatar,
// name, and truncated description, instead of the "list of clients" data.
function renderToolGroups(groups) {
  const container = document.getElementById("tools-list");
  container.innerHTML = "";
  for (const group of groups) {
    if (group.tools.length === 0) continue;
    const card = document.createElement("section");
    card.className = "card";

    const titleRow = document.createElement("div");
    titleRow.className = "card-title-row";
    titleRow.innerHTML = `<h2 class="card-title">${escapeHtml(group.server || "(other)")}</h2><span class="mcp-status">${group.tools.length}</span>`;
    card.appendChild(titleRow);

    for (const tool of group.tools) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "book-client tool-client";
      if (tool.description) button.title = tool.description;
      // `tool.name` is gateway-prefixed with its server name (e.g.
      // "mcp_portfolio_list_my_clients") -- humanized for display so
      // labels fit the sidebar card; `mcp_call_tool` itself still gets the
      // full, unstripped `tool.name` (see showPlaygroundView/runActiveTool).
      const label = tool.title || capitalize(humanizeToolName(tool.name, { dropServer: true }));
      button.innerHTML = `
        <div class="avatar-circle">${escapeHtml(initialsFor(label))}</div>
        <div style="flex:1; min-width:0;">
          <div class="book-client-name">${escapeHtml(label)}</div>
          ${tool.description ? `<div class="tool-client-desc">${escapeHtml(tool.description)}</div>` : ""}
        </div>`;
      button.addEventListener("click", () => showPlaygroundView(tool, button));
      card.appendChild(button);
    }
    container.appendChild(card);
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
    noteMcpOk();
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
  // Scoped to #book-clients, not a bare `.book-client` query -- the Tools
  // list's buttons reuse the same `.book-client` CSS class for styling and
  // would otherwise also get wired to generateClientSnapshot() here.
  document.querySelectorAll("#book-clients .book-client").forEach((el) => {
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
// `entry.toolsCalled` accumulates live as `mcp_client_snapshot`'s progress
// channel reports each tool call (see generateClientSnapshot) -- same
// row/summary language as the chat log's agent-group (toolCallRowHtml/
// activityDisclosureHtml/summarizeToolNames), so a lookup in progress
// looks like a live turn
// instead of a frozen "Looking into X…" string with no visible progress.
function renderSnapshotEntryHtml(entry) {
  const promptHtml = `<div class="log-entry user-prompt">${escapeHtml(entry.prompt)}</div>`;
  const rowsHtml = entry.toolsCalled.map(toolCallRowHtml).join("");

  if (entry.loading) {
    const summary = entry.toolsCalled.length
      ? `Running ${capitalize(humanizeToolName(entry.toolsCalled[entry.toolsCalled.length - 1]))}…`
      : `Looking into ${entry.query}…`;
    return `${promptHtml}${activityDisclosureHtml(summary, rowsHtml, false)}`;
  }

  const recapHtml = entry.toolsCalled.length
    ? activityDisclosureHtml(summarizeToolNames(entry.toolsCalled), rowsHtml, false)
    : "";

  if (entry.error) {
    return `${promptHtml}${recapHtml}<div class="playground-error">${escapeHtml(entry.error)}</div>`;
  }
  return `${promptHtml}${recapHtml}${buildClientSnapshotArticleHtml(entry.snapshot)}`;
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
    toolsCalled: [],
  };
  syntheticEntries.push(entry);
  renderChatLog();

  const channel = new Channel();
  channel.onmessage = (progress) => {
    entry.toolsCalled.push(progress.tool);
    renderChatLog();
  };

  try {
    const snapshot = await invoke("mcp_client_snapshot", { query: clientName, onEvent: channel });
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

function showMcpLoginModal(reason) {
  document.getElementById("mcp-login-modal-message").textContent = reason
    ? `Tools, your book, and meeting briefs need a separate Keycloak login. (${reason})`
    : "Tools, your book, and meeting briefs need a separate Keycloak login.";
  document.getElementById("mcp-login-modal").classList.remove("hidden");
}

function hideMcpLoginModal() {
  document.getElementById("mcp-login-modal").classList.add("hidden");
}

// The header's "Tools" toggle is only meaningfully clickable once MCP is
// logged in (the sidebar's tool list/Today rail are both empty until
// then) -- hidden entirely when this launch has no MCP Gateway configured
// at all, muted (but still clickable, to offer the login modal) otherwise.
function updateToolsToggleState() {
  const toggle = document.getElementById("header-view-toggle");
  if (!toggle) return;
  if (!mcpAvailable) {
    toggle.classList.add("hidden");
    return;
  }
  toggle.classList.remove("hidden");
  toggle.classList.toggle("muted", !mcpConnected);
}

// Does just the MCP Gateway's interactive Keycloak login, nothing else --
// used both by `connectSandbox` (which needs the resulting identity's
// `preferred_username` *before* it can resolve a sandbox) and by
// `attemptMcpLogin` below. Throws on failure; callers decide how to
// surface that (inline on the splash vs. the login modal).
async function mcpLogin() {
  const identity = await invoke("mcp_login");
  // The MCP Gateway's own identity -- "your book only" refers to this
  // login, which is the one that actually determines which tools/data
  // the user sees, so the header should reflect it once known (it may
  // differ from the OpenShell-sandbox identity main() set initially).
  if (identity.display_name) {
    document.getElementById("header-avatar").textContent = initialsFor(identity.display_name);
    document.getElementById("header-identity-name").textContent = identity.display_name;
  }
  mcpConnected = true;
  noteMcpOk();
  updateToolsToggleState();
  hideMcpLoginModal();
  return identity;
}

// Loads the sidebar's tool list and Today rail -- soft-fails (logged, not
// re-prompted) since a successful MCP login already proved the session is
// good; a failure here is a data-fetch problem, not an auth problem.
async function loadMcpToolsAndToday() {
  try {
    const groups = await invoke("mcp_list_tools");
    knownServerPrefixes = groups
      .map((g) => `${(g.server || "").replace(/-/g, "_")}_`)
      .filter((p) => p !== "_")
      .sort((a, b) => b.length - a.length);
    renderToolGroups(groups);
    noteMcpOk();
  } catch (err) {
    console.error("parrot-tauri: mcp_list_tools failed", err);
  }
  try {
    const today = await invoke("mcp_today");
    renderToday(today);
    noteMcpOk();
  } catch (err) {
    console.error("parrot-tauri: mcp_today failed", err);
  }
}

// Full MCP login + tools/Today, with failure surfaced via the login modal
// instead of thrown -- used by the modal's own "Log in" retry button, and
// by the explicit `--sandbox` startup path (main()) where the sandbox is
// already known and MCP login is purely an add-on, not a prerequisite.
async function attemptMcpLogin() {
  try {
    await mcpLogin();
  } catch (err) {
    console.error("parrot-tauri: mcp_login failed", err);
    showMcpLoginModal(String(err));
    return;
  }
  await loadMcpToolsAndToday();
}

async function submit(promptText) {
  const prompt = promptText.trim();
  if (!prompt || turnRunning) return;

  showChatView();
  const input = document.getElementById("composer-input");
  input.value = "";
  input.style.height = "auto";
  turnRunning = true;
  turnThinkingStartedAt = null;
  turnThinkingEndedAt = null;
  document.getElementById("composer-hint").textContent = "running…";

  // `submit_turn` returns as soon as it has *spawned* the exec (fire-and-
  // forget `runtime.spawn`, not awaited) -- its own promise resolves long
  // before the turn actually finishes streaming. So `turnRunning` is only
  // ever cleared from here, off the channel's own terminal status, not
  // from an `invoke().finally()` -- otherwise this flips false (and the
  // agent-group summary below stops being "live") while the turn is still
  // very much in progress.
  const channel = new Channel();
  let sawTerminalStatus = false;
  channel.onmessage = (snapshot) => {
    // Detect this turn's thinking phase incrementally (only in newly
    // arrived entries) -- see turnThinkingStartedAt/turnThinkingEndedAt's
    // doc comment for why this can't be derived from a pure re-render.
    for (const entry of snapshot.log.slice(lastBackendLog.length)) {
      const type = entry.event.type;
      const isThinking = type === "Thinking" || type === "ThinkingTokens";
      if (isThinking && turnThinkingStartedAt === null) {
        turnThinkingStartedAt = Date.now();
      } else if (!isThinking && turnThinkingStartedAt !== null && turnThinkingEndedAt === null) {
        turnThinkingEndedAt = Date.now();
      }
    }
    lastBackendLog = snapshot.log;

    if (snapshot.status.type !== "Running" && !sawTerminalStatus) {
      sawTerminalStatus = true;
      turnRunning = false;
      document.getElementById("composer-hint").textContent = "";
      noteSandboxOk();
      turnsCompleted += 1;
      updateComposerPlaceholder();
    }

    // Render after the turnRunning update above, not before -- the
    // agent-group unit needs the up-to-date value to collapse on this
    // same render instead of staying open until some unrelated later
    // render happens to fire.
    renderChatLog();
    if (!document.getElementById("chat-layout").classList.contains("hidden")) {
      renderEvidenceRows(evidenceRowsFromLog(lastBackendLog));
    }
    renderStatus(snapshot.status);
  };

  try {
    await invoke("submit_turn", { prompt, onEvent: channel });
  } catch (err) {
    console.error("parrot-tauri: submit_turn failed", err);
    // Only reachable if submit_turn itself returned Err before spawning
    // (e.g. "not connected"/"a turn is already running") -- in that case
    // no channel message ever arrives to clear this otherwise.
    if (!sawTerminalStatus) {
      turnRunning = false;
      document.getElementById("composer-hint").textContent = "";
    }
  }
}

async function main() {
  const greeting = await invoke("greeting");
  applyTheme(greeting.theme);

  // Identity is resolved independently of any sandbox (see ParrotClient::
  // connect/resolve_workspace), so this renders regardless of whether
  // `greeting.sandbox` is set yet. `attemptMcpLogin` overwrites the
  // avatar/name with the MCP Gateway's own identity once that login
  // succeeds, since that's the one "your book only" actually refers to.
  const displayName = greeting.display_name || greeting.workspace || "Banker";
  document.getElementById("header-avatar").textContent = initialsFor(displayName);
  document.getElementById("header-identity-name").textContent = displayName;

  document.getElementById("splash-greeting").textContent = greeting.identity_greeting;
  document.getElementById("splash-gateway").textContent =
    `Gateway: ${greeting.gateway_name} (${greeting.gateway_endpoint})`;

  mcpAvailable = greeting.mcp_available;
  updateToolsToggleState();
  document.getElementById("mcp-login-modal-btn").addEventListener("click", attemptMcpLogin);
  document.getElementById("mcp-login-modal-dismiss").addEventListener("click", hideMcpLoginModal);

  document.getElementById("header-view-toggle").addEventListener("click", () => {
    const inChat = !document.getElementById("chat-layout").classList.contains("hidden");
    if (inChat) {
      if (!mcpConnected) {
        showMcpLoginModal();
        return;
      }
      if (activeTool) showPlaygroundView(activeTool);
      else showPlaygroundLanding();
    } else {
      showChatView();
    }
  });
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
  updateComposerPlaceholder();
  input.addEventListener("keydown", (event) => {
    // Only reachable once a sandbox is attached -- #app (and this input)
    // stays `display: none` until then, so this can't be used to bypass
    // Connect.
    if (sandboxName) dismissSplash();
    if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      submit(input.value);
    }
  });
  input.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = `${input.scrollHeight}px`;
  });

  if (greeting.sandbox) {
    // `--sandbox` was given explicitly at launch -- same auto-dismissing
    // splash as before.
    applySandboxInfo(greeting.sandbox, greeting.sandbox_phase);
    if (greeting.no_splash) {
      dismissSplash();
    } else {
      setTimeout(dismissSplash, SPLASH_DURATION_MS);
    }
    if (greeting.prompt) {
      submit(greeting.prompt);
    }
    if (mcpAvailable) {
      attemptMcpLogin();
    }
  } else {
    // No `--sandbox`: identity is known, but which sandbox to attach to
    // isn't -- show the Connect button and wait for the user (ignoring
    // `no_splash`, since there'd be nothing to auto-dismiss into).
    pendingPrompt = greeting.prompt || null;
    document.getElementById("header-readonly-text").textContent = "SSO · not connected";
    const connectBtn = document.getElementById("splash-connect-btn");
    connectBtn.classList.remove("hidden");
    connectBtn.addEventListener("click", connectSandbox);
  }

  input.focus();
}

main().catch((err) => {
  console.error("parrot-tauri: failed to initialize", err);
  const greetingEl = document.getElementById("splash-greeting");
  greetingEl.textContent = `Error: ${err}`;
});
