// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* the "obsidian" module, part SETTINGS (api-parity stage 0: split out of
   api.js, behaviour-neutral): Modal, SuggestModal / FuzzySuggestModal, the
   setting controls (BaseComponent .. ExtraButtonComponent), Setting,
   SettingTab / PluginSettingTab, SettingGroup, SettingPage,
   ConfirmationModal, MenuItem / Menu, the app.setting hook and
   AbstractInputSuggest. Segment contract: see api-core.js. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("ui:modal", function (K) {
    const { host, be } = K;
    const { Component, Scope, installDom, mk, prepareFuzzySearch, renderMatches, scopes, setIcon, touch } = K;
    /* ------------------------------------------------------------- Modal */
    const openModals = [];
    class Modal {
      constructor(app) {
        installDom();
        this.app = app;
        this.scope = new Scope();
        this.shouldRestoreSelection = true;
        this.containerEl = mk("div", { cls: "modal-container mod-dim os-plugin-modal" });
        this.bgEl = this.containerEl.createDiv({ cls: "modal-bg" });
        this.modalEl = this.containerEl.createDiv({ cls: "modal" });
        this.closeButtonEl = this.modalEl.createDiv({ cls: "modal-close-button", attr: { "aria-label": "Close" } });
        this.titleEl = this.modalEl.createDiv({ cls: "modal-title" });
        this.contentEl = this.modalEl.createDiv({ cls: "modal-content" });
        this.closeButtonEl.addEventListener("click", () => this.close());
        this.bgEl.addEventListener("click", () => this.close());
        this.scope.register([], "Escape", () => { this.close(); return false; });
        this._open = false;
        this._closeCb = null;
      }
      /* api-settings: the callback runs after onClose(), once per close */
      setCloseCallback(cb) { this._closeCb = typeof cb === "function" ? cb : null; return this; }
      /* a history "back" (mouse back button / swipe) dismisses the modal */
      onHistoryBack() { this.close(); }
      open() {
        if (this._open) return;
        this._open = true;
        this._prevFocus = document.activeElement;
        document.body.appendChild(this.containerEl);
        openModals.push(this); scopes.push(this.scope);
        try { this.onOpen(); } catch (e) { console.error("[plugin] Modal.onOpen:", e); }
        /* plug-val: stock moves focus into a freshly opened modal: when onOpen focused nothing inside it, the first
           button / field takes focus, so Return answers obsidian-git's Discard modal (stock-F23-a.png: focus ring
           on "Discard all 8 files"); before, the keys went to the editor behind the modal */
        if (!this.modalEl.contains(document.activeElement)) try {
          const f = this.contentEl.querySelector("button, input:not([type=hidden]), textarea, select, [tabindex]:not([tabindex='-1'])")
            || this.modalEl.querySelector(".modal-button-container button");
          if (f) f.focus();
        } catch (_) {}
        touch();
      }
      close() {
        if (!this._open) return;
        this._open = false;
        this.containerEl.remove();
        const i = openModals.indexOf(this); if (i >= 0) openModals.splice(i, 1);
        const j = scopes.lastIndexOf(this.scope); if (j >= 0) scopes.splice(j, 1);
        try { this.onClose(); } catch (e) { console.error("[plugin] Modal.onClose:", e); }
        if (this._closeCb) try { this._closeCb(); } catch (e) { console.error("[plugin] Modal close callback:", e); }
        /* an editor that had focus gets it back through its EditorView: a bare DOM focus() makes CM6
           read the browser's caret (the first text node, e.g. after LP's hidden "# ") as the new
           selection, so Templater's insert landed at 0:2 instead of the caret it was opened at */
        if (this.shouldRestoreSelection && this._prevFocus && this._prevFocus.focus) try {
          const EV = (window.__osModules || {})["@codemirror/view"], ev = EV && EV.EditorView.findFromDOM(this._prevFocus);
          if (ev && this._prevFocus.isConnected) ev.focus(); else this._prevFocus.focus();
        } catch (_) {}
        touch();
      }
      onOpen() {}
      onClose() {}
      setTitle(t) { this.titleEl.textContent = t; return this; }
      setContent(c) { this.contentEl.textContent = ""; if (c instanceof Node) this.contentEl.appendChild(c); else this.contentEl.textContent = String(c); return this; }
      /* plug-val3: stock Modal.setDimBackground(dim) toggles the backdrop dimming (container class mod-dim, on by default).
         Excalidraw's FloatingModal (Frame Settings) calls it from its constructor, so without it the command threw a
         TypeError and no modal opened (E19) */
      setDimBackground(dim) { this.shouldDim = !!dim; this.containerEl.toggleClass("mod-dim", !!dim); return this; }
    }

    /* ---------------------------------------------- SuggestModal / Fuzzy */
    class SuggestModal extends Modal {
      constructor(app) {
        super(app);
        this.limit = 100;
        this.emptyStateText = "No results found.";
        this.modalEl.addClass("prompt");
        this.titleEl.remove(); this.contentEl.remove();
        this.inputEl = this.modalEl.createEl("input", { cls: "prompt-input", attr: { type: "text", spellcheck: "false" } });
        this.resultContainerEl = this.modalEl.createDiv({ cls: "prompt-results" });
        this.instructionsEl = this.modalEl.createDiv({ cls: "prompt-instructions" });
        this._items = []; this._sel = 0;
        this.inputEl.addEventListener("input", () => this._update());
        this.scope.register([], "ArrowDown", () => { this._move(1); return false; });
        this.scope.register([], "ArrowUp", () => { this._move(-1); return false; });
        this.scope.register([], "Enter", e => { this._choose(e); return false; });
      }
      /* a re-opened SuggestModal starts with an empty query: plugins (Templater) keep one instance and
         open() it per command, so the last query must not leak into the next run. A value set
         after the close (a deliberate prefill before open()) is kept; onChooseSuggestion and
         onClose still read the typed query, since it is only cleared at the NEXT open. */
      open() {
        if (!this._open && this._closedQ !== undefined && this.inputEl.value === this._closedQ) this.inputEl.value = "";
        super.open();
      }
      close() { const was = this._open; super.close(); if (was) this._closedQ = this.inputEl.value; }
      setPlaceholder(p) { this.inputEl.setAttribute("placeholder", p); }
      setInstructions(list) { this.instructionsEl.empty(); for (const i of list || []) { const d = this.instructionsEl.createDiv({ cls: "prompt-instruction" }); d.createSpan({ cls: "prompt-instruction-command", text: i.command }); d.createSpan({ text: i.purpose }); } }
      onOpen() { this._update(); this.inputEl.focus(); }
      async _update() {
        let r; try { r = await this.getSuggestions(this.inputEl.value); } catch (e) { console.error(e); r = []; }
        this._items = (r || []).slice(0, this.limit); this._sel = 0;
        this.resultContainerEl.empty();
        if (!this._items.length) { try { this.onNoSuggestion(); } catch (e) { console.error(e); } touch(); return; }
        this._items.forEach((v, i) => {
          const el = this.resultContainerEl.createDiv({ cls: "suggestion-item" + (i === 0 ? " is-selected" : "") });
          try { this.renderSuggestion(v, el); } catch (e) { console.error(e); }
          el.addEventListener("mousemove", () => this._setSel(i));
          el.addEventListener("click", e => { this._sel = i; this._choose(e); });
        });
        touch();
      }
      _setSel(i) { const k = this.resultContainerEl.children; if (k[this._sel]) k[this._sel].classList.remove("is-selected"); this._sel = i; if (k[i]) { k[i].classList.add("is-selected"); k[i].scrollIntoView({ block: "nearest" }); } }
      _move(d) { if (this._items.length) this._setSel((this._sel + d + this._items.length) % this._items.length); }
      _choose(e) { const v = this._items[this._sel]; if (v !== undefined) this.selectSuggestion(v, e); }
      /* the empty state: the result list shows emptyStateText */
      onNoSuggestion() { this.resultContainerEl.empty(); this.resultContainerEl.createDiv({ cls: "suggestion-empty", text: this.emptyStateText }); }
      /* choose the highlighted suggestion, as Enter does */
      selectActiveSuggestion(e) { this._choose(e); }
      selectSuggestion(v, e) { this.close(); this.onChooseSuggestion(v, e); }
      getSuggestions() { return []; }
      renderSuggestion(v, el) { el.setText(String(v)); }
      onChooseSuggestion() {}
    }
    class FuzzySuggestModal extends SuggestModal {
      getSuggestions(q) {
        const f = prepareFuzzySearch(q), out = [];
        for (const item of this.getItems()) { const m = f(this.getItemText(item)); if (m) out.push({ item, match: m }); }
        if (q) out.sort((a, b) => b.match.score - a.match.score);
        return out;
      }
      renderSuggestion(fm, el) { renderMatches(el, this.getItemText(fm.item), fm.match.matches); }
      onChooseSuggestion(fm, e) { this.onChooseItem(fm.item, e); }
      getItems() { return []; }
      getItemText(i) { return String(i); }
      onChooseItem() {}
    }

    /* ------------------------------------------------- Setting + controls */
    class BaseComponent {
      constructor() { this.disabled = false; }
      then(cb) { cb(this); return this; }
      setDisabled(d) { this.disabled = !!d; return this; }
    }
    class ValueComponent extends BaseComponent {
      registerOptionListener(listeners, key) { listeners[key] = v => { if (v !== undefined) this.setValue(v); return this.getValue(); }; return this; }
    }
    class AbstractTextComponent extends ValueComponent {
      constructor(inputEl) { super(); this.inputEl = inputEl; this._cb = null; inputEl.addEventListener("input", () => this.onChanged()); }
      setDisabled(d) { super.setDisabled(d); this.inputEl.disabled = !!d; return this; }
      getValue() { return this.inputEl.value; }
      setValue(v) { this.inputEl.value = v == null ? "" : v; return this; }
      setPlaceholder(p) { this.inputEl.placeholder = p; return this; }
      onChanged() { if (this._cb) this._cb(this.getValue()); }
      onChange(cb) { this._cb = cb; return this; }
    }
    class TextComponent extends AbstractTextComponent {
      constructor(containerEl) { installDom(); super(containerEl.createEl("input", { attr: { type: "text", spellcheck: "false" } })); }
    }
    class SearchComponent extends AbstractTextComponent {
      constructor(containerEl) {
        installDom();
        const box = containerEl.createDiv({ cls: "search-input-container" });
        super(box.createEl("input", { attr: { type: "search", spellcheck: "false" } }));
        this.containerEl = box;
        this.clearButtonEl = box.createDiv({ cls: "search-input-clear-button" });
        this.clearButtonEl.addEventListener("click", () => { this.setValue(""); this.onChanged(); });
      }
      setClass(c) { this.containerEl.addClass(c); return this; }
    }
    class TextAreaComponent extends AbstractTextComponent {
      constructor(containerEl) { installDom(); super(containerEl.createEl("textarea", { attr: { spellcheck: "false" } })); }
    }
    class MomentFormatComponent extends TextComponent {
      constructor(containerEl) { super(containerEl); this.sampleEl = undefined; this.defaultFormat = ""; }
      setDefaultFormat(f) { this.defaultFormat = f; if (!this.inputEl.placeholder) this.setPlaceholder(f); this.updateSample(); return this; }
      setSampleEl(el) { this.sampleEl = el; this.updateSample(); return this; }
      setValue(v) { super.setValue(v); this.updateSample(); return this; }
      onChanged() { this.updateSample(); super.onChanged(); }
      /* the sample element shows the current time in the entered format (the default format when empty) */
      updateSample() {
        if (!this.sampleEl) return;
        const f = this.getValue() || this.defaultFormat || "", m = window.moment;
        this.sampleEl.textContent = typeof m === "function" ? m().format(f) : f;
      }
    }
    class ToggleComponent extends ValueComponent {
      constructor(containerEl) {
        super(); installDom();
        this.toggleEl = containerEl.createDiv({ cls: "checkbox-container" });
        this.toggleEl.createEl("input", { attr: { type: "checkbox", tabindex: "0" } });
        this._v = false; this._cb = null;
        this.toggleEl.addEventListener("click", e => { e.preventDefault(); this.onClick(); });
      }
      setDisabled(d) { super.setDisabled(d); this.toggleEl.toggleClass("is-disabled", !!d); return this; }
      getValue() { return this._v; }
      setValue(v) { this._v = !!v; this.toggleEl.toggleClass("is-enabled", this._v); const i = this.toggleEl.querySelector("input"); if (i) i.checked = this._v; return this; }
      setTooltip(t) { this.toggleEl.setAttribute("aria-label", t); this.toggleEl.title = t; return this; }
      onClick() { if (this.disabled) return; this.setValue(!this._v); if (this._cb) this._cb(this._v); }
      onChange(cb) { this._cb = cb; return this; }
    }
    class DropdownComponent extends ValueComponent {
      constructor(containerEl) { super(); installDom(); this.selectEl = containerEl.createEl("select", { cls: "dropdown" }); this._cb = null; this.selectEl.addEventListener("change", () => { if (this._cb) this._cb(this.getValue()); }); }
      setDisabled(d) { super.setDisabled(d); this.selectEl.disabled = !!d; return this; }
      addOption(v, label) { this.selectEl.createEl("option", { value: v, text: label }); return this; }
      addOptions(o) { for (const [v, l] of Object.entries(o)) this.addOption(v, l); return this; }
      getValue() { return this.selectEl.value; }
      setValue(v) { this.selectEl.value = v; return this; }
      onChange(cb) { this._cb = cb; return this; }
    }
    class SliderComponent extends ValueComponent {
      constructor(containerEl) {
        super(); installDom();
        /* the current value is printed before the slider, as stock does (setDisplayFormat formats it) */
        this.valueEl = containerEl.createEl("span", { cls: "slider-value" });
        this.sliderEl = containerEl.createEl("input", { cls: "slider", attr: { type: "range" } }); this._cb = null;
        this.sliderEl.addEventListener("input", () => { this._show(); if (this._cb) this._cb(this.getValue()); });
        this._show();
      }
      _show() { if (this.valueEl) this.valueEl.textContent = this.getValuePretty(); }
      setDisabled(d) { super.setDisabled(d); this.sliderEl.disabled = !!d; return this; }
      setLimits(min, max, step) { if (min != null) this.sliderEl.min = min; if (max != null) this.sliderEl.max = max; this.sliderEl.step = step === "any" || step == null ? "any" : step; this._show(); return this; }
      getValue() { return +this.sliderEl.value; }
      setValue(v) { this.sliderEl.value = v; this._show(); return this; }
      getValuePretty() { const v = this.getValue(); if (this._fmt) { try { return String(this._fmt(v)); } catch (e) { console.error("[plugin] slider display format:", e); } } return String(v); }
      /* api-settings: a custom formatter for the value shown next to the slider */
      setDisplayFormat(f) { this._fmt = typeof f === "function" ? f : null; this._show(); return this; }
      setDynamicTooltip() { return this; }
      showTooltip() {}
      setInstant() { return this; }
      onChange(cb) { this._cb = cb; return this; }
    }
    class ColorComponent extends ValueComponent {
      constructor(containerEl) { super(); installDom(); this.colorPickerEl = containerEl.createEl("input", { attr: { type: "color" } }); this._cb = null; this.colorPickerEl.addEventListener("input", () => { if (this._cb) this._cb(this.getValue()); }); }
      setDisabled(d) { super.setDisabled(d); this.colorPickerEl.disabled = !!d; return this; }
      getValue() { return this.colorPickerEl.value; }
      setValue(v) { this.colorPickerEl.value = v; return this; }
      getValueRgb() { const h = this.getValue().replace("#", ""); return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16) }; }
      setValueRgb(c) { const x = n => (n | 0).toString(16).padStart(2, "0"); return this.setValue("#" + x(c.r) + x(c.g) + x(c.b)); }
      getValueHsl() { return { h: 0, s: 0, l: 0 }; }
      setValueHsl() { return this; }
      onChange(cb) { this._cb = cb; return this; }
    }
    class ButtonComponent extends BaseComponent {
      constructor(containerEl) { super(); installDom(); this.buttonEl = containerEl.createEl("button"); this._cb = null; this.buttonEl.addEventListener("click", e => { if (!this.disabled && this._cb) this._cb(e); }); }
      setDisabled(d) { super.setDisabled(d); this.buttonEl.disabled = !!d; return this; }
      setCta() { this.buttonEl.addClass("mod-cta"); return this; }
      removeCta() { this.buttonEl.removeClass("mod-cta"); return this; }
      setWarning() { this.buttonEl.addClass("mod-warning"); return this; }
      setDestructive() { this.buttonEl.addClass("mod-destructive"); return this; }
      removeDestructive() { this.buttonEl.removeClass("mod-destructive"); return this; }
      setTooltip(t) { this.buttonEl.setAttribute("aria-label", t); this.buttonEl.title = t; return this; }
      setButtonText(t) { this.buttonEl.textContent = t; return this; }
      setIcon(i) { setIcon(this.buttonEl, i); return this; }
      setClass(c) { this.buttonEl.addClass(c); return this; }
      onClick(cb) { this._cb = cb; return this; }
    }
    class ExtraButtonComponent extends BaseComponent {
      constructor(containerEl) { super(); installDom(); this.extraSettingsEl = containerEl.createDiv({ cls: "clickable-icon extra-setting-button" }); this._cb = null; this.extraSettingsEl.addEventListener("click", () => { if (!this.disabled && this._cb) this._cb(); }); }
      setDisabled(d) { super.setDisabled(d); this.extraSettingsEl.toggleClass("is-disabled", !!d); return this; }
      setTooltip(t) { this.extraSettingsEl.setAttribute("aria-label", t); this.extraSettingsEl.title = t; return this; }
      setIcon(i) { setIcon(this.extraSettingsEl, i); return this; }
      onClick(cb) { this._cb = cb; return this; }
    }
    /* api-settings: a determinate progress bar, value 0..100 (unset until setValue) */
    class ProgressBarComponent extends ValueComponent {
      constructor(containerEl) {
        super(); installDom();
        this.progressBar = containerEl.createDiv({ cls: "setting-progress-bar", attr: { role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100" } });
        this.lineEl = this.progressBar.createDiv({ cls: "setting-progress-bar-line" });
        this._v = undefined;
      }
      getValue() { return this._v; }
      setValue(v) {
        this._v = v;
        const n = Math.max(0, Math.min(100, Number(v) || 0));
        this.lineEl.style.width = n + "%"; this.progressBar.setAttribute("aria-valuenow", String(n));
        return this;
      }
    }
    /* api-settings: a read-only value label on a setting row, with an optional warning marker */
    class DisplayValueComponent {
      constructor(containerEl) {
        installDom();
        this.valueEl = containerEl.createDiv({ cls: "setting-display-value" });
        this.statusEl = null;
      }
      setValue(v) { this.valueEl.textContent = v == null ? "" : String(v); return this; }
      setStatus(s) {
        if (s === "warning") {
          if (!this.statusEl) { this.statusEl = this.valueEl.createSpan({ cls: "setting-display-status mod-warning" }); setIcon(this.statusEl, "alert-triangle"); }
          this.valueEl.addClass("mod-warning");
        } else {
          if (this.statusEl) { this.statusEl.remove(); this.statusEl = null; }
          this.valueEl.removeClass("mod-warning");
        }
        return this;
      }
    }
    /* api-settings: the button type of ConfirmationModal.addButton. A click runs the handler,
       then closes the modal (after the handler's promise settles, if it returns one). */
    class ConfirmationButton extends ButtonComponent {
      constructor(containerEl, modal) {
        super(containerEl);
        this.modal = modal || null;
        this.buttonEl.addEventListener("click", e => {
          if (this.disabled) return;
          let r;
          try { r = this._handler ? this._handler(e) : undefined; } catch (er) { console.error("[plugin] confirmation button:", er); }
          const done = () => { if (this.modal) this.modal.close(); };
          if (r && typeof r.then === "function") r.then(done, er => { console.error("[plugin] confirmation button:", er); done(); }); else done();
        });
      }
      onClick(h) { this._handler = h; return this; }
      setInitialFocus() { if (this.modal) this.modal._initialFocus = this; return this; }
      setSecondary() { this.buttonEl.addClass("mod-secondary"); return this; }
      setCancel() { this.buttonEl.addClass("mod-cancel"); return this; }
    }
    class Setting {
      constructor(containerEl) {
        installDom();
        this.settingEl = containerEl.createDiv({ cls: "setting-item" });
        this.infoEl = this.settingEl.createDiv({ cls: "setting-item-info" });
        this.nameEl = this.infoEl.createDiv({ cls: "setting-item-name" });
        this.descEl = this.infoEl.createDiv({ cls: "setting-item-description" });
        this.controlEl = this.settingEl.createDiv({ cls: "setting-item-control" });
        this.components = [];
        this.errorEl = null;
      }
      setName(n) { if (n instanceof Node) { this.nameEl.empty(); this.nameEl.appendChild(n); } else this.nameEl.setText(n == null ? "" : String(n)); return this; }
      setDesc(d) { if (d instanceof Node) { this.descEl.empty(); this.descEl.appendChild(d); } else this.descEl.setText(d == null ? "" : String(d)); return this; }
      /* api-settings: a persistent validation message; empty / null clears it. Stock draws it under the row's control,
         right-aligned in the warning colour (templater Template hotkey "This template already has a hotkey") */
      setErrorMessage(msg) {
        if (msg) {
          // PR #33 review: clear() empties controlEl, so an error element it detached is made again, not written unseen
          if (!this.errorEl || this.errorEl.parentNode !== this.controlEl) this.errorEl = this.controlEl.createDiv({ cls: "setting-item-error" });
          this.errorEl.textContent = String(msg); this.settingEl.addClass("is-invalid");
        } else {
          if (this.errorEl) this.errorEl.remove();
          this.errorEl = null; this.settingEl.removeClass("is-invalid");
        }
        return this;
      }
      setClass(c) { this.settingEl.addClass(c); return this; }
      setTooltip(t) { this.nameEl.title = t; return this; }
      setHeading() { this.settingEl.addClass("setting-item-heading"); return this; }
      /* setVisibility(v): a false hides the whole row (excalidraw export dialog: "selected elements only" without a selection) */
      setVisibility(v) { this.settingEl.style.display = v ? "" : "none"; return this; }
      /* setNavigable(cb): the row reads as a link to somewhere else — a chevron is the first control and a
         click anywhere on the row calls cb() with no arguments and this = the Setting; a second call
         replaces cb (stock 1.13.7, scratch oracle goal cp-settings-tab) */
      setNavigable(cb) {
        this._navCb = cb;
        if (!this._navEl) {
          this.settingEl.addClass("mod-navigable"); this.settingEl.addClass("tappable");
          this._navEl = this.controlEl.createDiv({ cls: "setting-item-chevron" }); setIcon(this._navEl, "chevron-right");
          this.settingEl.addEventListener("click", () => { if (typeof this._navCb === "function") this._navCb.call(this); });
        }
        return this;
      }
      setDisabled(d) { this.settingEl.toggleClass("is-disabled", !!d); for (const c of this.components) if (c && typeof c.setDisabled === "function") c.setDisabled(d); return this; }
      _add(C, cb) { const c = new C(this.controlEl); this.components.push(c); if (cb) cb(c); return this; }
      addButton(cb) { return this._add(ButtonComponent, cb); }
      addExtraButton(cb) { return this._add(ExtraButtonComponent, cb); }
      addToggle(cb) { return this._add(ToggleComponent, cb); }
      addText(cb) { return this._add(TextComponent, cb); }
      addSearch(cb) { return this._add(SearchComponent, cb); }
      addTextArea(cb) { return this._add(TextAreaComponent, cb); }
      addMomentFormat(cb) { return this._add(MomentFormatComponent, cb); }
      addDropdown(cb) { return this._add(DropdownComponent, cb); }
      addColorPicker(cb) { return this._add(ColorComponent, cb); }
      addSlider(cb) { return this._add(SliderComponent, cb); }
      addProgressBar(cb) { return this._add(ProgressBarComponent, cb); }
      /* the display value labels the row; it is not one of the row's input components */
      addDisplayValue(cb) { const d = new DisplayValueComponent(this.controlEl); if (cb) cb(d); return this; }
      /* a caller-built component: cb gets the control element and returns the component */
      addComponent(cb) { const c = cb(this.controlEl); this.components.push(c); return this; }
      then(cb) { cb(this); return this; }
      clear() { this.controlEl.empty(); this.components = []; return this; }
    }
    /* ----------------------------------------- declarative settings (api-settings)
       SettingTab.getSettingDefinitions() returns definitions (rows with a control /
       render / action), groups and lists (type "group" | "list") and navigable pages
       (type "page"). update() stores them in settingItems; the base display() renders
       settingItems into containerEl (a tab that overrides display() renders itself).
       control rows read tab.getControlValue(key) (?? defaultValue) and persist through
       tab.setControlValue(key, value) after validate() accepts; visible / disabled
       predicates are evaluated on render and again by refreshDomState(), which only
       flips state on the rendered nodes. Behaviour checked against stock 1.13.7 by the
       harness oracle (scripts/api-probes.d/settings.js). */
    const flagOf = (v, dflt) => {
      if (typeof v === "function") { try { return !!v(); } catch (e) { console.error("[plugin] setting predicate:", e); return dflt; } }
      return v === undefined ? dflt : !!v;
    };
    const valOf = v => { if (typeof v === "function") { try { return v(); } catch (e) { console.error("[plugin] setting value:", e); return undefined; } } return v; };
    const onActivate = (el, f) => {
      el.tabIndex = 0;
      el.addEventListener("click", e => { if (e.target && e.target.closest && e.target.closest("input,select,textarea,button,.clickable-icon")) return; f(e); });
      el.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); f(e); } });
    };
    function sdTeardown(tab) {
      const d = tab._dom; tab._dom = null;
      if (!d) return;
      for (const f of d.cleanups.splice(0)) try { f(); } catch (e) { console.error("[plugin] setting cleanup:", e); }
      if (d.page) try { d.page.hide(); } catch (e) { console.error("[plugin] SettingPage.hide:", e); }
    }
    function sdApply(d) {
      for (const n of d.nodes) {
        const vis = flagOf(n.visible, true) && !n.searchHidden;
        n.el.hidden = !vis; n.el.toggleClass("is-hidden", !vis);
        if (n.setting) { n.isDisabled = flagOf(n.disabled, false); n.setting.setDisabled(n.isDisabled); }
      }
    }
    function sdControl(d, s, c) {
      const tab = d.tab;
      const read = () => { let v; try { v = tab.getControlValue(c.key); } catch (e) { console.error("[plugin] getControlValue:", e); } return v == null ? c.defaultValue : v; };
      const commit = async v => {
        if (typeof c.validate === "function") {
          let m; try { m = await c.validate(v); } catch (e) { m = String((e && e.message) || e); }
          if (m) { s.setErrorMessage(m); return; }
        }
        s.setErrorMessage(null);
        try { await tab.setControlValue(c.key, v); } catch (e) { console.error("[plugin] setControlValue:", e); }
        if (tab._dom === d) tab.refreshDomState();
      };
      const v0 = read(), str = v => (typeof v === "string" ? v : "");
      const suggest = (t, kind) => {
        const AIS = K.AbstractInputSuggest, vault = K.app && K.app.vault;
        if (typeof AIS !== "function" || !vault) return;
        const pool = () => {
          if (kind === "file") return (vault.getFiles ? vault.getFiles() : []).filter(f => { try { return !c.filter || c.filter(f); } catch (e) { return false; } }).map(f => f.path);
          const out = [], walk = f => { if (!f || !f.children) return; if (f.path !== "/" || c.includeRoot) { let ok = true; try { ok = !c.filter || c.filter(f); } catch (e) { ok = false; } if (ok) out.push(f.path); } for (const ch of f.children) walk(ch); };
          walk(vault.getRoot ? vault.getRoot() : null); return out;
        };
        const sg = new (class extends AIS {
          getSuggestions(q) { const l = String(q || "").toLowerCase(); return pool().filter(p => p.toLowerCase().includes(l)); }
          renderSuggestion(p, el) { el.setText(p); }
        })(K.app, t.inputEl);
        sg.onSelect(p => { t.setValue(p); commit(p); });
      };
      switch (c.type) {
        case "toggle": s.addToggle(t => { t.setValue(!!v0); t.onChange(commit); }); break;
        case "dropdown": s.addDropdown(x => { x.addOptions(c.options || {}); if (v0 != null) x.setValue(String(v0)); x.onChange(commit); }); break;
        case "text": case "file": case "folder":
          s.addText(t => { if (c.placeholder) t.setPlaceholder(c.placeholder); t.setValue(str(v0)); t.onChange(commit); if (c.type !== "text") suggest(t, c.type); });
          break;
        case "textarea": s.addTextArea(t => { if (c.placeholder) t.setPlaceholder(c.placeholder); if (c.rows) t.inputEl.rows = c.rows; t.setValue(str(v0)); t.onChange(commit); }); break;
        case "number":
          s.addText(t => {
            t.inputEl.type = "number";
            if (c.min != null) t.inputEl.min = c.min; if (c.max != null) t.inputEl.max = c.max; if (c.step != null) t.inputEl.step = c.step;
            if (c.placeholder) t.setPlaceholder(c.placeholder);
            t.setValue(typeof v0 === "number" && Number.isFinite(v0) ? String(v0) : "");
            t.onChange(x => { const n = x === "" ? NaN : Number(x); if (Number.isFinite(n)) commit(n); });
          });
          break;
        case "slider": {
          s.addSlider(x => {
            x.setLimits(c.min, c.max, c.step);
            if (typeof v0 === "number") x.setValue(v0);
            if (c.displayFormat) x.setDisplayFormat(c.displayFormat);
            const show = () => { const p = x.getValuePretty(); x.valueEl.textContent = p; x.valueEl.hidden = p === ""; };
            show(); x.onChange(v => { show(); commit(v); });
          });
          break;
        }
        case "color": s.addColorPicker(x => { if (typeof v0 === "string" && v0) x.setValue(v0); x.onChange(commit); }); break;
        case "secret": s.setErrorMessage("unsupported in opensidian: secret storage is not available"); s.setDisabled(true); return;
        default: s.setErrorMessage("unsupported setting control type: " + String(c.type)); return;
      }
      if (typeof c.validate === "function" && v0 !== undefined)
        Promise.resolve().then(() => c.validate(v0)).then(m => { if (m && tab._dom === d) s.setErrorMessage(m); }, e => console.error("[plugin] validate:", e));
    }
    function sdDef(d, g, def, idx, list) {
      if (!def || typeof def !== "object") return null;
      const s = new Setting(g.listEl);
      if (def.name != null) s.setName(def.name);
      /* a DocumentFragment desc empties itself into the first render; keep its nodes so a re-render (back from a
         sub-page, sdRender) moves the same nodes, listeners included, into the new row (templater documentation links) */
      if (def.desc instanceof DocumentFragment) {
        if (!def._descNodes || def.desc.childNodes.length) def._descNodes = Array.from(def.desc.childNodes);
        const f = document.createDocumentFragment(); for (const n of def._descNodes) f.appendChild(n);
        s.setDesc(f);
      } else if (def.desc != null) s.setDesc(def.desc);
      const node = { el: s.settingEl, setting: s, def, visible: def.visible, disabled: def.control ? def.control.disabled : def.disabled };
      d.nodes.push(node);
      if (def.type === "page") {
        const dv = valOf(def.displayValue), st = valOf(def.status);
        if ((dv != null && dv !== "") || st === "warning") s.addDisplayValue(x => { x.setValue(dv == null ? "" : String(dv)); x.setStatus(st === "warning" ? "warning" : null); });
        setIcon(s.controlEl.createSpan({ cls: "setting-page-chevron" }), "chevron-right");
        s.settingEl.addClass("mod-clickable");
        onActivate(s.settingEl, () => sdOpenPage(d.tab, def));
      } else if (def.control && typeof def.control === "object") sdControl(d, s, def.control);
      else if (typeof def.render === "function") {
        try { const c = def.render(s, g); if (typeof c === "function") d.cleanups.push(c); } catch (e) { console.error("[plugin] setting render:", e); }
      } else if (typeof def.action === "function") {
        s.settingEl.addClass("mod-clickable");
        onActivate(s.settingEl, () => { if (!node.isDisabled) try { def.action(s.settingEl, idx); } catch (e) { console.error("[plugin] setting action:", e); } });
      }
      if (list) {
        if (typeof list.onDelete === "function") {
          s.addExtraButton(b => b.setIcon("x").setTooltip("Delete").onClick(() => list.onDelete(idx)));
          s.settingEl.tabIndex = 0;
          s.settingEl.addEventListener("keydown", e => { if ((e.key === "Delete" || e.key === "Backspace") && e.target === s.settingEl) { e.preventDefault(); list.onDelete(idx); } });
        }
        if (typeof list.onReorder === "function") {
          const h = s.settingEl.createDiv({ cls: "setting-drag-handle" }); s.settingEl.prepend(h); setIcon(h, "grip-vertical");
          s.settingEl.draggable = true;
          s.settingEl.addEventListener("dragstart", e => { d.drag = idx; if (e.dataTransfer) e.dataTransfer.effectAllowed = "move"; });
          s.settingEl.addEventListener("dragover", e => { if (d.drag != null) e.preventDefault(); });
          s.settingEl.addEventListener("drop", e => { e.preventDefault(); const from = d.drag; d.drag = null; if (from != null && from !== idx) list.onReorder(from, idx); });
        }
      }
      return node;
    }
    function sdGroup(d, el, def, path) {
      const g = new SettingGroup(el);
      if (def.heading) g.setHeading(def.heading);
      if (def.cls) for (const c of String(def.cls).split(/\s+/)) if (c) g.addClass(c);
      for (const f of def.extraButtons || []) if (typeof f === "function") g.addExtraButton(f);
      const isList = def.type === "list", items = Array.isArray(def.items) ? def.items : [];
      d.nodes.push({ el: g.groupEl, visible: def.visible });
      if (isList) g.groupEl.addClass("mod-list");
      if (isList && def.addItem && typeof def.addItem.action === "function")
        g.addExtraButton(b => b.setIcon("plus").setTooltip(String(def.addItem.name || "")).onClick(() => def.addItem.action(b.extraSettingsEl)));
      const kids = items.map((it, i) => sdDef(d, g, it, i, isList ? def : null));
      if (isList && !items.length && def.emptyState != null) {
        const e = g.listEl.createDiv({ cls: "setting-list-empty" });
        if (def.emptyState instanceof Node) e.appendChild(def.emptyState); else e.textContent = String(def.emptyState);
      }
      if (def.search && typeof def.search.match === "function") {
        const qs = d.tab._queries || (d.tab._queries = Object.create(null));
        const filter = q => {
          kids.forEach((n, i) => {
            if (!n) return; const it = items[i];
            let hide = false;
            if (q && !(it && it.searchable === false)) { try { hide = !def.search.match(it, q); } catch (e) { console.error("[plugin] group search:", e); } }
            n.searchHidden = hide;
          });
          sdApply(d);
        };
        g.addSearch(sc => {
          if (def.search.placeholder) sc.setPlaceholder(def.search.placeholder);
          sc.setValue(qs[path] || "");
          sc.onChange(q => { qs[path] = q; filter(q); });
        });
        if (qs[path]) d.after.push(() => filter(qs[path]));
      }
      return g;
    }
    function sdItems(d, el, items, path) {
      let loose = null;
      (Array.isArray(items) ? items : []).forEach((it, i) => {
        if (!it || typeof it !== "object") return;
        if (it.type === "group" || it.type === "list") { loose = null; sdGroup(d, el, it, path + "/" + i); return; }
        if (!loose) loose = new SettingGroup(el);
        sdDef(d, loose, it, i, null);
      });
    }
    function sdRender(tab) {
      sdTeardown(tab);
      tab.containerEl.empty();
      const d = tab._dom = { tab, nodes: [], cleanups: [], after: [], page: null, drag: null };
      const stack = tab._pages || [];
      const top = stack[stack.length - 1];
      if (!top) sdItems(d, tab.containerEl, tab.settingItems, "");
      else {
        let p = null;
        if (typeof top.page === "function") { try { p = top.page(); } catch (e) { console.error("[plugin] SettingPage factory:", e); } }
        if (!(p instanceof SettingPage)) {
          p = new SettingPage();
          const items = top.items;
          p.display = function () { this.containerEl.empty(); sdItems(d, this.containerEl, items, "p" + stack.length); };
        }
        if (!p.title) p.title = top.name == null ? "" : String(top.name);
        const back = new ExtraButtonComponent(p.titlebarEl);
        back.setIcon("chevron-left").setTooltip("Back").onClick(() => {
          stack.pop(); const y = (tab._pageScroll || []).pop(); sdRender(tab);
          const sc = sdScroller(tab.containerEl); if (sc) sc.scrollTop = y || 0;
        });
        p.titlebarEl.prepend(back.extraSettingsEl);
        d.page = p;
        tab.containerEl.appendChild(p.rootEl);
        try { p.display(); } catch (e) { console.error("[plugin] SettingPage.display:", e); }
      }
      sdApply(d);
      for (const f of d.after.splice(0)) f();
    }
    /* stock: "<" on a sub-page returns to the parent page at the scroll position it was left at (templater's
       User scripts > sits at the bottom of its tab); a sub-page opens at its top */
    const sdScroller = el => { for (let n = el; n && n.nodeType === 1; n = n.parentElement) { const o = getComputedStyle(n).overflowY; if (o === "auto" || o === "scroll") return n; } return null; };
    /* update() while a sub-page is on screen stays on that page, as in stock (templater re-reads its definitions after "Done" in
       Template hotkey and shows the new row there): the open pages are looked up again by name in the new definitions */
    const sdFindPage = (items, name) => {
      for (const it of Array.isArray(items) ? items : []) {
        if (!it || typeof it !== "object") continue;
        if (it.type === "page" && it.name === name) return it;
        if (it.type === "group" || it.type === "list") { const f = sdFindPage(it.items, name); if (f) return f; }
      }
      return null;
    };
    function sdRebase(items, stack) { const out = []; let scope = items; for (const p of stack) { const f = p && sdFindPage(scope, p.name); if (!f) break; out.push(f); scope = f.items; } return out; }
    function sdOpenPage(tab, def) {
      const sc = sdScroller(tab.containerEl);
      (tab._pageScroll || (tab._pageScroll = [])).push(sc ? sc.scrollTop : 0);
      (tab._pages || (tab._pages = [])).push(def); sdRender(tab);
      if (sc) sc.scrollTop = 0;
    }

    /* a tab with setting definitions is rendered from them, as stock 1.13.7 does: its own display() is the
       legacy fallback and is not called (excalidraw overrides display() with its single-page settings and
       hands 1.13+ its multi-page definitions); a tab with none runs display() */
    // PR #33 review: definitions that became empty take their rendering down (rows and their callbacks, as hide()
    // does) before display() runs, so no stale declarative rows stay on screen live
    const sdShow = tab => {
      if (tab.settingItems && tab.settingItems.length) return sdRender(tab);
      if (tab._dom) { sdTeardown(tab); tab.containerEl.empty(); }
      return tab.display();
    };
    class SettingTab {
      constructor() {
        installDom();
        this.containerEl = mk("div", { cls: "vertical-tab-content" });
        this._icon = undefined;   // stock default: undefined (apioracle settings-1.13.7); the accessor below
        this.settingItems = [];
        this._dom = null; this._pages = []; this._pageScroll = [];
      }
      /* the settings sidebar icon (d.ts SettingTab.icon, since 1.11). A plugin assigns it in its ctor
         (obsidian-git "git-pull-request", tasks "list-checks", templater "templater-icon"); the
         Community plugins group in Settings draws it next to the tab's name (ui/plugins/settings.js),
         and an assignment after the tab was added redraws that group. */
      get icon() { return this._icon; }
      set icon(v) { this._icon = v; if (K.setting && K.setting.pluginTabs.includes(this) && K.settingTabsChanged) K.settingTabsChanged(); }
      getSettingDefinitions() { return []; }
      update() {
        let defs;
        try { defs = this.getSettingDefinitions(); } catch (e) { console.error("[plugin] getSettingDefinitions:", e); defs = []; }
        this.settingItems = Array.isArray(defs) ? defs : [];
        const d = this._dom;   // a tab whose rendering is on screen re-renders
        if (d && this.containerEl.isConnected && (d.page ? this.containerEl.contains(d.page.rootEl) : d.nodes.some(n => this.containerEl.contains(n.el)))) { this._pages = sdRebase(this.settingItems, this._pages || []); this._pageScroll = (this._pageScroll || []).slice(0, this._pages.length); sdShow(this); }
      }
      getControlValue(key) { const v = this.app && this.app.vault; return v && typeof v.getConfig === "function" ? v.getConfig(key) : undefined; }
      setControlValue(key, value) {
        const v = this.app && this.app.vault;
        if (v && typeof v.setConfig === "function") return v.setConfig(key, value);
        throw new Error("unsupported in opensidian: app config is read-only to plugins");
      }
      refreshDomState() { if (this._dom) sdApply(this._dom); }
      display() { if (this.settingItems && this.settingItems.length) sdRender(this); }
      // stock: hide() leaves containerEl's children alone (a ctor-built header/content survives a switch or close);
      // only a declarative rendering (settingItems, owned by this class) is taken down
      hide() { const d = this._dom; sdTeardown(this); this._pages = []; this._pageScroll = []; if (d) this.containerEl.empty(); }
    }
    class PluginSettingTab extends SettingTab {
      // stock: id / name are the plugin manifest's, set here (a subclass ctor reads them); no manifest is a TypeError
      constructor(app, plugin) { super(); this.app = app; this.plugin = plugin; this.id = plugin.manifest.id; this.name = plugin.manifest.name; }
      getSettingDefinitions() { return []; }
      /* conventional plugin storage: this.plugin.settings, persisted with saveData */
      getControlValue(key) { const s = this.plugin && this.plugin.settings; return s != null && typeof s === "object" ? s[key] : undefined; }
      setControlValue(key, value) {
        const p = this.plugin; if (!p) return;
        if (p.settings == null || typeof p.settings !== "object") p.settings = {};
        p.settings[key] = value;
        return typeof p.saveData === "function" ? p.saveData(p.settings) : undefined;
      }
    }
    /* SettingGroup(containerEl): a .setting-group with an optional heading (+ header
       extra buttons), an optional search row before the items, and addSetting(cb). */
    class SettingGroup {
      constructor(containerEl) {
        installDom();
        this.groupEl = containerEl.createDiv({ cls: "setting-group" });
        this.headerEl = this.groupEl.createDiv({ cls: "setting-group-heading setting-item-heading" }); this.headerEl.hidden = true;
        this._headText = this.headerEl.createDiv({ cls: "setting-group-heading-text" });
        this._headCtl = null; this._searchEl = null;
        this.listEl = this.groupEl.createDiv({ cls: "setting-items" });
      }
      setHeading(t) { this.headerEl.hidden = false; this._headText.textContent = ""; if (t instanceof Node) this._headText.appendChild(t); else this._headText.textContent = String(t); return this; }
      addClass(...c) { for (const x of c) if (x) this.groupEl.addClass(x); return this; }
      addSetting(cb) { const s = new Setting(this.listEl); if (cb) cb(s); return this; }
      addSearch(cb) {
        if (!this._searchEl) { this._searchEl = mk("div", { cls: "setting-group-search" }); this.groupEl.insertBefore(this._searchEl, this.listEl); }
        const c = new SearchComponent(this._searchEl); if (cb) cb(c); return this;
      }
      addExtraButton(cb) {
        if (!this._headCtl) this._headCtl = this.headerEl.createDiv({ cls: "setting-group-controls" });
        this.headerEl.hidden = false;
        const b = new ExtraButtonComponent(this._headCtl); if (cb) cb(b); return this;
      }
    }
    /* SettingPage: a settings sub-page — rootEl holds titlebarEl (the title) and containerEl
       (the content display() renders). The fields are prototype accessors so a subclass sees
       them before its own constructor body runs; assigning title updates the titlebar. */
    class SettingPage {
      constructor() {
        installDom();
        this.app = K.app; this.icon = "";
        const rootEl = mk("div", { cls: "setting-page" });
        const titlebarEl = rootEl.createDiv({ cls: "setting-page-titlebar" });
        const titleEl = titlebarEl.createDiv({ cls: "setting-page-title" });
        const containerEl = rootEl.createDiv({ cls: "setting-page-content" });
        Object.defineProperty(this, "_sp", { value: { rootEl, titlebarEl, titleEl, containerEl, title: "" }, enumerable: false });
      }
      display() {}
      update() { this.containerEl.empty(); this.display(); }
      hide() { this.containerEl.empty(); }
    }
    for (const k of ["rootEl", "titlebarEl", "containerEl", "title"]) {
      Object.defineProperty(SettingPage.prototype, k, {
        configurable: true, enumerable: false,
        get() { return this._sp ? this._sp[k] : undefined; },
        set(v) {
          if (!this._sp) { Object.defineProperty(this, k, { value: v, writable: true, configurable: true, enumerable: true }); return; }
          if (k === "title") { this._sp.title = v == null ? "" : String(v); this._sp.titleEl.textContent = this._sp.title; }
          else this._sp[k] = v;
        },
      });
    }
    class ConfirmationModal extends Modal {
      constructor(app_) { super(app_); this.buttonContainerEl = this.modalEl.createDiv({ cls: "modal-button-container" }); this._initialFocus = null; }
      addClass(c) { if (c) this.modalEl.addClass(c); return this; }
      addButton(cb) { const b = new ConfirmationButton(this.buttonContainerEl, this); if (cb) cb(b); return this; }
      addCheckbox(label, cb) {
        const l = this.buttonContainerEl.createEl("label", { cls: "mod-checkbox" });
        const i = l.createEl("input", { attr: { type: "checkbox" } }); l.appendText(" " + String(label));
        i.addEventListener("change", () => { if (cb) cb(i.checked); });
        return this;
      }
      /* a dismissal button; text defaults to "Cancel" (a function argument is called on click) */
      addCancelButton(arg) {
        const text = typeof arg === "string" && arg ? arg : "Cancel", cb = typeof arg === "function" ? arg : null;
        return this.addButton(b => { b.setButtonText(text).setCancel().onClick(() => { if (cb) cb(); }); });
      }
      open() { super.open(); const f = this._initialFocus; if (f && this._open) try { f.buttonEl.focus(); } catch (_) {} }
    }

    /* -------------------------------------------------------------- Menu
       Shown through main.js's own context-menu machine (placeMenu/closeMenu),
       so a plugin menu looks like ours and publishes the same [menu:] census. */
    /* api-settings: the separator entry addSeparator() adds (rendered as a divider) */
    class MenuSeparator {
      constructor(menu) { this.menu = menu || null; this.dom = document.createElement("div"); this.dom.className = "sep"; }
    }
    class MenuItem {
      constructor(menu) {
        this.menu = menu;
        this.dom = document.createElement("div");
        this.dom.className = "menu-item";
        this.iconEl = document.createElement("span"); this.iconEl.className = "menu-item-icon";
        this.titleEl = document.createElement("span"); this.titleEl.className = "menu-item-title";
        this.dom.append(this.iconEl, this.titleEl);
        this.callback = null; this.section = "";
        this.dom.onmousedown = e => e.stopPropagation();
        this.dom.onclick = e => { if (this.disabled) return; menu.hide(); if (this.callback) try { this.callback(e); } catch (er) { console.error("[plugin] menu item:", er); } };
      }
      setTitle(t) { if (t instanceof Node) { this.titleEl.textContent = ""; this.titleEl.appendChild(t); } else this.titleEl.textContent = t; return this; }
      setIcon(i) { if (i) setIcon(this.iconEl, i); else this.iconEl.textContent = ""; return this; }
      setChecked(c) { this.dom.classList.toggle("mod-checked", !!c); return this; }
      setDisabled(d) { this.disabled = !!d; this.dom.classList.toggle("dis", !!d); return this; }
      setIsLabel(l) { this.dom.classList.toggle("is-label", !!l); return this; }
      setWarning(w) { this.dom.classList.toggle("mod-warning", !!w); return this; }
      setSection(s) { this.section = s; return this; }
      onClick(cb) { this.callback = cb; return this; }
      setSubmenu() { return new Menu(); }
    }
    class Menu extends Component {
      constructor() { super(); this.items = []; this._hide = []; this.dom = null; this.parentEl = null; }
      /* the element the menu belongs to; it carries has-active-menu while the menu is shown */
      setParentElement(el) { this.parentEl = el instanceof Element ? el : null; return this; }
      /* a history "back" dismisses the menu */
      onHistoryBack() { this.hide(); }
      setNoIcon() { return this; }
      setUseNativeMenu() { return this; }
      addItem(cb) { const it = new MenuItem(this); this.items.push(it); cb(it); return this; }
      addSeparator() { this.items.push(new MenuSeparator(this)); return this; }
      showAtMouseEvent(e) { return this.showAtPosition({ x: e.clientX, y: e.clientY }); }
      showAtPosition(p) {
        this._settled = true;
        installDom();
        if (typeof closeMenu === "function") closeMenu();
        const m = document.createElement("div");
        m.className = "ctxmenu menu os-plugin-menu";
        for (const it of this.items) {
          if (it === null) { const s = document.createElement("div"); s.className = "sep"; m.appendChild(s); }
          else if (it instanceof MenuSeparator) m.appendChild(it.dom);
          else m.appendChild(it.dom);
        }
        this.dom = m;
        if (typeof placeMenu === "function") placeMenu(m, p.x, p.y);
        else { m.style.left = p.x + "px"; m.style.top = p.y + "px"; document.body.appendChild(m); }
        if (this.parentEl) this.parentEl.addClass("has-active-menu");
        this.load();
        // main.js closes menus from its document mousedown / Escape; notice that
        const watch = () => { if (this.dom && !this.dom.isConnected) this._closed(); };
        this._mo = new MutationObserver(watch); this._mo.observe(document.body, { childList: true });
        return this;
      }
      _closed() { if (this._mo) this._mo.disconnect(); this._mo = null; const d = this.dom; this.dom = null; if (!d) return; if (this.parentEl) this.parentEl.removeClass("has-active-menu"); this.unload(); for (const f of this._hide.splice(0)) try { f(); } catch (e) { console.error(e); } }
      hide() { this._settled = true; if (this.dom && typeof closeMenu === "function" && typeof menuEl !== "undefined" && menuEl === this.dom) closeMenu(); else if (this.dom) this.dom.remove(); this._closed(); }
      close() { this.hide(); }
      onHide(cb) { this._hide.push(cb); }
      /* a menu for an event: shown at the event's position once the event's dispatch is over (a task,
         not a microtask — microtasks run between the listeners of a user event), so every listener
         can still add items; a plugin that already showed or hid it keeps its choice */
      static forEvent(e) {
        const m = new Menu(); m._evt = e;
        setTimeout(() => {
          if (m._settled) return;
          const x = e && typeof e.clientX === "number" ? e.clientX : 0, y = e && typeof e.clientY === "number" ? e.clientY : 0;
          try { m.showAtPosition({ x, y }); } catch (err) { console.error(err); }
        }, 0);
        return m;
      }
    }

    /* SecretComponent (typings 1.11.1, setValue/onChange 1.11.4) — goal cp-secrets. A picker over
       app.secretStorage (the OS keyring, see api-vault.js): the value is a secret ID, never the
       secret itself; onChange gets the chosen id, or null for none. "New secret…" opens an inline
       id + password form that stores through app.secretStorage.setSecret and selects the new id;
       the typed value is never echoed, kept or put in the DOM after the save. With no keyring
       (or restricted mode) the picker is disabled and shows secretStorage's unsupported error —
       it never pretends a secret was stored. */
    const SC_NEW = "+new";   // not a valid secret id, so it can never collide with one
    class SecretComponent extends BaseComponent {
      constructor(app, containerEl) {
        super(); installDom();
        this.app = app; this.containerEl = containerEl; this._cb = null; this._value = null; this._form = null;
        this.selectEl = containerEl.createEl("select", { cls: "dropdown os-secret-select" });
        this.errorEl = containerEl.createDiv({ cls: "os-secret-error" }); this.errorEl.hidden = true;
        this.selectEl.addEventListener("change", () => {
          const v = this.selectEl.value;
          if (v === SC_NEW) { this.selectEl.value = this._value == null ? "" : this._value; this._openForm(); return; }
          this._pick(v === "" ? null : v);
        });
        this._fill();
      }
      _store() { return this.app && this.app.secretStorage; }
      _fill() {
        const sel = this.selectEl; sel.empty();
        let ids = [];
        try { const s = this._store(); if (!s) throw new Error("secretStorage: no app"); ids = s.listSecrets(); this.errorEl.hidden = true; }
        catch (e) { this.errorEl.setText(String((e && e.message) || e)); this.errorEl.hidden = false; sel.disabled = true; sel.createEl("option", { value: "", text: "Secret storage unavailable" }); return; }
        sel.disabled = !!this.disabled;
        sel.createEl("option", { value: "", text: "No secret" });
        for (const id of ids.slice().sort()) sel.createEl("option", { value: id, text: id });
        if (this._value != null && !ids.includes(this._value)) sel.createEl("option", { value: this._value, text: this._value + " (missing)" });
        sel.createEl("option", { value: SC_NEW, text: "New secret…" });
        sel.value = this._value == null ? "" : this._value;
      }
      _pick(id) { this._value = id; this._fill(); if (this._cb) this._cb(id); }
      _openForm() {
        if (this._form) return;
        const f = this._form = this.containerEl.createDiv({ cls: "os-secret-new" });
        const idEl = f.createEl("input", { attr: { type: "text", placeholder: "id (e.g. my-api-key)", spellcheck: "false", autocomplete: "off" } });
        const valEl = f.createEl("input", { attr: { type: "password", placeholder: "secret", autocomplete: "new-password" } });
        const msg = f.createDiv({ cls: "os-secret-error" }); msg.hidden = true;
        const close = () => { valEl.value = ""; f.remove(); this._form = null; };
        f.createEl("button", { text: "Save", cls: "mod-cta" }).addEventListener("click", () => {
          const id = idEl.value.trim();
          try { this._store().setSecret(id, valEl.value); } catch (e) { msg.setText(String((e && e.message) || e)); msg.hidden = false; return; }
          close(); this._pick(id);
        });
        f.createEl("button", { text: "Cancel" }).addEventListener("click", close);
        idEl.focus();
      }
      setDisabled(d) { super.setDisabled(d); this._fill(); return this; }
      setValue(value) { this._value = value == null || value === "" ? null : String(value); this._fill(); return this; }
      onChange(cb) { this._cb = cb; return this; }
    }
    Object.assign(K.exportClasses, { ConfirmationButton, DisplayValueComponent, MenuSeparator, ProgressBarComponent, SecretComponent });
    Object.assign(K, { AbstractTextComponent, BaseComponent, ButtonComponent, ColorComponent, ConfirmationButton, ConfirmationModal, DisplayValueComponent, DropdownComponent, ExtraButtonComponent, FuzzySuggestModal, Menu, MenuItem, MenuSeparator, Modal, MomentFormatComponent, PluginSettingTab, ProgressBarComponent, SearchComponent, SecretComponent, Setting, SettingGroup, SettingPage, SettingTab, SliderComponent, SuggestModal, TextAreaComponent, TextComponent, ToggleComponent, ValueComponent, openModals });
  });
  reg("ui:setting", function (K) {
    const { host, be } = K;
    const ui = () => window.__osPluginSettings;   // ui/plugins/settings.js: puts tabs on the settings page
    const isOpen = () => typeof settingsOpen !== "undefined" && !!settingsOpen;
    const tabsChanged = () => { const s = ui(); if (s) try { s.tabsChanged(); } catch (e) { console.error(e); } };
    /* the app's own panes as SettingTab objects, under stock 1.13.7's tab ids (about = General,
       file = Files and links). Their content is opensidian's settings page, which main.js builds into
       #spage, so display() asks settings.js to show the pane and containerEl stays an empty detached
       div. Core plugins ("plugins") has no tab: the pane is gone by operator ruling (goal/noplugins). */
    /* built without SettingTab's constructor: that installs the DOM augments, which must wait for a plugin */
    const appTabProto = Object.create(K.SettingTab ? K.SettingTab.prototype : Object.prototype, {
      display: { value() { const s = ui(); if (s) s.builtin(this); }, writable: true, configurable: true },
      hide: { value() {}, writable: true, configurable: true },
    });
    const appTab = (id, name, pane) => {
      const t = Object.create(appTabProto), c = document.createElement("div");
      c.className = "vertical-tab-content";
      Object.defineProperty(t, "app", { get: () => K.app, enumerable: true, configurable: true });
      Object.defineProperty(t, "_pane", { value: pane });
      return Object.assign(t, { containerEl: c, icon: undefined, settingItems: [], _dom: null, _pages: [], id, name });
    };
    const settingTabs = [["about", "General", "general"], ["appearance", "Appearance", "appearance"], ["interface", "Interface", "interface"],
      ["editor", "Editor", "editor"], ["file", "Files and links", "fileslinks"], ["hotkeys", "Hotkeys", "hotkeys"],
      ["keychain", "Keychain", "keychain"], ["community-plugins", "Community plugins", "community"]].map(a => appTab(...a));
    const byId = id => settingTabs.find(t => t.id === id) || setting.pluginTabs.find(t => t && t.id === id) || null;
    /* app.setting, stock 1.13.7's behaviour (scripts/api-probes.d/shost.js):
       openTab(t) hides the active tab, makes t active (lastTabId = t.id) and runs t.display(); it never
       opens the modal (a tab opened while it is closed renders into its detached containerEl).
       open() is a no-op while open, else opens on lastTabId's tab (gone: General). close() hides the
       active tab, activeTab = null, lastTabId kept. closeActiveTab() does the same with the modal left
       open. openTabById(id): the tab with that exact id (built-in or plugin) after openTab, or null with
       nothing changed. One divergence: openTabById on a CLOSED modal also opens it (stock leaves it
       closed) — this app's own callers and plugins that skip open() expect the tab on screen. */
    const setting = {
      activeTab: null, lastTabId: undefined, pluginTabs: [], settingTabs,
      open() {
        if (isOpen() || typeof openSettings !== "function") return;
        const s = ui(); if (s) s.pane(byId(setting.lastTabId) || settingTabs[0]);
        openSettings();
      },
      close() { if (isOpen() && typeof closeSettings === "function") closeSettings(); else setting.closeActiveTab(); },
      openTab(t) {
        setting.closeActiveTab();
        // stock: activeTab takes the argument first, so openTab(null) / openTab() leave activeTab
        // null / undefined and throw a TypeError on t.id with lastTabId unchanged
        setting.activeTab = t;
        setting.lastTabId = t.id;
        const s = ui(); if (s) s.mount(t);
        // a tab with setting definitions renders from them (SettingTab.display), not through its own display() override
        if (t.settingItems && t.settingItems.length && K.SettingTab) K.SettingTab.prototype.display.call(t); else t.display();
      },
      openTabById(id) {
        const t = byId(id);
        if (!t) return null;
        setting.openTab(t);
        if (!isOpen()) setting.open();
        return t;
      },
      closeActiveTab() { const t = setting.activeTab; if (t) { try { t.hide(); } catch (e) { console.error("[plugin] SettingTab.hide:", e); } setting.activeTab = null; } },
      addSettingTab(t) { if (t && typeof t.update === "function") { try { t.update(); } catch (e) { console.error("[plugin] SettingTab.update:", e); } } setting.pluginTabs.push(t); tabsChanged(); },
      removeSettingTab(t) {
        if (t && setting.activeTab === t) setting.closeActiveTab();
        const i = setting.pluginTabs.indexOf(t); if (i >= 0) { setting.pluginTabs.splice(i, 1); tabsChanged(); }
      },
    };
    Object.assign(K, { setting, settingTabsChanged: tabsChanged });
  });
  reg("ui:inputsuggest", function (K) {
    const { host, be } = K;
    const { EA, td } = K;
    /* AbstractInputSuggest (td-census-fix: Templater 2.25.0's folder/file pickers
       in its settings tab subclass it, at module scope): a suggestion popover bound
       to an <input> / contenteditable. Typing calls getSuggestions(value); a click
       or Enter calls selectSuggestion(item, evt), whose default hands the item to
       the onSelect callback; Escape / blur close it. */
    class AbstractInputSuggest extends (EA ? EA.PopoverSuggest : class { constructor(a) { this.app = a; this.isOpen = false; this.suggestEl = document.createElement("div"); } open() { this.isOpen = true; } close() { this.isOpen = false; } }) {
      constructor(app_, textInputEl) {
        super(app_);
        this.textInputEl = textInputEl; this.limit = 100; this._cb = null; this._items = []; this._sel = 0;
        this.suggestEl.className = "suggestion-container os-isug"; this.suggestEl.hidden = true;
        const el = textInputEl;
        if (!el || typeof el.addEventListener !== "function") return;
        el.addEventListener("input", () => this._refresh());
        el.addEventListener("focus", () => this._refresh());
        el.addEventListener("blur", () => setTimeout(() => this.close(), 150));
        el.addEventListener("keydown", e => {
          if (!this.isOpen || !this._items.length) return;
          if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); this._sel = (this._sel + (e.key === "ArrowDown" ? 1 : this._items.length - 1)) % this._items.length; this._draw(); }
          else if (e.key === "Enter") { e.preventDefault(); this._choose(this._sel, e); }
          else if (e.key === "Escape") { e.preventDefault(); this.close(); }
        });
      }
      getValue() { const el = this.textInputEl; return el ? (el.isContentEditable ? el.textContent : String(el.value || "")) : ""; }
      setValue(v) { const el = this.textInputEl; if (!el) return; if (el.isContentEditable) el.textContent = String(v); else el.value = String(v); }
      onSelect(cb) { this._cb = cb; return this; }
      getSuggestions() { return []; }
      selectSuggestion(item, evt) { if (this._cb) this._cb(item, evt); }
      _refresh() {
        if (this._picking) return;   // an "input" a selectSuggestion fires (templater: setValue + trigger("input") + close) does not reopen it, as in stock
        const q = this.getValue();
        Promise.resolve().then(() => this.getSuggestions(q)).then(items => {
          this._items = (items || []).slice(0, this.limit || 100); this._sel = 0;
          if (!this._items.length || document.activeElement !== this.textInputEl) { this.close(); return; }
          this._draw();
        }).catch(e => console.error("[plugin] AbstractInputSuggest.getSuggestions:", e));
      }
      _draw() {
        const el = this.suggestEl; el.textContent = "";
        this._items.forEach((it, i) => {
          const d = document.createElement("div");
          d.className = "suggestion-item" + (i === this._sel ? " is-selected" : "");
          try { this.renderSuggestion(it, d); } catch (e) { console.error("[plugin] renderSuggestion:", e); }
          d.addEventListener("mousedown", e => { e.preventDefault(); this._choose(i, e); });
          el.appendChild(d);
        });
        const r = this.textInputEl.getBoundingClientRect();
        Object.assign(el.style, { position: "fixed", left: r.left + "px", top: r.bottom + "px", minWidth: r.width + "px", zIndex: 1000 });
        if (el.parentNode !== document.body) document.body.appendChild(el);
        el.hidden = false; this.open();
      }
      _choose(i, e) {
        const it = this._items[i]; if (it === undefined) return;
        this._picking = true;
        try { this.selectSuggestion(it, e); } catch (er) { console.error("[plugin] selectSuggestion:", er); } finally { this._picking = false; }
        this.close();
      }
      close() { super.close(); this.suggestEl.hidden = true; this.suggestEl.textContent = ""; this._items = []; }
    }
    AbstractInputSuggest.prototype.limit = 100;   // the default cap, also visible before construction
    Object.assign(K, { AbstractInputSuggest });
  });
})();
