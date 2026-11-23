//! Render a custom badge inside the sheet opened by a native directory card.
use super::*;

#[derive(Default)]
pub(super) struct BadgeDetails {
   pub(super) pending: Option<ProfileBadge>,
   // Phoenix may attach the sheet's children in a later batch.
   opening: Option<(i32, Option<ProfileBadge>)>,
   tiers: HashMap<String, Node>,
   metadata_style: Option<Node>,
}

impl BadgeDetails {
   pub(super) fn deleted(&mut self, node: i32) {
      if self.opening.as_ref().is_some_and(|(id, _)| *id == node) {
         self.opening = None;
      }
   }
}

impl Patches {
   pub(super) fn update_badge_details(
      &mut self,
      named: &[i32],
      out: &mut Vec<Op>,
      touched: &mut HashSet<i32>,
   ) {
      if !self
         .contributions
         .iter()
         .any(|c| c.target == Target::ProfileBadges)
      {
         return;
      }
      for id in named {
         if self.mirror.accessibility_id(*id) == Some("badge-details-scroll") {
            self.badge_details.opening = Some((*id, self.badge_details.pending.take()));
         }
      }
      let Some((scroll, badge)) = self.badge_details.opening.as_ref() else {
         return;
      };
      let scroll = *scroll;
      let Some(body) = self
         .mirror
         .kids(scroll)
         .first()
         .and_then(|id| self.mirror.live_snapshot(*id))
      else {
         return;
      };
      let Some(hero) = body.kids.first() else {
         return;
      };
      let row = metadata(hero);
      if let Some((flow, at, original)) = row {
         self
            .badge_details
            .tiers
            .insert(original.to_uppercase(), flow.kids[at].clone());
         self.badge_details.metadata_style = Some(flow.clone());
      }
      let draft = badge
         .as_ref()
         .and_then(|badge| detail_body(&body, row, badge, &self.badge_details));
      if badge.is_some() && draft.is_none() {
         return;
      }
      self.badge_details.opening = None;
      if let Some(draft) = draft {
         let mut all = Vec::new();
         let root = self.emit(draft, None, out, &mut all);
         self.slots.push(Slot {
            kind: SlotKind::BadgeDetails,
            parent: scroll,
            anchor: scroll,
            before: Before::Start,
            place: Place::Screen,
            roots: vec![root],
            all,
         });
         touched.insert(scroll);
      }
   }
}

fn text(node: &Node) -> Option<&str> {
   (node.kind == ViewKind::Text).then(|| prop(&node.props, Prop::TEXT)?.as_str())?
}

fn is_tier(value: &str) -> bool {
   ["common", "rare", "epic", "mythic", "uncommon", "legendary"]
      .iter()
      .any(|name| value.eq_ignore_ascii_case(name))
}

fn metadata(node: &Node) -> Option<(&Node, usize, &str)> {
   for (at, group) in node.kids.iter().enumerate() {
      if let Some(tier) = group
         .kids
         .iter()
         .filter_map(text)
         .find(|name| is_tier(name))
      {
         return Some((node, at, tier));
      }
   }
   node.kids.iter().find_map(metadata)
}

fn first(node: &Node, kind: ViewKind) -> Option<&Node> {
   if node.kind == kind {
      return Some(node);
   }
   node
      .kids
      .iter()
      .find_map(|child| first(child, kind.clone()))
}

fn visual_node(node: &Node) -> Draft {
   let mut out = visual_shallow(node);
   out.kids = node.kids.iter().map(visual_node).collect();
   out
}

fn replace_text(node: &mut Draft, value: &str) {
   node.set(Prop::named("spans"), None);
   node.set(Prop::TEXT, Some(Value::Str(value.to_owned())));
}

fn caption(template: &Node, value: &str) -> Draft {
   let mut out = visual_shallow(template);
   replace_text(&mut out, value);
   out
}

