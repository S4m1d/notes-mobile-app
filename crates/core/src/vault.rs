//! File operations inside a vault. A vault is a plain directory tree of
//! markdown files; nothing here keeps any extra metadata in it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{err, Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
}

/// Checks a single file or directory name typed by the user or received from a peer.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 200 {
        return err("name must be 1-200 characters long");
    }
    if name.starts_with('.') {
        return err("name must not start with a dot");
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
        return err("name contains invalid characters");
    }
    Ok(())
}

pub fn validate_vault_name(name: &str) -> Result<()> {
    validate_name(name)?;
    if name.len() > 64 || !name.chars().all(|c| c.is_alphanumeric() || " _-".contains(c)) {
        return err("vault name may contain only letters, digits, spaces, '_' and '-' (max 64)");
    }
    Ok(())
}

pub fn is_note_name(name: &str) -> bool {
    name.len() > 3 && name.to_ascii_lowercase().ends_with(".md")
}

/// Validates a '/'-separated path relative to the vault root. Rejects anything
/// that could escape the vault or touch hidden files.
pub fn validate_rel_path(rel: &str) -> Result<()> {
    if rel.is_empty() {
        return err("empty path");
    }
    for part in rel.split('/') {
        validate_name(part).map_err(|e| Error(format!("bad path '{rel}': {e}")))?;
    }
    Ok(())
}

/// Resolves a validated relative path against the vault root. An empty path is the root itself.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    validate_rel_path(rel)?;
    Ok(rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part)))
}

pub fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("")
}

/// Lists visible sub-directories and notes of `dir`: directories first, then
/// notes, both sorted case-insensitively.
pub fn list(root: &Path, dir: &str) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for entry in fs::read_dir(resolve(root, dir)?)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else { continue };
        let file_type = entry.file_type()?;
        if name.starts_with('.') || file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            items.push(Item { name, is_dir: true });
        } else if file_type.is_file() && is_note_name(&name) {
            items.push(Item { name, is_dir: false });
        }
    }
    items.sort_by_key(|i| (!i.is_dir, i.name.to_lowercase()));
    Ok(items)
}

/// A search result: the path of a note or directory relative to the vault root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rel: String,
    pub is_dir: bool,
}

/// Finds notes and directories anywhere in the vault whose name contains
/// `query`, ignoring case. Results are sorted by path, at most `limit` of them.
pub fn search(root: &Path, query: &str, limit: usize) -> Result<Vec<Hit>> {
    let query = query.trim().to_lowercase();
    let mut hits = Vec::new();
    if query.is_empty() {
        return Ok(hits);
    }
    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        for item in list(root, &dir)? {
            let rel = join_rel(&dir, &item.name);
            if item.name.to_lowercase().contains(&query) {
                hits.push(Hit { rel: rel.clone(), is_dir: item.is_dir });
            }
            if item.is_dir {
                pending.push(rel);
            }
        }
    }
    hits.sort_by_key(|h| h.rel.to_lowercase());
    hits.truncate(limit);
    Ok(hits)
}

pub fn create_dir(root: &Path, parent: &str, name: &str) -> Result<String> {
    let name = name.trim();
    validate_name(name)?;
    let rel = join_rel(parent, name);
    let path = resolve(root, &rel)?;
    if path.exists() {
        return err(format!("'{name}' already exists"));
    }
    fs::create_dir(path)?;
    Ok(rel)
}

/// A note with this name is the template for new notes in its directory.
pub const TEMPLATE_NAME: &str = "template.md";

/// Creates a note; `.md` is appended to the name when missing. The note starts
/// with the content of `template.md` from the same directory when there is
/// one, and empty otherwise.
pub fn create_note(root: &Path, parent: &str, name: &str) -> Result<String> {
    let name = name.trim();
    let name = if is_note_name(name) { name.to_string() } else { format!("{name}.md") };
    validate_name(name.trim_end_matches(".md"))?;
    validate_name(&name)?;
    let rel = join_rel(parent, &name);
    let path = resolve(root, &rel)?;
    if path.exists() {
        return err(format!("'{name}' already exists"));
    }
    let template = resolve(root, &join_rel(parent, TEMPLATE_NAME))?;
    let content = match fs::symlink_metadata(&template) {
        Ok(meta) if meta.is_file() => fs::read(&template)?,
        _ => Vec::new(),
    };
    fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(&content)?;
    Ok(rel)
}

