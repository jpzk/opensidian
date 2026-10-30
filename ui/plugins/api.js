// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* plugin-loader (recon M1), ledger item 4: the "obsidian" module a community
   plugin require()s, and the plugin manager that drives its lifecycle.
   Written from the public obsidian-api typings (obsidian.d.ts) and plugin
   source only (clean room).

   api-parity stage 0: this file is the ASSEMBLER. The code lives in per-part
   files that load before it (index.html), each registering named segments
   on window.__osApiParts:
     api-core.js       Events Component Notice Scope Platform requireApiVersion
                       Keymap, the plugin registry, commands, Plugin, manager
     api-functions.js  icons, normalizePath ... getLinkpath (part fns)
     api-dom.js        the lazy DOM helpers (createEl, empty, setText, ...)
     api-ui.js         Modal SuggestModal Setting* *Component Menu (settings)
     api-vault.js      TFile/TFolder, VFS + adapters, Vault, MetadataCache, app
     views.js          View ItemView FileView WorkspaceLeaf + app.workspace
     editor-api.js     MarkdownRenderer/processors + Editor / MarkdownView
   api.js takes the backend handle ONCE via __osPluginHost.grant() (plugins_state
   / plugins_set / plugin_io), runs the segments ONCE in ORDER (the order the
   code ran in as one file), deletes the registry, builds the API object in its
   fixed key order (+ K.exportClasses / K.exportValues a part adds), and
   freezes the host object. The shared context K is never handed to a plugin.

   The parts talk to main.js only through names main.js defines at top level
   (CMDS, hkRebuild, placeMenu, closeMenu, updateTitle, fileToChord) and only at
   call time, never at load — same rule as census.js; Obsidian's DOM helpers
   are installed LAZILY, when the first plugin is evaluated: restricted mode
   (the default) changes no prototype and runs no plugin code.

   Unload contract: everything a plugin registered — commands, ribbon icons,
   status bar items, setting tabs, DOM listeners, intervals, event refs,
   children, styles.css, open modals/notices it owns — is removed by
   Plugin.unload(). The 9NN phases assert it through the [plg:] census. */