type Metadata<'a> = (&'a Node, usize, &'a str);

fn metadata_draft(
   row: Option<Metadata<'_>>,
   label: &Node,
   badge: &ProfileBadge,
   styles: &BadgeDetails,
) -> Draft {
   let row = row.or_else(|| metadata(styles.metadata_style.as_ref()?));
   let mut meta = row.map_or_else(
      || Draft {
         kind: ViewKind::Other("flow".into()),
         props: vec![(Prop::GAP, Value::F64(4.0))],
         kids: Vec::new(),
         press: None,
      },
      |(flow, _, _)| visual_shallow(flow),
   );
   let before = row.map_or(&[][..], |(flow, at, _)| &flow.kids[..at]);
   let label = before
      .iter()
      .find(|node| text(node).is_some_and(|s| !matches!(s, "·" | "•" | "⋅")))
      .or_else(|| row.and_then(|(flow, at, _)| first(&flow.kids[at], ViewKind::Text)))
      .unwrap_or(label);
   if let Some(provider) = &badge.provider {
      meta.kids.push(caption(label, provider));
   }
   if let Some(tier) = &badge.rarity {
      if !meta.kids.is_empty() {
         let separator = before.last().filter(|s| {
            s.kind == ViewKind::View || text(s).is_some_and(|s| matches!(s, "·" | "•" | "⋅"))
         });
         meta
            .kids
            .push(separator.map_or_else(|| caption(label, "·"), visual_node));
      }
      let wanted = tier.to_uppercase();
      let chip = if let Some(native) = styles.tiers.get(&wanted) {
         visual_node(native)
      } else if let Some((flow, at, original)) = row {
         let mut group = visual_node(&flow.kids[at]);
         retier(&mut group, original, &wanted);
         group
      } else {
         caption(label, &wanted)
      };
      meta.kids.push(chip);
   }
   meta
}

struct Header<'a> {
   image: u32,
   title: u32,
   flow: Option<u32>,
   badge: &'a ProfileBadge,
   metadata: Option<Draft>,
}

impl Header<'_> {
   fn draw(&mut self, node: &Node) -> Option<Draft> {
      if self.flow.is_some() && node.src == self.flow {
         return self.metadata.take();
      }
      if node.src == Some(self.image) {
         return Some(Draft {
            kind: ViewKind::Image,
            props: vec![
               (Prop::URI, Value::Str(self.badge.icon.clone())),
               (Prop::named("width"), Value::F64(132.0)),
               (Prop::HEIGHT, Value::F64(132.0)),
               (Prop::named("scale"), Value::Str("fit".into())),
            ],
            kids: Vec::new(),
            press: None,
         });
      }
      if node.kind == ViewKind::Text {
         return (node.src == Some(self.title))
            .then(|| caption(node, &self.badge.label.to_uppercase()));
      }
      let mut out = visual_shallow(node);
      for kid in &node.kids {
         if let Some(child) = self.draw(kid) {
            out.kids.push(child);
         }
         if kid.src == Some(self.title)
            && self.flow.is_none()
            && let Some(meta) = self.metadata.take()
         {
            out.kids.push(meta);
         }
      }
      Some(out)
   }
}

// Copy the native hero only. A donor without a tier still supplies the drawer,
// badge image and title. Its achievement description belongs to the donor.
fn detail_body(
   body: &Node,
   row: Option<Metadata<'_>>,
   badge: &ProfileBadge,
   styles: &BadgeDetails,
) -> Option<Draft> {
   let hero = body.kids.first()?;
   let title = first(hero, ViewKind::Text)?;
   let subtitle = hero
      .kids
      .iter()
      .filter_map(|n| first(n, ViewKind::Text))
      .find(|n| n.src != title.src)
      .unwrap_or(title);
   let mut header = Header {
      image: first(hero, ViewKind::Image)?.src?,
      title: title.src?,
      flow: row.and_then(|(flow, _, _)| flow.src),
      badge,
      metadata: Some(metadata_draft(row, subtitle, badge, styles)),
   };
   let mut out = visual_shallow(body);
   out.kids.push(header.draw(hero)?);
   Some(out)
}

