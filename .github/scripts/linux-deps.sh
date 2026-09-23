#!/usr/bin/env bash
# System libraries needed to build ScreenCut on Ubuntu/Debian.
#   Tauri:  webkit2gtk, appindicator (tray icon), rsvg, patchelf (AppImage)
#   xcap:   xcb/xrandr (X11), dbus (Wayland portal), pipewire (+ clang for its
#           bindings), gbm/egl (wlroots capture)
set -euo pipefail
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
  libxcb1-dev libxrandr-dev libdbus-1-dev \
  libpipewire-0.3-dev libspa-0.2-dev clang libclang-dev \
  libgbm-dev libegl-dev libwayland-dev
