//! Locate badge controls in the current profile and borrow their native UI.

use std::collections::HashMap;

use phoenix_ui::{Node, Prop, ViewKind};
use phoenix_wire::Value;

use super::{
   Draft, Handler, ProfileBadge, avatar_user,
   mirror::{Mirror, prop},
};

impl Mirror {
   fn uri(&self, id: i32) -> Option<&str> {
      match prop(self.props(id)?, Prop::URI)? {
         Value::Str(uri) => Some(uri),
         _ => None,
      }
   }

   fn is_badge_icon(&self, id: i32) -> bool {
      self.kind(id) == Some(&ViewKind::Image)
         && self
            .uri(id)
            .is_some_and(|uri| uri.contains("/badge-icons/"))
   }

   /// Search siblings of the profile pager, nearest first. In particular,
   /// an avatar on a mutuals page or in a chat behind the sheet isn't the
   /// profile owner.
   fn profile_neighbor(&self, pager: i32, test: &impl Fn(i32) -> bool) -> Option<i32> {
      let mut branch = pager;
      for _ in 0..24 {
         let Some(parent) = self.parent(branch) else {
            break;
         };
         if self.kind(parent) == Some(&ViewKind::Navigator) {
            break;
         }
         for sibling in self.kids(parent) {
            if *sibling != branch
               && let Some(found) = self.find_control(*sibling, 14, test)
            {
               return Some(found);
            }
         }
         if self
            .kind(parent)
            .is_some_and(|kind| kind.as_str() == "sheet")
            || self.accessibility_id(parent) == Some("you-root")
         {
            break;
         }
         branch = parent;
      }
      // Some layouts put the same header inside the Main page. The
      // remaining pager children belong to its other tabs.
      let pages = *self.kids(pager).first()?;
      let main = *self.kids(pages).first()?;
      self.find_control(main, 14, test)
   }

   fn find_control(&self, id: i32, depth: usize, test: &impl Fn(i32) -> bool) -> Option<i32> {
      if test(id) {
         return Some(id);
      }
      if depth == 0
         || matches!(self.kind(id)?.as_str(), "navigator" | "sheet" | "list")
         || matches!(
            self.accessibility_id(id),
            Some("you-root" | "profile-tab-pages" | "you-tab-pages")
         )
      {
         return None;
      }
      self
         .kids(id)
         .iter()
         .find_map(|kid| self.find_control(*kid, depth - 1, test))
   }

   pub fn profile_badge_user(&self, pager: i32) -> Option<String> {
      let avatar = self.profile_neighbor(pager, &|id| self.accessibility_id(id) == Some("Avatar"));
      let image = if let Some(avatar) = avatar {
         self.find_control(avatar, 6, &|id| {
            self.kind(id) == Some(&ViewKind::Image) && self.uri(id).and_then(avatar_user).is_some()
         })?
      } else {
         // Some profile headers expose the image without the Avatar wrapper.
         self.profile_neighbor(pager, &|id| {
            self.kind(id) == Some(&ViewKind::Image)
               && self.uri(id).and_then(avatar_user).is_some()
               && self.props(id).is_some_and(|props| {
                  [Prop::named("width"), Prop::named("height")]
                     .iter()
                     .any(|key| match prop(props, *key) {
                        Some(Value::I32(size)) => *size >= 40,
                        Some(Value::F64(size)) => *size >= 40.0,
                        _ => false,
                     })
               })
         })?
      };
      avatar_user(self.uri(image)?)
   }

   pub fn profile_badge_parent(&self, pager: i32) -> Option<i32> {
      let image = self.profile_neighbor(pager, &|id| self.is_badge_icon(id))?;
      self.parent(self.parent(image)?)
   }

   pub fn badge_directory(&self, title: i32) -> Option<i32> {
      let mut id = title;
      for _ in 0..10 {
         id = self.parent(id)?;
         if let Some(scroll) = self
            .kids(id)
            .iter()
            .find(|kid| self.kind(**kid) == Some(&ViewKind::Scroll))
         {
            return self.find_control(*scroll, 8, &|node| {
               self.kind(node).is_some_and(|kind| kind.as_str() == "flow")
                  && self.badge_card(node).is_some()
            });
         }
         if self
            .kind(id)
            .is_some_and(|kind| matches!(kind.as_str(), "navigator" | "sheet"))
         {
            break;
         }
      }
      None
   }

   pub fn badge_counter(&self, row: i32) -> Option<(i32, usize)> {
      self.kids(row).iter().find_map(|&kid| {
         if self.kind(kid) != Some(&ViewKind::Text) {
            return None;
         }
         Some((kid, self.text(kid)?.strip_prefix('+')?.parse().ok()?))
      })
   }

   fn badge_image(&self, card: i32, directory: bool) -> Option<i32> {
      self
         .find_control(card, 6, &|id| self.is_badge_icon(id))
         .or_else(|| {
            // The directory also uses local assets for collectible badges.
            directory
               .then(|| self.find_control(card, 6, &|id| self.kind(id) == Some(&ViewKind::Image)))?
         })
   }

