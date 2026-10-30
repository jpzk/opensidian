// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* the "obsidian" module, part FNS (api-parity stage 0: split out of api.js,
   behaviour-neutral): the top-level functions — icons (setIcon getIcon
   addIcon removeIcon getIconIds), normalizePath, debounce, prepare*Search,
   renderMatches, parseFrontMatter*, requestUrl, htmlToMarkdown,
   sanitizeHTMLToDom, parseYaml / stringifyYaml, getAllTags, parseLinktext,
   getLinkpath; api-parity stage 1 (fns:links, exported through
   K.exportValues): getLanguage, base64/hex/ArrayBuffer helpers, setTooltip /
   displayTooltip, getFrontMatterInfo, parseFrontMatterStringArray,
   parsePropertyId, stripHeading*, resolveSubpath, iterate*Refs,
   sortSearchResults / renderResults, and the design-gated loaders.
   Segment contract: see api-core.js. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("fns:icons", function (K) {
    const { host, be } = K;
    /* ------------------------------------------------------------- icons
       A tiny built-in set (drawn here, lucide-style 24x24 strokes) + addIcon().
       An unknown name draws a neutral square so a button never renders empty. */
    const P = (d) => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
    const ICONS = {
      "dice": P('<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1"/><circle cx="15.5" cy="15.5" r="1"/><circle cx="12" cy="12" r="1"/>'),
      "calendar": P('<rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/>'),
      "calendar-days": P('<rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18M8 14h.01M12 14h.01M16 14h.01M8 18h.01M12 18h.01"/>'),
      "calendar-range": P('<rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18M7 14h4M13 18h4"/>'),
      "plus": P('<path d="M12 5v14M5 12h14"/>'),
      "plus-circle": P('<circle cx="12" cy="12" r="10"/><path d="M12 8v8M8 12h8"/>'),
      "x": P('<path d="M18 6L6 18M6 6l12 12"/>'),
      "cross": P('<path d="M18 6L6 18M6 6l12 12"/>'),
      "check": P('<path d="M20 6L9 17l-5-5"/>'),
      "trash": P('<path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"/>'),
      "trash-2": P('<path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6"/>'),
      "pencil": P('<path d="M17 3l4 4L7 21H3v-4z"/>'),
      "settings": P('<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>'),
      "gear": P('<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>'),
      "search": P('<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>'),
      "chevron-down": P('<path d="M6 9l6 6 6-6"/>'),
      "chevron-up": P('<path d="M18 15l-6-6-6 6"/>'),
      "chevron-right": P('<path d="M9 18l6-6-6-6"/>'),
      "chevron-left": P('<path d="M15 18l-6-6 6-6"/>'),
      "arrow-up": P('<path d="M12 19V5M5 12l7-7 7 7"/>'),
      "arrow-down": P('<path d="M12 5v14M19 12l-7 7-7-7"/>'),
      "file": P('<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6"/>'),
      "file-text": P('<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M8 13h8M8 17h6"/>'),
      "folder": P('<path d="M3 7V5a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>'),
      "star": P('<path d="M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8L12 17.8 5.8 21l1.2-6.8-5-4.9 6.9-1z"/>'),
      "info": P('<circle cx="12" cy="12" r="10"/><path d="M12 16v-4M12 8h.01"/>'),
      "command": P('<path d="M9 6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3z"/>'),
      "terminal": P('<path d="M4 17l6-6-6-6M12 19h8"/>'),
      "more-vertical": P('<circle cx="12" cy="5" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="12" cy="19" r="1"/>'),
      "more-horizontal": P('<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>'),
      "grip-vertical": P('<circle cx="9" cy="6" r="1"/><circle cx="9" cy="12" r="1"/><circle cx="9" cy="18" r="1"/><circle cx="15" cy="6" r="1"/><circle cx="15" cy="12" r="1"/><circle cx="15" cy="18" r="1"/>'),
      "eye": P('<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>'),
      "eye-off": P('<path d="M3 3l18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9.9 5.1A9 9 0 0 1 12 5c6 0 10 7 10 7a17 17 0 0 1-3.2 3.9M6.6 6.6A17 17 0 0 0 2 12s4 7 10 7a9 9 0 0 0 5.4-1.6"/>'),
      "zap": P('<path d="M13 2L3 14h9l-1 8 10-12h-9z"/>'),
      "link": P('<path d="M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>'),
      "refresh-cw": P('<path d="M21 2v6h-6M3 22v-6h6M3.5 9a9 9 0 0 1 15-3.4L21 8M3 16l2.5 2.4A9 9 0 0 0 20.5 15"/>'),
      "filter": P('<path d="M22 3H2l8 9.46V19l4 2v-8.54z"/>'),
      "copy": P('<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>'),
      "external-link": P('<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3"/>'),
      "image": P('<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="M21 15l-5-5L5 21"/>'),
      "palette": P('<circle cx="12" cy="12" r="10"/><circle cx="8" cy="10" r="1"/><circle cx="12" cy="7" r="1"/><circle cx="16" cy="10" r="1"/>'),
      // plug-top5: obsidian-git's source control view (its buttons name these through data-icon)
      "minus": P('<path d="M5 12h14"/>'),
      "minus-circle": P('<circle cx="12" cy="12" r="10"/><path d="M8 12h8"/>'),
      "arrow-up-circle": P('<circle cx="12" cy="12" r="10"/><path d="M12 16V8M8 12l4-4 4 4"/>'),
      "upload": P('<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"/>'),
      "download": P('<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/>'),
      "undo": P('<path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-15-6.7L3 13"/>'),
      "go-to-file": P('<path d="M14 3H6a2 2 0 0 0-2 2v4M14 3v6h6M14 3l6 6v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-1"/><path d="M2 15h10M9 12l3 3-3 3"/>'),
      // plug-top5: excalidraw's view header actions, side tool panel and menus
      "save": P('<path d="M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M7 3v5h8V3M7 21v-7h10v7"/>'),
      "presentation": P('<path d="M2 4h20M4 4v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V4M12 16v5M8 21h8"/>'),
      "maximize-2": P('<path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/>'),
      "minimize-2": P('<path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7"/>'),
      "circle-plus": P('<circle cx="12" cy="12" r="10"/><path d="M12 8v8M8 12h8"/>'),
      "square-pen": P('<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18 2l4 4-10 10H8v-4z"/>'),
      "rotate-ccw": P('<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>'),
      "monitor-play": P('<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4M10 7l5 3-5 3z"/>'),
      "layout-dashboard": P('<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/>'),
      "file-code": P('<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6M10 13l-2 2 2 2M14 13l2 2-2 2"/>'),
      "book-open-text": P('<path d="M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2zM22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z"/><path d="M16 9h3M16 13h3M5 9h3M5 13h3"/>'),
      "swatch-book": P('<path d="M11 17a4 4 0 0 1-8 0V5a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2z"/><path d="M16.7 13H19a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H7M11 8l2.3-2.3a2 2 0 0 1 2.8 0l2.8 2.8a2 2 0 0 1 0 2.8L9.9 20.2"/><circle cx="7" cy="17" r="1"/>'),
      "locate-fixed": P('<circle cx="12" cy="12" r="7"/><circle cx="12" cy="12" r="3"/><path d="M2 12h3M19 12h3M12 2v3M12 19v3"/>'),
      "hash": P('<path d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 18"/>'),
      "file-output": P('<path d="M4 7V4a2 2 0 0 1 2-2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2"/><path d="M14 2v6h6M2 15h8M7 12l3 3-3 3"/>'),
      "copy-plus": P('<rect x="8" y="8" width="14" height="14" rx="2"/><path d="M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2M15 12v6M12 15h6"/>'),
      "clipboard-copy": P('<rect x="8" y="2" width="8" height="4" rx="1"/><path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2M16 4h2a2 2 0 0 1 2 2v4M21 14H11M15 10l-4 4 4 4"/>'),
      /* plug-val E-ICONS: lucide names the top-5 plugins draw (excalidraw getIconAsJSX side stack / tools / hamburger /
         settings, git + tasks + templater buttons) that fell back to the square. Paths: lucide-static 1.38.0 (ISC);
         github / twitter / youtube from lucide-static 0.460.0 (ISC, brand icons left lucide in 1.x). document /
         checkmark / check-in-circle / reset are the legacy aliases (file / check / circle-check / rotate-ccw). */
      "book-open": P('<path d="M12 5v16"/><path d="M20.001 19A2 2 0 0022 17V5a2 2 0 00-1.999-2L16 3.002A5 5 0 0012 5a5 5 0 00-4-2H4a2 2 0 00-2 2v12a2 2 0 001.999 2H8a5 5 0 014 2 5 5 0 014-2z"/>'),
      "bookmark": P('<path d="M17 3a2 2 0 0 1 2 2v15a1 1 0 0 1-1.496.868l-4.512-2.578a2 2 0 0 0-1.984 0l-4.512 2.578A1 1 0 0 1 5 20V5a2 2 0 0 1 2-2z"/>'),
      "camera": P('<path d="M13.997 4a2 2 0 0 1 1.76 1.05l.486.9A2 2 0 0 0 18.003 7H20a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h1.997a2 2 0 0 0 1.759-1.048l.489-.904A2 2 0 0 1 10.004 4z"/><circle cx="12" cy="13" r="3"/>'),
      "code-xml": P('<path d="m18 16 4-4-4-4"/><path d="m6 8-4 4 4 4"/><path d="m14.5 4-5 16"/>'),
      "crop": P('<path d="M6 2v14a2 2 0 0 0 2 2h14"/><path d="M18 22V8a2 2 0 0 0-2-2H2"/>'),
      "file-audio": P('<path d="M4 6.835V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2h-.343"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M2 19a2 2 0 0 1 4 0v1a2 2 0 0 1-4 0v-4a6 6 0 0 1 12 0v4a2 2 0 0 1-4 0v-1a2 2 0 0 1 4 0"/>'),
      "file-code-2": P('<path d="M4 12.15V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2h-3.35"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="m5 16-3 3 3 3"/><path d="m9 22 3-3-3-3"/>'),
      "file-code-corner": P('<path d="M4 12.15V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2h-3.35"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="m5 16-3 3 3 3"/><path d="m9 22 3-3-3-3"/>'),
      "file-input": P('<path d="M4 11V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-1"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M2 15h10"/><path d="m9 18 3-3-3-3"/>'),
      "file-plus": P('<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M9 15h6"/><path d="M12 18v-6"/>'),
      "file-symlink": P('<path d="M4 11V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h7"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="m10 18 3-3-3-3"/>'),
      "ghost": P('<path d="M15 10v1"/><path d="M7.528 20.472a1.6 1.6 0 012.277 0l1.057 1.056a1.6 1.6 0 002.276 0l1.057-1.056a1.6 1.6 0 012.277 0l1.114 1.114a1.4 1.4 0 002.414-1V10a8 8 0 00-16 0v10.586a1.4 1.4 0 002.414 1z"/><path d="M9 10v1"/>'),
      "github": P('<path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4"/><path d="M9 18c-4.51 2-5-2-7-2"/>'),
      "glasses": P('<circle cx="6" cy="15" r="4"/><circle cx="18" cy="15" r="4"/><path d="M14 15a2 2 0 0 0-2-2 2 2 0 0 0-2 2"/><path d="M2.5 13 5 7c.7-1.3 1.4-2 3-2"/><path d="M21.5 13 19 7c-.7-1.3-1.5-2-3-2"/>'),
      "globe": P('<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>'),
      "graduation-cap": P('<path d="M21.42 10.922a1 1 0 0 0-.019-1.838L12.83 5.18a2 2 0 0 0-1.66 0L2.6 9.08a1 1 0 0 0 0 1.832l8.57 3.908a2 2 0 0 0 1.66 0z"/><path d="M22 10v6"/><path d="M6 12.5V16a6 3 0 0 0 12 0v-3.5"/>'),
      "heart": P('<path d="M2 9.5a5.5 5.5 0 0 1 9.591-3.676.56.56 0 0 0 .818 0A5.49 5.49 0 0 1 22 9.5c0 2.29-1.5 4-3 5.5l-5.492 5.313a2 2 0 0 1-3 .019L5 15c-1.5-1.5-3-3.2-3-5.5"/>'),
      "image-down": P('<path d="M10.3 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v10l-3.1-3.1a2 2 0 0 0-2.814.014L6 21"/><path d="m14 19 3 3v-5.5"/><path d="m17 22 3-3"/><circle cx="9" cy="9" r="2"/>'),
      "layout-template": P('<rect width="18" height="7" x="3" y="3" rx="1"/><rect width="9" height="7" x="3" y="14" rx="1"/><rect width="5" height="7" x="16" y="14" rx="1"/>'),
      "maximize": P('<path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/>'),
      "megaphone": P('<path d="M11 6a13 13 0 0 0 8.4-2.8A1 1 0 0 1 21 4v12a1 1 0 0 1-1.6.8A13 13 0 0 0 11 14H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z"/><path d="M6 14a12 12 0 0 0 2.4 7.2 2 2 0 0 0 3.2-2.4A8 8 0 0 1 10 14"/><path d="M8 6v8"/>'),
      "minimize": P('<path d="M8 3v3a2 2 0 0 1-2 2H3"/><path d="M21 8h-3a2 2 0 0 1-2-2V3"/><path d="M3 16h3a2 2 0 0 1 2 2v3"/><path d="M16 21v-3a2 2 0 0 1 2-2h3"/>'),
      "monitor-cog": P('<path d="M12 17v4"/><path d="m14.305 7.53.923-.382"/><path d="m15.228 4.852-.923-.383"/><path d="m16.852 3.228-.383-.924"/><path d="m16.852 8.772-.383.923"/><path d="m19.148 3.228.383-.924"/><path d="m19.53 9.696-.382-.924"/><path d="m20.772 4.852.924-.383"/><path d="m20.772 7.148.924.383"/><path d="M22 13v2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7"/><path d="M8 21h8"/><circle cx="18" cy="6" r="3"/>'),
      "pen-line": P('<path d="M13 21h8"/><path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/>'),
      "scan": P('<path d="M3 7V5a2 2 0 0 1 2-2h2"/><path d="M17 3h2a2 2 0 0 1 2 2v2"/><path d="M21 17v2a2 2 0 0 1-2 2h-2"/><path d="M7 21H5a2 2 0 0 1-2-2v-2"/>'),
      "sigma": P('<path d="M18 7V5a1 1 0 0 0-1-1H6.5a.5.5 0 0 0-.4.8l4.5 6a2 2 0 0 1 0 2.4l-4.5 6a.5.5 0 0 0 .4.8H17a1 1 0 0 0 1-1v-2"/>'),
      "text-select": P('<path d="M14 21h1"/><path d="M14 3h1"/><path d="M19 3a2 2 0 0 1 2 2"/><path d="M21 14v1"/><path d="M21 19a2 2 0 0 1-2 2"/><path d="M21 9v1"/><path d="M3 14v1"/><path d="M3 9v1"/><path d="M5 21a2 2 0 0 1-2-2"/><path d="M5 3a2 2 0 0 0-2 2"/><path d="M7 12h10"/><path d="M7 16h6"/><path d="M7 8h8"/><path d="M9 21h1"/><path d="M9 3h1"/>'),
      "twitter": P('<path d="M22 4s-.7 2.1-2 3.4c1.6 10-9.4 17.3-18 11.6 2.2.1 4.4-.6 6-2C3 15.5.5 9.6 3 5c2.2 2.6 5.6 4.1 9 4-.9-4.2 4-6.6 7-3.8 1.1 0 3-1.2 3-1.2z"/>'),
      "youtube": P('<path d="M2.5 17a24.12 24.12 0 0 1 0-10 2 2 0 0 1 1.4-1.4 49.56 49.56 0 0 1 16.2 0A2 2 0 0 1 21.5 7a24.12 24.12 0 0 1 0 10 2 2 0 0 1-1.4 1.4 49.55 49.55 0 0 1-16.2 0A2 2 0 0 1 2.5 17"/><path d="m10 15 5-3-5-3z"/>'),
      "arrow-up-right": P('<path d="M7 7h10v10"/><path d="M7 17 17 7"/>'),
      "clipboard-paste": P('<path d="M11 14h10"/><path d="M16 4h2a2 2 0 0 1 2 2v1.344"/><path d="m17 18 4-4-4-4"/><path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 1.793-1.113"/><rect x="8" y="2" width="8" height="4" rx="1"/>'),
      "clipboard-type": P('<rect width="8" height="4" x="8" y="2" rx="1" ry="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><path d="M9 12v-1h6v1"/><path d="M11 17h2"/><path d="M12 11v6"/>'),
      "diff": P('<path d="M12 3v14"/><path d="M5 10h14"/><path d="M5 21h14"/>'),
      "file-x": P('<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="m14.5 12.5-5 5"/><path d="m9.5 12.5 5 5"/>'),
      "git-pull-request": P('<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M13 6h3a2 2 0 0 1 2 2v7"/><line x1="6" x2="6" y1="9" y2="21"/>'),
      "history": P('<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>'),
      "scissors": P('<circle cx="6" cy="6" r="3"/><path d="M8.12 8.12 12 12"/><path d="M20 4 8.12 15.88"/><circle cx="6" cy="18" r="3"/><path d="M14.8 14.8 20 20"/>'),
      "text-cursor": P('<path d="M17 22h-1a4 4 0 0 1-4-4V6a4 4 0 0 1 4-4h1"/><path d="M7 22h1a4 4 0 0 0 4-4"/><path d="M7 2h1a4 4 0 0 1 4 4"/>'),
      "circle-check": P('<circle cx="12" cy="12" r="10"/><path d="m16 9-5.5 5.5L8 12"/>'),
      "document": P('<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/>'),
      "checkmark": P('<path d="M20 6 9 17l-5-5"/>'),
      "check-in-circle": P('<circle cx="12" cy="12" r="10"/><path d="m16 9-5.5 5.5L8 12"/>'),
      "reset": P('<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>'),
      /* obsidian-git status bar / conflict icons (lucide-static 1.38.0 circle-alert, git-branch, git-commit-horizontal,
         circle-pause under their pre-1.0 names the plugin uses) */
      "alert-circle": P('<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>'),
      "git-branch": P('<path d="M15 6a9 9 0 0 0-9 9V3"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/>'),
      "git-commit": P('<circle cx="12" cy="12" r="3"/><line x1="3" x2="9" y1="12" y2="12"/><line x1="15" x2="21" y1="12" y2="12"/>'),
      "pause-circle": P('<circle cx="12" cy="12" r="10"/><line x1="10" x2="10" y1="15" y2="9"/><line x1="14" x2="14" y1="15" y2="9"/>'),
    };
    const custom = new Map();           // addIcon: name -> svg inner content (viewBox 0 0 100 100)
    const DEFAULT_ICON = P('<rect x="4" y="4" width="16" height="16" rx="3"/>');
    function iconSvg(name) {
      name = String(name || "").replace(/^lucide-/, "");
      if (custom.has(name)) return '<svg viewBox="0 0 100 100" width="100%" height="100%" fill="currentColor">' + custom.get(name) + "</svg>";
      return ICONS[name] || DEFAULT_ICON;
    }
    function setIcon(el, name) {
      if (!el) return;
      el.innerHTML = iconSvg(name);
      const s = el.firstElementChild; if (s) { s.classList.add("svg-icon"); s.dataset.icon = String(name || ""); }
    }
    function getIcon(name) { const d = document.createElement("div"); setIcon(d, name); return d.firstElementChild; }
    const addIcon = (name, svg) => { custom.set(String(name), String(svg)); };
    const removeIcon = name => { custom.delete(String(name)); };
    const getIconIds = () => [...Object.keys(ICONS), ...custom.keys()];

    Object.assign(K, { P, addIcon, getIcon, getIconIds, removeIcon, setIcon });
  });
  reg("fns:util", function (K) {
    const { host, be } = K;
    /* ----------------------------------------------------------- utility */
    const normalizePath = p => {
      let s = String(p == null ? "" : p).replace(/\\/g, "/").replace(/[  ]/g, " ").replace(/\/+/g, "/").replace(/^\/+|\/+$/g, "");
      s = s.normalize("NFC");
      return s === "" ? "/" : s;
    };
    const debounce = (cb, ms = 0, resetTimer = false) => {
      let t = null, last = null;
      const f = function (...a) {
        last = a;
        if (t && !resetTimer) return f;
        clearTimeout(t);
        t = setTimeout(() => { t = null; cb.apply(this, last); }, ms);
        return f;
      };
      f.cancel = () => { clearTimeout(t); t = null; return f; };
      f.run = () => { if (t) { clearTimeout(t); t = null; return cb.apply(null, last); } };
      return f;
    };
    Object.assign(K, { debounce, normalizePath });
  });
  reg("fns:search", function (K) {
    const { host, be } = K;
    // fuzzy subsequence match: score favours early + contiguous hits (same idea as the app's palette)
    function prepareFuzzySearch(q) {
      const ql = String(q).toLowerCase().replace(/\s+/g, "");
      return text => {
        if (!ql) return { score: 0, matches: [] };
        const t = String(text).toLowerCase(); let i = 0, score = 0, prev = -2; const m = [];
        for (let j = 0; j < t.length && i < ql.length; j++) if (t[j] === ql[i]) { score -= (j - prev - 1) * 0.1 + j * 0.001; if (j === prev + 1 && m.length) m[m.length - 1][1] = j + 1; else m.push([j, j + 1]); prev = j; i++; }
        return i === ql.length ? { score, matches: m } : null;
      };
    }
    function prepareSimpleSearch(q) {
      const words = String(q).toLowerCase().split(/\s+/).filter(Boolean);
      return text => { const t = String(text).toLowerCase(), m = []; for (const w of words) { const i = t.indexOf(w); if (i < 0) return null; m.push([i, i + w.length]); } return { score: 0, matches: m }; };
    }
    function renderMatches(el, text, matches, offset) {
      let at = 0;
      offset = +offset || 0;
      for (const [a0, b0] of matches || []) { const a = a0 + offset, b = b0 + offset; /* offset is ADDED to each range */ if (b <= 0 || a >= text.length) continue; if (a > at) el.appendText(text.slice(at, a)); el.createSpan({ cls: "suggestion-highlight", text: text.slice(Math.max(a, 0), b) }); at = b; }
      if (at < text.length) el.appendText(text.slice(at));
    }
    const parseFrontMatterEntry = (fm, key) => {
      if (!fm) return null;
      if (key instanceof RegExp) { for (const k of Object.keys(fm)) if (key.test(k)) return fm[k]; return null; }
      return Object.prototype.hasOwnProperty.call(fm, key) ? fm[key] : null;
    };
    const parseFrontMatterAliases = fm => { const v = parseFrontMatterEntry(fm, /^alias(es)?$/i); return v == null ? null : [].concat(v).map(String); };
    const parseFrontMatterTags = fm => { const v = parseFrontMatterEntry(fm, /^tags?$/i); return v == null ? null : [].concat(v).flatMap(x => String(x).split(/[,\s]+/)).filter(Boolean).map(t => t.startsWith("#") ? t : "#" + t); };
    const requestUrl = () => Promise.reject(new Error("requestUrl is not supported in opensidian yet (M1)"));
    /* HTML is parsed into a <template>: its content document has no browsing context, so
       nothing in it runs or loads. Markdown is built by walking that DOM; every text node is
       escaped, so no tag can come through from the input as markup (code spans/blocks keep
       their text verbatim: markdown renders code as text, never as HTML). */
    const inertParse = h => { const t = document.createElement("template"); t.innerHTML = String(h); return t.content; };
    const UNSAFE_URL = v => { const s = String(v).replace(/[\u0000- \u007f-\u009f]/g, "").toLowerCase();
      return /^(javascript|vbscript):/.test(s) || (/^data:/.test(s) && !/^data:image\/(png|gif|jpe?g|webp|bmp|avif)[;,]/.test(s)); };
    const MD_SKIP = new Set(["script", "style", "noscript", "template", "iframe", "frame", "frameset", "object", "embed", "applet", "head", "title", "meta", "link", "base"]);
    const MD_BLOCK = new Set(["p", "div", "section", "article", "header", "footer", "main", "nav", "aside", "figure", "figcaption", "address", "details", "summary", "table", "thead", "tbody", "tfoot", "dl", "dt", "dd", "form", "fieldset", "center"]);
    const mdEsc = s => s.replace(/\\/g, "\\\\").replace(/[*_`[\]]/g, c => "\\" + c).replace(/&(?=#?[A-Za-z0-9]+;)/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const mdWrap = (m, s) => { const t = s.trim(); if (!t) return s; return s.slice(0, s.length - s.trimStart().length) + m + t + m + s.slice(s.trimEnd().length); };
    const mdTicks = s => { let n = 0; for (const r of s.match(/`+/g) || []) n = Math.max(n, r.length); return n; };
    const mdTight = s => s.replace(/[ \t]+\n/g, "\n").replace(/\n{2,}/g, "\n").trim();
    const mdUrl = u => String(u).trim().replace(/[\s()<>]/g, c => "%" + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0"));
    function mdNode(n) {
      if (n.nodeType === 3) return mdEsc(n.nodeValue.replace(/\s+/g, " "));
      if (n.nodeType !== 1 && n.nodeType !== 9 && n.nodeType !== 11) return "";
      const tag = n.nodeType === 1 ? n.localName.toLowerCase() : "";
      if (MD_SKIP.has(tag)) return "";
      const kids = () => Array.from(n.childNodes, mdNode).join("");
      const h = /^h([1-6])$/.exec(tag);
      if (h) return "\n\n" + "#".repeat(+h[1]) + " " + mdTight(kids()).replace(/\n/g, " ") + "\n\n";
      switch (tag) {
        case "br": return "\n";
        case "hr": return "\n\n---\n\n";
        case "strong": case "b": return mdWrap("**", kids());
        case "em": case "i": return mdWrap("*", kids());
        case "del": case "s": case "strike": return mdWrap("~~", kids());
        case "code": {
          const t = n.textContent; if (!t) return "";
          const f = "`".repeat(mdTicks(t) + 1), pad = /^`|`$/.test(t) ? " " : "";
          return f + pad + t.replace(/\n/g, " ") + pad + f;
        }
        case "pre": {
          const t = n.textContent.replace(/\n$/, ""), c = n.querySelector("code");
          const lang = c && /(?:^|\s)lang(?:uage)?-([\w+-]+)/.exec(c.className || "");
          const f = "`".repeat(Math.max(3, mdTicks(t) + 1));
          return "\n\n" + f + (lang ? lang[1] : "") + "\n" + t + "\n" + f + "\n\n";
        }
        case "a": {
          const text = kids(), href = n.getAttribute("href");
          if (!href || UNSAFE_URL(href) || !text.trim()) return text;
          return text.slice(0, text.length - text.trimStart().length) + "[" + text.trim() + "](" + mdUrl(href) + ")" + text.slice(text.trimEnd().length);
        }
        case "img": {
          const src = n.getAttribute("src");
          if (!src || UNSAFE_URL(src)) return "";
          return "![" + mdEsc((n.getAttribute("alt") || "").replace(/\s+/g, " ")) + "](" + mdUrl(src) + ")";
        }
        case "ul": case "ol": {
          let i = +(n.getAttribute("start") || 1) || 1;
          const items = [];
          for (const li of n.children) {
            if (li.localName !== "li") continue;
            const mark = tag === "ol" ? (i++) + ". " : "- ";
            const body = mdTight(Array.from(li.childNodes, mdNode).join(""));
            items.push(mark + body.replace(/\n/g, "\n" + " ".repeat(mark.length)));
          }
          return "\n\n" + items.join("\n") + "\n\n";
        }
        case "li": return "\n\n- " + mdTight(kids()) + "\n\n";
        case "blockquote": return "\n\n" + mdTight(kids()).split("\n").map(l => "> " + l).join("\n") + "\n\n";
        case "tr": return "\n" + kids().trim() + "\n";
        case "td": case "th": return " " + kids().trim() + " ";
      }
      if (MD_BLOCK.has(tag)) return "\n\n" + kids().trim() + "\n\n";
      return kids();
    }
    const htmlToMarkdown = h => {
      if (h == null || h === "") return "";
      const root = typeof Node !== "undefined" && h instanceof Node ? h : inertParse(h);
      return mdNode(root).split("\n").map(l => l.replace(/\s+$/, "")).join("\n").replace(/\n{3,}/g, "\n\n").trim();
    };
    /* sanitizeHTMLToDom: an inert fragment with no active content left in it. Dropped elements:
       script-like/embedding ones, base (re-points every relative URL), meta (refresh/http-equiv),
       link (stylesheet/preload fetches), form (credential phishing + submit-on-enter), nested
       template (its content is not walked), SVG animate/set (can rewrite href to javascript:),
       and style elements that fetch (@import / url()). Plain <style> is kept. On every element:
       on* handlers, srcdoc, and javascript:/vbscript:/non-image data: URLs in URL attributes go;
       inline style attributes that fetch (url( / @import / expression() go. */
    const SAN_DROP = "script,iframe,frame,frameset,object,embed,applet,base,meta,link,form,template,animate,set,animateMotion,animateTransform";
    const SAN_URL_ATTR = new Set(["href", "src", "action", "formaction", "data", "poster", "background", "lowsrc", "ping", "codebase", "cite", "longdesc", "dynsrc", "xlink:href"]);
    const sanitizeHTMLToDom = h => {
      const f = inertParse(h == null ? "" : h);
      for (const s of f.querySelectorAll(SAN_DROP)) s.remove();
      for (const s of f.querySelectorAll("style")) if (/@import|url\s*\(/i.test(s.textContent)) s.remove();
      for (const el of f.querySelectorAll("*")) {
        for (const a of Array.from(el.attributes)) {
          const n = a.name.toLowerCase();
          if (n.startsWith("on") || n === "srcdoc" || (n === "srcset" && /javascript:|vbscript:|data:/i.test(a.value))
            || ((SAN_URL_ATTR.has(n) || n.endsWith(":href")) && UNSAFE_URL(a.value))
            || (n === "style" && /url\s*\(|@import|expression\s*\(/i.test(a.value))) el.removeAttribute(a.name);
        }
      }
      return f;
    };

    Object.assign(K, { htmlToMarkdown, parseFrontMatterAliases, parseFrontMatterEntry, parseFrontMatterTags, prepareFuzzySearch, prepareSimpleSearch, renderMatches, requestUrl, sanitizeHTMLToDom });
  });
  reg("fns:yaml", function (K) {
    const { host, be } = K;
    /* YAML 1.2 core schema over block mappings / sequences (nested by indentation), flow [..] {..}, quoted keys and
       scalars, | > block scalars. Invalid documents throw YAMLParseError (duplicate key, unclosed flow or quote,
       bad indentation), so a note with broken frontmatter has none, as in stock. */
    function plainScalar(v) {
      if (v === "") return null;
      // every pattern below starts with one of these characters: other text skips nine regex tests (many short items)
      if (!"nNtTfF~0123456789+-.".includes(v[0])) return v;
      if (/^(null|Null|NULL|~)$/.test(v)) return null;
      if (/^(true|True|TRUE)$/.test(v)) return true;
      if (/^(false|False|FALSE)$/.test(v)) return false;
      if (/^[-+]?[0-9]+$/.test(v)) return Number(v);
      if (/^0x[0-9a-fA-F]+$/.test(v)) return parseInt(v.slice(2), 16);
      if (/^0o[0-7]+$/.test(v)) return parseInt(v.slice(2), 8);
      if (/^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$/.test(v)) return Number(v);
      if (/^[-+]?\.(inf|Inf|INF)$/.test(v)) return v[0] === "-" ? -Infinity : Infinity;
      if (/^\.(nan|NaN|NAN)$/.test(v)) return NaN;
      return v;
    }
    function parseYaml(src) {
      const lines = String(src).replace(/\r\n?/g, "\n").split("\n"), n = lines.length;
      // vl / vc: the line an inline "- " item continues on (sequence below) and the column its content starts at
      let i = 0, vl = -1, vc = 0;
      const err = (m, at = i) => { const e = new Error(m + " (line " + (at + 1) + ")"); e.name = "YAMLParseError"; return e; };
      const UNTERM = {}, anchors = Object.create(null);
      const indent = l => l.match(/^ */)[0].length;
      // the current line's indentation: an inline item ('- - x', '- k: v') reads its content as a line indented to its column
      const lead = () => i === vl ? vc : indent(lines[i]);
      const skip = () => { while (i < n && (/^\s*$/.test(lines[i]) || /^\s*#/.test(lines[i]))) i++; };
      const isSeq = t => /^-(\s|$)/.test(t);
      const noComment = s => { const m = s.match(/(^|\s)#/); return (m ? s.slice(0, m.index) : s).trim(); };
      /* A quoted or flow value may span lines. B = { s, more }: B.s is a window on the value folded so far and
         B.more(keep, esc) drops the first keep characters of the window (text the caller has consumed), then folds
         the next line onto it (false when the document ended; the caller then moves its index back by keep). The
         scanners below pull a line only when they run off the end of B.s and carry on from where they stopped, so a
         multi-line value is scanned once. The window holds about one line: growing one string by a line and then
         indexing it copies the whole string (V8 flattens the rope), so a window over the whole value made every
         fold cost the length folded so far (quadratic in the line count: a note of short lines froze the UI). */
      const X = { n: "\n", t: "\t", r: "\r", 0: "\0", a: "\x07", b: "\b", e: "\x1b", f: "\f", v: "\v", " ": " ", "/": "/", '"': '"', "\\": "\\", N: "\x85", _: "\xa0" };
      function quoted(B, p) {   // -> [value, index after the closing quote]; throws UNTERM
        const q = B.s[p]; let v = ""; p++;
        for (;;) {
          if (p >= B.s.length) { if (B.more && B.more(p)) { p = 0; continue; } throw UNTERM; }
          const s = B.s, c = s[p];
          if (q === "'") { if (c === "'") { if (s[p + 1] === "'") { v += "'"; p += 2; continue; } return [v, p + 1]; } v += c; p++; continue; }
          if (c === '"') return [v, p + 1];
          if (c !== "\\") { v += c; p++; continue; }
          const e = s[p + 1];
          // a backslash ending the line: fold, then read the window from that backslash again (an escaped break drops it)
          if (e === undefined) { if (B.more && B.more(p, true)) { p = 0; continue; } throw UNTERM; }
          if (e in X) { v += X[e]; p += 2; continue; }
          const w = { x: 2, u: 4, U: 8 }[e], h = w && s.slice(p + 2, p + 2 + w);
          if (!w || !/^[0-9a-fA-F]+$/.test(h) || h.length !== w) throw err("bad escape \\" + e);
          // a \U past U+10FFFF names no character: a parse error (String.fromCodePoint would throw a raw RangeError)
          const cp = parseInt(h, 16); if (cp > 0x10ffff) throw err("bad escape \\" + e + h);
          v += String.fromCodePoint(cp); p += 2 + w;
        }
      }
      function flow(B) {        // one flow collection starting at B.s[0]; throws UNTERM when it runs off the end
        let p = 0;
        // ws / the plain scalar test character codes (a regex per character was most of a long flow value's time);
        // non-ASCII still goes through /\s/, so the whitespace set is unchanged
        const isWs = (s, p) => { const c = s.charCodeAt(p); return c === 32 || (c >= 9 && c <= 13) || (c > 127 && /\s/.test(s[p])); };
        const ws = () => { const s = B.s; while (p < s.length && isWs(s, p)) p++; };
        const need = () => {    // skip whitespace, folding lines until there is a character to read; UNTERM at the end
          ws(); while (p >= B.s.length) { if (!(B.more && B.more(p))) throw UNTERM; p = 0; ws(); }
        };
        function item(inMap) {
          need();
          const c = B.s[p];
          if (c === "[") { p++; const a = []; need(); if (B.s[p] === "]") { p++; return a; }
            for (;;) { a.push(item(false)); need();
              if (B.s[p] === ",") { p++; need(); if (B.s[p] === "]") { p++; return a; } continue; }
              if (B.s[p] === "]") { p++; return a; } throw err("bad flow sequence"); } }
          if (c === "{") { p++; const o = {}; need(); if (B.s[p] === "}") { p++; return o; }
            for (;;) { const k = item(true); need();
              // a plain value that ends its line folds the next one in (it may go on there); a duplicate key is still
              // reported at the line the value started on, as before (the fold is not part of the error position)
              let v = null, at = i; if (B.s[p] === ":") { p++; need(); at = i; const c = B.s[p]; v = item(false); if (/[\[{"']/.test(c)) at = i; ws(); }
              const ks = String(k); if (Object.prototype.hasOwnProperty.call(o, ks)) throw err("duplicated mapping key " + ks, at); o[ks] = v;
              need();
              if (B.s[p] === ",") { p++; need(); if (B.s[p] === "}") { p++; return o; } continue; }
              if (B.s[p] === "}") { p++; return o; } throw err("bad flow mapping"); } }
          if (c === '"' || c === "'") { const [v, e] = quoted(B, p); p = e; return v; }
          let st = p, acc = "";
          for (;;) {   // a plain scalar that reaches the end of the line goes on with the folded next line
            const s = B.s;
            for (; p < s.length; p++) {   // stops at , [ ] { } and at a ":" before whitespace, , ] } or the end
              const c = s.charCodeAt(p);
              if (c === 44 || c === 91 || c === 93 || c === 123 || c === 125) break;
              if (c === 58 && (p + 1 >= s.length || ",]}".includes(s[p + 1]) || isWs(s, p + 1))) break;
            }
            if (p < s.length) break;
            const part = s.slice(st, p);   // the window drops what was read, so the scalar so far is kept here
            if (!(B.more && B.more(p))) break;
            acc += part; st = p = 0;
          }
          return plainScalar((acc + B.s.slice(st, p)).trim());
        }
        const v = item(false); ws();
        if (p < B.s.length && B.s[p] !== "#") throw err("content after flow collection");
        return v;
      }
      const KEY_HEAD = /^([^\s#\-?:,\[\]{}&*!|>'"%@`]|[-?:][^\s])/, KEY_COLON = /:(\s|$)/g;
      function keyOf(t) {        // "key: rest" -> [key, rest]; null when t is not a mapping entry
        let k, p;
        if (t[0] === '"' || t[0] === "'") { try { [k, p] = quoted({ s: t }, 0); } catch (e) { if (e === UNTERM) return null; throw e; } }
        else {
          // the key runs to the first ": " / ":" at the end of the line (whitespace before it trimmed) and holds no "#".
          // A linear scan: a lazy regex with a lookahead here backtracked quadratically over long runs of spaces.
          const h = KEY_HEAD.exec(t); if (!h) return null;
          KEY_COLON.lastIndex = h[0].length; const c = KEY_COLON.exec(t); if (!c) return null;
          if (t.lastIndexOf("#", c.index) >= h[0].length) return null;
          p = c.index; while (p > h[0].length && /\s/.test(t[p - 1])) p--;
          k = t.slice(0, p).trim();
        }
        const m = t.slice(p).match(/^\s*:(\s+|$)/);
        return m ? [k, t.slice(p + m[0].length)] : null;
      }
      function inline(s, ind) {  // a scalar or flow value that starts on the current (already consumed) line
        if (s[0] === '"' || s[0] === "'" || s[0] === "[" || s[0] === "{") {
          // pre: the folded character just before the window (what the window dropped ended with)
          const dq = s[0] === '"', B = { s, pre: "", more(keep, esc) {
            if (i >= n) return false;
            // line folding (stock): an empty line is "\n" with no space before the next text; a double-quoted line
            // ending in an unescaped backslash escapes the break (next line joins with no space: "b\" + "c" -> "bc").
            // esc: quoted() stopped on that backslash (it is B.s[keep], the window's last character); a backslash
            // the scanner reads at the end of a line is unescaped exactly when the run of them there is odd
            const t = lines[i].trim(), u = B.s, last = u.length ? u[u.length - 1] : B.pre;
            if (dq && esc) B.s = u.slice(keep, -1) + t;
            else B.s = u.slice(keep) + (t ? (last === "\n" ? t : " " + t) : "\n");
            if (keep > 0) B.pre = u[keep - 1];
            i++; return true;
          } };
          try {
            if (!dq && s[0] !== "'") return flow(B);
            const [v, e] = quoted(B, 0), rest = B.s.slice(e).trim();
            if (rest && rest[0] !== "#") throw err("content after quoted scalar");
            return v;
          } catch (e) {
            if (e !== UNTERM) throw e;
            throw err(s[0] === "[" || s[0] === "{" ? "unclosed flow collection" : "unclosed quoted scalar");
          }
        }
        // multi-line plain scalar: continuation lines indented past ind fold to one space, each empty line in between
        // is a "\n" (stock: "p: first\n  second\n\n  third" -> "first second\nthird"); trailing empty lines are not part of it
        let v = noComment(s), nl = 0;
        while (i < n) {
          const l = lines[i];
          if (!l.trim()) { nl++; i++; continue; }
          if (/^\s*#/.test(l) || indent(l) <= ind) break;
          const t = l.trim(); if (keyOf(t)) throw err("mapping values are not allowed here");
          v += (nl ? "\n".repeat(nl) : " ") + noComment(t); nl = 0; i++;
        }
        i -= nl;
        return plainScalar(v);
      }
      function block(head, ind) {   // | or > with chomping indicator; content = following lines indented past ind
        const lit = head[0] === "|", chomp = /-/.test(head) ? "-" : /\+/.test(head) ? "+" : "";
        // explicit indentation indicator (|2, >1-): content indent = ind + digit, extra spaces stay in the text
        // (stock: "i: |2" over "    four sp" -> "  four sp\n"); without it the first content line sets the indent
        const ii = head.match(/^[|>][-+]?([1-9])/), body = []; let bi = ii ? ind + Number(ii[1]) : -1;
        while (i < n) {
          const l = lines[i];
          if (l.trim()) { const d = indent(l); if (d <= ind) break; if (bi < 0) bi = d; if (d < bi) break; body.push(l.slice(bi)); }
          else body.push("");
          i++;
        }
        let tail = 0; while (body.length && body[body.length - 1] === "") { body.pop(); tail++; }
        let out = lit ? body.join("\n") : "";
        if (!lit) for (let k = 0; k < body.length; k++) {   // folded: breaks become spaces; blank / more-indented lines keep theirs
          const l = body[k], q = body[k - 1];
          out += k === 0 ? l : l === "" ? "\n" : q === "" ? l : /^\s/.test(l) || /^\s/.test(q) ? "\n" + l : " " + l;
        }
        if (!body.length) return chomp === "+" ? "\n".repeat(tail) : "";
        return chomp === "-" ? out : chomp === "+" ? out + "\n".repeat(tail + 1) : out + "\n";
      }
      function value(rest, ind, item) {   // the value after "key:" (or "- " when item) at indentation ind
        let t = rest.trim(), anc = null, tag = null, m;
        for (;;) {   // node properties: &anchor and !tag, in either order
          if ((m = t.match(/^&([^\s,\[\]{}]+)(\s+|$)/))) { anc = m[1]; t = t.slice(m[0].length); continue; }
          if ((m = t.match(/^(![^\s,\[\]{}]*)(\s+|$)/))) { tag = m[1]; t = t.slice(m[0].length); continue; }
          break;
        }
        if ((m = t.match(/^\*([^\s,\[\]{}]+)\s*(#.*)?$/))) {
          if (!Object.prototype.hasOwnProperty.call(anchors, m[1])) throw err("unidentified alias " + m[1]);
          return anchors[m[1]];
        }
        let v;
        if (!t || t[0] === "#") {
          skip(); v = null;
          if (i < n) {
            const d = lead(), s = lines[i].slice(d);
            if (d > ind) v = node(d);
            else if (d === ind && !item && isSeq(s)) v = sequence(d);
          }
        } else if (/^[|>][-+0-9]*(\s+#.*)?$/.test(t)) v = block(t, ind);
        else v = tag === "!!str" && !/^["'\[{]/.test(t) ? noComment(t) : inline(t, ind);
        if (tag === "!!str" && v !== null && typeof v !== "object") v = String(v);
        if (anc !== null) anchors[anc] = v;
        return v;
      }
      function mapping(ind) {
        const o = {};
        for (;;) {
          skip(); if (i >= n) return o;
          const d = lead(); if (d < ind) return o;
          if (d > ind) throw err("bad indentation of a mapping entry");
          const t = lines[i].slice(d); if (isSeq(t)) return o;
          const kv = keyOf(t); if (!kv) throw err("can not read a block mapping entry");
          if (Object.prototype.hasOwnProperty.call(o, kv[0])) throw err("duplicated mapping key " + kv[0]);
          i++; o[kv[0]] = value(kv[1], d);
        }
      }
      function sequence(ind) {
        const a = [];
        for (;;) {
          skip(); if (i >= n) return a;
          const d = lead(); if (d < ind) return a;
          if (d > ind) throw err("bad indentation of a sequence entry");
          const t = lines[i].slice(d); if (!isSeq(t)) return a;
          const sp = t.slice(1).match(/^\s*/)[0].length, col = d + 1 + sp, rest = t.slice(1 + sp);
          // the item's content is a node at col on this same line: lines[i].slice(col) is rest already, so only its
          // indentation is recorded (PR #33 review: rebuilding the line per level cost O(line) per level, quadratic
          // time and one line copy per level held for a single line of nested "- - - ...")
          if (rest && (isSeq(rest) || (rest[0] !== "#" && keyOf(rest)))) { vl = i; vc = col; a.push(node(col)); continue; }
          i++; a.push(value(rest, d, true));
        }
      }
      function node(ind) {
        const t = lines[i].slice(ind);
        if (isSeq(t)) return sequence(ind);
        if (keyOf(t)) return mapping(ind);
        i++; return inline(t.trim(), ind - 1);
      }
      skip(); if (i >= n) return null;
      // nesting past the JS stack (a line of "- - - ..." with no item limit) is a parse error, not a raw RangeError
      let v; try { v = node(indent(lines[i])); } catch (e) { if (e instanceof RangeError) throw err("nesting too deep"); throw e; }
      skip();
      if (i < n) throw err("end of the stream or a document separator is expected");
      return v;
    }
    /* block-style YAML like the frontmatter a vault keeps: lists as `key:` + `  - item` lines (excalidraw's
       converted-note `tags:`), nested maps indented two spaces, plain scalars unless quoting is needed */
    function yScal(v) {
      if (v === null || v === undefined) return "null";
      if (typeof v === "number") return Number.isFinite(v) ? String(v) : v !== v ? ".nan" : v > 0 ? ".inf" : "-.inf";
      if (typeof v === "boolean") return String(v);
      if (v instanceof Date) return v.toISOString();
      const s = String(v);
      // PR #33 review: plain only when parseYaml reads it back as this string (plainScalar is the one rule for
      // "1.", "1.e5", "0o17", "+.inf"); the regex keeps quoting the YAML 1.1 words and 1_000 other parsers read
      const plain = s !== "" && s === s.trim() && !/[\n\r\t]/.test(s) && !/^[-?:,\[\]{}#&*!|>'"%@`]/.test(s)
        && !/: |:$| #/.test(s) && typeof plainScalar(s) === "string" && !/^(true|false|yes|no|on|off|null|~|[-+]?(\d[\d_]*)?\.?\d+([eE][-+]?\d+)?|0x[0-9a-f]+|\.(inf|nan))$/i.test(s);
      return plain ? s : JSON.stringify(s);
    }
    const isMap = v => v !== null && typeof v === "object" && !Array.isArray(v) && !(v instanceof Date);
    function yBlock(o, ind) {
      let out = "";
      for (const k of Object.keys(o)) {
        const v = o[k], key = ind + (/^[\w][\w .\/-]*$/.test(k) && k === k.trim() ? k : JSON.stringify(k)) + ":";
        if (v === undefined || typeof v === "function") continue;
        if (Array.isArray(v)) {
          if (!v.length) { out += key + " []\n"; continue; }
          out += key + "\n";
          for (const x of v) out += ind + "  - " + yItem(x) + "\n";
        } else if (isMap(v)) out += Object.keys(v).length ? key + "\n" + yBlock(v, ind + "  ") : key + " {}\n";
        else out += key + " " + yScal(v) + "\n";
      }
      return out;
    }
    // PR #33 review: a top-level list renders its items as yBlock's lists do (a map or list item as flow JSON,
    // not "[object Object]" / "a,b") and an empty one as [], not an empty document
    const yItem = x => isMap(x) || Array.isArray(x) ? JSON.stringify(x) : yScal(x);
    const stringifyYaml = o => isMap(o) ? yBlock(o, "")
      : Array.isArray(o) ? (o.length ? o.map(x => "- " + yItem(x)).join("\n") + "\n" : "[]\n") : yScal(o) + "\n";
    Object.assign(K, { parseYaml, stringifyYaml });
  });
  reg("fns:links", function (K) {
    const { host, be } = K;
    const { parseFrontMatterTags } = K;
    function getAllTags(c) {
      if (!c) return null;
      const t = (c.tags || []).map(x => x.tag);
      const ft = c.frontmatter ? parseFrontMatterTags(c.frontmatter) : null;
      return (ft || []).concat(t);
    }
    const parseLinktext = lt => { const s = String(lt), i = s.indexOf("#"); return i < 0 ? { path: s, subpath: "" } : { path: s.slice(0, i), subpath: s.slice(i) }; };
    const getLinkpath = lt => parseLinktext(lt).path;
    Object.assign(K, { getAllTags, getLinkpath, parseLinktext });

    /* ------------------------------------------------- language + binary
       opensidian's UI ships one language, English; getLanguage() reports it. */
    const getLanguage = () => "en";
    const u8 = b => (ArrayBuffer.isView(b) ? new Uint8Array(b.buffer, b.byteOffset, b.byteLength) : new Uint8Array(b));
    function arrayBufferToBase64(buf) {
      const a = u8(buf); let s = "";
      for (let i = 0; i < a.length; i += 0x8000) s += String.fromCharCode.apply(null, a.subarray(i, i + 0x8000));
      return btoa(s);
    }
    function base64ToArrayBuffer(b64) {      // atob: forgiving base64 (whitespace, missing padding), InvalidCharacterError on junk
      const s = atob(String(b64)), a = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) a[i] = s.charCodeAt(i);
      return a.buffer;
    }
    function arrayBufferToHex(buf) {
      const a = u8(buf), d = "0123456789abcdef"; let s = "";
      for (let i = 0; i < a.length; i++) s += d[a[i] >> 4] + d[a[i] & 15];
      return s;
    }
    function hexToArrayBuffer(hex) {         // one byte per complete pair; a non-hex pair reads 0
      const h = String(hex), a = new Uint8Array(h.length >> 1);
      for (let i = 0; i < a.length; i++) a[i] = parseInt(h.substr(i * 2, 2), 16) || 0;
      return a.buffer;
    }
    const getBlobArrayBuffer = blob => blob.arrayBuffer();

    /* ------------------------------------------------------------ tooltips
       setTooltip records the text + options on the element (aria-label,
       data-tooltip-*); hovering such an element shows it after its delay.
       displayTooltip shows one div.tooltip in <body> next to the target;
       only one is shown at a time and it goes when the pointer leaves the
       target, on pointerdown/wheel/keydown, or when the target is detached. */
    const tipped = new WeakSet();
    let tipEl = null, tipFor = null, tipOff = null, hoverT = null, hoverOn = false;
    function hideTooltip() {
      clearTimeout(hoverT); hoverT = null;
      if (tipOff) { tipOff(); tipOff = null; }
      if (tipEl) { tipEl.remove(); tipEl = null; }
      tipFor = null;
    }
    function displayTooltip(el, text, opts) {
      hideTooltip();
      opts = opts || {};
      const s = text == null ? "" : String(text);
      if (!el || !s) return;
      const doc = el.ownerDocument || document, win = doc.defaultView || window;
      const t = doc.createElement("div");
      t.className = "tooltip";
      const cls = opts.classes == null ? [] : [].concat(opts.classes);
      for (const c of cls) if (c) t.classList.add(String(c));
      t.textContent = s;
      const st = t.style;
      st.position = "fixed"; st.zIndex = "2147483000"; st.pointerEvents = "none"; st.maxWidth = "300px";
      st.padding = "4px 8px"; st.borderRadius = "4px"; st.fontSize = "12px"; st.lineHeight = "1.4"; st.whiteSpace = "pre-wrap";
      st.background = "var(--background-modifier-message, rgba(30,30,30,0.95))"; st.color = "var(--text-on-accent, #fff)";
      doc.body.appendChild(t);
      const r = el.getBoundingClientRect(), w = t.offsetWidth, h = t.offsetHeight;
      const gap = Number.isFinite(+opts.gap) ? +opts.gap : 8;
      const vw = win.innerWidth, vh = win.innerHeight, pl = opts.placement || "bottom";
      let x = r.left + (r.width - w) / 2, y = r.bottom + gap;
      if (pl === "top") y = r.top - h - gap;
      else if (pl === "left") { x = r.left - w - gap; y = r.top + (r.height - h) / 2; }
      else if (pl === "right") { x = r.right + gap; y = r.top + (r.height - h) / 2; }
      if (pl === "bottom" && y + h > vh) y = r.top - h - gap;
      if (pl === "top" && y < 0) y = r.bottom + gap;
      st.left = Math.round(Math.max(4, Math.min(x, vw - w - 4))) + "px";
      st.top = Math.round(Math.max(4, Math.min(y, vh - h - 4))) + "px";
      const off = () => hideTooltip();
      const gone = new MutationObserver(() => { if (!el.isConnected) hideTooltip(); });
      gone.observe(doc.body, { childList: true, subtree: true });
      el.addEventListener("mouseleave", off);
      doc.addEventListener("pointerdown", off, true); doc.addEventListener("wheel", off, true); doc.addEventListener("keydown", off, true);
      tipEl = t; tipFor = el;
      tipOff = () => { gone.disconnect(); el.removeEventListener("mouseleave", off);
        doc.removeEventListener("pointerdown", off, true); doc.removeEventListener("wheel", off, true); doc.removeEventListener("keydown", off, true); };
    }
    function tipOpts(el) {
      const d = el.dataset, o = {};
      if (d.tooltipPosition) o.placement = d.tooltipPosition;
      if (d.tooltipClasses) o.classes = d.tooltipClasses.split(" ");
      if (d.tooltipGap != null) o.gap = +d.tooltipGap;
      return o;
    }
    function hoverInstall() {
      if (hoverOn) return; hoverOn = true;
      document.addEventListener("mouseover", e => {
        let el = e.target && e.target.nodeType === 1 ? e.target : e.target && e.target.parentElement;
        while (el && !tipped.has(el)) el = el.parentElement;
        if (!el || el === tipFor) return;
        clearTimeout(hoverT);
        const d = Number.isFinite(+el.dataset.tooltipDelay) && el.dataset.tooltipDelay !== "" ? +el.dataset.tooltipDelay : 300;
        hoverT = setTimeout(() => { hoverT = null; if (el.isConnected && el.matches(":hover")) displayTooltip(el, el.getAttribute("aria-label"), tipOpts(el)); }, d);
      }, true);
    }
    function setTooltip(el, tooltip, opts) {
      if (!el) return;
      opts = opts || {};
      el.setAttribute("aria-label", tooltip == null ? "" : String(tooltip));
      if (opts.placement != null) el.setAttribute("data-tooltip-position", String(opts.placement));
      if (opts.delay != null) el.setAttribute("data-tooltip-delay", String(opts.delay));
      if (opts.classes != null) el.setAttribute("data-tooltip-classes", [].concat(opts.classes).join(" "));
      if (opts.gap != null) el.setAttribute("data-tooltip-gap", String(opts.gap));
      tipped.add(el); hoverInstall();
      if (tipFor === el) { if (tooltip) tipEl.textContent = String(tooltip); else hideTooltip(); }
    }

    /* ------------------------------------------- frontmatter / properties
       getFrontMatterInfo: a block opens with a first line that is exactly
       "---" and closes at the next line that is exactly "---" (LF or CRLF). */
    function getFrontMatterInfo(content) {
      const s = String(content == null ? "" : content);
      const none = { exists: false, frontmatter: "", from: 0, to: 0, contentStart: 0 };
      const first = /^---(\r?\n)/.exec(s);
      if (!first) return none;
      const from = first[0].length;
      for (let p = from; p <= s.length;) {
        const nl = s.indexOf("\n", p), eol = nl < 0 ? s.length : nl;
        const line = s.slice(p, eol).replace(/\r$/, "");
        if (line === "---") return { exists: true, frontmatter: s.slice(from, p), from, to: p, contentStart: nl < 0 ? s.length : nl + 1 };
        if (nl < 0) break;
        p = nl + 1;
      }
      return none;
    }
    function parseFrontMatterStringArray(fm, key, nospaces) {   // nospaces: accepted, values are not split (as stock)
      const v = K.parseFrontMatterEntry(fm, key);
      if (Array.isArray(v)) return v.filter(x => typeof x === "string");
      if (typeof v === "string" && v !== "") return [v];
      return null;
    }
    function parsePropertyId(id) {
      const s = String(id == null ? "" : id), i = s.indexOf(".");
      if (i > 0) { const t = s.slice(0, i); if (t === "note" || t === "file" || t === "formula") return { type: t, name: s.slice(i + 1) }; }
      return { type: "note", name: s };
    }
    const stripHeading = h => String(h == null ? "" : h).replace(/[\s:|#^[\]*%&\\/]+/g, " ").trim();
    const stripHeadingForLink = h => String(h == null ? "" : h).replace(/[\s:|#^[\]\\]+/g, " ").trim();

    /* --------------------------------------------------- subpaths / refs
       resolveSubpath: "#A#B" walks headings in order (case-insensitive,
       stripped), a heading's section ends at the next heading of the same or
       a higher level (end null at EOF); "#^id" is a block, with its list item. */
    function resolveSubpath(cache, subpath) {
      if (!cache) return null;
      const sp = String(subpath == null ? "" : subpath).replace(/^#/, "");
      if (!sp) return null;
      if (sp[0] === "^") {
        const id = sp.slice(1), bl = cache.blocks || {};
        const block = bl[id] || bl[id.toLowerCase()];
        if (!block || !block.position) return null;
        const ln = block.position.start.line;
        const list = (cache.listItems || []).find(li => li.position && li.position.start.line === ln);
        return { type: "block", block, list, start: block.position.start, end: block.position.end };
      }
      const want = sp.split("#").map(x => stripHeading(x).toLowerCase()).filter(Boolean);
      const hs = cache.headings || [];
      if (!want.length) return null;
      let k = 0, at = -1;
      for (let i = 0; i < hs.length && k < want.length; i++) {
        if (stripHeading(hs[i].heading).toLowerCase() === want[k]) { k++; if (k === want.length) at = i; }
      }
      if (at < 0) return null;
      const current = hs[at];
      let next = null;
      for (let i = at + 1; i < hs.length; i++) if (hs[i].level <= current.level) { next = hs[i]; break; }
      return { type: "heading", current, next, start: current.position.start, end: next ? next.position.start : null };
    }
    function iterateRefs(refs, cb) {
      for (const r of refs || []) if (cb(r)) return true;
      return false;
    }
    const iterateCacheRefs = (cache, cb) => !!cache && (iterateRefs(cache.links, cb) || iterateRefs(cache.embeds, cb));

    /* ----------------------------------------------------- search results */
    function sortSearchResults(results) { results.sort((a, b) => b.match.score - a.match.score); }
    function renderResults(el, text, result, offset) { K.renderMatches(el, text, result && result.matches, offset); }

    const NEW = { getLanguage, arrayBufferToBase64, base64ToArrayBuffer, arrayBufferToHex, hexToArrayBuffer, getBlobArrayBuffer,
      setTooltip, displayTooltip, getFrontMatterInfo, parseFrontMatterStringArray, parsePropertyId, stripHeading, stripHeadingForLink,
      resolveSubpath, iterateRefs, iterateCacheRefs, sortSearchResults, renderResults };
    Object.assign(K, NEW);
    Object.assign(K.exportValues, NEW);

    /* ------------------------------------------------------ lib loaders
       PDF.js, MathJax and Prism are vendored under ui/vendor/<lib>/ (exact pin,
       sha256 + license in each VENDOR.md / SHA256SUMS) and loaded on first use
       from 'self' under the unchanged CSP: no network, no eval. Each loader
       resolves with the real library (also published on window like a plugin
       expects: pdfjsLib, MathJax, Prism) and rejects honestly when the bytes
       cannot be loaded; a failed load is retried by the next call. Mermaid is
       still not vendored: loadMermaid rejects. */
    const unsupported = what => new Error("unsupported in opensidian: " + what);
    const vendorUrl = p => new URL("vendor/" + p, document.baseURI).href;
    const once = {};
    function loadOnce(key, start) {
      if (!once[key]) once[key] = start().catch(e => { once[key] = null; throw e; });
      return once[key];
    }
    function loadScript(path, lib) {
      return new Promise((res, rej) => {
        const s = document.createElement("script");
        s.src = vendorUrl(path);
        s.onload = () => res();
        s.onerror = () => { s.remove(); rej(new Error(lib + " failed to load (ui/vendor/" + path + ")")); };
        document.head.appendChild(s);
      });
    }

    function loadPdfJs() {
      if (window.pdfjsLib && window.pdfjsLib.getDocument) return Promise.resolve(window.pdfjsLib);
      return loadOnce("pdfjs", () => import(vendorUrl("pdfjs/pdf.min.mjs")).then(mod => {
        if (!mod || typeof mod.getDocument !== "function") throw new Error("PDF.js loaded without getDocument");
        mod.GlobalWorkerOptions.workerSrc = vendorUrl("pdfjs/pdf.worker.min.mjs");
        const lib = Object.assign({}, mod);   // a plain object like a global build, not the frozen namespace
        window.pdfjsLib = lib;
        return lib;
      }, e => { throw new Error("PDF.js failed to load (ui/vendor/pdfjs/pdf.min.mjs): " + (e && e.message || e)); }));
    }

    const mathJaxReady = () => !!(window.MathJax && window.MathJax.startup && window.MathJax.startup.document &&
      typeof window.MathJax.tex2chtml === "function");
    function loadMathJax() {
      if (mathJaxReady()) return Promise.resolve(window.MathJax);
      return loadOnce("mathjax", () => {
        // configuration MathJax reads at startup: never typeset the app on its own, no context menu
        window.MathJax = { startup: { typeset: false }, options: { enableMenu: false } };
        return loadScript("mathjax/tex-chtml-full.js", "MathJax")
          .then(() => window.MathJax.startup.promise)
          .then(() => {
            if (!mathJaxReady()) throw new Error("MathJax loaded without tex2chtml");
            return window.MathJax;
          });
      });
    }
    function renderMath(source, display) {
      if (!mathJaxReady()) throw new Error("renderMath: MathJax is not loaded yet (await loadMathJax() first)");
      return window.MathJax.tex2chtml(String(source), { display: !!display });
    }
    function finishRenderMath() {
      // write the CHTML styles the math rendered so far needs into the page
      return loadMathJax().then(MJ => { const d = MJ.startup.document; d.clear(); d.updateDocument(); });
    }

    function loadPrism() {
      if (window.Prism && typeof window.Prism.highlight === "function") return Promise.resolve(window.Prism);
      return loadOnce("prism", () => {
        // read by Prism at startup: no highlightAll over the app, no worker message handler
        window.Prism = { manual: true, disableWorkerMessageHandler: true };
        return loadScript("prism/prism.js", "Prism").then(() => {
          if (!window.Prism || typeof window.Prism.highlight !== "function") throw new Error("Prism loaded without highlight");
          return window.Prism;
        });
      });
    }

    const GATED = {
      loadMathJax, loadPdfJs, loadPrism, renderMath, finishRenderMath,
      loadMermaid: () => Promise.reject(unsupported("loadMermaid (Mermaid is not bundled)")),
    };
    Object.assign(K.exportValues, GATED);
  });
})();
