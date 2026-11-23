//! Badge preview and directory share the same contribution result and account.
use super::*;

#[derive(Default)]
pub(super) struct BadgeViews {
   pub(super) directories: HashMap<i32, Option<String>>,
   pub(super) native_visible: HashMap<i32, usize>,
   counter_style: Option<Props>,
}

#[derive(Debug, PartialEq, Eq)]
struct Preview {
   native: usize,
   custom: usize,
   hidden: usize,
}

impl Preview {
   fn layout(icons: usize, counter: Option<usize>, badges: usize) -> Self {
      // A +N indicates a full native row. Keep one original alongside the custom
      // preview when there is room, and include every displaced icon in +N.
      let capacity = if counter.is_some() {
         icons.max(1)
      } else {
         icons.max(3)
      };
      let custom = badges
         .min(3)
         .min(capacity - usize::from(icons > 0 && capacity > 1));
      let native = icons.min(capacity - custom);
      Self {
         native,
         custom,
         hidden: counter
            .unwrap_or(0)
            .saturating_add(icons - native)
            .saturating_add(badges - custom),
      }
   }
}

impl Patches {
   pub(super) fn badge_drafts(
      &mut self,
      index: usize,
      target: Target,
      ids: &[ContributionId],
      place: &Place,
   ) -> Vec<Draft> {
      let parent = self.slots[index].parent;
      let mut items = Vec::new();
      for id in ids {
         if let Some(Outcome::Badges(badges)) = self.results.get(&(*id, place.clone())) {
            items.extend_from_slice(badges);
         }
      }
      if target == Target::BadgeDirectory {
         return badges::cards(&self.mirror, parent, true, &items);
      }
      self.badge_views.native_visible.remove(&parent);
      let counter = self.mirror.badge_counter(parent);
      if let Some((id, _)) = counter
         && let Some(props) = self.mirror.props(id)
      {
         self.badge_views.counter_style = Some(props.clone());
      }
      if items.is_empty() {
         return Vec::new();
      }
      let icons = self.mirror.kids(parent).len() - usize::from(counter.is_some());
      let preview = Preview::layout(icons, counter.map(|(_, count)| count), items.len());
      let mut drafts = badges::cards(&self.mirror, parent, false, &items[..preview.custom]);
      if drafts.is_empty() {
         return drafts;
      }
      self
         .badge_views
         .native_visible
         .insert(parent, preview.native);
      if preview.hidden != 0 {
         let mut props = self.badge_views.counter_style.clone().unwrap_or_default();
         props.retain(|(k, _)| {
            !matches!(
               k.name(),
               Some("accessibilityId" | "accessibilityLabel" | "on" | "worklet" | "spans")
            )
         });
         set_prop(
            &mut props,
            Prop::TEXT,
            Some(Value::Str(format!("+{}", preview.hidden))),
         );
         drafts.push(Draft {
            kind: ViewKind::Text,
            props,
            kids: Vec::new(),
            src: None,            press: None,
         });
      }
      drafts
   }

   pub(super) fn badge_profile_pressed(&self, node: i32) -> Option<String> {
      // The pill's enclosing press target receives taps even on the +N text.
      for slot in &self.slots {
         if slot.kind != SlotKind::Target(Target::ProfileBadges) {
            continue;
         }
         let ancestors = |id| std::iter::successors(Some(id), |id| self.mirror.parent(*id));
         if ancestors(slot.parent).take(4).any(|id| id == node)
            || ancestors(node).take(5).any(|id| id == slot.parent)
         {
            if let Place::User(user) = &slot.place {
               return user.clone();
            }
         }
      }
      None
   }

   pub(super) fn badge_directory_owner(&mut self) -> Option<String> {
      if let Some(user) = self.seen.badge_user.take() {
         return Some(user);
      }
      // A directory opened without a press is attributable only when every
      // mounted pager resolves to the same user, including background pagers.
      let mut pagers = self.anchors.get(&Target::ProfileBadges)?.iter();
      let owner = self.mirror.profile_badge_user(*pagers.next()?)?;
      pagers
         .all(|id| self.mirror.profile_badge_user(*id).as_ref() == Some(&owner))
         .then_some(owner)
   }