fn visual_shallow(node: &Node) -> Draft {
   let mut props = node.props.clone();
   props.retain(|(key, _)| {
      !matches!(
         key.name(),
         Some(
            "worklet"
               | "on"
               | "viewMotion"
               | "portalSlot"
               | "accessibilityId"
               | "accessibilityLabel"
               | "alpha"
               | "opacity"
               | "scaleX"
               | "scaleY"
         )
      )
   });
   Draft {
      kind: node.kind.clone(),
      props,
      kids: Vec::new(),
      press: None,
   }
}

// The directory may be opened before a native drawer of this tier was seen.
// Phoenix names its tier art with the resource's lowercase tier name.
fn retier(node: &mut Draft, original: &str, wanted: &str) {
   if node.kind == ViewKind::Text
      && prop(&node.props, Prop::TEXT)
         .and_then(Value::as_str)
         .is_some_and(|s| s.eq_ignore_ascii_case(original))
   {
      replace_text(node, wanted);
   }
   for (_, value) in &mut node.props {
      if let Value::Str(uri) = value
         && uri.starts_with("res://")
      {
         *uri = uri
            .replace(&original.to_lowercase(), &wanted.to_lowercase())
            .replace(&original.to_uppercase(), wanted);
      }
   }
   for kid in &mut node.kids {
      retier(kid, original, wanted);
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn node(id: u32, kind: ViewKind, text: Option<&str>, kids: Vec<Node>) -> Node {
      Node {
         src: Some(id),
         kind,
         kids,
         props: text
            .map(|s| (Prop::TEXT, Value::Str(s.to_owned())))
            .into_iter()
            .collect(),
      }
   }

   fn labels(node: &Draft, result: &mut Vec<String>) {
      if let Some(value) = prop(&node.props, Prop::TEXT).and_then(Value::as_str) {
         result.push(value.to_owned());
      }
      for kid in &node.kids {
         labels(kid, result);
      }
   }

   #[test]
   fn custom_header_replaces_donor_even_without_a_native_tier() {
      let badge = ProfileBadge {
         icon: "https://example.org/icon.png".into(),
         label: "Explorer".into(),
         provider: Some("Community".into()),
         rarity: Some("Rare".into()),
      };
      for with_tier in [false, true] {
         let mut metadata_nodes = vec![node(6, ViewKind::Text, Some("Unlocked date"), vec![])];
         if with_tier {
            metadata_nodes.push(node(7, ViewKind::Text, Some("·"), vec![]));
            metadata_nodes.push(node(
               8,
               ViewKind::View,
               None,
               vec![node(9, ViewKind::Text, Some("EPIC"), vec![])],
            ));
         }
         let hero = node(
            2,
            ViewKind::View,
            None,
            vec![
               node(3, ViewKind::Image, None, vec![]),
               node(4, ViewKind::Text, Some("Original badge"), vec![]),
               node(5, ViewKind::Other("flow".into()), None, metadata_nodes),
            ],
         );
         let body = node(
            1,
            ViewKind::View,
            None,
            vec![hero, node(10, ViewKind::Text, Some("Donor status"), vec![])],
         );
         let row = metadata(&body.kids[0]);
         let rendered = detail_body(&body, row, &badge, &BadgeDetails::default()).unwrap();
         let mut text = Vec::new();
         labels(&rendered, &mut text);
         assert_eq!(text, ["EXPLORER", "Community", "·", "RARE"]);
         let image = &rendered.kids[0].kids[0];
         assert_eq!(
            prop(&image.props, Prop::URI).and_then(Value::as_str),
            Some(badge.icon.as_str())
         );
      }
   }
}
