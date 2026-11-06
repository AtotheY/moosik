use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LibraryItem {
    Folder(String),
    Playlist(String),
}

/// Local references only. A playlist can be both filed and pinned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistLayout {
    pub folders: BTreeMap<String, BTreeSet<String>>,
    // Legacy folder keys are already unique. Keep them as IDs when loading a flat library.
    titles: BTreeMap<String, String>,
    parents: BTreeMap<String, String>,
    pub pins: BTreeSet<String>,
    item_order: Vec<LibraryItem>,
    pub folder_order: Vec<String>,
    pub expanded_folders: BTreeSet<String>,
}

impl PlaylistLayout {
    /// Older profiles used map order. Keep that order until a folder is moved.
    pub fn ordered_folders(&self) -> Vec<&String> {
        let mut seen = std::collections::HashSet::new();
        self.item_order
            .iter()
            .filter_map(|item| {
                if let LibraryItem::Folder(id) = item {
                    Some(id)
                } else {
                    None
                }
            })
            .chain(self.folder_order.iter())
            .chain(self.folders.keys())
            .filter(|name| self.folders.contains_key(*name) && seen.insert(name.as_str()))
            .collect()
    }

    pub fn ranks(&self) -> HashMap<&LibraryItem, usize> {
        self.item_order
            .iter()
            .enumerate()
            .map(|(i, item)| (item, i))
            .collect()
    }

    pub fn item_parent(&self, item: &LibraryItem) -> Option<&str> {
        match item {
            LibraryItem::Folder(id) => self.parent(id),
            LibraryItem::Playlist(id) => self.playlist_parent(id),
        }
    }

    /// Materialize sibling order only when the user puts an item at a specific slot.
    /// Include saved IDs that the remote library has not paged back in yet.
    pub fn insert_beside(
        &mut self,
        moving: &[LibraryItem],
        target: LibraryItem,
        after: bool,
        playlists: &[String],
    ) {
        if moving.is_empty() || moving.contains(&target) {
            return;
        }
        let parent = self.item_parent(&target).map(str::to_owned);
        if moving.iter().any(
            |item| matches!(item,LibraryItem::Folder(id) if !self.can_place(id,parent.as_deref())),
        ) {
            return;
        }
        let mut siblings: Vec<_> = self
            .item_order
            .iter()
            .filter(|item| self.item_parent(item) == parent.as_deref())
            .cloned()
            .collect();
        let mut seen: std::collections::HashSet<_> = siblings.iter().cloned().collect();
        let folders = self
            .siblings(parent.as_deref())
            .into_iter()
            .cloned()
            .map(LibraryItem::Folder);
        let songs = playlists
            .iter()
            .filter(|id| {
                self.playlist_parent(id) == parent.as_deref()
                    && (parent.is_some() || !self.pins.contains(*id))
            })
            .cloned()
            .map(LibraryItem::Playlist);
        siblings.extend(
            folders
                .chain(songs)
                .filter(|item| seen.insert(item.clone())),
        );
        siblings.retain(|item| !moving.contains(item));
        let Some(i) = siblings.iter().position(|item| item == &target) else {
            return;
        };
        for item in moving {
            match item {
                LibraryItem::Folder(id) => {
                    self.reparent(id, parent.as_deref());
                }
                LibraryItem::Playlist(id) => self.move_to(&[id.clone()].into(), parent.as_deref()),
            }
        }
        siblings.splice(
            i + usize::from(after)..i + usize::from(after),
            moving.iter().cloned(),
        );
        seen.extend(moving.iter().cloned());
        self.item_order.retain(|item| !seen.contains(item));
        self.item_order.extend(siblings);
    }

    pub fn forget_playlist(&mut self, id: &str) {
        self.item_order
            .retain(|item| !matches!(item,LibraryItem::Playlist(pid) if pid==id));
    }

    pub fn title<'a>(&'a self, id: &'a str) -> &'a str {
        self.titles.get(id).map(String::as_str).unwrap_or(id)
    }

    pub fn parent(&self, id: &str) -> Option<&str> {
        self.parents
            .get(id)
            .filter(|p| self.folders.contains_key(*p))
            .map(String::as_str)
    }

    pub fn playlist_parent(&self, id: &str) -> Option<&str> {
        self.folders
            .iter()
            .find(|(_, ids)| ids.contains(id))
            .map(|(p, _)| p.as_str())
    }

