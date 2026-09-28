# Changelog

## 0.3.0 — 2026-09-29

### New
- **Screen recording** of a screen, a region or a window to MP4 (H.264 + AAC), with system audio and the microphone mixed into one track.
  - macOS: ScreenCaptureKit + AVFoundation. The microphone needs macOS 15.
  - Windows: Windows.Graphics.Capture + Media Foundation.
  - Linux: GStreamer, on X11 and on Wayland through the desktop portal. Falls back to WebM when the MP4 encoders are not installed.
  - A floating control shows the elapsed time and **Detener**. On macOS and Windows it never appears in the video.
- **Sessions**: pin a screen, region or window and grab it instantly with the usual shortcut (`Ctrl+Shift+X` / `⇧⌘X`) while you are in a call. There is also a **Capturar** button and a tray entry.
  - The session records your microphone and the computer's audio as separate tracks.
  - **Local transcription** with whisper.cpp: 99 languages, auto-detect or a fixed language, optionally live during the call. Models are downloaded once, or imported from a file.
  - A viewer shows each image with the transcript next to it. Clicking a line shows the image that was on screen at that moment, and the time range of each image can be edited.
  - Each session is exported as a standalone `index.html` plus `transcript.txt`, `.srt` and `.md`.
  - Interrupted sessions are recovered and can be transcribed again from the viewer.
- Window capture in the target picker (screens and windows with thumbnails).

### Changed
- **macOS 13 Ventura or later is now required** (ScreenCaptureKit).
- On Linux, the `.deb` and `.rpm` now depend on the GStreamer base, good, PulseAudio and PipeWire plugins, and recommend the ugly, bad and libav plugins for MP4.

### Known limitations
- Whisper transcribes each chunk of audio in one language. If someone switches language in the middle of a sentence, that part may be dropped.
- On Windows, monitors larger than 4K are recorded at their native size.
- Linux recording is untested on real desktops beyond the X11 smoke test in CI. On Wayland, the global shortcut does not work, so use the **Capturar** button or the tray menu during a session.
- The app is not signed with a Developer ID, so macOS asks for Screen Recording and Microphone permission again after each update.

## 0.2.0 — 2026-09-28

### New
- macOS (universal: Apple Silicon and Intel) and Linux (`.deb`, `.rpm`, `.AppImage`) builds.
- On macOS the region capture shortcut is `⇧⌘X`.
