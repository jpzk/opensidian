// SPDX-License-Identifier: GPL-3.0-or-later
// Opensidian CM6 vendor entry: ONE instance of every module, published on a
// frozen registry. The editor and (later) plugins' require() read the SAME
// objects, so a plugin's StateField/Facet is identity-equal to the host's.
import * as state from "@codemirror/state";
import * as view from "@codemirror/view";
import * as language from "@codemirror/language";
import * as commands from "@codemirror/commands";
import * as search from "@codemirror/search";
import * as autocomplete from "@codemirror/autocomplete";
import * as lint from "@codemirror/lint";
import * as langMarkdown from "@codemirror/lang-markdown";
import * as langHtml from "@codemirror/lang-html";
import * as langCss from "@codemirror/lang-css";
import * as langJavascript from "@codemirror/lang-javascript";
import * as lezerCommon from "@lezer/common";
import * as lezerHighlight from "@lezer/highlight";
import * as lezerLr from "@lezer/lr";
import * as lezerMarkdown from "@lezer/markdown";

const mods = {
  "@codemirror/state": state,
  "@codemirror/view": view,
  "@codemirror/language": language,
  "@codemirror/commands": commands,
  "@codemirror/search": search,
  "@codemirror/autocomplete": autocomplete,
  "@codemirror/lint": lint,
  "@codemirror/lang-markdown": langMarkdown,
  "@codemirror/lang-html": langHtml,
  "@codemirror/lang-css": langCss,
  "@codemirror/lang-javascript": langJavascript,
  "@lezer/common": lezerCommon,
  "@lezer/highlight": lezerHighlight,
  "@lezer/lr": lezerLr,
  "@lezer/markdown": lezerMarkdown,
};
const reg = Object.create(null);
for (const k of Object.keys(mods)) reg[k] = Object.freeze(Object.assign(Object.create(null), mods[k]));
Object.defineProperty(window, "__osModules", { value: Object.freeze(reg), writable: false, configurable: false, enumerable: false });
