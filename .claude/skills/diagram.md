---
description: Generate clean, professional SVG diagrams from Mermaid syntax or plain-text descriptions. Produces hand-crafted SVG with automatic light/dark mode support.
---

# Diagram Generation Skill

You generate **hand-crafted SVG diagrams** — not Mermaid renders. The input
may be Mermaid syntax (which you parse to understand the structure) or a
plain-text description. Either way, the output is a standalone `.svg` file
written to the `docs/diagrams/` directory.

## Before you start

1. Read the theme reference:
   `.claude/skills/diagram-theme.md`
2. Look at 1–2 existing diagrams in `docs/diagrams/` to calibrate on the
   visual style (spacing, font sizes, element positioning).

## Input handling

- **Mermaid input:** Parse the Mermaid syntax to extract nodes, edges,
  subgraphs, labels, and flow direction. Then render the equivalent structure
  as hand-crafted SVG elements using the theme.
- **Text input:** Interpret the description to determine the best diagram
  type (sequence, flowchart, architecture, etc.) and generate accordingly.
- **Existing diagram update:** If the user points to an existing SVG in
  `docs/diagrams/`, read it, understand the structure, and modify it.

## Output requirements

### Structure

Every SVG must follow this structure:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}"
     font-family="Inter, -apple-system, BlinkMacSystemFont, sans-serif">
  <style>
    /* CSS classes + dark mode overrides (see below) */
  </style>
  <defs>
    <!-- Arrow markers, gradients -->
  </defs>

  <!-- Background -->
  <!-- Header bar -->
  <!-- Title -->
  <!-- Content (nodes, arrows, labels, containers) -->
