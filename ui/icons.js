// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #130: THE icon source for the graph card's glyphs and every settings cog in the app. The glyph geometry is
   Lucide's (lucide-icons/lucide, ISC — see THIRD-PARTY.md): settings, rotate-ccw, x, chevron-down. The class names
   are the public lucide-<name> / right-triangle hooks themes and plugins select on (observed in the vault-compatible
   app's DOM, no asset copied). One copy, loaded before every other script (index.html): the graph card (main.js
   GF_ICON), the vault bar's Settings gear (#sgear), the plugin API setIcon("settings"/"gear")
   (plugins/api-functions.js) and the canvas settings button (plugins/canvas-nav.js) all draw from here, so the
   Open-graph-settings cog can never again drift from the other cogs. */
const SVG_ICON = {
  "lucide-settings": '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>',
  "lucide-rotate-ccw": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  "lucide-x": '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  "right-triangle": '<path d="m6 9 6 6 6-6"/>',   // Lucide chevron-down (expanded); CSS turns it -90deg when collapsed
};
const svgIcon = id => '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="svg-icon ' + id + '">' + SVG_ICON[id] + '</svg>';