/// Deletes a note or a directory with everything in it.
pub fn delete(root: &Path, rel: &str) -> Result<()> {
    validate_rel_path(rel)?;
    let path = resolve(root, rel)?;
    if fs::symlink_metadata(&path)?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn read_note(root: &Path, rel: &str) -> Result<String> {
    validate_rel_path(rel)?;
    let bytes = fs::read(resolve(root, rel)?)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn write_note(root: &Path, rel: &str, text: &str) -> Result<()> {
    validate_rel_path(rel)?;
    write_atomic(&resolve(root, rel)?, text.as_bytes())
}

/// Writes through a hidden temporary file and renames it over the target, so
/// a crash never leaves a half-written note behind.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or(Error("bad file name".into()))?;
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        for bad in ["", "..", "../x.md", "a/../b.md", "/etc/passwd", "a//b.md", ".git/config", "a/.hidden.md", "a\\b.md"] {
            assert!(validate_rel_path(bad).is_err(), "{bad} should be rejected");
        }
        for good in ["a.md", "dir/sub/note.md", "with space/note 1.md"] {
            assert!(validate_rel_path(good).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn create_list_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_dir(root, "", "work").unwrap();
        assert_eq!(create_note(root, "work", "todo").unwrap(), "work/todo.md");
        assert_eq!(create_note(root, "", "Inbox.md").unwrap(), "Inbox.md");
        assert!(create_note(root, "", "Inbox").is_err());
        fs::write(root.join("image.png"), b"x").unwrap();
        fs::write(root.join(".hidden.md"), b"x").unwrap();

        let items = list(root, "").unwrap();
        assert_eq!(
            items,
            vec![Item { name: "work".into(), is_dir: true }, Item { name: "Inbox.md".into(), is_dir: false }]
        );

        write_note(root, "work/todo.md", "- [ ] ship\n").unwrap();
        assert_eq!(read_note(root, "work/todo.md").unwrap(), "- [ ] ship\n");
        assert_eq!(list(root, "work").unwrap().len(), 1);

        delete(root, "work").unwrap();
        assert_eq!(list(root, "").unwrap().len(), 1);
    }

    #[test]
    fn new_note_starts_with_the_directory_template() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_dir(root, "", "journal").unwrap();
        create_dir(root, "journal", "old").unwrap();
        write_note(root, "journal/template.md", "# Day\n\n- [ ] plan\n").unwrap();

        create_note(root, "journal", "monday").unwrap();
        assert_eq!(read_note(root, "journal/monday.md").unwrap(), "# Day\n\n- [ ] plan\n");
        // The template applies to its own directory only.
        create_note(root, "journal/old", "sunday").unwrap();
        assert_eq!(read_note(root, "journal/old/sunday.md").unwrap(), "");
        create_note(root, "", "inbox").unwrap();
        assert_eq!(read_note(root, "inbox.md").unwrap(), "");
        // The template itself is an ordinary note and is never overwritten.
        assert!(create_note(root, "journal", "template").is_err());
        assert_eq!(create_note(root, "", "template").unwrap(), "template.md");
        assert_eq!(read_note(root, "template.md").unwrap(), "");
    }

    #[test]
    fn search_matches_names_in_the_whole_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        create_dir(root, "", "Projects").unwrap();
        create_dir(root, "Projects", "rust").unwrap();
        create_note(root, "Projects/rust", "Trust notes").unwrap();
        create_note(root, "", "shopping").unwrap();
        fs::create_dir(root.join(".rusty")).unwrap();
        fs::write(root.join("rust.txt"), b"x").unwrap();

        let hits = search(root, " RUST ", 10).unwrap();
        assert_eq!(
            hits,
            vec![
                Hit { rel: "Projects/rust".into(), is_dir: true },
                Hit { rel: "Projects/rust/Trust notes.md".into(), is_dir: false },
            ]
        );
        assert_eq!(search(root, "rust", 1).unwrap().len(), 1);
        assert!(search(root, "", 10).unwrap().is_empty());
        assert!(search(root, "nothing", 10).unwrap().is_empty());
    }
}
