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

## Metadata

Every file is tagged with title, artist, genre, date and cover from SoundCloud, plus
`publisher_metadata` when the uploader provides it (real artist, album, ISRC, composer, ℗ line, label).

With `metadata_lookup = true` (default), suprpull also asks [MusicBrainz](https://musicbrainz.org):
by ISRC first, otherwise by artist and title (promo noise like `[FREE DL]` is stripped, and
`Artist - Title` uploads are split). A match is only applied when it is certain: same title and
artist, search score ≥ 90 and at most 3 s length difference. It then adds album, album artist,
track number, original release date, MusicBrainz ids and the cover from the Cover Art Archive.
Requests are limited to one per second, as MusicBrainz asks. When a set is downloaded, the set
stays the album and keeps its cover, so all files belong together.

| Tag | m4a | opus | mp3 |
|---|---|---|---|
| title, artist, album, album artist, track, date, genre, composer, copyright | ✔ | ✔ | ✔ |
| cover | ✔ | – | ✔ |
| label, ISRC, MusicBrainz ids | – | ✔ | ✔ |

## Requirements

- Rust 1.89 or newer
- `ffmpeg` in `$PATH` (or set `ffmpeg_path` in the config)

## Install

Builds from source and installs to `~/.local/bin` (no root needed):

```sh
curl -fsSL https://raw.githubusercontent.com/alpwrk/suprpull/main/install.sh | sh
```

From a checkout, `./install.sh` builds that checkout instead. To remove the binary
(config and history are kept):

```sh
curl -fsSL https://raw.githubusercontent.com/alpwrk/suprpull/main/install.sh | sh -s -- --uninstall
```

## Usage

Run `suprpull` (or `cargo run --release` from the checkout).

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
| `q` (empty input or list) | quit |
| `Esc` | clear input / close popup |

## Files

| File | Path |
|---|---|
| Config | `~/.config/suprpull/config.toml` (created on first start) |
| History | `~/.local/share/suprpull/history.jsonl` |
| client_id cache | `~/.cache/suprpull/client_id.json` |
| Downloads | `~/Music/suprpull/` by default |

Config keys: `output_dir`, `default_format` (`best`, `aac`, `opus`, `mp3`, `convert_mp3`),
`concurrency` (1–8), `ffmpeg_path`, `metadata_lookup` (true/false), `musicbrainz_contact`
(optional URL or e-mail for the MusicBrainz User-Agent).

## Disclaimer

This uses SoundCloud's unofficial web API, which is against SoundCloud's terms of use.
Use it only for personal use and for content you are allowed to download.