   pub(super) fn update_badge_slots(
      &mut self,
      ops: &[Op],
      named: &[i32],
      out: &mut Vec<Op>,
      touched: &mut HashSet<i32>,
      deletes: &mut Vec<i32>,
   ) {
      if !self
         .contributions
         .iter()
         .any(|c| c.target == Target::ProfileBadges)
      {
         return;
      }
      let topology_changed = ops.iter().any(|op| {
         matches!(op, Op::Children { .. } | Op::ChildEdits { .. })
            || matches!(op, Op::Update{props,..} if prop(props, Prop::URI).is_some())
      });
      // Reuse the original owner when a directory flow remounts. A profile
      // pager can be reused for a different account or lose its avatar first.
      for index in (0..self.slots.len()).rev() {
         let slot = &self.slots[index];
         let SlotKind::Target(target @ (Target::ProfileBadges | Target::BadgeDirectory)) =
            slot.kind
         else {
            continue;
         };
         if named.contains(&slot.anchor) || touched.contains(&slot.parent) {
            continue;
         }
         let parent = slot.parent;
         if topology_changed {
            let location = match target {
               Target::ProfileBadges => self
                  .mirror
                  .profile_badge_parent(slot.anchor)
                  .zip(self.mirror.profile_badge_user(slot.anchor))
                  .map(|(row, user)| (row, Place::User(Some(user)))),
               _ => self
                  .mirror
                  .badge_directory(slot.anchor)
                  .map(|grid| (grid, slot.place.clone())),
            };
            if location
               .as_ref()
               .is_none_or(|(row, place)| *row != parent || *place != slot.place)
            {
               let slot = self.slots.remove(index);
               self.forget_slot(&slot);
               deletes.extend(slot.all);
               self.badge_views.native_visible.remove(&parent);
               touched.insert(parent);
               continue;
            }
         }
         let needs_cards = ops.iter().any(|op| match op {
            Op::Children { id, .. } | Op::ChildEdits { id, .. } => *id == parent,
            Op::Update { id, props } => {
               self.mirror.parent(*id) == Some(parent)
                  && (prop(props, Prop::TEXT).is_some()
                     || prop(props, Prop::ACCESSIBILITY_LABEL).is_some())
            },
            _ => false,
         });
         if needs_cards {
            let old = std::mem::take(&mut self.slots[index].all);
            self.forget_ids(&old);
            deletes.extend(old);
            self.fill(index, out);
            touched.insert(parent);
         }
      }
      if topology_changed {
         // A pager/title can arrive before its avatar or directory cards.
         for target in [Target::ProfileBadges, Target::BadgeDirectory] {
            let pending: Vec<_> = self
               .anchors
               .get(&target)
               .into_iter()
               .flatten()
               .copied()
               .filter(|id| {
                  !self
                     .slots
                     .iter()
                     .any(|s| s.anchor == *id && s.kind == SlotKind::Target(target))
               })
               .collect();
            for anchor in pending {
               if let Some(parent) = self.place(target, anchor, out) {
                  touched.insert(parent);
               }
            }
         }
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   #[test]
   fn displaced_original_and_undisplayed_custom_badges_count_once() {
      assert_eq!(
         Preview::layout(4, Some(5), 7),
         Preview {
            native: 1,
            custom: 3,
            hidden: 12
         }
      );
      assert_eq!(
         Preview::layout(1, None, 1),
         Preview {
            native: 1,
            custom: 1,
            hidden: 0
         }
      );
      assert_eq!(
         Preview::layout(2, None, 6),
         Preview {
            native: 1,
            custom: 2,
            hidden: 5
         }
      );
   }
}
