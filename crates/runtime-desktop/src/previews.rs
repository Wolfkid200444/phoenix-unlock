//! Local image previews requested through the desktop media fetch hook.

use std::{
   collections::{HashSet, VecDeque},
   hash::{BuildHasher, RandomState},
   io::{Cursor, Read, Write},
   net::{Ipv4Addr, TcpListener, TcpStream},
   sync::{Mutex, OnceLock, PoisonError},
   time::Duration,
};

use base64::{
   Engine,
   engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use image::{
   DynamicImage, ImageFormat, ImageReader, Limits, Rgba, RgbaImage, imageops::FilterType,
};
use rquickjs::{Ctx, Function, Object, function::Opt};

use phoenix_runtime::fetch;

const PREFIX: &str = "https://media.discordapp.net/attachments/0/0/phoenix-preview-";
const MAX_PREVIEWS: usize = 4096;
const THUMBNAIL_SIZE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
   Attachment,
   Embed,
   Video,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Preview {
   source: String,
   radius: u16,
   kind: Kind,
   placeholder: Option<String>,
}

struct Server {
   port: u16,
   key: RandomState,
   previews: Mutex<VecDeque<(u64, Preview)>>,
}

static SERVER: OnceLock<Option<Server>> = OnceLock::new();
static REGISTERED: OnceLock<()> = OnceLock::new();
static REQUESTS: Mutex<Option<HashSet<(Kind, u32, u32)>>> = Mutex::new(None);
static SERVED: OnceLock<()> = OnceLock::new();
static FETCH_FAILED: OnceLock<()> = OnceLock::new();
static DECODE_FAILED: OnceLock<()> = OnceLock::new();

pub(crate) fn api<'js>(ctx: &Ctx<'js>, phoenix: &Object<'js>) -> rquickjs::Result<()> {
   let media = Object::new(ctx.clone())?;
   media.set("blurredPreview", Function::new(ctx.clone(), register)?)?;
   media.set("revealPreview", Function::new(ctx.clone(), reveal)?)?;
   phoenix.set("media", media)
}

fn register(
   source: String,
   radius: f64,
   kind: String,
   Opt(placeholder): Opt<Option<String>>,
) -> Option<String> {
   let allowed = [
      "https://media.discordapp.net/",
      "https://cdn.discordapp.com/",
      "https://cdn.discordapp.net/",
      "https://media.discord.com/",
      "https://cdn.discord.com/",
      "https://images-ext-1.discordapp.net/",
      "https://images-ext-2.discordapp.net/",
   ];
   if source.len() > 4096
      || source.contains(char::is_control)
      || source.starts_with(PREFIX)
      || !allowed.iter().any(|host| source.starts_with(host))
      || !radius.is_finite()
      || !(1.0..=64.0).contains(&radius)
   {
      return None;
   }

   let server = SERVER.get_or_init(start).as_ref()?;
   let preview = Preview {
      source,
      radius: radius.round() as u16,
      placeholder: placeholder
         .flatten()
         .filter(|value| !value.is_empty() && value.len() <= 128),
      kind: match kind.as_str() {
         "attachment" => Kind::Attachment,
         "embed" => Kind::Embed,
         "video" => Kind::Video,
         _ => return None,
      },
   };
   let extension = if preview.kind == Kind::Video {
      "mp4"
   } else {
      "png"
   };
   let id = server.key.hash_one(&preview);
   let mut entries = server
      .previews
      .lock()
      .unwrap_or_else(PoisonError::into_inner);
   entries.retain(|(key, _)| *key != id);
   entries.push_back((id, preview));
   if entries.len() > MAX_PREVIEWS {
      entries.pop_front();
   }
   REGISTERED.get_or_init(|| log::info!("nsfw-blur: registered a preview"));
   Some(format!("{PREFIX}{id:016x}.{extension}"))
}

fn preview_id(path: &str) -> Option<u64> {
   let filename = path.split_once('?').map_or(path, |(base, _)| base);
   let (hex, extension) = filename.rsplit_once('.')?;
   if !matches!(extension, "png" | "mp4") {
      return None;
   }
   (hex.len() == 16)
      .then(|| u64::from_str_radix(hex, 16).ok())
      .flatten()
}

