---
name: youtube
description: Extract information from YouTube videos — transcripts, summaries, visual analysis, metadata. Use this skill whenever the user shares a YouTube link or asks to review, summarize, analyze, or extract information from a YouTube video. Also use when the user says "watch this" or "check this video".
---

# YouTube Video Review

Extract transcripts, metadata, and visual content from YouTube videos for summarization, analysis, and reporting.

## When to Use

- User shares a YouTube link (youtube.com, youtu.be)
- User asks to summarize, review, or analyze a video
- User wants transcript extraction
- User wants visual/slide analysis from a video

## Tools Available

All installed locally on Mac Mini:

- **youtube-transcript-api** — pull captions/subtitles directly (fastest method)
- **yt-dlp** — download video/audio, thumbnails, metadata
- **ffmpeg** — extract frames from video for visual analysis

## Workflow

### Step 1: Extract Video ID

Parse the YouTube URL to get the video ID:
- `youtube.com/watch?v=VIDEO_ID`
- `youtu.be/VIDEO_ID`
- `youtube.com/shorts/VIDEO_ID`

### Step 2: Get Metadata

```bash
yt-dlp --dump-json --no-download "VIDEO_URL" 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
print(f'Title: {d[\"title\"]}')
print(f'Channel: {d[\"channel\"]}')
print(f'Duration: {d[\"duration\"]}s ({d[\"duration\"]//60}m{d[\"duration\"]%60}s)')
print(f'Upload: {d.get(\"upload_date\", \"unknown\")}')
print(f'Views: {d.get(\"view_count\", \"unknown\")}')
print(f'Description: {d.get(\"description\", \"\")[:500]}')
"
```

### Step 3: Get Transcript

Try transcript API first (fast, no download needed):

```bash
python3 -c "
from youtube_transcript_api import YouTubeTranscriptApi
transcript = YouTubeTranscriptApi().fetch('VIDEO_ID')
for entry in transcript:
    mins = int(entry.start) // 60
    secs = int(entry.start) % 60
    print(f'[{mins}:{secs:02d}] {entry.text}')
"
```

The `fetch()` result yields entries with properties like `entry.start` and `entry.text`.

If transcript API fails (no captions available), fall back to audio extraction + Whisper:

```bash
# Download audio only
yt-dlp -x --audio-format mp3 -o "/tmp/yt-%(id)s.%(ext)s" "VIDEO_URL"

# Transcribe via Whisper server on DGX
curl -s http://192.168.22.3:8007/v1/audio/transcriptions \
  -F "file=@/tmp/yt-VIDEO_ID.mp3" \
  -F "model=whisper-large-v3-turbo"
```

Note: Whisper fallback works for videos up to ~30 min. For longer videos, split the audio first:
```bash
ffmpeg -i /tmp/yt-VIDEO_ID.mp3 -f segment -segment_time 600 -c copy /tmp/yt-VIDEO_ID-%03d.mp3
```

### Step 4: Visual Analysis (if needed)

For videos with slides, diagrams, code, or important visual content:

```bash
# Download video (low res is fine for frame extraction)
yt-dlp -f "worst[ext=mp4]" -o "/tmp/yt-%(id)s.%(ext)s" "VIDEO_URL"

# Create output directory for extracted frames
mkdir -p /tmp/yt-frames

# Extract frames every N seconds (adjust based on video length)
# For a 10min video, every 30s gives ~20 frames
ffmpeg -i /tmp/yt-VIDEO_ID.mp4 -vf "fps=1/30" /tmp/yt-frames/frame-%04d.jpg
```

Then read the extracted frames to analyze visual content.

### Step 5: Produce Output

Based on what the user asked for:

- **Quick summary** — 3-5 bullet points from transcript
- **Detailed summary** — section-by-section breakdown with timestamps
- **Key quotes** — notable statements with timestamps
- **Visual analysis** — describe slides/diagrams from extracted frames
- **Full report** — metadata + summary + key points + visual analysis
- **Comparison** — analyze multiple videos side by side

## Output Format

Always include at the top:
- Video title
- Channel name
- Duration
- Link

Then the requested analysis.

## Cleanup

After processing, clean up temp files:
```bash
rm -f /tmp/yt-VIDEO_ID.* /tmp/yt-frames/frame-*.jpg
rmdir /tmp/yt-frames 2>/dev/null || true
```

## Delegation Alternative

For heavy processing or when local tools are insufficient, delegate to Nicole on DGX:
```
mcp__agents__chat(agent="nicole", message="review this youtube video and give me a detailed summary: VIDEO_URL")
```

Nicole has the same toolset on DGX with GPU-accelerated Whisper for faster transcription.

## Troubleshooting

| Issue | Fix |
|-------|-----|
| No transcript available | Fall back to audio download + Whisper |
| Age-restricted video | May need cookies: `yt-dlp --cookies-from-browser chrome` |
| Private video | Cannot access — ask user for alternative |
| Very long video (>1hr) | Split audio into segments before Whisper |
| Rate limited | Wait and retry, or use different IP |