</svg>
```

### Dual-mode CSS (light + dark)

Use CSS classes on ALL visual elements instead of inline `fill`/`stroke`
attributes. This enables the `@media (prefers-color-scheme: dark)` block to
override colors automatically.

Here is the full CSS block to embed. Include ALL of these classes — only omit
semantic colors you don't use in the specific diagram:

```xml
<style>
  /* ── Light mode (default) ── */
  .bg             { fill: #f8fafc }
  .surface        { fill: #f1f5f9 }
  .card           { fill: #ffffff }
  .header-bar     { fill: #1e293b }
  .header-text    { fill: #f8fafc }
  .header-muted   { fill: #94a3b8 }
  .text           { fill: #1e293b }
  .text-mid       { fill: #475569 }
  .text-muted     { fill: #64748b }
  .text-faint     { fill: #94a3b8 }
  .line-subtle    { stroke: #cbd5e1 }
  .line-default   { stroke: #475569 }
  .border-subtle  { stroke: #e2e8f0 }

  /* Semantic: blue */
  .fill-blue      { fill: #dbeafe }
  .stroke-blue    { stroke: #2563eb }
  .text-blue      { fill: #1e40af }
  .arrow-blue     { fill: #4f46e5 }
  .line-blue      { stroke: #4f46e5 }

  /* Semantic: indigo */
  .fill-indigo    { fill: #e0e7ff }
  .stroke-indigo  { stroke: #4f46e5 }
  .text-indigo    { fill: #3730a3 }
  .arrow-indigo   { fill: #4f46e5 }
  .line-indigo    { stroke: #4f46e5 }

  /* Semantic: green */
  .fill-green     { fill: #dcfce7 }
  .stroke-green   { stroke: #16a34a }
  .text-green     { fill: #166534 }
  .arrow-green    { fill: #16a34a }
  .line-green     { stroke: #16a34a }

  /* Semantic: amber */
  .fill-amber     { fill: #fef3c7 }
  .stroke-amber   { stroke: #d97706 }
  .text-amber     { fill: #92400e }
  .arrow-amber    { fill: #d97706 }
  .line-amber     { stroke: #d97706 }

  /* Semantic: pink */
  .fill-pink      { fill: #fce7f3 }
  .stroke-pink    { stroke: #db2777 }
  .text-pink      { fill: #9d174d }
  .arrow-pink     { fill: #db2777 }
  .line-pink      { stroke: #db2777 }

  /* Semantic: purple */
  .fill-purple    { fill: #f3e8ff }
  .stroke-purple  { stroke: #9333ea }
  .text-purple    { fill: #6b21a8 }
  .arrow-purple   { fill: #9333ea }
  .line-purple    { stroke: #9333ea }

  /* Semantic: red */
  .fill-red       { fill: #fee2e2 }
  .stroke-red     { stroke: #dc2626 }
  .text-red       { fill: #991b1b }
  .arrow-red      { fill: #dc2626 }
  .line-red       { stroke: #dc2626 }

  /* Semantic: teal */
  .fill-teal      { fill: #ccfbf1 }
  .stroke-teal    { stroke: #0d9488 }
  .text-teal      { fill: #115e59 }
  .arrow-teal     { fill: #0d9488 }
  .line-teal      { stroke: #0d9488 }

  /* Containers */
  .alt-border     { stroke: #94a3b8 }
  .alt-label-bg   { fill: #e2e8f0 }
  .alt-label-text { fill: #475569 }

  /* Phase headers */
  .phase-bg-indigo   { fill: #eef2ff; stroke: #c7d2fe }
  .phase-text-indigo { fill: #3730a3 }
  .phase-bg-pink     { fill: #fdf2f8; stroke: #fbcfe8 }
  .phase-text-pink   { fill: #9d174d }
  .phase-bg-green    { fill: #f0fdf4; stroke: #bbf7d0 }
  .phase-text-green  { fill: #166534 }
  .phase-bg-amber    { fill: #fefce8; stroke: #fde68a }
  .phase-text-amber  { fill: #854d0e }

  /* ── Dark mode ── */
  @media (prefers-color-scheme: dark) {
    .bg             { fill: #0f172a }
    .surface        { fill: #1e293b }
    .card           { fill: #1e293b }
    .header-bar     { fill: #0f172a }
    .header-text    { fill: #e2e8f0 }
    .header-muted   { fill: #64748b }
    .text           { fill: #e2e8f0 }
    .text-mid       { fill: #94a3b8 }
    .text-muted     { fill: #94a3b8 }
    .text-faint     { fill: #64748b }
    .line-subtle    { stroke: #475569 }
    .line-default   { stroke: #94a3b8 }
    .border-subtle  { stroke: #334155 }

    .fill-blue      { fill: #1e3a5f }
    .stroke-blue    { stroke: #60a5fa }
    .text-blue      { fill: #93c5fd }
    .arrow-blue     { fill: #818cf8 }
    .line-blue      { stroke: #818cf8 }

    .fill-indigo    { fill: #272461 }
    .stroke-indigo  { stroke: #818cf8 }
    .text-indigo    { fill: #a5b4fc }
    .arrow-indigo   { fill: #818cf8 }
    .line-indigo    { stroke: #818cf8 }

    .fill-green     { fill: #14382a }
    .stroke-green   { stroke: #4ade80 }
    .text-green     { fill: #86efac }
    .arrow-green    { fill: #4ade80 }
    .line-green     { stroke: #4ade80 }

    .fill-amber     { fill: #3b2f10 }
    .stroke-amber   { stroke: #fbbf24 }
    .text-amber     { fill: #fcd34d }
    .arrow-amber    { fill: #fbbf24 }
    .line-amber     { stroke: #fbbf24 }

    .fill-pink      { fill: #4a1942 }
    .stroke-pink    { stroke: #f472b6 }
    .text-pink      { fill: #f9a8d4 }
    .arrow-pink     { fill: #f472b6 }
    .line-pink      { stroke: #f472b6 }

    .fill-purple    { fill: #2e1a47 }
    .stroke-purple  { stroke: #a78bfa }
    .text-purple    { fill: #c4b5fd }
    .arrow-purple   { fill: #a78bfa }
    .line-purple    { stroke: #a78bfa }

    .fill-red       { fill: #451a1a }
    .stroke-red     { stroke: #f87171 }
    .text-red       { fill: #fca5a5 }
    .arrow-red      { fill: #f87171 }
    .line-red       { stroke: #f87171 }

    .fill-teal      { fill: #0f3d3a }
    .stroke-teal    { stroke: #2dd4bf }
    .text-teal      { fill: #5eead4 }
    .arrow-teal     { fill: #2dd4bf }
    .line-teal      { stroke: #2dd4bf }

    .alt-border     { stroke: #475569 }
    .alt-label-bg   { fill: #334155 }
    .alt-label-text { fill: #94a3b8 }

    .phase-bg-indigo   { fill: #1e1b4b; stroke: #312e81 }
    .phase-text-indigo { fill: #a5b4fc }
    .phase-bg-pink     { fill: #3b0a2a; stroke: #831843 }
    .phase-text-pink   { fill: #f9a8d4 }
    .phase-bg-green    { fill: #052e16; stroke: #14532d }
    .phase-text-green  { fill: #86efac }
    .phase-bg-amber    { fill: #2a1f06; stroke: #451a03 }
    .phase-text-amber  { fill: #fcd34d }
  }
</style>
```

### Arrow markers in `<defs>`

Use class-based fills so dark mode can override them. Define one `arrow-default`
and one per semantic color you use:

```xml
<defs>
  <marker id="arrow-default" viewBox="0 0 10 7" refX="10" refY="3.5"
          markerWidth="10" markerHeight="7" orient="auto-start-reverse">
    <polygon points="0 0, 10 3.5, 0 7" class="text-mid"/>
  </marker>
  <marker id="arrow-blue" viewBox="0 0 10 7" refX="10" refY="3.5"
          markerWidth="10" markerHeight="7" orient="auto-start-reverse">
    <polygon points="0 0, 10 3.5, 0 7" class="arrow-blue"/>
  </marker>
  <!-- ... one per color used ... -->
  <linearGradient id="header-grad" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0%" class="header-bar" stop-color="#1e293b"/>
    <stop offset="100%" class="header-bar" stop-color="#334155"/>
  </linearGradient>
</defs>
```

**Important:** SVG `<marker>` elements do NOT inherit CSS `fill` from
`@media` queries in most renderers. To make arrow colors work in dark mode,
define **two** markers per color — one with the light fill and one with the
dark fill — and use CSS to hide/show them:

```xml
<!-- Alternative: use inline style with currentColor on the line -->
```

Actually, the simplest reliable approach: use **inline `fill` on markers**
(they don't respond well to CSS classes in all renderers) and accept that
arrow tips stay the same color in both modes. The arrow *lines* will still
change via CSS classes on the `<line>` elements, and the tip color difference
is negligible for semantic colors that already have good contrast in both
modes. This is the pragmatic trade-off — markers are the one SVG element
where CSS class overrides are unreliable across renderers.

### Sizing

- Set `viewBox` to fit the content — measure your content and add ~40px
  padding on each side.
- Do NOT set `width`/`height` attributes — let the `viewBox` control aspect
  ratio so the SVG scales responsively.
- Aim for a viewBox width of 900–1200 and height proportional to content.

### Element guidelines

- **Actors/nodes:** Pill shape (rx=18) for actors in sequence diagrams.
  Rounded rect (rx=6–8) for flowchart nodes.
- **Step labels:** Small rounded rect (rx=4) as a background behind step
  text for readability.
- **Containers/groups:** Dashed stroke (6,3) with an ALT/OPT/LOOP label
  badge in the top-left corner.
- **Lifelines:** Dashed (4,4) vertical lines from actor pills.
- **Phase headers:** Full-width tinted row with bold phase title, using the
  phase-bg and phase-text classes.
- **Return arrows:** Dashed (5,3) lines.
- **Spacing:** 30–40px between steps. 20px padding inside containers.

## Diagram types

### Sequence diagram
Follow the layout in the existing `flow-*.svg` files: actors as pills at the
top, vertical lifelines, horizontal arrows with labels, ALT/OPT boxes for
branching, phase headers for grouping.

### Flowchart / process diagram
Top-to-bottom or left-to-right layout. Rounded rect nodes. Connector lines
with arrow markers. Decision diamonds as rotated squares. Use subgraph
containers for grouping related nodes.

### Architecture diagram
Nested containers for layers (cluster, namespace, pod). Actor nodes inside.
Labeled connection lines between components.

### Generic
For any other type, use the color palette and sizing conventions but lay out
the elements as appropriate for the content.

## Workflow

1. Read the user's input (Mermaid or text).
2. Determine diagram type and content structure.
3. Plan the layout (estimate viewBox dimensions, position elements).
4. Write the SVG file to `docs/diagrams/{name}.svg`.
5. Tell the user where the file is and suggest they open it in a browser to
   verify.

## Quality checks

Before finishing:
- Every text element must have a class for fill (not an inline `fill`
  attribute) — except inside `<marker>` and `<linearGradient>`.
- The `<style>` block must contain both the light defaults and the
  `@media (prefers-color-scheme: dark)` overrides.
- The SVG must be valid XML — close all tags, escape `<` and `&` in text.
- No `width`/`height` attributes on the root `<svg>` — viewBox only.
- Font family must be set on the root `<svg>` element.
