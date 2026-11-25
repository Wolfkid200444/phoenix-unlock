// @description Blurs image and video previews in age-restricted channels
// @version 0.1.0
// @author wolfieeeeeeee

const { gateway, media } = phoenix;
const PREVIEW_PREFIX = "https://media.discordapp.net/attachments/0/0/phoenix-preview-";

const settings = phoenix.settings.define({
  allChannels: {
    type: "switch",
    title: "Blur in all channels",
    description: "Also blur image previews outside age-restricted channels",
    default: false,
  },
});

const BLUR_RADIUS = 64;
const CHANNEL_TTL_MS = 60_000;
const IMAGE_EXTENSION = /\.(?:gif|jpe?g|png|webp)$/i;
const VIDEO_EXTENSION = /\.(?:m4v|mkv|mov|mp4|webm)$/i;
const MESSAGE_PATH = /^\/channels\/(\d+)\/messages(?:\/\d+)?(?:\?|$)/;

type Channel = Pick<Phoenix.Channel, "id" | "nsfw" | "parent_id">;

const channels = new Map<string, { channel: Channel; expires: number }>();
const lookups = new Map<string, Promise<Channel | undefined>>();

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const rememberChannel = (value: unknown) => {
  if (!isRecord(value) || typeof value.id !== "string" || !/^\d+$/.test(value.id)) {
    return;
  }
  const channel: Channel = {
    id: value.id,
    nsfw: value.nsfw === true,
    parent_id: typeof value.parent_id === "string" && /^\d+$/.test(value.parent_id)
      ? value.parent_id
      : null,
  };
  channels.set(channel.id, { channel, expires: Date.now() + CHANNEL_TTL_MS });
  return channel;
};

const cachedChannel = (id: string) => {
  const entry = channels.get(id);
  if (entry && entry.expires > Date.now()) return entry.channel;
};

const loadChannel = (id: string): Promise<Channel | undefined> => {
  const cached = cachedChannel(id);
  if (cached) return Promise.resolve(cached);

  const existing = lookups.get(id);
  if (existing) return existing;

  const pending = phoenix.discord.get(`/channels/${id}`)
    .then((response) => {
      if (!response.ok) return;
      const value = response.json();
      if (isRecord(value) && value.id === id) {
        return cachedChannel(id) ?? rememberChannel(value);
      }
    })
    .catch(() => undefined)
    .finally(() => lookups.delete(id));

  lookups.set(id, pending);
  return pending;
};

const isAgeRestricted = async (id: string) => {
  const channel = await loadChannel(id);
  if (channel?.nsfw) return true;
  return channel?.parent_id
    ? (await loadChannel(channel.parent_id))?.nsfw === true
    : false;
}

type MessageMedia = Pick<Phoenix.Message,
  "attachments" | "embeds" | "referenced_message" | "message_snapshots"
> & Partial<Pick<Phoenix.Message, "components">>;

type GalleryEmbed = Phoenix.Embed & {
  images?: Phoenix.EmbedMedia[];
  components?: Phoenix.Component[];
};

const embedMedia = (embed: GalleryEmbed) => [embed.image, embed.thumbnail, ...(embed.images ?? [])];
const loggedEmbedShapes = new Set<string>();

const logEmbedShape = (embed: GalleryEmbed, rewritten: boolean) => {
  const known = ["image", "rich", "video", "gifv", "link", "article", "components"];
  const type = embed.type && known.includes(embed.type) ? embed.type : "other";
  const shape = `type=${type} image=${Boolean(embed.image)} thumbnail=${Boolean(embed.thumbnail)} video=${Boolean(embed.video)} gallery=${Boolean(embed.images?.length)} components=${Boolean(embed.components?.length)} rewritten=${rewritten}`;
  if (loggedEmbedShapes.has(shape)) return;
  loggedEmbedShapes.add(shape);
  phoenix.log.info(`nsfw-blur: embed ${shape}`);
};

