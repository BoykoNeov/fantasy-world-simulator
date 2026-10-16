//! Taxonomies: categorical trees kept as data. The goods tree is the first (build step 6); units,
//! faiths, species and techs will follow the same pattern.
//!
//! A tree is written as each node's children (see `data/goods.toml`). Nodes are numbered depth
//! first from the root, so a node's descendants are the run of numbers right after it. Readers ask
//! the tree ("the leaves under food", "the class of grain") instead of naming goods in code, so a
//! reader keeps working whichever depth a module version keeps its values at.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The goods tree shipped in `data/goods.toml`.
pub const STANDARD_GOODS: &str = include_str!("../data/goods.toml");

/// A tree as its data file writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeDef {
    pub root: String,
    /// Each node's children, in order. Leaves have no entry.
    pub children: BTreeMap<String, Vec<String>>,
    /// What a node is counted in; a node without one uses its parent's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub units: BTreeMap<String, String>,
}

impl TreeDef {
    pub fn standard_goods() -> TreeDef {
        Self::from_toml(STANDARD_GOODS).expect("bundled goods tree is valid")
    }

    pub fn from_toml(text: &str) -> Result<TreeDef, String> {
        toml::from_str(text).map_err(|e| format!("goods tree: {e}"))
    }
}

impl Default for TreeDef {
    /// The shipped goods tree: what a scenario written before trees were saved gets.
    fn default() -> Self {
        Self::standard_goods()
    }
}

/// A node's number in its tree.
pub type NodeId = u16;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub parent: Option<NodeId>,
    /// 0 for the root, 1 for the top-level classes.
    pub depth: u8,
    /// One past the node's last descendant: its descendants are `id + 1 .. end`.
    end: NodeId,
    /// Its own unit or its nearest ancestor's.
    unit: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Taxonomy {
    pub nodes: Vec<Node>,
}

impl Taxonomy {
    /// Build and check a tree: every name once, every listed parent reachable from the root, at
    /// least one class under the root.
    pub fn new(def: &TreeDef) -> Result<Taxonomy, String> {
        Taxonomy::named(def, "goods tree")
    }

    /// [`Taxonomy::new`] for a tree of something other than goods (`what` names it in refusals:
    /// "troop tree").
    pub fn named(def: &TreeDef, what: &str) -> Result<Taxonomy, String> {
        let mut t = Taxonomy { nodes: Vec::new() };
        fn visit(t: &mut Taxonomy, def: &TreeDef, name: &str, parent: Option<NodeId>, depth: u8) -> Result<(), String> {
            if t.find(name).is_some() {
                return Err(format!("{name} appears twice in the tree"));
            }
            if t.nodes.len() >= NodeId::MAX as usize {
                return Err("too many nodes".into());
            }
            let id = t.nodes.len() as NodeId;
            let unit = def.units.get(name).cloned().or_else(|| parent.and_then(|p| t.nodes[p as usize].unit.clone()));
            t.nodes.push(Node { name: name.into(), parent, depth, end: id + 1, unit });
            for child in def.children.get(name).into_iter().flatten() {
                let deeper = depth.checked_add(1).ok_or("the tree is too deep")?;
                visit(t, def, child, Some(id), deeper)?;
            }
            t.nodes[id as usize].end = t.nodes.len() as NodeId;
            Ok(())
        }
        visit(&mut t, def, &def.root, None, 0).map_err(|e| format!("{what}: {e}"))?;
        let unknown = def.children.keys().chain(def.units.keys()).find(|n| t.find(n).is_none());
        if let Some(n) = unknown {
            return Err(format!("{what}: {n} is not under {}", def.root));
        }
        if t.classes().is_empty() {
            return Err(format!("{what}: {} has no classes under it", def.root));
        }
        Ok(t)
    }

    pub fn standard_goods() -> Taxonomy {
        Taxonomy::new(&TreeDef::standard_goods()).expect("bundled goods tree is valid")
    }

