# Encode backends

## Default: FFmpeg (HEVC)

`encode_backend` defaults to `ffmpeg` (HEVC / `libx265`, tag `hvc1`).  
Requires `ffmpeg.exe` with **libx265** next to the app (GPL static build).

Capture remains **Windows Graphics Capture** or **GDI** (unchanged).

## Fallback: Media Foundation

Optional UI choice. Encoding uses `windows-capture` 2.0.1 → WinRT `MediaTranscoder` (H.264).

**Known limits of this crate path**

- Public API exposes only subtype, bitrate (bits/s), size, and frame rate.
- There is **no** public switch for hardware vs software encode.
- `SetHardwareAccelerationEnabled(true)` is hard-coded inside the dependency.
- There is **no** CBR / CRF / maxrate control beyond `SetBitrate`.

Do **not** conclude that hardware encoding ignores bitrate until A/B measurements (HW vs local SW patch) are recorded with `tools/analyze-recording.ps1`.

### Optional software-MF A/B (diagnostics only)

1. Copy the crates.io `windows-capture` sources into `third_party/windows-capture`.
2. Change both `SetHardwareAccelerationEnabled(true)` calls to `false` in `src/encoder.rs`.
3. In the app `Cargo.toml`:

```toml
[patch.crates-io]
windows-capture = { path = "third_party/windows-capture" }
```

4. Build a **separate** test binary; do not ship this patch as the default product path.
5. Run the same 10‑minute scenario with HW (unpatched) and SW (patched); compare with the analyze script.

## FFmpeg HEVC details

Resolve `ffmpeg.exe` in order:

1. `ffmpeg_path` from settings (if set)
2. Next to the application executable
3. `ffmpeg/ffmpeg.exe` next to the executable
4. `PATH`

Required encoder: **libx265**. Audio (when enabled): AAC 96 kb/s; default is **audio off**.

Video: raw BGRA on stdin, capped queue of **2** frames (drop + log on overflow). Mux as MP4 with `-tag:v hvc1`.

### Starter quality profiles (default Balanced)

| Profile | FPS | Resolution | Target video bitrate (HEVC) |
|---------|-----|------------|-----------------------------|
| Economy (archive) | 1 | max 1080p | 450 kb/s |
| Balanced (default) | 2 | max 1080p | 800 kb/s |
| Readable | 5 | max 1080p | 1600 kb/s |

FFmpeg applies `-b:v` / `-maxrate` / `-bufsize` for size control.

### Storage orientation (decimal MB/GB)

| Average stream | ≈ size / 10 min | ≈ size / 40 h |
|----------------|-----------------|---------------|
| 450 kb/s | 33.8 MB | 8.1 GB |
| 800 kb/s | 60 MB | 14.4 GB |
| 1600 kb/s | 120 MB | 28.8 GB |

Confirm with ffprobe after real recordings.

## Audio tracks

If **system audio** and **microphone** are both off:

- **FFmpeg:** `-an` — no audio stream.
- **Media Foundation:** the app disables the audio encoder (`AudioSettingsBuilder::disabled(true)`) and does **not** feed silence. `windows-capture` still constructs an audio stream descriptor in its MediaStreamSource; a residual empty/disabled audio track in the MP4 is a known crate limitation. Full omission on MF would require a dependency change beyond this pass.

## Packaging FFmpeg

See `third_party/ffmpeg/README.md`. Prefer a documented GPL static build with **libx265**; ship `ffmpeg.exe` as a **separate** process and include license texts. Do not download FFmpeg during a recording session.
