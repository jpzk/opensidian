# shots/144-security

Settings > Security screenshots for #144 / PR #193, taken on the box at feat/144-security 8212687 (harness 5bace45, display :193, vault Security-demo, 1100x700).

- sec-01-security-tab-top.png: top of the tab (Lockdown, Network, Media)
- sec-01-security-tab-media.png: Media and rendering
- sec-01-security-tab-plugins.png: Plugins and clipboard
- sec-01-security-tab-engine.png: Engine (JIT)
- sec-02-lockdown-confirm.png: the Lockdown confirm dialog

## After the review fixes (PR #193 comment 6949681867), head ec15533

Taken on the box at feat/144-security ec1553336e85 (harness phase/144-security f3b57eda, display lease, phases seclockdown / secremote, 1100x700). Before = the 8212687 shots in assets/pr193-shots.

- sec-11-after-lockdown-confirm.png: the Lockdown confirm, default focus on Cancel (Esc closes it; Enter = Cancel)
- sec-12-after-lockdown-marker.png: Lockdown on: the marker is its own row in the left sidebar above the vault bar; the vault name is not covered
- sec-13-after-remote-blocked.png: Load external resources off: the placeholder says the remote image "was blocked", no "could not be found"
