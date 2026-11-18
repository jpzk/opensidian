# shots/220-i18n-46a2d03 — Language setting + translated UI (#220, PR #228)

Head: feat/220-i18n 46a2d03 (PR jpzk/opensidian#228), harness phase/220-i18n ea770897 (h#92).
Develop base: 8e9199d (has no Language setting and no locales/, so there is nothing to compare).
Box run: t1 i18n, run 20261118T122911Z, bin 63fc9c1f (new box hz-176635734) (Fedora 44, boxd display lease, 1280x800).

Per language `<c>` in en, zh-CN, ru, es, ja:
- i18n-settings-<c>.png — Settings > General with that language active
- i18n-list-<c>.png — the Language dropdown open: Auto (system) + the 6 native names, checkmark on the active one
- i18n-menu-<c>.png — the tab context menu in that language
- i18n-palette-<c>.png — the command palette in that language

Extra:
- i18n-auto-ja.png — Auto (system) with LANG=ja_JP.UTF-8 → Japanese
- i18n-fallback.png — a partial ja draft (translator preview, OPENSIDIAN_LOCALES_DIR) missing keys → those show in English

ko (한국어): BLOCKED "box font". The box has no Hangul font (fc-list :lang=ko returns nothing, last checked
2026-11-18 12:2xZ on the new box), so Korean text renders blank. The 한국어 row in the list shots is blank for the same reason.
The ko .ftl parses and has the same keys as en (T0 cargo test). Shots will follow once the box has a Hangul font.

Known English leftovers (outside the extraction scope, listed in PR #228): button labels such as "Check for updates",
and the "Pin" / "Bookmark…" tab menu items.

This set replaces shots/220-i18n (f5d72bc). 46a2d03 adds the review fixes: the latest language switch wins; canvas menu anchors and icons match translated titles.