const componentMedia = (components: Phoenix.Component[]): Phoenix.UnfurledMediaItem[] =>
  components.flatMap((component) => {
    switch (component.type) {
      case 9:
        return component.accessory.type === 11 ? [component.accessory.media] : [];
      case 11:
        return [component.media];
      case 12:
        return component.items.map((item) => item.media);
      case 17:
        return componentMedia(component.components);
      default:
        return [];
    }
  });

const blurPreview = (media: { url: string; proxy_url?: string; placeholder?: string }, radius: number, kind: "attachment" | "embed" | "video") => {
  const source = media.proxy_url || media.url;
  if (source.startsWith(PREVIEW_PREFIX)) return false;
  const placeholder = kind === "video" ? media.placeholder : undefined;
  const preview = phoenix.media?.blurredPreview(source, radius, kind, placeholder) ??
    (media.proxy_url && media.url !== source ? phoenix.media?.blurredPreview(media.url, radius, kind, placeholder) : undefined);
  if (!preview) return false;
  // The video player can use either URL directly, outside the image fetch hook.
  if (kind === "video") return false;
  media.proxy_url = preview;
  return true;
};

const blurEmbed = (embed: GalleryEmbed, radius: number) => {
  let changed = false;
  for (const image of embedMedia(embed)) {
    if (image && blurPreview(image, radius, "embed") && image.proxy_url) {
      image.url = image.proxy_url;
      changed = true;
    }
  }
  for (const item of componentMedia(embed.components ?? [])) {
    if (blurComponent(item, radius)) changed = true;
  }
  if (embed.video && blurPreview(embed.video, radius, "video")) changed = true;
  const poster = embedMedia(embed).find((image) => image?.url.startsWith(PREVIEW_PREFIX));
  if (!poster?.url.startsWith(PREVIEW_PREFIX)) return changed;

  if (embed.type === "image" && embed.url !== poster.url) {
    embed.url = poster.url;
    return true;
  }
  return changed;
};

