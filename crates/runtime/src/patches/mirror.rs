use std::collections::{
   HashMap,
   HashSet,
};

use phoenix_ui::{
   ChildEdit,
   Node,
   Op,
   Prop,
   Props,
   ViewKind,
};
use phoenix_wire::Value;

const FIRST: Prop = Prop::named("first");

/// Rust's view tree as its batches describe it: who holds whom, and the
/// accessibility id each node carries.
#[derive(Default)]
pub struct Mirror {
   kids:     HashMap<i32, Vec<i32>>,
   parent:   HashMap<i32, i32>,
   ids:      HashMap<i32, String>,
   sheets:   HashSet<i32>,
   /// Live host properties let badge cards and drawers use the current theme
   /// even when their container and contents arrived in separate batches.
   nodes:    HashMap<i32, (ViewKind, Props)>,
   /// The list index of each virtualized list's first mounted row.
   firsts:   HashMap<i32, i32>,
   /// The message box inputs and their text, newest last.
   composer: Vec<(i32, String)>,
}

/// What one batch created, by node.
pub type Created<'ops> = HashMap<i32, (&'ops ViewKind, &'ops Props)>;

impl Mirror {
   /// Applies `ops` and returns the nodes they created, plus the nodes they
   /// deleted.
   pub fn apply<'ops>(&mut self, ops: &'ops [Op]) -> (Created<'ops>, Vec<i32>) {
      let mut created = HashMap::new();
      let mut deleted = Vec::new();
      for op in ops {
         match op {
            Op::Create { id, kind, props } => {
               created.insert(*id, (kind, props));
               self.nodes.insert(*id, (kind.clone(), props.clone()));
               if let Some(Value::I32(first)) = prop(props, FIRST) {
                  self.firsts.insert(*id, *first);
               }
               if let Some(Value::Str(aid)) = prop(props, Prop::ACCESSIBILITY_ID) {
                  if aid == "composer" && !self.composer.iter().any(|(input, _)| input == id) {
                     self.composer.push((*id, String::new()));
                  }
                  self.ids.insert(*id, aid.clone());
               }
               if kind.as_str() == "sheet" {
                  self.sheets.insert(*id);
               }
            },
            Op::Children { id, ids } => {
               self.replace_children(*id, ids.clone());
            },
            Op::ChildEdits { id, edits } => {
               let new = apply_edits(self.kids(*id), edits);
               self.replace_children(*id, new);
            },
            Op::Delete { ids } => {
               for id in ids {
                  self.replace_children(*id, Vec::new());
                  self.kids.remove(id);
                  self.nodes.remove(id);
                  self.parent.remove(id);
                  self.ids.remove(id);
                  self.sheets.remove(id);
                  self.firsts.remove(id);
                  self.composer.retain(|(input, _)| input != id);
               }
               deleted.extend_from_slice(ids);
            },
            Op::Update { id, props } => {
               if let Some((_, current)) = self.nodes.get_mut(id) {
                  for (key, value) in props {
                     phoenix_ui::set_prop(current, *key, Some(value.clone()));
                  }
               }
               if let Some(aid) = prop(props, Prop::ACCESSIBILITY_ID) {
                  if let Value::Str(aid) = aid {
                     self.ids.insert(*id, aid.clone());
                  } else {
                     self.ids.remove(id);
                  }
               }
               if let Some(Value::I32(first)) = prop(props, FIRST) {
                  self.firsts.insert(*id, *first);
               }
               if let Some(Value::Str(text)) = prop(props, Prop::VALUE) {
                  self.set_composer_text(*id, text.clone());
               }
            },
            _ => {},
         }
      }
      (created, deleted)
   }

   fn replace_children(&mut self, id: i32, kids: Vec<i32>) {
      if let Some(old) = self.kids.get(&id) {
         for kid in old {
            if self.parent.get(kid) == Some(&id) {
               self.parent.remove(kid);
            }
         }
      }
      for kid in &kids {
         self.parent.insert(*kid, id);
      }
      self.kids.insert(id, kids);
   }

   pub fn kind(&self, id: i32) -> Option<&ViewKind> {
      self.nodes.get(&id).map(|(kind, _)| kind)
   }

   pub fn props(&self, id: i32) -> Option<&Props> {
      self.nodes.get(&id).map(|(_, props)| props)
   }

   pub fn text(&self, id: i32) -> Option<&str> {
      match prop(self.props(id)?, Prop::TEXT)? {
         Value::Str(text) => Some(text),
         _ => None,
      }
   }

   pub fn live_snapshot(&self, id: i32) -> Option<Node> {
      let (kind, props) = self.nodes.get(&id)?;
      Some(Node {
         kind: kind.clone(),
         props: props.clone(),
         kids: self.kids(id).iter().map(|kid| self.live_snapshot(*kid)).collect::<Option<_>>()?,
         src: u32::try_from(id).ok(),
      })
   }

   /// The row at list index `index` of the virtualized list `id`.
   pub fn row(&self, id: i32, index: i32) -> Option<i32> {
      let offset = index - self.firsts.get(&id).copied().unwrap_or(0);
      self.kids(id).get(usize::try_from(offset).ok()?).copied()
   }

   pub fn kids(&self, id: i32) -> &[i32] {
      self.kids.get(&id).map_or(&[], Vec::as_slice)
   }

   pub fn has_kids(&self, id: i32) -> bool {
      self.kids.contains_key(&id)
   }

   pub fn parent(&self, id: i32) -> Option<i32> {
      self.parent.get(&id).copied()
   }

   pub fn accessibility_id(&self, id: i32) -> Option<&str> {
      self.ids.get(&id).map(String::as_str)
   }

   /// The first node at or under `id`, depth first, whose accessibility id
   /// passes `test`.
   pub fn find_down(&self, id: i32, test: &impl Fn(&str) -> bool, depth: usize) -> Option<i32> {
      if self.accessibility_id(id).is_some_and(test) {
         return Some(id);
      }
      if depth == 0 {
         return None;
      }
      self
         .kids(id)
         .iter()
         .find_map(|kid| self.find_down(*kid, test, depth - 1))
   }

   /// The sheet created last, which is on top.
   pub fn newest_sheet(&self) -> Option<i32> {
      self.sheets.iter().max().copied()
   }

   /// The sheet holding `id`, if any.
   pub fn sheet_of(&self, id: i32) -> Option<i32> {
      let mut node = self.parent(id);
      for _ in 0..40 {
         let current = node?;
         if self.sheets.contains(&current) {
            return Some(current);
         }
         node = self.parent(current);
      }
      None
   }

   pub fn composer(&self) -> Option<(i32, &str)> {
      self
         .composer
         .last()
         .map(|(input, text)| (*input, text.as_str()))
   }

   pub fn is_composer(&self, id: i32) -> bool {
      self.composer.iter().any(|(input, _)| *input == id)
   }

   pub fn set_composer_text(&mut self, id: i32, text: String) {
      if let Some((_, current)) = self.composer.iter_mut().find(|(input, _)| *input == id) {
         *current = text;
      }
   }

   /// `id` and what this batch created under it, as a template.
   pub fn snapshot(&self, id: i32, created: &Created<'_>) -> Option<Node> {
      let (kind, props) = created.get(&id)?;
      Some(Node {
         kind:  (*kind).clone(),
         props: (*props).clone(),
         kids:  self
            .kids(id)
            .iter()
            .filter_map(|kid| self.snapshot(*kid, created))
            .collect(),
         src:   None,
      })
   }
}

pub fn prop(props: &Props, key: Prop) -> Option<&Value> {
   props
      .iter()
      .find(|(existing, _)| *existing == key)
      .map(|(_, value)| value)
}

fn apply_edits(old: &[i32], edits: &[ChildEdit]) -> Vec<i32> {
   let mut out = Vec::with_capacity(old.len());
   let mut at = 0;
   for edit in edits {
      let keep = usize::try_from(edit.keep).unwrap_or_default();
      let del = usize::try_from(edit.del).unwrap_or_default();
      out.extend(old.iter().skip(at).take(keep));
      at = (at + keep + del).min(old.len());
      out.extend_from_slice(&edit.ins);
   }
   out.extend(old.iter().skip(at));
   out
}