    pub fn find(&self, name: &str) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.name == name).map(|i| i as NodeId)
    }

    pub fn name(&self, id: NodeId) -> &str {
        &self.nodes[id as usize].name
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id as usize].parent
    }

    pub fn depth(&self, id: NodeId) -> u8 {
        self.nodes[id as usize].depth
    }

    pub fn unit(&self, id: NodeId) -> Option<&str> {
        self.nodes[id as usize].unit.as_deref()
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        (id + 1..self.nodes[id as usize].end).filter(move |&c| self.nodes[c as usize].parent == Some(id))
    }

    /// Whether `node` is `ancestor` or below it.
    pub fn is_under(&self, node: NodeId, ancestor: NodeId) -> bool {
        (ancestor..self.nodes[ancestor as usize].end).contains(&node)
    }

    /// The leaves at or below `id`, in tree order.
    pub fn leaves_under(&self, id: NodeId) -> Vec<NodeId> {
        (id..self.nodes[id as usize].end).filter(|&c| self.nodes[c as usize].end == c + 1).collect()
    }

    /// The top-level classes: the root's children.
    pub fn classes(&self) -> Vec<NodeId> {
        self.children(0).collect()
    }

    /// The place among the classes of the class `id` belongs to; `None` for the root.
    pub fn class_of(&self, id: NodeId) -> Option<usize> {
        let mut n = id;
        while self.depth(n) > 1 {
            n = self.parent(n)?;
        }
        (self.depth(n) == 1).then(|| self.classes().iter().position(|&c| c == n).expect("a depth-1 node is a class"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tree_has_eight_goods_and_coins() {
        let t = Taxonomy::standard_goods();
        let names = |v: Vec<NodeId>| v.into_iter().map(|n| t.name(n).to_string()).collect::<Vec<_>>();
        assert_eq!(names(t.classes()), ["food", "raw", "manufactured", "money"]);
        assert_eq!(names(t.leaves_under(0)), ["grain", "fish", "livestock", "timber", "stone", "ore", "cloth", "tools", "coin"]);
        // Money came last, so the goods before it kept their numbers.
        assert_eq!((t.find("tools"), t.find("money"), t.find("coin")), (Some(11), Some(12), Some(13)));
        let coin = t.find("coin").unwrap();
        assert_eq!((t.class_of(coin), t.unit(coin)), (Some(3), Some("coins")));
        let fish = t.find("fish").unwrap();
        assert_eq!(t.parent(fish), t.find("food"));
        assert_eq!((t.depth(fish), t.class_of(fish), t.unit(fish)), (2, Some(0), Some("rations")));
        assert_eq!((t.class_of(t.find("tools").unwrap()), t.class_of(0)), (Some(2), None));
        assert!(t.is_under(fish, 0) && !t.is_under(fish, t.find("raw").unwrap()));
        assert_eq!((t.unit(t.find("ore").unwrap()), t.unit(t.find("tools").unwrap())), (Some("cartloads"), Some("sets")));
        assert_eq!(t.unit(t.find("manufactured").unwrap()), None);
    }

    #[test]
    fn bad_trees_are_refused() {
        let tree = |text: &str| Taxonomy::new(&TreeDef::from_toml(text).unwrap()).unwrap_err();
        assert!(tree("root = 'g'\n[children]\ng = ['a', 'b']\nb = ['a']").contains("a appears twice"));
        assert!(tree("root = 'g'\n[children]\ng = ['a']\nx = ['y']").contains("x is not under g"));
        assert!(tree("root = 'g'\n[children]\ng = ['a']\n[units]\nq = 'kg'").contains("q is not under g"));
        assert!(tree("root = 'g'\n[children]").contains("no classes"));
        // A loop names a node twice.
        assert!(tree("root = 'g'\n[children]\ng = ['a']\na = ['g']").contains("g appears twice"));
    }
}
