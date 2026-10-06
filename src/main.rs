//! x-media-bot — send it an X/Twitter link, it sends back the post's photos,
//! videos and GIFs (resolved through the FxTwitter API).
//!
//! X "GIFs" are silent mp4s; they are remuxed with ffmpeg (`-an`, `+faststart`)
//! so Telegram plays them as animations.

mod fx;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use teloxide::payloads::{
    SendAnimationSetters, SendMediaGroupSetters, SendMessageSetters, SendPhotoSetters,
    SendVideoSetters,
};
use teloxide::prelude::*;
use teloxide::types::{
    InputFile, InputMedia, InputMediaPhoto, InputMediaVideo, LinkPreviewOptions, MessageEntityKind,
    ParseMode, ReplyParameters,
};
use teloxide::utils::command::BotCommands;
use tokio::io::AsyncWriteExt;

/// Upload cap of the public Bot API.
const PUBLIC_UPLOAD_CAP: u64 = 50 * 1000 * 1000;
/// Upload cap of a self-hosted Bot API server (`TELOXIDE_API_URL`).
const LOCAL_UPLOAD_CAP: u64 = 2000 * 1000 * 1000;
/// sendPhoto rejects photos over 10 MB.
const PHOTO_CAP: u64 = 10 * 1000 * 1000;
const CAPTION_MAX: usize = 1024;
const SOURCE: &str = "https://github.com/Nachtalb/x-media-bot";
const AVATAR: &[u8] = include_bytes!("../assets/avatar.jpg");
const NAME: &str = "X Media";
const SHORT_ABOUT: &str = "Sends the photos, videos and GIFs of X/Twitter links.";
const ABOUT: &str = "Send me an X/Twitter link and I'll send you its photos, videos and GIFs. \
                     Works in groups too.";

#[derive(Clone, Copy)]
struct UploadCap(u64);

#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "what this bot does.")]
    Start,
    #[command(description = "show this help text.")]
    Help,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    anyhow::ensure!(
        std::env::var_os("TELOXIDE_TOKEN").is_some(),
        "TELOXIDE_TOKEN must be set"
    );
    // Large uploads outlast teloxide's default 17 s request timeout.
    let client = teloxide::net::default_reqwest_settings()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let bot = Bot::from_env_with_client(client);
    let cap = UploadCap(if std::env::var_os("TELOXIDE_API_URL").is_some() {
        LOCAL_UPLOAD_CAP
    } else {
        PUBLIC_UPLOAD_CAP
    });
    tracing::info!(api = %bot.api_url(), cap_mb = cap.0 / 1_000_000, "bot api");
    let http = reqwest::Client::builder()
        .user_agent(concat!("x-media-bot/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    if let Err(err) = publish_bot_metadata(&bot).await {
        tracing::warn!(?err, "failed to publish bot metadata");
    }
    if let Err(err) = set_profile_photo(&bot, &http).await {
        tracing::warn!(?err, "failed to set profile photo");
    }

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(handle_command),
        )
        .branch(dptree::endpoint(handle_message));

    tracing::info!("starting long-polling dispatcher");
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![http, cap])
        .default_handler(|_| async {})
        .error_handler(LoggingErrorHandler::with_custom_text(
            "update handler error",
        ))
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    Ok(())
}

async fn publish_bot_metadata(bot: &Bot) -> Result<()> {
    use teloxide::payloads::{SetMyDescriptionSetters, SetMyShortDescriptionSetters};
    // The setters are rate-limited (setMyName heavily): only call them on a mismatch.
    let commands = Command::bot_commands();
    if bot.get_my_commands().await? != commands {
        bot.set_my_commands(commands).await?;
    }
    if bot.get_my_short_description().await?.short_description != SHORT_ABOUT {
        bot.set_my_short_description()
            .short_description(SHORT_ABOUT)
            .await?;
    }
    if bot.get_my_description().await?.description != ABOUT {
        bot.set_my_description().description(ABOUT).await?;
    }
    if bot.get_me().await?.first_name != NAME {
        bot.set_my_name().name(NAME).await?;
    }
    Ok(())
}

/// setMyProfilePhoto isn't in teloxide-core 0.13 yet, so it's a raw multipart call.
async fn set_profile_photo(bot: &Bot, http: &reqwest::Client) -> Result<()> {
    // Not Url::join: the token's "123:" would parse as a URL scheme.
    let base = bot.api_url();
    let url = format!(
        "{}/bot{}/setMyProfilePhoto",
        base.as_str().trim_end_matches('/'),
        bot.token()
    );
    let form = reqwest::multipart::Form::new()
        .text("photo", r#"{"type":"static","photo":"attach://avatar"}"#)
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(AVATAR)
                .file_name("avatar.jpg")
                .mime_str("image/jpeg")?,
        );
    let resp: serde_json::Value = http.post(url).multipart(form).send().await?.json().await?;
    anyhow::ensure!(resp["ok"] == true, "setMyProfilePhoto: {resp}");
    Ok(())
}

