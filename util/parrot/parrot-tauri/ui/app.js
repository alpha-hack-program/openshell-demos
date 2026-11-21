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

import { renderToolForm } from "./schema-form.js?v=3";

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
// locally-synthesized "brief" entries (src/meeting_brief.rs's output,
// never sent through the sandbox at all -- see generateBrief() below).
// Each synthetic entry records how many backend log entries existed when
// it was created, so re-merging after a later real turn still places it
// correctly instead of always jumping to the end.
let lastBackendLog = [];
const syntheticEntries = []; // { afterCount, prompt, brief, error, loading }

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
      merged.push({ kind: "brief", data: synthByPosition[si] });
      si++;
    }
    if (i < lastBackendLog.length) {
      merged.push({ kind: "agent", data: lastBackendLog[i] });
    }
  }
  return merged;
}

function renderChatLog() {
  const scroller = document.getElementById("log");
  const entries = document.getElementById("log-entries");
  const timeline = mergedTimeline();
  document.getElementById("log-empty-state").classList.toggle("hidden", timeline.length > 0);
  entries.innerHTML = timeline
    .map((item) => {
      if (item.kind === "agent") {
        const { cls, text, markdown } = classifyEntry(item.data);
        const html = markdown ? renderMarkdown(escapeHtml(text)) : escapeHtml(text);
        return `<div class="log-entry ${cls}">${html}</div>`;
      }
      return renderBriefEntryHtml(item.data);
    })
    .join("");
  scroller.scrollTop = scroller.scrollHeight;
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
      return `<button class="book-client" type="button" data-prompt="Tell me about ${escapeHtml(c.name)}">
        <div class="avatar-circle">${escapeHtml(initialsFor(c.name))}</div>
        <div style="flex:1; min-width:0;">
          <div class="book-client-name">${escapeHtml(c.name)}</div>
          <div class="badge-row">${badges.join("")}</div>
        </div>
      </button>`;
    })
    .join("");
  document.querySelectorAll(".book-client").forEach((el) => {
    el.addEventListener("click", () => {
      showChatView();
      submit(el.dataset.prompt);
    });
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
    document.getElementById("log").scrollTo({ top: document.getElementById("log").scrollHeight, behavior: "smooth" });
    return;
  }
  briefRequested = true;
  btn.disabled = true;
  btn.textContent = "Generating…";
  showChatView();

  const entry = {
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
    cachedBrief = brief;
    renderEvidenceRows(evidenceRowsFromBrief(brief));
    const reg = brief.regulatory;
    document.getElementById("regulatory-card").classList.toggle("hidden", !reg);
    if (reg) {
      document.getElementById("regulatory-text").textContent = reg.text;
      // Just the source filename -- "fictional corpus" was the mockup's
      // own flavor text for its hand-written illustrative example; this
      // is a real search_regulatory_guidance result, not fabricated, and
      // labeling it otherwise would be actively misleading.
      document.getElementById("regulatory-source").textContent = reg.source;
    }
    renderAttention(brief);
    btn.textContent = "Brief ready · open";
    btn.disabled = false;
  } catch (err) {
    entry.loading = false;
    entry.error = String(err);
    briefRequested = false;
    btn.textContent = "Generate brief";
    btn.disabled = false;
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
