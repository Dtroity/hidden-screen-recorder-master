# Analyze a finished MP4 from Hidden Screen Recorder comparative tests.
# Prefer ffprobe when available; always report file size. Does not delete or overwrite files.
#
# Usage:
#   .\tools\analyze-recording.ps1 -Path "C:\Videos\test.mp4"
#   .\tools\analyze-recording.ps1 -Path "C:\Videos\*.mp4" -Backend media_foundation -Profile economy

param(
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [string]$Backend = "",
    [string]$Profile = "",
    [string]$Notes = ""
)

$ErrorActionPreference = "Stop"

function Format-Mbps([double]$bps) {
    if ($bps -le 0) { return "n/a" }
    return ("{0:N3} Mb/s" -f ($bps / 1e6))
}

function Format-HoursGb([double]$bps) {
    # Decimal GB for 40 hours (TZ storage table).
    if ($bps -le 0) { return "n/a" }
    $bytes = $bps / 8.0 * 40.0 * 3600.0
    return ("{0:N2} GB / 40h" -f ($bytes / 1e9))
}

$ffprobe = Get-Command ffprobe -ErrorAction SilentlyContinue
$files = @()
if (Test-Path -LiteralPath $Path -PathType Leaf) {
    $files = @(Get-Item -LiteralPath $Path)
} else {
    $files = @(Get-Item $Path -ErrorAction SilentlyContinue | Where-Object { -not $_.PSIsContainer })
}

if ($files.Count -eq 0) {
    Write-Error "No files matched: $Path"
}

foreach ($file in $files) {
    $sizeBytes = $file.Length
    $sizeMb = $sizeBytes / 1e6
    $row = [ordered]@{
        file           = $file.FullName
        backend        = $Backend
        profile        = $Profile
        size_MB_dec    = [math]::Round($sizeMb, 2)
        size_bytes     = $sizeBytes
        duration_s     = $null
        width          = $null
        height         = $null
        avg_fps        = $null
        video_bit_rate = $null
        format_bit_rate= $null
        has_audio      = $null
        video_codec    = $null
        audio_codec    = $null
        est_40h_GB_dec = $null
        notes          = $Notes
        ffprobe        = [bool]$ffprobe
    }

    if ($ffprobe) {
        $json = & ffprobe -v error -show_format -show_streams -of json -- $file.FullName | ConvertFrom-Json
        $row.duration_s = [double]$json.format.duration
        $row.format_bit_rate = [int64]$json.format.bit_rate
        $row.est_40h_GB_dec = Format-HoursGb ([double]$json.format.bit_rate)
        $video = $json.streams | Where-Object { $_.codec_type -eq "video" } | Select-Object -First 1
        $audio = $json.streams | Where-Object { $_.codec_type -eq "audio" } | Select-Object -First 1
        if ($video) {
            $row.width = [int]$video.width
            $row.height = [int]$video.height
            $row.avg_fps = $video.avg_frame_rate
            $row.video_bit_rate = if ($video.bit_rate) { [int64]$video.bit_rate } else { $null }
            $row.video_codec = $video.codec_name
        }
        $row.has_audio = [bool]$audio
        if ($audio) { $row.audio_codec = $audio.codec_name }
    } else {
        $row.notes = ($Notes + " ; ffprobe not found — only size reported").Trim(" ;")
    }

    Write-Output ("==== {0} ====" -f $file.Name)
    Write-Output ("size_MB_dec (decimal) : {0}" -f $row.size_MB_dec)
    Write-Output ("duration_s            : {0}" -f $row.duration_s)
    Write-Output ("resolution            : {0}x{1}" -f $row.width, $row.height)
    Write-Output ("avg_fps               : {0}" -f $row.avg_fps)
    Write-Output ("video_bit_rate        : {0}" -f $(if ($row.video_bit_rate) { Format-Mbps $row.video_bit_rate } else { "n/a" }))
    Write-Output ("format_bit_rate       : {0}" -f $(if ($row.format_bit_rate) { Format-Mbps $row.format_bit_rate } else { "n/a" }))
    Write-Output ("has_audio             : {0}" -f $row.has_audio)
    Write-Output ("est_40h_storage       : {0}" -f $row.est_40h_GB_dec)
    Write-Output ("backend/profile       : {0} / {1}" -f $row.backend, $row.profile)
    Write-Output ("notes                 : {0}" -f $row.notes)
    Write-Output ""

    # Machine-readable line for pasting into a results sheet.
    $row | ConvertTo-Json -Compress
}
