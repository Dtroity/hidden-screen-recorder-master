# Comparative encode bench (10 minutes)

**Status:** protocol only. Fill the results table with **measured** values after you run recordings. Do not invent numbers.

## Fixed parameters

| Item | Value |
|------|--------|
| Duration | exactly 10:00 |
| Encode size | 1280×720 (`max720p`) |
| Cursor | on |
| System audio / mic | **off** (verify no audio stream with FFmpeg; MF may still reserve a disabled track — see docs) |
| Content scenes | S1 static desk + small text · S2 text scroll · S3 busy UI |

Manual content playback is required (scroll a document / open apps). Automating UI content is out of scope.

## Matrix

| ID | Backend | Notes |
|----|---------|--------|
| A1 | `media_foundation` (default HW) | baseline; bitrate e.g. 2000 kb/s @ 10 FPS |
| A2 | MF + local SW patch | only if `[patch.crates-io]` applied; same settings as A1 |
| F1 | `ffmpeg` Economy | 1 FPS, 300 kb/s |
| F2 | `ffmpeg` Balanced | 2 FPS, 650 kb/s |
| F3 | `ffmpeg` Readable | 5 FPS, 1200 kb/s |

## Capture metrics

During each run note CPU % and Working Set (Task Manager / PerfMon). After stop, run:

```powershell
.\tools\analyze-recording.ps1 -Path "C:\path\to\file.mp4" -Backend media_foundation -Profile custom -Notes "S1"
```

Record: size_MB_dec, duration_s, resolution, avg_fps, video_bit_rate, format_bit_rate, has_audio, dropped_frames (from debug log), visual readability (ok / partial / fail), moov/finalize ok.

## Results (fill after runs)

| ID | Scene | size_MB | duration_s | video_Mb/s | has_audio | CPU% | RAM | text_ok | notes |
|----|-------|---------|------------|------------|-----------|------|-----|---------|-------|
| A1 | | | | | | | | | **not run in CI** |
| A2 | | | | | | | | | |
| F1 | | | | | | | | | |
| F2 | | | | | | | | | |
| F3 | | | | | | | | | |

## Interpretation rules

1. Do not blame HW encode until A1 vs A2 are both measured.
2. Compare measured `format_bit_rate` / `video_bit_rate` to the UI target.
3. Estimate weekly archive size with `est_40h_GB_dec` from the analyze script (decimal GB).