pub(crate) fn resolve_url(url: &str) -> Option<String> {
   let server = SERVER.get()?.as_ref()?;
   let marker = url.strip_prefix(PREFIX).and_then(preview_id);
   let (id, preview) = if let Some(id) = marker {
      (id, find_preview(id)?)
   } else {
      if !is_still_format(url) {
         return None;
      }
      let identity = video_identity(url)?;
      let entries = server
         .previews
         .lock()
         .unwrap_or_else(PoisonError::into_inner);
      let (id, preview) = entries.iter().rev().find(|(_, preview)| {
         preview.kind == Kind::Video && video_identity(&preview.source) == Some(identity)
      })?;
      (*id, preview.clone())
   };
   let size = request_size(url);
   log_request(preview.kind, size);
   // Chat requests fitted thumbnails. The media viewer requests the original size.
   let format = url
      .split_once('?')
      .and_then(|(_, query)| query.split('&').find_map(|arg| arg.strip_prefix("format=")));
   let still_format = matches!(format, Some("jpg" | "jpeg" | "png" | "webp"));
   let video_format = matches!(format, Some("mp4" | "webm" | "mov"));
   let original = if preview.kind == Kind::Video {
      video_format || (size == (0, 0) && !still_format)
   } else {
      size == (0, 0)
   };
   if original && marker.is_some() {
      return Some(preview.source);
   }
   Some(format!("http://127.0.0.1:{}/{id:016x}.png", server.port))
}

fn request_format(url: &str) -> Option<&str> {
   url.split_once('?')?
      .1
      .split('&')
      .find_map(|arg| arg.strip_prefix("format="))
}

fn is_still_format(url: &str) -> bool {
   matches!(request_format(url), Some("jpg" | "jpeg" | "png" | "webp"))
}

fn video_identity(url: &str) -> Option<(&str, Option<&str>)> {
   let (base, query) = url.split_once('?').unwrap_or((url, ""));
   let (host, path) = base.strip_prefix("https://")?.split_once('/')?;
   let signature = query.split('&').find_map(|arg| arg.strip_prefix("hm="));
   let host = match host {
      "media.discordapp.net"
      | "cdn.discordapp.com"
      | "cdn.discordapp.net"
      | "media.discord.com"
      | "cdn.discord.com" => "discord-media",
      "images-ext-1.discordapp.net" | "images-ext-2.discordapp.net" => "discord-external",
      _ => return None,
   };
   // External proxies encode the resource in the path. Attachment paths are signed.
   if path.starts_with("attachments/") || path.starts_with("external/") {
      return Some((path, signature.or(Some(host))));
   }
   None
}

fn request_size(url: &str) -> (u32, u32) {
   let query = url.split_once('?').map_or("", |(_, query)| query);
   let dimension = |name| {
      query
         .split('&')
         .filter_map(|arg| arg.split_once('='))
         .find_map(|(key, value)| {
            if key == name {
               value.parse::<u32>().ok()
            } else {
               None
            }
         })
         .unwrap_or(0)
   };
   (dimension("width"), dimension("height"))
}

fn log_request(kind: Kind, (width, height): (u32, u32)) {
   let mut requests = REQUESTS.lock().unwrap_or_else(PoisonError::into_inner);
   let requests = requests.get_or_insert_with(HashSet::new);
   if requests.len() < 32 && requests.insert((kind, width, height)) {
      log::info!("nsfw-blur: desktop requested {kind:?} preview width={width} height={height}");
   }
}

fn start() -> Option<Server> {
   let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
   let port = listener.local_addr().ok()?.port();
   for _ in 0..4 {
      let worker = listener.try_clone().ok()?;
      std::thread::Builder::new()
         .name("sunbird-preview".to_owned())
         .spawn(move || {
            for stream in worker.incoming().map_while(Result::ok) {
               serve(stream);
            }
         })
         .ok()?;
   }
   Some(Server {
      port,
      key: RandomState::new(),
      previews: Mutex::new(VecDeque::new()),
   })
}

fn reveal(url: String) {
   if let Some(marker) = url.strip_prefix(PREFIX) {
      if let Some(preview) = preview_id(marker).and_then(find_preview) {
         let _ = phoenix_ui_desktop::open_url(&preview.source);
      }
   } else if url.starts_with("https://") && url.len() <= 4096 && !url.contains(char::is_control) {
      let _ = phoenix_ui_desktop::open_url(&url);
   }
}

fn request_preview(stream: &mut TcpStream) -> Option<Preview> {
   let mut request = [0; 1024];
   let len = stream.read(&mut request).ok()?;
   let request = std::str::from_utf8(&request[..len]).ok()?;
   let (line, _) = request.split_once("\r\n")?;
   let mut parts = line.split_ascii_whitespace();
   if parts.next()? != "GET" {
      return None;
   }
   find_preview(preview_id(parts.next()?.strip_prefix('/')?)?)
}