async fn handle_command(bot: Bot, msg: Message, cmd: Command) -> Result<()> {
    let text = match cmd {
        Command::Start => format!("{ABOUT}\n\nSource: <a href=\"{SOURCE}\">GitHub</a>"),
        Command::Help => Command::descriptions().to_string(),
    };
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}

/// All X status ids linked in the message (text or caption, plain or hidden links).
fn linked_ids(msg: &Message) -> Vec<u64> {
    let entities = msg
        .parse_entities()
        .or_else(|| msg.parse_caption_entities());
    let mut ids = Vec::new();
    for e in entities.unwrap_or_default() {
        let id = match e.kind() {
            MessageEntityKind::Url => fx::status_id(e.text()),
            MessageEntityKind::TextLink { url } => fx::status_id(url.as_str()),
            _ => None,
        };
        if let Some(id) = id.filter(|id| !ids.contains(id)) {
            ids.push(id);
        }
    }
    ids
}

async fn handle_message(
    bot: Bot,
    msg: Message,
    http: reqwest::Client,
    cap: UploadCap,
) -> Result<()> {
    let ids = linked_ids(&msg);
    if ids.is_empty() {
        if msg.chat.is_private() {
            bot.send_message(msg.chat.id, ABOUT)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        }
        return Ok(());
    }
    for id in ids {
        if let Err(err) = send_post(&bot, &msg, &http, cap.0, id).await {
            tracing::error!(id, ?err, "failed to send post");
            let _ = bot
                .send_message(msg.chat.id, format!("Couldn't fetch that post: {err:#}"))
                .reply_parameters(ReplyParameters::new(msg.id))
                .await;
        }
    }
    Ok(())
}

async fn send_post(
    bot: &Bot,
    msg: &Message,
    http: &reqwest::Client,
    cap: u64,
    id: u64,
) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("x-media-{id}-{}", msg.id.0));
    tokio::fs::create_dir_all(&dir).await?;
    let res = send_post_in(bot, msg, http, cap, id, &dir).await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    res
}

enum Ready {
    Photo(PathBuf),
    Video(PathBuf, fx::Item),
    Gif(PathBuf, fx::Item),
}

async fn send_post_in(
    bot: &Bot,
    msg: &Message,
    http: &reqwest::Client,
    cap: u64,
    id: u64,
    dir: &Path,
) -> Result<()> {
    let Some(tweet) = fx::fetch(http, id).await? else {
        anyhow::bail!("post not found (deleted or private?)");
    };
    let items = tweet.media();
    if items.is_empty() {
        if msg.chat.is_private() {
            bot.send_message(msg.chat.id, "That post has no photos, videos or GIFs.")
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        }
        return Ok(());
    }

    let mut ready = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let path = dir.join(format!("{i}.bin"));
        match item.kind.as_str() {
            "photo" => {
                download(http, std::slice::from_ref(&item.url), PHOTO_CAP, &path).await?;
                ready.push(Ready::Photo(path));
            }
            "gif" => {
                download(http, &item.mp4_urls(), cap, &path).await?;
                let out = dir.join(format!("{i}.mp4"));
                remux_gif(&path, &out).await?;
                ready.push(Ready::Gif(out, item));
            }
            _ => {
                download(http, &item.mp4_urls(), cap, &path).await?;
                ready.push(Ready::Video(path, item));
            }
        }
    }

    let caption = caption(&tweet);
    let reply = ReplyParameters::new(msg.id);
    let chat = msg.chat.id;

    // GIFs can't go into albums: send each as its own animation.
    let (gifs, album): (Vec<_>, Vec<_>) =
        ready.into_iter().partition(|r| matches!(r, Ready::Gif(..)));
    let mut caption = Some(caption);
    for g in gifs {
        let Ready::Gif(path, item) = g else {
            unreachable!()
        };
        let mut req = bot
            .send_animation(chat, InputFile::file(path).file_name("animation.mp4"))
            .width(item.width.into())
            .height(item.height.into())
            .reply_parameters(reply.clone());
        if let Some(c) = caption.take() {
            req = req.caption(c);
        }
        req.await?;
    }

    if let [only] = album.as_slice() {
        let c = caption.take().unwrap_or_default();
        match only {
            Ready::Photo(p) => {
                bot.send_photo(chat, InputFile::file(p))
                    .caption(c)
                    .reply_parameters(reply)
                    .await?;
            }
            Ready::Video(p, item) => {
                bot.send_video(chat, InputFile::file(p).file_name("video.mp4"))
                    .caption(c)
                    .width(item.width.into())
                    .height(item.height.into())
                    .duration(item.secs().into())
                    .supports_streaming(true)
                    .reply_parameters(reply)
                    .await?;
            }
            Ready::Gif(..) => unreachable!(),
        }
        return Ok(());
    }

    for chunk in album.chunks(10) {
        let media = chunk
            .iter()
            .map(|r| {
                let c = caption.take();
                match r {
                    Ready::Photo(p) => {
                        let mut m = InputMediaPhoto::new(InputFile::file(p));
                        m.caption = c;
                        InputMedia::Photo(m)
                    }
                    Ready::Video(p, item) => {
                        let mut m = InputMediaVideo::new(InputFile::file(p).file_name("video.mp4"));
                        m.caption = c;
                        m.width = Some(item.width);
                        m.height = Some(item.height);
                        m.duration = Some(item.secs());
                        m.supports_streaming = Some(true);
                        InputMedia::Video(m)
                    }
                    Ready::Gif(..) => unreachable!(),
                }
            })
            .collect::<Vec<_>>();
        bot.send_media_group(chat, media)
            .reply_parameters(reply.clone())
            .await?;
    }
    Ok(())
}

