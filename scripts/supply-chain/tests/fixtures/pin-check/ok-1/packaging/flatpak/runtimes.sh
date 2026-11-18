#!/bin/bash
# fixture: commit pins through variables, every assignment 64 hex
case "$ARCH" in
  x86_64)  COMMIT_PLATFORM=1111111111111111111111111111111111111111111111111111111111111111 ;;
  aarch64) COMMIT_PLATFORM=2222222222222222222222222222222222222222222222222222222222222222 ;;
esac
flatpak --user install -y --noninteractive --no-related flathub runtime/org.gnome.Platform/$ARCH/49
flatpak --user update -y --noninteractive --no-related --commit="$COMMIT_PLATFORM" runtime/org.gnome.Platform/$ARCH/49
