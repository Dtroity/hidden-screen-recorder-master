# Bundling FFmpeg (optional)

The application does **not** download FFmpeg at runtime.

## Recommended layout

Place next to `hidden-screen-recorder.exe` / `recorder-agent.exe`:

```text
ffmpeg.exe
```

or:

```text
ffmpeg/ffmpeg.exe
```

Alternatively set `ffmpeg_path` in `%APPDATA%\hidden-screen-recorder\config\settings.json`.

## Build requirements

- Windows x64 CLI build
- Encoder **libx265** present (`ffmpeg -encoders | findstr libx265`) — required (HEVC default)
- AAC encoder for optional audio

Suggested public builds (verify checksums yourself before shipping):

- [BtbN GitHub releases](https://github.com/BtbN/FFmpeg-Builds/releases)
- [Gyan.dev essentials](https://www.gyan.dev/ffmpeg/builds/)

**Recommended for this app (Windows x64):**

| Choose | Why |
|--------|-----|
| **`ffmpeg-n9.0-latest-win64-gpl-9.0.zip`** (GPL, **static**) | Stable 9.0; includes **libx265**; one `ffmpeg.exe` |
| Avoid `master-latest` for production | Rolling build, can break day-to-day |
| Avoid `*-shared` unless you know you need it | Needs accompanying DLLs next to the exe |
| `*-lgpl-*` | Often **without** libx265 — recording will fail |

Unpack and copy **`ffmpeg.exe`** into `OfficeRecorder\bin\` (next to the app exe).

FFmpeg HEVC is the **default** encode backend.

## Licensing

- FFmpeg itself is LGPL/GPL depending on how it was built.
- **libx264** is GPL. Distributing an FFmpeg binary linked with libx264 typically requires complying with the GPL **for that FFmpeg binary**.
- This Rust application stays MIT and talks to FFmpeg only as a **separate process** (no static linking of libav*).
- Ship license texts alongside `ffmpeg.exe` (for example `LICENSE.txt`, `COPYING.GPLv2` from the chosen build).
- Do **not** statically link GPL FFmpeg libraries into the MIT application binary.

If you cannot distribute GPL binaries with your installer, leave FFmpeg out of the package and document that users must supply their own `ffmpeg.exe` when enabling the FFmpeg backend.