fn find_preview(id: u64) -> Option<Preview> {
   let server = SERVER.get()?.as_ref()?;
   let entries = server
      .previews
      .lock()
      .unwrap_or_else(PoisonError::into_inner);
   entries
      .iter()
      .find(|(key, _)| *key == id)
      .map(|(_, preview)| preview.clone())
}

fn video_poster_url(source: &str) -> String {
   let (base, query) = source.split_once('?').unwrap_or((source, ""));
   let base = base
      .replacen(
         "https://cdn.discordapp.com/",
         "https://media.discordapp.net/",
         1,
      )
      .replacen(
         "https://cdn.discordapp.net/",
         "https://media.discordapp.net/",
         1,
      )
      .replacen("https://cdn.discord.com/", "https://media.discord.com/", 1);
   let mut query = query
      .split('&')
      .filter(|part| {
         !part.is_empty() && !matches!(part.split('=').next(), Some("format" | "width" | "height"))
      })
      .collect::<Vec<_>>();
   query.extend(["format=jpg", "width=1024", "height=1024"]);
   format!("{base}?{}", query.join("&"))
}

fn serve(mut stream: TcpStream) {
   let timeout = Some(Duration::from_secs(2));
   let _ = stream.set_read_timeout(timeout);
   let _ = stream.set_write_timeout(timeout);
   let body = request_preview(&mut stream).and_then(|preview| {
      let poster;
      let source = if preview.kind == Kind::Video {
         poster = video_poster_url(&preview.source);
         poster.as_str()
      } else {
         &preview.source
      };
      let image = match fetch::get(source) {
         Ok(bytes) => blur(&bytes, f32::from(preview.radius)),
         Err(_) => {
            FETCH_FAILED.get_or_init(|| log::warn!("nsfw-blur: preview source fetch failed"));
            None
         },
      };
      if image.is_some() {
         return image;
      }
      if preview.kind == Kind::Video {
         return video_placeholder(preview.placeholder.as_deref(), f32::from(preview.radius));
      }
      DECODE_FAILED.get_or_init(|| log::warn!("nsfw-blur: preview source unavailable"));
      None
   });
   if let Some(bytes) = body {
      let result = write!(stream,
         "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
         bytes.len(),
      ).and_then(|()| stream.write_all(&bytes));
      if result.is_ok() {
         SERVED.get_or_init(|| log::info!("nsfw-blur: blurred image delivered"));
      }
   } else {
      let _ = stream
         .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
   }
}

fn video_placeholder(hash: Option<&str>, radius: f32) -> Option<Vec<u8>> {
   let image = hash
      .and_then(|hash| {
         [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD]
            .into_iter()
            .find_map(|codec| codec.decode(hash).ok())
      })
      .and_then(|hash| thumbhash::thumb_hash_to_rgba(&hash).ok())
      .and_then(|(width, height, pixels)| {
         RgbaImage::from_raw(
            u32::try_from(width).ok()?,
            u32::try_from(height).ok()?,
            pixels,
         )
      });
   let preview = match image {
      Some(image) => DynamicImage::ImageRgba8(image)
         .resize(320, 320, FilterType::Triangle)
         .blur(radius),
      None => DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 180, Rgba([38, 40, 46, 255]))),
   };
   encode_png(&preview)
}

fn encode_png(image: &DynamicImage) -> Option<Vec<u8>> {
   let mut result = Cursor::new(Vec::new());
   image.write_to(&mut result, ImageFormat::Png).ok()?;
   Some(result.into_inner())
}

fn blur(bytes: &[u8], radius: f32) -> Option<Vec<u8>> {
   let mut reader = ImageReader::new(Cursor::new(bytes))
      .with_guessed_format()
      .ok()?;
   let mut limits = Limits::default();
   limits.max_image_width = Some(8192);
   limits.max_image_height = Some(8192);
   limits.max_alloc = Some(128 << 20);
   reader.limits(limits);
   let image = reader.decode().ok()?;
   let preview = if image.width().max(image.height()) > THUMBNAIL_SIZE {
      image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE)
   } else {
      image
   };
   encode_png(&preview.blur(radius))
}

#[cfg(test)]
mod tests {
   use super::*;
   use image::{Rgb, RgbImage};

