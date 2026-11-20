#!/bin/bash
# fixture: the commit variable is never assigned in this file
flatpak --user install -y flathub runtime/org.gnome.Platform/x86_64/49
flatpak --user update -y --commit=${COMMIT_PLATFORM} runtime/org.gnome.Platform/x86_64/49