const blurComponent = (item: Phoenix.UnfurledMediaItem, radius: number) => {
  if (/^video\//i.test(item.content_type ?? "") || VIDEO_EXTENSION.test(item.url.split("?", 1)[0])) {
    return blurPreview(item, radius, "video");
  }
  if (item.content_type && !/^image\//i.test(item.content_type)) return false;
  if (!blurPreview(item, radius, "embed") || !item.proxy_url) return false;
  item.url = item.proxy_url;
  return true;
};

const transformMessage = (message: MessageMedia | null | undefined, radius: number): boolean => {
  if (!message) return false;

  let changed = false;
  for (const attachment of message.attachments ?? []) {
    if (/^video\//i.test(attachment.content_type ?? "") || VIDEO_EXTENSION.test(attachment.filename)) {
      if (blurPreview(attachment, radius, "video")) changed = true;
      continue;
    }
    if (!/^image\//i.test(attachment.content_type ?? "") &&
        !IMAGE_EXTENSION.test(attachment.filename)) continue;
    if (blurPreview(attachment, radius, "attachment")) changed = true;
  }
  for (const embed of message.embeds ?? []) {
    const original = { ...embed };
    const rewritten = blurEmbed(embed, radius);
    logEmbedShape(original, rewritten);
    if (rewritten) changed = true;
  }
  for (const item of componentMedia(message.components ?? [])) {
    if (blurComponent(item, radius)) changed = true;
  }
  if (transformMessage(message.referenced_message, radius)) changed = true;
  for (const snapshot of message.message_snapshots ?? []) {
    if (transformMessage(snapshot.message, radius)) changed = true;
  }
  return changed;
};

const videoLinks = (message: Phoenix.Message | undefined) => {
  const attachments = (message?.attachments ?? []).filter((attachment) =>
    /^video\//i.test(attachment.content_type ?? "") || VIDEO_EXTENSION.test(attachment.filename),
  );
  const embeds: GalleryEmbed[] = message?.embeds ?? [];
  const components = componentMedia([
    ...(message?.components ?? []), ...embeds.flatMap((embed) => embed.components ?? []),
  ]).filter((item) => /^video\//i.test(item.content_type ?? ""));
  return [...attachments, ...embeds.flatMap((embed) => embed.video ? [embed.video] : []), ...components];
};

const previewLinks = (message: Phoenix.Message | undefined) => {
  const links = [
    ...(message?.attachments ?? []).map((attachment) => attachment.proxy_url),
    ...(message?.embeds ?? []).flatMap((value: GalleryEmbed) => [
      ...embedMedia(value), value.video, ...componentMedia(value.components ?? []),
    ].map((item) => item?.proxy_url)),
    ...componentMedia(message?.components ?? []).map((item) => item.proxy_url),
  ];
  return [...new Set(links.filter((url) => url?.startsWith(PREVIEW_PREFIX)))];
};

if (media) {
  phoenix.ui.addMessageAction({
    label: "Open unblurred media",
    visible: ({ message }) => previewLinks(message).length > 0 || videoLinks(message).length > 0,
    async onPress({ message }) {
      const previews = previewLinks(message);
      const videos = videoLinks(message);
      const options = [
        ...previews.map((_, index) => `Preview ${index + 1}`),
        ...videos.map((_, index) => videos.length === 1 ? "Original video" : `Original video ${index + 1}`),
      ];
      const index = options.length === 1 ? 0 : await phoenix.ui.choose({
        title: "Open unblurred media",
        options,
      });
      const video = videos[index - previews.length];
      const preview = video?.url || video?.proxy_url || previews[index];
      if (preview) media.revealPreview(preview);
      else if (video) phoenix.ui.toast("The original video URL is unavailable");
    },
  });
}

for (const event of [gateway.channelCreate, gateway.channelUpdate, gateway.threadCreate, gateway.threadUpdate]) {
  if (!event) continue;
  phoenix.on(event, (channel) => {
    rememberChannel(channel);
  });
}
if (gateway.guildCreate) {
  phoenix.on(gateway.guildCreate, (guild) => {
    for (const channel of guild.channels ?? []) rememberChannel(channel);
  });
}

for (const event of [gateway.messageCreate, gateway.messageUpdate]) {
  if (!event) continue;
  phoenix.hook(event, {
    before(message) {
      if (!phoenix.media) return;
      const id = message.channel_id;
      if (settings.allChannels) {
        transformMessage(message, BLUR_RADIUS);
        return;
      }
      if (typeof id !== "string" || !/^\d+$/.test(id)) return;
      const channel = channels.get(id)?.channel;
      const parentId = channel?.parent_id;
      // Gateway hooks are synchronous. Use the last known state while refreshing it.
      if (!cachedChannel(id) || (parentId && !cachedChannel(parentId))) {
        void isAgeRestricted(id);
      }
      if (channel?.nsfw || (parentId && channels.get(parentId)?.channel.nsfw)) {
        transformMessage(message, BLUR_RADIUS);
      }
    },
  });
}

const transformResponse = async (res: Phoenix.InterceptedResponse) => {
  const id = MESSAGE_PATH.exec(res.path)?.[1];
  if (!phoenix.media || !id || res.status !== 200) return;
  if (!settings.allChannels && !(await isAgeRestricted(id))) return;

  const data = res.json<MessageMedia[] | MessageMedia>();
  if (!data) return;

  let changed = false;
  for (const message of Array.isArray(data) ? data : [data]) {
    if (transformMessage(message, BLUR_RADIUS)) changed = true;
  }
  return changed ? { body: data } : undefined;
};

phoenix.discord.onResponse(
  { method: "GET", pathPrefix: "/channels/", pathIncludes: "/messages" },
  transformResponse,
);
