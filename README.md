# shots/196-canvas-recording-trusted (#196, PR #198)
Box t1 run (BOX_BX=1 display lease), harness h#64 ef53247, same media vault, 1100x700.
- 196-before-develop-restricted.png: develop 8e9199d, restricted vault: tone.mp3 / clip.mp4 cards show "Cannot play ... media cards play in a trusted vault only" (no Play button).
- 196-after-head-restricted.png: PR head 825c363, restricted vault: same cards show a Play button ("restricted vault: read on click").
- 196-after-head-trusted-recording.png: PR head 825c363, trusted vault: canvas card for a recorded Recording *.webm shows the player with Play.
