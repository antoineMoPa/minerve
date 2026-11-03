# Assets

## Boot clip (`boot.txt`)

The ASCII video shown when `minerve` starts. It plays inside the TUI, over the normal
layout, and is embedded in the binary with `include_str!` (see `src/boot.rs`).

### Format

- 61 frames, 120x50 characters each, separated by a line that contains only a form feed (`\f`).
- The converter samples the source at 12 fps (61 frames for the 5 second clip). The runtime
  plays them at 24 fps, so the clip runs in about 2.5 seconds.
- Blank characters are the transparent background: the TUI draws its dot pattern behind them.

### Regenerating

Requires `ffmpeg` and `python3`. The source is a 748x1244 clip named `video.mp4`.

```sh
scripts/make_boot_ascii.sh [video] [threshold]
```

- `video`: defaults to `video.mp4` in the current directory.
- `threshold`: grayscale value (0-255). Pixels at or below it become blank. Defaults to `110`.

The script crops a 499x415 region at offset (93, 52) around the statue's face, scales it to
120x50 with area sampling, and writes `assets/boot.txt` directly.

To change the framing, edit the `CROP` line in the script. Then rebuild the binary so
`include_str!` picks up the new frames.

### Runtime behavior

- Set `MINERVE_NO_BOOT=1` to skip the clip.
- Any key press during playback removes the boot screen at once.
- When the clip ends, the dot pattern erodes over 900 ms while two dashes sweep in from the
  sides and erase the caption "loading ancient knowledge". The sweep starts at the clip's midpoint.
- Playback speed (`FPS`), erosion time (`ERODE`), and blend width (`EDGE_BLEND` in `src/tui.rs`)
  are constants in the source.

### Annex: `scripts/make_boot_ascii.sh`

```sh
#!/usr/bin/env bash
# Regenerates assets/boot.txt from a video.
#   usage: make_boot_ascii.sh [video] [threshold]
# video     default video.mp4 (in the current directory)
# threshold grayscale 0-255; pixels <= threshold become spaces (transparent
#           background, nothing is drawn). Default 110.
# Frames are separated by a line containing only a form feed ("\f").
# A region around the statue's face is kept (crop), scaled to 120x50 chars.
# Each frame is exactly COLS x ROWS chars (no trailing trim, so the runtime
# can center it). The crop keeps the picture's aspect ratio in terminal cells.
set -euo pipefail
VIDEO="${1:-video.mp4}"
THRESHOLD="${2:-110}"
OUT="$(dirname "$0")/../assets/boot.txt"
COLS=120
ROWS=50
FPS=12
# Crop around the statue's face (about 1.5x zoom on the top half of the frame), then scale to COLS x ROWS.
# crop=w:h:x:y in source pixels (source is 748x1244). Face is near x=340, y=320.
CROP="crop=499:415:93:52"
ffmpeg -loglevel error -i "$VIDEO" \
  -vf "$CROP,fps=$FPS,scale=$COLS:$ROWS:flags=area" -pix_fmt gray -f rawvideo - \
| python3 -c '
import sys
RAMP = ".:-=+*#%@"  # bright pixels -> dense chars (for dark terminals)
COLS, ROWS, THRESH = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
size = COLS * ROWS
data = sys.stdin.buffer.read()
span = 255 - THRESH
def glyph(v):
    if v <= THRESH:
        return " "
    return RAMP[min(len(RAMP) - 1, (v - THRESH) * len(RAMP) // (span + 1))]
out = []
for off in range(0, len(data) - size + 1, size):
    px = data[off:off + size]
    lines = []
    for r in range(ROWS):
        lines.append("".join(glyph(v) for v in px[r * COLS:(r + 1) * COLS]))
    out.append("\n".join(lines))
sys.stdout.write("\n\f\n".join(out) + "\n")
' "$COLS" "$ROWS" "$THRESHOLD" > "$OUT"
echo "wrote $OUT ($(grep -c $'\f' "$OUT") frame separators, threshold $THRESHOLD)"
```
