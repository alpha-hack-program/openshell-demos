// Polls GET /api/graph (see ../../src/main.rs) and draws a three-column
// SVG graph: users -> sandboxes -> MCP servers. Sandbox node fill color
// interpolates green (risk 0) -> amber -> red (risk 3, the "why" shown on
// hover as risk_level); nodes with no heartbeat within
// graph.heartbeat_stale_secs render dimmed regardless of risk color. No
// build-time routing/state library — one page, one poll loop, plain
// Preact + hooks is enough.
import { Fragment, h, render } from "preact";
import { useEffect, useState } from "preact/hooks";

const REFRESH_MS = 5000;
const COL_WIDTH = 280;
const ROW_HEIGHT = 44;
const TOP_MARGIN = 40;
const LEFT_MARGIN = 40;

const RISK_STOPS = [
  [63, 185, 80], // green — risk 0
  [210, 153, 34], // amber — risk 1.5
  [248, 81, 73], // red — risk 3
];

function riskColor(score) {
  if (score == null) return "#238636"; // no verdict yet — neutral dark green
  const t = Math.max(0, Math.min(3, score)) / 3;
  const segment = t < 0.5 ? 0 : 1;
  const localT = t < 0.5 ? t / 0.5 : (t - 0.5) / 0.5;
  const [r1, g1, b1] = RISK_STOPS[segment];
  const [r2, g2, b2] = RISK_STOPS[segment + 1];
  const lerp = (a, b) => Math.round(a + (b - a) * localT);
  return `rgb(${lerp(r1, r2)}, ${lerp(g1, g2)}, ${lerp(b1, b2)})`;
}

function isStale(sandbox, staleSecs, nowUnix) {
  if (sandbox.last_seen_unix == null) return true;
  return nowUnix - sandbox.last_seen_unix > staleSecs;
}

function useGraph() {
  const [graph, setGraph] = useState({
    sandboxes: [],
    heartbeat_stale_secs: 90,
    generated_at_unix: 0,
  });

  useEffect(() => {
    let cancelled = false;
    async function tick() {
      try {
        const res = await fetch("/api/graph");
        const data = await res.json();
        if (!cancelled) setGraph(data);
      } catch (e) {
        console.error("audit-dashboard: failed to fetch /api/graph", e);
      }
    }
    tick();
    const id = setInterval(tick, REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  return graph;
}

function App() {
  const graph = useGraph();
  const [hover, setHover] = useState(null);
  const nowUnix = Math.floor(Date.now() / 1000);

  const users = [...new Set(graph.sandboxes.map((s) => s.workspace))].sort();
  const mcpServers = [
    ...new Set(graph.sandboxes.flatMap((s) => s.mcp_servers)),
  ].sort();

  const userX = LEFT_MARGIN;
  const sandboxX = LEFT_MARGIN + COL_WIDTH;
  const mcpX = LEFT_MARGIN + COL_WIDTH * 2;

  const yFor = (list, key) =>
    Object.fromEntries(list.map((item, i) => [key(item), TOP_MARGIN + i * ROW_HEIGHT]));

  const userY = yFor(users, (u) => u);
  const sandboxY = yFor(graph.sandboxes, (s) => s.sandbox);
  const mcpY = yFor(mcpServers, (m) => m);

  const rows = Math.max(users.length, graph.sandboxes.length, mcpServers.length, 1);
  const height = TOP_MARGIN + rows * ROW_HEIGHT + 20;
  const width = mcpX + COL_WIDTH;

  return h(
    Fragment,
    null,
    h("h1", null, "OpenShell audit trail"),
    h(
      "p",
      { class: "subtitle" },
      `Live from Prometheus, refreshed every ${REFRESH_MS / 1000}s — namespace-scoped session-auditor metrics.`,
    ),
    h(
      "div",
      { class: "legend" },
      h("span", null, h("i", { class: "swatch", style: { background: riskColor(0) } }), "no risk"),
      h("span", null, h("i", { class: "swatch", style: { background: riskColor(3) } }), "risk detected"),
      h("span", null, h("i", { class: "swatch", style: { background: "#8b949e", opacity: 0.5 } }), "no recent heartbeat"),
    ),
    h(
      "svg",
      { viewBox: `0 0 ${width} ${height}` },
      graph.sandboxes.map((s) =>
        h("line", {
          key: `us-${s.sandbox}`,
          x1: userX + 90,
          y1: (userY[s.workspace] ?? 0) + 10,
          x2: sandboxX,
          y2: sandboxY[s.sandbox] + 10,
          stroke: "#30363d",
          "stroke-width": 1.5,
        }),
      ),
      graph.sandboxes.flatMap((s) =>
        s.mcp_servers.map((m) =>
          h("line", {
            key: `sm-${s.sandbox}-${m}`,
            x1: sandboxX + 200,
            y1: sandboxY[s.sandbox] + 10,
            x2: mcpX,
            y2: (mcpY[m] ?? 0) + 10,
            stroke: "#30363d",
            "stroke-width": 1.5,
          }),
        ),
      ),
      users.map((u) =>
        h(
          Fragment,
          { key: `u-${u}` },
          h("circle", { cx: userX + 8, cy: userY[u] + 10, r: 8, fill: "#58a6ff" }),
          h("text", { x: userX + 24, y: userY[u] + 15, fill: "#e6edf3", "font-size": 13 }, u),
        ),
      ),
      graph.sandboxes.map((s) => {
        const stale = isStale(s, graph.heartbeat_stale_secs, nowUnix);
        return h(
          Fragment,
          { key: `s-${s.sandbox}` },
          h("circle", {
            cx: sandboxX + 8,
            cy: sandboxY[s.sandbox] + 10,
            r: 10,
            fill: riskColor(s.risk_score),
            opacity: stale ? 0.35 : 1,
            stroke: stale ? "#8b949e" : "#0b0f14",
            "stroke-width": 2,
            onMouseEnter: () => setHover(s),
            onMouseLeave: () => setHover((h) => (h === s ? null : h)),
          }),
          h(
            "text",
            { x: sandboxX + 26, y: sandboxY[s.sandbox] + 15, fill: "#e6edf3", "font-size": 13 },
            `${s.sandbox} (${s.agent || "?"})${stale ? " — offline" : ""}`,
          ),
        );
      }),
      mcpServers.map((m) =>
        h(
          Fragment,
          { key: `m-${m}` },
          h("circle", { cx: mcpX + 8, cy: mcpY[m] + 10, r: 8, fill: "#8957e5" }),
          h("text", { x: mcpX + 24, y: mcpY[m] + 15, fill: "#e6edf3", "font-size": 13 }, m),
        ),
      ),
    ),
    hover &&
      h(
        "div",
        { class: "tooltip" },
        h("strong", null, hover.sandbox),
        h("br"),
        `agent: ${hover.agent || "unknown"}`,
        h("br"),
        `risk: ${hover.risk_level || "none"} (score ${hover.risk_score ?? 0})`,
        h("br"),
        `mcp servers: ${hover.mcp_servers.length ? hover.mcp_servers.join(", ") : "(none observed yet)"}`,
        h("br"),
        `last seen: ${
          hover.last_seen_unix
            ? new Date(hover.last_seen_unix * 1000).toLocaleTimeString()
            : "never"
        }`,
      ),
  );
}

render(h(App), document.getElementById("app"));
