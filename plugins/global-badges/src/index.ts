// @name Global Badges
// @description Show Equibadges in the native profile badge row and Badge Directory
// @version 2.0.0
// @author Wolfie

const ttl = 30 * 60 * 1000;
const rarities = new Map<number, Phoenix.ProfileBadge["rarity"]>([
  [1, "Common"], [2, "Rare"], [3, "Epic"], [5, "Mythic"],
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parse(groups: Record<string, unknown>) {
  const badges: Phoenix.ProfileBadge[] = [];
  const seen = new Set<string>();
  for (const [provider, items] of Object.entries(groups)) {
    if (!Array.isArray(items) || /^(discord|valenced)$/i.test(provider)) continue;
    for (const item of items) {
      if (!isRecord(item) || typeof item.tooltip !== "string" || typeof item.badge !== "string") continue;
      const label = item.tooltip.trim();
      const icon = item.badge.trim();
      if (!label || !/^https?:\/\/[^/\s]+/i.test(icon)) continue;
      const key = JSON.stringify([provider, label, icon]);
      if (seen.has(key)) continue;
      seen.add(key);
      const rarity = typeof item.rarity === "number" ? rarities.get(item.rarity) : undefined;
      badges.push({ icon, label, provider, rarity });
    }
  }
  return badges;
}

type Cached = { at: number; groups: Record<string, unknown> };
const cache = new Map<string, Cached>();
const pending = new Map<string, Promise<Phoenix.ProfileBadge[]>>();
let active = true;
const saved = phoenix.storage.get<unknown>("equibadges");
if (isRecord(saved)) {
  for (const [id, entry] of Object.entries(saved).slice(-100)) {
    if (isRecord(entry) && typeof entry.at === "number" && isRecord(entry.groups)) {
      cache.set(id, { at: entry.at, groups: entry.groups });
    }
  }
}

async function fetchBadges(id: string) {
  const url = `https://badges.equicord.org/${id}?exclude=discord&separated=true&capitalize=true`;
  const res = await phoenix.fetch(url);
  let groups: Record<string, unknown> = {};
  if (res.status !== 404) {
    if (!res.ok) throw new Error(`Equibadges HTTP ${res.status}`);
    const body = res.json<unknown>();
    if (!isRecord(body) || !isRecord(body.badges)) throw new Error("Invalid Equibadges response");
    groups = body.badges;
  }
  if (active) {
    cache.delete(id);
    cache.set(id, { at: Date.now(), groups });
    const oldest = cache.keys().next();
    if (cache.size > 100 && !oldest.done) cache.delete(oldest.value);
    try {
      phoenix.storage.set("equibadges", Object.fromEntries(cache));
    } catch (error) {
      phoenix.log.warn("Could not save badge cache", String(error));
    }
  }
  return parse(groups);
}

const registration = phoenix.ui.addProfileBadges({
  async render({ userId }) {
    if (!userId || !/^\d{15,20}$/.test(userId)) return [];
    const cached = cache.get(userId);
    if (cached && Date.now() - cached.at < ttl) return parse(cached.groups);
    const existing = pending.get(userId);
    if (existing) return existing;
    const request = fetchBadges(userId).catch(error => {
      phoenix.log.warn("Badge lookup failed", String(error));
      return cached ? parse(cached.groups) : [];
    }).finally(() => pending.delete(userId));
    pending.set(userId, request);
    return request;
  },
});

phoenix.settings.define({
  refresh: { type: "button", title: "Refresh badges", onPress() {
    for (const entry of cache.values()) entry.at = 0;
    phoenix.storage.set("equibadges", Object.fromEntries(cache));
    registration.update?.();
  } },
});

phoenix.onUnload(() => { active = false; });
