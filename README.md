# suprpull

A terminal UI for downloading SoundCloud tracks and sets, inspired by how
[cobalt](https://github.com/imputnet/cobalt) talks to SoundCloud.

## How it works

1. Scrapes the public web `client_id` from soundcloud.com's JS bundles and caches it
   (refreshed when `__sc_version` changes or the API answers 401/403).
2. Resolves the link via `api-v2.soundcloud.com/resolve` (short links, private links and sets work).
   Set tracks that arrive as stubs are loaded via `/tracks?ids=`.
3. Skips tracks that are region-blocked or only available as a Go+ preview.
4. Picks a transcoding (AAC, Opus or MP3), requests the signed stream URL and lets
   ffmpeg fetch it (HLS or progressive), tagging title/artist/album/track number and cover art.

## Requirements

- Rust (edition 2024)
- `ffmpeg` in `$PATH` (or set `ffmpeg_path` in the config)

## Usage

```sh
cargo run --release
```

Paste a link and press Enter, then choose a format in the popup.

| Key | Action |
|---|---|
| `Enter` | resolve link / confirm format |
| `Tab` | switch focus between input and list |
| `1` / `2` (list) or `F1` / `F2` | queue / history tab |
| `↑` `↓` `j` `k` | move selection |
| `d` | remove job |
| `r` | retry failed job |
| `o` | open folder |
| `q` / `Ctrl-C` | quit |

## Files

| File | Path |
|---|---|
| Config | `~/.config/suprpull/config.toml` (created on first start) |
| History | `~/.local/share/suprpull/history.jsonl` |
| client_id cache | `~/.cache/suprpull/client_id.json` |
| Downloads | `~/Music/suprpull/` by default |

Config keys: `output_dir`, `default_format` (`best`, `aac`, `opus`, `mp3`, `convert_mp3`),
`concurrency` (1–8), `ffmpeg_path`.

## Disclaimer

This uses SoundCloud's unofficial web API, which is against SoundCloud's terms of use.
Use it only for personal use and for content you are allowed to download.