fn caption(t: &fx::Tweet) -> String {
    let footer = format!(
        "\n\n{} (@{})\n{}",
        t.author.name, t.author.screen_name, t.url
    );
    let room = CAPTION_MAX.saturating_sub(footer.chars().count());
    let text = t.text.trim();
    let text = if text.chars().count() > room {
        let cut: String = text.chars().take(room.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        text.to_string()
    };
    format!("{text}{footer}").trim().to_string()
}

/// Download the first url that fits under `cap` bytes; bigger ones are skipped.
async fn download(http: &reqwest::Client, urls: &[String], cap: u64, path: &Path) -> Result<()> {
    'urls: for url in urls {
        let mut resp = http.get(url).send().await?.error_for_status()?;
        if resp.content_length().is_some_and(|n| n > cap) {
            continue;
        }
        let mut file = tokio::fs::File::create(path).await?;
        let mut size = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            size += chunk.len() as u64;
            if size > cap {
                continue 'urls;
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        return Ok(());
    }
    anyhow::bail!(
        "media is larger than Telegram's {} MB limit",
        cap / 1_000_000
    )
}

/// X GIFs are mp4s: drop any audio and move the moov atom to the front.
async fn remux_gif(input: &Path, output: &Path) -> Result<()> {
    let out = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-an",
            "-movflags",
            "+faststart",
        ])
        .arg(output)
        .output()
        .await
        .context("spawn ffmpeg")?;
    if !out.status.success() {
        anyhow::bail!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tweet(text: &str) -> fx::Tweet {
        serde_json::from_value(serde_json::json!({
            "url": "https://x.com/a/status/1", "text": text,
            "author": {"name": "A", "screen_name": "a"}
        }))
        .unwrap()
    }

    #[test]
    fn caption_fits_telegram_limit() {
        assert_eq!(
            caption(&tweet("hi")),
            "hi\n\nA (@a)\nhttps://x.com/a/status/1"
        );
        assert_eq!(caption(&tweet("")), "A (@a)\nhttps://x.com/a/status/1");
        let long = caption(&tweet(&"é".repeat(5000)));
        assert_eq!(long.chars().count(), CAPTION_MAX);
        assert!(long.ends_with("https://x.com/a/status/1"));
    }

    /// Synthesise an mp4 with audio and moov at the end, remux it, check the result.
    #[tokio::test]
    async fn gif_remux_drops_audio_and_faststarts() {
        let dir = std::env::temp_dir().join(format!("x-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, out) = (dir.join("in.mp4"), dir.join("out.mp4"));
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=1:s=64x64",
            ])
            .args([
                "-f", "lavfi", "-i", "sine=d=1", "-c:v", "libx264", "-c:a", "aac",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(ok.success());
        assert!(
            atoms(&src).iter().position(|a| a == "mdat")
                < atoms(&src).iter().position(|a| a == "moov")
        );

        remux_gif(&src, &out).await.unwrap();
        let a = atoms(&out);
        assert!(
            a.iter().position(|a| a == "moov") < a.iter().position(|a| a == "mdat"),
            "{a:?}"
        );
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "csv=p=0",
            ])
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "video");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn atoms(p: &Path) -> Vec<String> {
        let d = std::fs::read(p).unwrap();
        let (mut out, mut i) = (Vec::new(), 0usize);
        while i + 8 <= d.len() {
            let size = u32::from_be_bytes(d[i..i + 4].try_into().unwrap()) as usize;
            out.push(String::from_utf8_lossy(&d[i + 4..i + 8]).into_owned());
            if size < 8 {
                break;
            }
            i += size;
        }
        out
    }
}
