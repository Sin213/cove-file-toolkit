use serde::Serialize;
use std::collections::HashMap;
use string_interner::{DefaultBackend, StringInterner, Symbol};

pub type SymbolId = u32;

const FLAG_IS_DIR: u8 = 1;

/// Join a parent directory and a leaf name into a full path string. Skips
/// the platform separator when `parent` already ends with one — otherwise
/// filesystem roots like `/` or `C:\` would produce `//home` / `C:\\Users`,
/// which then disagree with user-typed paths in path-mode search and sort.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        return name.to_string();
    }
    let last = parent.chars().last();
    let already_separated = matches!(last, Some('/')) || last == Some(std::path::MAIN_SEPARATOR);
    if already_separated {
        format!("{parent}{name}")
    } else {
        format!("{}{}{}", parent, std::path::MAIN_SEPARATOR, name)
    }
}

/// Per-root identity stored inside the index. `id` matches the configured
/// IndexRoot's stable id; `path` is the configured path; `canonical_path`
/// is the resolved path at scan time (or the configured path when the root
/// was unavailable). Carries through cache and search results so entries
/// can be attributed, preserved, removed, or filtered by root.
#[derive(Serialize, Clone, Debug)]
pub struct RootMeta {
    pub id: String,
    pub path: String,
    pub canonical_path: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct SearchHit {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub root_id: String,
    pub root_path: String,
}

pub struct FileIndex {
    pub interner: StringInterner<DefaultBackend>,
    pub parent_id: Vec<SymbolId>,
    pub name_id: Vec<SymbolId>,
    pub root_sym: Vec<SymbolId>,
    pub size: Vec<u64>,
    pub mtime: Vec<i64>,
    pub flags: Vec<u8>,
    /// Map root_id -> metadata. Populated from the walker / cache.
    pub roots: HashMap<String, RootMeta>,
}

impl Default for FileIndex {
    fn default() -> Self {
        Self {
            interner: StringInterner::new(),
            parent_id: Vec::new(),
            name_id: Vec::new(),
            root_sym: Vec::new(),
            size: Vec::new(),
            mtime: Vec::new(),
            flags: Vec::new(),
            roots: HashMap::new(),
        }
    }
}

impl FileIndex {
    pub fn len(&self) -> usize {
        self.name_id.len()
    }

    pub fn push(
        &mut self,
        parent: &str,
        name: &str,
        size: u64,
        mtime: i64,
        is_dir: bool,
        root_id: &str,
    ) {
        let pid = self.interner.get_or_intern(parent).to_usize() as SymbolId;
        let nid = self.interner.get_or_intern(name).to_usize() as SymbolId;
        let rid = self.interner.get_or_intern(root_id).to_usize() as SymbolId;
        self.parent_id.push(pid);
        self.name_id.push(nid);
        self.root_sym.push(rid);
        self.size.push(size);
        self.mtime.push(mtime);
        self.flags.push(if is_dir { FLAG_IS_DIR } else { 0 });
    }

    pub fn upsert_root(&mut self, meta: RootMeta) {
        self.roots.insert(meta.id.clone(), meta);
    }

    pub fn get_name(&self, idx: usize) -> Option<&str> {
        let sym = string_interner::Symbol::try_from_usize(self.name_id[idx] as usize)?;
        self.interner.resolve(sym)
    }

    pub fn get_parent(&self, idx: usize) -> Option<&str> {
        let sym = string_interner::Symbol::try_from_usize(self.parent_id[idx] as usize)?;
        self.interner.resolve(sym)
    }

    pub fn get_root_id(&self, idx: usize) -> Option<&str> {
        let sym = string_interner::Symbol::try_from_usize(self.root_sym[idx] as usize)?;
        self.interner.resolve(sym)
    }

    pub fn get_hit(&self, idx: usize) -> SearchHit {
        let name = self.get_name(idx).unwrap_or("").to_string();
        let parent = self.get_parent(idx).unwrap_or("").to_string();
        let path = join_path(&parent, &name);
        let root_id = self.get_root_id(idx).unwrap_or("").to_string();
        let root_path = self
            .roots
            .get(&root_id)
            .map(|r| r.path.clone())
            .unwrap_or_default();
        SearchHit {
            name,
            path,
            is_dir: self.flags[idx] & FLAG_IS_DIR != 0,
            size: self.size[idx],
            mtime: self.mtime[idx],
            root_id,
            root_path,
        }
    }

    pub fn is_dir(&self, idx: usize) -> bool {
        self.flags[idx] & FLAG_IS_DIR != 0
    }

    /// Build a fresh FileIndex from `old` (entries owned by roots NOT in
    /// `replaced_roots`) plus `new_index` in full. Used after a scan to
    /// preserve entries for failed/missing roots while replacing entries
    /// for roots that were just rescanned.
    pub fn merge_replacing(
        old: &FileIndex,
        new_index: FileIndex,
        replaced_roots: &std::collections::HashSet<String>,
    ) -> FileIndex {
        let mut out = FileIndex::default();
        // Carry over metadata for roots we are NOT replacing.
        for (id, meta) in &old.roots {
            if !replaced_roots.contains(id) {
                out.upsert_root(meta.clone());
            }
        }
        // Replacement roots' metadata wins.
        for (_id, meta) in &new_index.roots {
            out.upsert_root(meta.clone());
        }
        // Preserve old entries owned by roots not being replaced.
        for i in 0..old.len() {
            let rid = old.get_root_id(i).unwrap_or("").to_string();
            if replaced_roots.contains(&rid) {
                continue;
            }
            let parent = old.get_parent(i).unwrap_or("").to_string();
            let name = old.get_name(i).unwrap_or("").to_string();
            out.push(
                &parent,
                &name,
                old.size[i],
                old.mtime[i],
                old.is_dir(i),
                &rid,
            );
        }
        // Append all new entries.
        for i in 0..new_index.len() {
            let rid = new_index.get_root_id(i).unwrap_or("").to_string();
            let parent = new_index.get_parent(i).unwrap_or("").to_string();
            let name = new_index.get_name(i).unwrap_or("").to_string();
            out.push(
                &parent,
                &name,
                new_index.size[i],
                new_index.mtime[i],
                new_index.is_dir(i),
                &rid,
            );
        }
        out
    }
}