   #[test]
   fn separates_full_size_viewer_from_chat_requests() {
      let original = "https://cdn.discordapp.com/example.png";
      let attachment = register(
         original.to_owned(),
         10.0,
         "attachment".to_owned(),
         Opt(None),
      )
      .unwrap();
      let embed = register(original.to_owned(), 10.0, "embed".to_owned(), Opt(None)).unwrap();
      assert_eq!(resolve_url(&attachment).as_deref(), Some(original));
      assert!(
         resolve_url(&format!("{attachment}?width=512&height=455"))
            .unwrap()
            .starts_with("http://127.0.0.1:")
      );
      assert_eq!(resolve_url(&embed).as_deref(), Some(original));
      assert!(
         resolve_url(&format!("{embed}?width=320&height=320"))
            .unwrap()
            .starts_with("http://127.0.0.1:")
      );
   }

   #[test]
   fn video_poster_keeps_the_attachment_signature() {
      let source = "https://cdn.discordapp.com/attachments/1/2/video.mp4?ex=1&is=2&hm=3&format=webm&width=200";
      assert_eq!(
         video_poster_url(source),
         "https://media.discordapp.net/attachments/1/2/video.mp4?ex=1&is=2&hm=3&format=jpg&width=1024&height=1024",
      );
      let preview = register(source.to_owned(), 10.0, "video".to_owned(), Opt(None)).unwrap();
      assert_eq!(resolve_url(&preview).as_deref(), Some(source));
      assert_eq!(
         resolve_url(&format!("{preview}?width=768&format=mp4")).as_deref(),
         Some(source)
      );
      assert!(
         resolve_url(&format!("{preview}?format=jpg"))
            .unwrap()
            .starts_with("http://127.0.0.1:")
      );
   }

   #[test]
   fn javascript_can_pass_an_undefined_thumbnail() {
      let runtime = rquickjs::Runtime::new().unwrap();
      let context = rquickjs::Context::full(&runtime).unwrap();
      context.with(|ctx| {
         let phoenix = Object::new(ctx.clone()).unwrap();
         api(&ctx, &phoenix).unwrap();
         ctx.globals().set("phoenix", phoenix).unwrap();
         let result = ctx.eval::<String, _>(
            r#"phoenix.media.blurredPreview("https://cdn.discordapp.com/example.png", 10, "attachment", undefined)"#,
         ).unwrap();
         assert!(result.starts_with(PREFIX));
      });
   }

   #[test]
   fn native_video_urls_only_redirect_still_requests() {
      let video = "https://cdn.discordapp.com/attachments/1/2/clip.mp4?ex=1&is=2&hm=3";
      register(video.to_owned(), 10.0, "video".to_owned(), Opt(None)).unwrap();
      assert_eq!(resolve_url(video), None);
      let playback = format!("{video}&width=768&format=mp4");
      assert_eq!(resolve_url(&playback), None);
      let poster = "https://media.discordapp.net/attachments/1/2/clip.mp4?ex=1&is=2&hm=3&width=512&format=webp";
      assert!(
         resolve_url(poster)
            .unwrap()
            .starts_with("http://127.0.0.1:")
      );
      let unrelated = poster.replace("hm=3", "hm=4");
      assert_eq!(resolve_url(&unrelated), None);
   }

   #[test]
   fn video_placeholder_is_a_png_even_without_a_thumbnail() {
      let tile = video_placeholder(None, 10.0).unwrap();
      assert_eq!(image::load_from_memory(&tile).unwrap().width(), 320);

      let pixels = RgbaImage::from_fn(32, 16, |x, _| Rgba([if x < 16 { 0 } else { 255 }; 4]));
      let hash = STANDARD.encode(thumbhash::rgba_to_thumb_hash(32, 16, pixels.as_raw()));
      let decoded =
         image::load_from_memory(&video_placeholder(Some(&hash), 10.0).unwrap()).unwrap();
      assert_eq!(decoded.width(), 320);
      assert_ne!(
         decoded.to_rgba8().get_pixel(0, 0),
         decoded.to_rgba8().get_pixel(319, 0)
      );
   }

   #[test]
   fn radius_changes_preview_pixels() {
      let input = RgbImage::from_fn(64, 64, |x, _| Rgb([if x < 32 { 0 } else { 255 }; 3]));
      let mut encoded = Cursor::new(Vec::new());
      input.write_to(&mut encoded, ImageFormat::Png).unwrap();
      let light = image::load_from_memory(&blur(encoded.get_ref(), 2.0).unwrap())
         .unwrap()
         .to_rgb8();
      let heavy = image::load_from_memory(&blur(encoded.get_ref(), 10.0).unwrap())
         .unwrap()
         .to_rgb8();
      assert_eq!(light.dimensions(), (64, 64));
      assert!(heavy.get_pixel(25, 32).0[0] > light.get_pixel(25, 32).0[0]);
   }
}
