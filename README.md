# #158 collapsed left sidebar after restart

Harness phase `cfxlayout` (harness phase/158-collapsed-sidebar-restore b794619b) on the box, 1100x700.
The app boots on stock Obsidian 1.13.7's workspace.json with every tab closed and the left sidebar collapsed.

- before2-8e9199d-cfxlayout-boot.png: develop 8e9199d. The left sidebar comes back OPEN ([side:l1r0]).
- after2-46a8e2c-cfxlayout-boot.png: fix head 46a8e2c. The left sidebar stays collapsed ([side:l0r0]), as in stock 1.13.7.