   pub fn badge_card(&self, parent: i32) -> Option<i32> {
      let directory = self
         .kind(parent)
         .is_some_and(|kind| kind.as_str() == "flow");
      self
         .kids(parent)
         .iter()
         .copied()
         .find(|&card| self.badge_image(card, directory).is_some())
   }
}

/// Native cards can include their tier in the accessibility label, their
/// caption or a named resource such as ic_badge_mythic.
fn tier(text: &str) -> Option<&'static str> {
   const TIERS: [&str; 5] = ["common", "uncommon", "rare", "epic", "mythic"];
   text
      .split(|ch: char| !ch.is_ascii_alphabetic())
      .find_map(|word| {
         TIERS
            .into_iter()
            .find(|tier| word.eq_ignore_ascii_case(tier))
      })
}

fn card_tier(mirror: &Mirror, card: i32) -> Option<&'static str> {
   let node_tier = |id| {
      [Prop::ACCESSIBILITY_LABEL, Prop::TEXT, Prop::URI]
         .iter()
         .find_map(|key| {
            let Value::Str(text) = prop(mirror.props(id)?, *key)? else {
               return None;
            };
            tier(text)
         })
   };
   node_tier(mirror.find_control(card, 6, &|id| node_tier(id).is_some())?)
}

pub(super) fn cards(
   mirror: &Mirror,
   parent: i32,
   directory: bool,
   badges: &[ProfileBadge],
) -> Vec<Draft> {
   if badges.is_empty() {
      return Vec::new();
   }
   let mut templates = HashMap::new();
   let mut first = None;
   for &card in mirror.kids(parent) {
      let Some(image) = mirror.badge_image(card, directory) else {
         continue;
      };
      let tier = directory.then(|| card_tier(mirror, card)).flatten();
      if templates.contains_key(&tier) {
         continue;
      }
      if let Some(template) = mirror.live_snapshot(card) {
         first.get_or_insert(tier);
         templates.insert(tier, (card, image, template));
      }
      if !directory {
         break;
      }
   }
   let Some(first) = first else {
      return Vec::new();
   };
   badges
      .iter()
      .map(|badge| {
         let tier = badge.rarity.as_deref().and_then(tier);
         let (native, image, template) = templates.get(&tier).unwrap_or(&templates[&first]);
         let mut card = customize_card(template, *image, badge);
         if directory {
            let label = [
               Some(badge.label.as_str()),
               badge.provider.as_deref(),
               badge.rarity.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            card.set(Prop::ACCESSIBILITY_LABEL, Some(Value::Str(label)));
            card.set(Prop::ON, Some(Value::Arr(vec![Value::Str("press".into())])));
            card.press = Some(Handler::Badge {
               native: *native,
               badge: badge.clone(),
            });
         }
         card
      })
      .collect()
}

fn customize_card(template: &Node, image: i32, badge: &ProfileBadge) -> Draft {
   let mut node = Draft {
      kind: template.kind.clone(),
      props: template.props.clone(),
      kids: template
         .kids
         .iter()
         .map(|child| customize_card(child, image, badge))
         .collect(),
      press: None,
   };
   node.props.retain(|(key, _)| {
      ![
         Prop::ON,
         Prop::ACCESSIBILITY_ID,
         Prop::ACCESSIBILITY_LABEL,
         Prop::named("worklet"),
         Prop::named("viewMotion"),
         Prop::named("portalSlot"),
      ]
      .contains(key)
   });
   if node.kind == ViewKind::Text
      && let Some(Value::Str(text)) = prop(&node.props, Prop::TEXT)
   {
      let caption = if tier(text).is_some_and(|tier| text.eq_ignore_ascii_case(tier)) {
         badge.rarity.as_deref().unwrap_or_default().to_uppercase()
      } else {
         badge.label.clone()
      };
      node.set(Prop::TEXT, Some(Value::Str(caption)));
      node.set(Prop::named("spans"), None);
   }
   if template.src == u32::try_from(image).ok() {
      node.set(Prop::URI, Some(Value::Str(badge.icon.clone())));
      node.set(Prop::TINT, None);
      node.set(Prop::named("scale"), Some(Value::Str("fit".to_owned())));
   }
   node
}

#[cfg(test)]
mod tests {
   use super::*;
   use phoenix_ui::{Op, Props};

   fn create(id: i32, kind: &str, props: Props) -> Op {
      Op::Create {
         id,
         kind: ViewKind::from(kind.to_owned()),
         props,
      }
   }

   fn children(id: i32, ids: &[i32]) -> Op {
      Op::Children {
         id,
         ids: ids.to_vec(),
      }
   }

   fn string(key: Prop, value: &str) -> (Prop, Value) {
      (key, Value::Str(value.to_owned()))
   }

   #[test]
   fn chat_profile_does_not_inherit_a_mounted_you_profile() {
      let mut mirror = Mirror::default();
      mirror.apply(&[
         create(1, "navigator", vec![]),
         create(2, "view", vec![string(Prop::ACCESSIBILITY_ID, "you-root")]),
         create(3, "sheet", vec![]),
         create(
            4,
            "scroll",
            vec![string(Prop::ACCESSIBILITY_ID, "you-tab-pages")],
         ),
         create(
            5,
            "scroll",
            vec![string(Prop::ACCESSIBILITY_ID, "profile-tab-pages")],
         ),
         create(6, "view", vec![string(Prop::ACCESSIBILITY_ID, "Avatar")]),
         create(7, "view", vec![string(Prop::ACCESSIBILITY_ID, "Avatar")]),
         create(
            8,
            "image",
            vec![string(
               Prop::URI,
               "https://cdn.discordapp.com/avatars/111/hash",
            )],
         ),
         create(
            9,
            "image",
            vec![string(
               Prop::URI,
               "https://cdn.discordapp.com/guilds/13/users/222/avatars/hash",
            )],
         ),
         children(1, &[2, 3]),
         children(2, &[4, 6]),
         children(3, &[5, 7]),
         children(6, &[8]),
         children(7, &[9]),
      ]);
      assert_eq!(mirror.profile_badge_user(4).as_deref(), Some("111"));
      assert_eq!(mirror.profile_badge_user(5).as_deref(), Some("222"));
      mirror.apply(&[Op::Update {
         id: 9,
         props: vec![string(Prop::URI, "res://default_avatar")],
      }]);
      assert_eq!(mirror.profile_badge_user(5), None);
      mirror.apply(&[children(3, &[5])]);
      assert_eq!(mirror.parent(7), None);
      assert_eq!(mirror.profile_badge_user(5), None);
   }

   #[test]
   fn preview_card_leaves_the_pill_in_charge_of_directory_navigation() {
      let mut mirror = Mirror::default();
      mirror.apply(&[
         create(1, "view", vec![]),
         create(2, "scroll", vec![]),
         create(3, "view", vec![]),
         create(4, "view", vec![string(Prop::ON, "press")]),
         create(
            5,
            "image",
            vec![string(
               Prop::URI,
               "https://cdn.discordapp.com/badge-icons/a.png",
            )],
         ),
         create(6, "text", vec![string(Prop::TEXT, "+5")]),
         children(1, &[2, 3]),
         children(3, &[4, 6]),
         children(4, &[5]),
      ]);
      let parent = mirror.profile_badge_parent(2).unwrap();
      assert_eq!(mirror.badge_counter(parent), Some((6, 5)));
      let badge = ProfileBadge {
         label: "Test".into(),
         icon: "https://example.org/icon.png".into(),
         provider: None,
         rarity: None,
      };
      let cards = cards(&mirror, parent, false, &[badge]);
      assert_eq!(prop(&cards[0].props, Prop::ON), None);
      assert!(cards[0].press.is_none());
   }

   #[test]
   fn directory_card_uses_current_native_image_size_and_matching_tier() {
      let mut mirror = Mirror::default();
      mirror.apply(&[
         create(1, "view", vec![]),
         create(2, "text", vec![]),
         create(3, "scroll", vec![]),
         create(4, "flow", vec![]),
         children(1, &[2, 3]),
         children(3, &[4]),
      ]);
      assert_eq!(mirror.badge_directory(2), None);
      mirror.apply(&[
         create(
            5,
            "view",
            vec![string(Prop::ACCESSIBILITY_LABEL, "Gift · Rare")],
         ),
         create(6, "image", vec![string(Prop::URI, "res://gift")]),
         create(
            7,
            "view",
            vec![string(Prop::ACCESSIBILITY_LABEL, "Booster · Epic")],
         ),
         create(
            8,
            "image",
            vec![string(Prop::URI, "res://boost"), string(Prop::ON, "press")],
         ),
         children(4, &[5, 7]),
         children(5, &[6]),
         children(7, &[8]),
      ]);
      mirror.apply(&[Op::Update {
         id: 8,
         props: vec![(Prop::HEIGHT, Value::F64(48.0))],
      }]);
      let parent = mirror.badge_directory(2).unwrap();
      let badge = ProfileBadge {
         label: "Test".into(),
         icon: "https://example.org/icon.png".into(),
         provider: Some("Equicord".into()),
         rarity: Some("Epic".into()),
      };
      let cards = cards(&mirror, parent, true, &[badge]);
      assert!(matches!(
         cards[0].press,
         Some(Handler::Badge { native: 7, .. })
      ));
      assert_eq!(
         prop(&cards[0].props, Prop::ON),
         Some(&Value::Arr(vec![Value::Str("press".into())]))
      );
      let icon = &cards[0].kids[0];
      assert_eq!(prop(&icon.props, Prop::HEIGHT), Some(&Value::F64(48.0)));
      assert_eq!(
         prop(&icon.props, Prop::URI).and_then(Value::as_str),
         Some("https://example.org/icon.png")
      );
      assert_eq!(prop(&icon.props, Prop::ON), None);
   }
}
