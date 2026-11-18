#!/bin/bash
# fixture: one assignment of the commit variable is a branch, not a commit
case "$ARCH" in
  x86_64)  COMMIT_PLATFORM=1111111111111111111111111111111111111111111111111111111111111111 ;;
  aarch64) COMMIT_PLATFORM=49 ;;
esac
flatpak --user install -y flathub runtime/org.gnome.Platform/$ARCH/49
flatpak --user update -y --commit="$COMMIT_PLATFORM" runtime/org.gnome.Platform/$ARCH/49