(function () {
  "use strict";
  const host = window.__osPluginHost;
  if (!host) return;
  let be = null;                      // {state, set, io, invoke} — never handed to a plugin
  host.grant(h => { be = h; });
  const parts = window.__osApiParts;
  try { delete window.__osApiParts; } catch (_) {}
  const ORDER = [
    ["core:events", "api-core.js"], ["fns:icons", "api-functions.js"], ["dom:helpers", "api-dom.js"],
    ["fns:util", "api-functions.js"], ["core:platform", "api-core.js"], ["fns:search", "api-functions.js"],
    ["core:notice", "api-core.js"], ["ui:modal", "api-ui.js"], ["workspace:views", "views.js"],
    ["editor:render", "editor-api.js"], ["editor:editor", "editor-api.js"], ["core:registry", "api-core.js"],
    ["core:moment", "api-core.js"], ["fns:yaml", "api-functions.js"], ["fns:links", "api-functions.js"],
    ["vault:model", "api-vault.js"], ["workspace:workspace", "views.js"], ["core:keymap", "api-core.js"],
    ["core:commands", "api-core.js"], ["ui:setting", "api-ui.js"], ["vault:app", "api-vault.js"],
    ["core:plugin", "api-core.js"], ["ui:inputsuggest", "api-ui.js"], ["vault:guarded", "api-vault.js"],
    ["core:manager", "api-core.js"],
  ];
  const K = Object.create(null);
  K.host = host; K.be = be;
  // state several parts read and write (was: top-level lets of the one file)
  K.st = { manifests: {}, enabledIds: [], trusted: false, layoutReady: false, vaultName: "", modelOn: false };
  K.exportClasses = Object.create(null);   // name -> class: appended to the API after the fixed list, es5-callable
  K.exportValues = Object.create(null);    // name -> value (functions, objects): appended after the fixed values
  for (const [id, file] of ORDER) {
    const seg = parts && parts[id];
    if (typeof seg !== "function") { console.error("[plugin] " + file + " did not load (" + id + ")"); return; }
    if (seg(K) === false) return;
  }
  const { API_VERSION, AbstractInputSuggest, AbstractTextComponent, App, BaseComponent, ButtonComponent, ColorComponent, Component, 
    ConfirmationModal, DataAdapter, DropdownComponent, EA, EditableFileView, Events, ExtraButtonComponent, FileSystemAdapter, FileView, 
    FuzzySuggestModal, GuardedFileSystemAdapter, ItemView, Keymap, MarkdownRenderChild, MarkdownRenderer, MarkdownView, Menu, MenuItem, Modal, 
    MomentFormatComponent, Notice, Platform, PlatformGuarded, Plugin, PluginSettingTab, Scope, SearchComponent, Setting, SettingGroup, SettingPage, 
    SettingTab, SliderComponent, SuggestModal, TAbstractFile, TFile, TFolder, TextAreaComponent, TextComponent, TextFileView, ToggleComponent, 
    ValueComponent, Vault, View, WorkspaceLeaf, addIcon, app, boot, bridge, censusTick, censusTok, debounce, disable, enable, getAllTags, getIcon, getIconIds, 
    getLinkpath, htmlToMarkdown, normalizePath, parseFrontMatterAliases, parseFrontMatterEntry, parseFrontMatterTags, parseLinktext, parseYaml, 
    prepareFuzzySearch, prepareSimpleSearch, refresh, registry, removeIcon, renderMatches, requestUrl, requireApiVersion, sanitizeHTMLToDom, setEnabled, 
    setIcon, setRestricted, setting, stringifyYaml } = K;

  /* ---------------------------------------------------- the API object
     ES5-callable classes: a TypeScript-to-ES5 bundle (the pinned sample-plugin
     1.0.0 is one) subclasses with  `_super.apply(this, arguments) || this`, and an
     ES6 class throws when called without `new`. Every exported class is a plain
     function F sharing the class's prototype: `new`/`super()` (new.target set) and
     the ES5 call (new.target unset: construct for this.constructor and RETURN it,
     which the ES5 pattern adopts as `_this`) both run the real constructor, and
     `instanceof` is unchanged (F.prototype === Cls.prototype). */
  function es5(Cls) {
    const F = function (...args) {
      const nt = new.target || (this && this.constructor) || F;
      return Reflect.construct(Cls, args, nt === F ? Cls : nt);
    };
    Object.defineProperty(F, "name", { value: Cls.name });
    F.prototype = Cls.prototype;
    Object.setPrototypeOf(F, Cls);           // statics (Menu.forEvent) inherit
    return F;
  }
  const X = {
    Events, Component, Plugin, App, Notice, Modal, SuggestModal, FuzzySuggestModal, Scope,
    Setting, SettingTab, PluginSettingTab, SettingGroup, SettingPage, ConfirmationModal, BaseComponent, ValueComponent, AbstractTextComponent,
    TextComponent, SearchComponent, TextAreaComponent, MomentFormatComponent, ToggleComponent,
    DropdownComponent, SliderComponent, ColorComponent, ButtonComponent, ExtraButtonComponent,
    Menu, MenuItem, View, ItemView, FileView, EditableFileView, TextFileView, MarkdownView,
    WorkspaceLeaf, MarkdownRenderChild, TAbstractFile, TFile, TFolder, Vault, DataAdapter, FileSystemAdapter,
  };
  if (EA) Object.assign(X, { Editor: EA.Editor, EditorSuggest: EA.EditorSuggest, PopoverSuggest: EA.PopoverSuggest });   // editor-api classes (es5-callable below)
  X.AbstractInputSuggest = AbstractInputSuggest;
  for (const k of Object.keys(K.exportClasses)) if (!(k in X)) X[k] = K.exportClasses[k];   // a part's new classes (api-parity stage 1)
  for (const k of Object.keys(X)) X[k] = es5(X[k]);
  Object.assign(X, {
    Platform, apiVersion: API_VERSION, requireApiVersion, setIcon, getIcon, addIcon, removeIcon, getIconIds,
    normalizePath, debounce, prepareFuzzySearch, prepareSimpleSearch, renderMatches,
    parseFrontMatterEntry, parseFrontMatterAliases, parseFrontMatterTags, requestUrl, request: requestUrl,
    htmlToMarkdown, sanitizeHTMLToDom,
    Keymap,
    parseYaml, stringifyYaml, getAllTags, parseLinktext, getLinkpath, MarkdownRenderer,
  });
  if (EA) Object.assign(X, { editorInfoField: EA.editorInfoField, editorViewField: EA.editorViewField, editorLivePreviewField: EA.editorLivePreviewField, editorEditorField: EA.editorEditorField });   // editor-api: real CM6 StateFields
  // moment: the vendored build, loaded by ensureModel() before any plugin is evaluated.
  // A getter, so esbuild's __toESM live bindings and `obsidian.moment` both see it.
  Object.defineProperty(X, "moment", { enumerable: true, get: () => window.moment });
  for (const k of Object.keys(K.exportValues)) if (!(k in X)) X[k] = K.exportValues[k];   // a part's new values (api-parity stage 1)
  const api = Object.freeze(X);
  const apiGuarded = Object.freeze(Object.defineProperties({}, Object.assign(Object.getOwnPropertyDescriptors(X), {
    Platform: { value: PlatformGuarded, enumerable: true },
    FileSystemAdapter: { value: es5(GuardedFileSystemAdapter), enumerable: true },
  })));

  host.api = api;
  host.apiGuarded = apiGuarded;
  Object.assign(host, {
    app, boot, enable, disable, setEnabled, setRestricted, refresh, censusTok, censusTick,
    tabs: () => setting.pluginTabs.slice(),
    state: () => ({ trusted: K.st.trusted, enabled: K.st.enabledIds.slice(), installed: Object.assign({}, K.st.manifests), loaded: [...registry.keys()] }),
    bridge,         // NOT frozen (freeze is shallow): item 5 sets bridge.editor = () => {editor, view} | null
  });
  Object.freeze(host);
})();
