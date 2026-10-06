# x-media-bot

Telegram bot that sends back the photos, videos and GIFs of X/Twitter links.
Post links are resolved through the [FxTwitter API](https://docs.fxtwitter.com/),
so no X login is needed.

## Usage

Send the bot a message with one or more `x.com` / `twitter.com` status links
(fixer domains like `fxtwitter.com` and `fixupx.com` work too). In groups it reacts to
links and ignores everything else.

- Photos and videos go out as one album (in chunks of 10), with the post text,
  the author and the link as the caption.
- X "GIFs" are silent mp4s. Each one is remuxed with
  `ffmpeg -map 0:v:0 -c:v copy -an -movflags +faststart` (no audio track, moov atom
  at the front, no re-encode) and sent as a Telegram animation. Animations can't go
  in albums, so each one is sent on its own.
- If a post has no media of its own, the quoted post's media is used.
- Videos use the highest bitrate mp4 that fits under the Bot API's 50 MB upload
  limit.

Commands: `/start`, `/help`.

## Configuration

| Env var | Required | Notes |
|---|---|---|
| `TELOXIDE_TOKEN` | yes | BotFather token. |
| `RUST_LOG` | no | Defaults to `info`. |

## Build & run

```bash
TELOXIDE_TOKEN=<token> cargo run      # needs ffmpeg on PATH
cargo test                            # the GIF test needs ffmpeg + ffprobe
docker build -t x-media-bot .
```

The image is `gcr.io/distroless/static-debian13:nonroot` with a static musl
build of the bot and a static ffmpeg in it. It runs with a read-only root
filesystem if `/tmp` is a writable mount.

## License

MIT