    pub fn ancestors<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::successors(Some(id), |id| self.parent(id)).take(self.folders.len())
    }

    pub fn path(&self, id: &str) -> String {
        let mut path: Vec<_> = self.ancestors(id).map(|id| self.title(id)).collect();
        path.reverse();
        path.join(" › ")
    }

    pub fn siblings(&self, parent: Option<&str>) -> Vec<&String> {
        self.ordered_folders()
            .into_iter()
            .filter(|id| self.parent(id) == parent)
            .collect()
    }

    /// Preorder traversal, respecting expansion in the sidebar and walking every node in pickers.
    pub fn tree(&self, expanded_only: bool) -> Vec<(&String, usize)> {
        let mut children = HashMap::<Option<&str>, Vec<&String>>::new();
        for folder in self.ordered_folders().into_iter().rev() {
            children
                .entry(self.parent(folder))
                .or_default()
                .push(folder);
        }
        let mut stack: Vec<_> = children
            .remove(&None)
            .unwrap_or_default()
            .into_iter()
            .map(|id| (id, 0))
            .collect();
        let mut rows = Vec::new();
        while let Some((id, depth)) = stack.pop() {
            rows.push((id, depth));
            if (!expanded_only || self.expanded_folders.contains(id))
                && let Some(kids) = children.remove(&Some(id.as_str()))
            {
                stack.extend(kids.into_iter().map(|child| (child, depth + 1)));
            }
        }
        rows
    }

    pub fn description(&self, id: &str) -> String {
        let folders = self.parents.values().filter(|p| p.as_str() == id).count();
        let playlists = self.folders[id].len();
        match (folders, playlists) {
            (0, 0) => "Folder · Empty".into(),
            (0, 1) => "Folder · 1 playlist".into(),
            (0, n) => format!("Folder · {n} playlists"),
            (1, 0) => "Folder · 1 subfolder".into(),
            (n, 0) => format!("Folder · {n} subfolders"),
            (f, p) => format!("Folder · {} items", f + p),
        }
    }

    pub fn create_folder(&mut self, title: &str, parent: Option<&str>) -> String {
        let id = (1..)
            .map(|i| format!("folder:{i}"))
            .find(|id| !self.folders.contains_key(id))
            .unwrap();
        self.folder_order = self.ordered_folders().into_iter().cloned().collect();
        self.folder_order.push(id.clone());
        self.folders.insert(id.clone(), Default::default());
        self.rename(&id, title);
        self.reparent(&id, parent);
        id
    }

    pub fn rename(&mut self, id: &str, title: &str) {
        self.titles.insert(id.to_owned(), title.to_owned());
    }

    pub fn can_place(&self, id: &str, parent: Option<&str>) -> bool {
        self.folders.contains_key(id)
            && parent.is_none_or(|p| {
                self.folders.contains_key(p) && !self.ancestors(p).any(|ancestor| ancestor == id)
            })
    }

    pub fn reparent(&mut self, id: &str, parent: Option<&str>) -> bool {
        if !self.can_place(id, parent) {
            return false;
        }
        if self.parent(id) != parent {
            self.item_order
                .retain(|item| !matches!(item,LibraryItem::Folder(f) if f==id));
        }
        self.parents.remove(id);
        if let Some(parent) = parent {
            self.parents.insert(id.to_owned(), parent.to_owned());
            let ancestors: Vec<_> = self.ancestors(parent).map(str::to_owned).collect();
            self.expanded_folders.extend(ancestors);
        }
        true
    }

    pub fn ungroup(&mut self, id: &str) {
        let parent = self.parent(id).map(str::to_owned);
        let children: Vec<_> = self.siblings(Some(id)).into_iter().cloned().collect();
        for child in children {
            self.reparent(&child, parent.as_deref());
        }
        if let Some(ids) = self.folders.remove(id) {
            self.move_to(&ids, parent.as_deref());
        }
        self.parents.remove(id);
        self.titles.remove(id);
        self.item_order
            .retain(|item| !matches!(item,LibraryItem::Folder(f) if f==id));
        self.folder_order.retain(|n| n != id);
        self.expanded_folders.remove(id);
    }

    pub fn move_out(&mut self, ids: &BTreeSet<String>) {
        let moves: Vec<_> = ids
            .iter()
            .filter_map(|id| {
                let folder = self.playlist_parent(id)?;
                Some((id.clone(), self.parent(folder).map(str::to_owned)))
            })
            .collect();
        for (id, parent) in moves {
            self.move_to(&[id].into(), parent.as_deref());
        }
    }

    pub fn is_filed(&self, id: &str) -> bool {
        self.folders.values().any(|ids| ids.contains(id))
    }

    pub fn move_to(&mut self, ids: &BTreeSet<String>, folder: Option<&str>) {
        self.item_order
            .retain(|item| !matches!(item, LibraryItem::Playlist(id) if ids.contains(id)));
        for members in self.folders.values_mut() {
            members.retain(|id| !ids.contains(id));
        }
        if let Some(name) = folder {
            self.folders
                .entry(name.to_owned())
                .or_default()
                .extend(ids.iter().cloned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_folders_keep_their_members_through_nesting_renaming_and_ungrouping() {
        let mut tree: PlaylistLayout = serde_json::from_str(r#"{"folders":{"Imports":["one"],"Today":["two"]},"pins":["one"],"folder_order":["Today","Imports"]}"#).unwrap();
        let child = tree.create_folder("Mixes", Some("Imports"));
        let other = tree.create_folder("Mixes", Some("Today"));
        assert_ne!(child, other);
        tree.move_to(&["one".into()].into(), Some(&child));
        assert!(tree.reparent("Imports", Some("Today")));
        tree.insert_beside(
            &[LibraryItem::Folder("Today".into())],
            LibraryItem::Folder(child.clone()),
            false,
            &[],
        );
        assert!(tree.parent("Today").is_none());
        assert!(!tree.reparent("Today", Some(&child)));
        tree.rename("Imports", "Spotify");
        assert_eq!(tree.path(&child), "Today › Spotify › Mixes");
        tree.move_out(&["one".into()].into());
        assert_eq!(tree.playlist_parent("one"), Some("Imports"));
        tree.ungroup("Imports");
        assert_eq!(tree.parent(&child), Some("Today"));
        assert_eq!(tree.playlist_parent("one"), Some("Today"));
        tree.move_to(&["one".into()].into(), None);
        assert!(tree.pins.contains("one") && !tree.is_filed("one"));
        let saved: PlaylistLayout =
            serde_json::from_value(serde_json::to_value(&tree).unwrap()).unwrap();
        assert_eq!(saved, tree);
        tree.expanded_folders.clear();
        assert_eq!(tree.tree(true).len(), 1);
        assert_eq!(tree.tree(false).len(), 3);
    }
}
