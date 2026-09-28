#!/usr/bin/env bash
# System libraries needed to build ScreenCut on Ubuntu/Debian.
#   Tauri:  webkit2gtk, appindicator (tray icon), rsvg, patchelf (AppImage)
#   xcap:   xcb/xrandr (X11), dbus (Wayland portal), gbm/egl/wayland (wlroots capture)
#   cpal:   ALSA (needed even with the pulseaudio feature)
#   video:  GStreamer + plugins-base
#   whisper.cpp: cmake, clang (bindgen)
set -euo pipefail
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
  libxcb1-dev libxrandr-dev libdbus-1-dev \
  libgbm-dev libegl-dev libwayland-dev \
  libasound2-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  cmake clang libclang-dev
