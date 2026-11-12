//! Config writer: renders a [`Draft`] to TOML, updating an existing file in place.
//!
//! In short:
//! - target missing: a new document from the draft (with `only_changed`, keys equal to their
//!   documented default are omitted);
//! - target exists: parsed with `toml_edit`; only keys the draft marks *touched* change, every
//!   other byte stays, and unknown keys survive;
//! - comment lines starting with `##` are generated and rewritten to the chosen
//!   [`CommentLevel`] on every save; any other comment line belongs to the user.
//!
//! [`render`] is pure; [`save`] adds the backup, the atomic replace and the post-condition check.

pub(crate) mod text;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use audigma_dsp::PcmRateHz;
use audigma_settings::meta::{Catalog, Field, FieldKind, MetaError};
use audigma_settings::{Settings, SettingsError};
use toml_edit::{DocumentMut, Item, Table, TableLike};

pub use text::CommentLevel;

use crate::draft::{Draft, DraftError};

/// What to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Omit keys whose value equals the documented default (and remove touched keys that were
    /// reset to it). When false they are written explicitly.
    pub only_changed: bool,
    pub comment_level: CommentLevel,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            only_changed: true,
            comment_level: CommentLevel::Brief,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error(transparent)]
    Draft(#[from] DraftError),
    #[error("the draft is not valid: {0}")]
    InvalidDraft(SettingsError),
    #[error("existing config is not valid TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("internal error: the written file is not a valid config: {0}")]
    PostCondition(SettingsError),
    #[error(
        "the written file would resolve differently from the draft at {rate} Hz \
         (the file has keys that differ from the draft; reload it): file {file}, draft {draft}"
    )]
    PlanMismatch {
        rate: u32,
        file: String,
        draft: String,
    },
}

/// Outcome of [`save`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    pub path: PathBuf,
    /// The file did not exist before.
    pub created: bool,
    /// The saved text equals the file's; nothing was written.
    pub unchanged: bool,
    /// The copy of the previous file, if one was made.
    pub backup: Option<PathBuf>,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> WriteError + '_ {
    move |source| WriteError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Renders the draft. `existing` is the target's current text, or `None` for a new file.
/// Does not check the post-condition (see [`verify`]).
pub fn render(
    draft: &Draft,
    existing: Option<&str>,
    opts: &WriteOptions,
) -> Result<String, WriteError> {
    let catalog = Catalog::load()?;
    draft
        .settings()
        .validate()
        .map_err(WriteError::InvalidDraft)?;
    let values = draft.values()?;
    let in_place = existing.is_some();
    let mut doc: DocumentMut = match existing {
        Some(t) => t.parse()?,
        None => {
            let mut d = DocumentMut::new();
            d["schema_version"] = toml_edit::value(i64::from(draft.settings().schema_version));
            d
        }
    };

    let mut emptied: BTreeSet<String> = BTreeSet::new();
    for f in catalog
        .fields()
        .iter()
        .filter(|f| f.kind == FieldKind::Value)
    {
        if in_place && !draft.is_touched(&f.path) {
            continue;
        }
        let have = lookup(&values, &f.path);
        match have {
            Some(v) if !(opts.only_changed && is_default(v, f)) => set_key(&mut doc, &f.path, v),
            _ => {
                if remove_key(&mut doc, &f.path) {
                    emptied.insert(parent(&f.path).to_string());
                }
            }
        }
    }
    prune_empty(doc.as_table_mut(), &emptied);
    comments(&mut doc, &catalog, opts.comment_level);
    Ok(doc.to_string())
}

/// Checks that `text` is a valid config that resolves like the draft.
pub fn verify(draft: &Draft, text: &str) -> Result<Settings, WriteError> {
    let written = Settings::from_toml_str(text).map_err(WriteError::PostCondition)?;
    for rate in [
        44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 705_600, 768_000,
    ] {
        let show = |s: &Settings| match s.explain_for_input_rate(PcmRateHz(rate)) {
            Ok(p) => format!("{p:?}"),
            Err(e) => format!("error: {e}"),
        };
        let (file, want) = (show(&written), show(draft.settings()));
        if file != want {
            return Err(WriteError::PlanMismatch {
                rate,
                file,
                draft: want,
            });
        }
    }
    Ok(written)
}

/// A rendered and verified save, not yet written: what [`save`] writes and what a preview shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The file the text would replace (symlinks resolved).
    pub target: PathBuf,
    /// The target's current text, `None` when it does not exist.
    pub existing: Option<String>,
    /// The full new text, already checked by [`verify`].
    pub text: String,
}

impl Prepared {
    /// The text equals the file's, so a save writes nothing.
    pub fn unchanged(&self) -> bool {
        self.existing.as_deref() == Some(self.text.as_str())
    }
}

/// Reads the target (if any), renders the draft and checks the post-condition. Writes nothing.
/// [`save`] and previews both go through this, so a preview is exactly what a save writes.
pub fn prepare(draft: &Draft, path: &Path, opts: &WriteOptions) -> Result<Prepared, WriteError> {
    // Write through a symlink instead of replacing it.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = match fs::read_to_string(&target) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(&target)(e)),
    };
    let text = render(draft, existing.as_deref(), opts)?;
    verify(draft, &text)?;
    Ok(Prepared {
        target,
        existing,
        text,
    })
}

/// Renders, verifies and writes the draft to `path`.
///
/// An existing file is updated in place and a timestamped (UTC) backup
/// `<file>.bak-YYYYMMDD-HHMMSS` is made first; the new text goes to a temporary file in the same
/// directory that then replaces the target atomically. A save that changes nothing writes
/// nothing. Parent directories are created.
pub fn save(draft: &Draft, path: &Path, opts: &WriteOptions) -> Result<SaveReport, WriteError> {
    let prepared = prepare(draft, path, opts)?;
    let created = prepared.existing.is_none();
    let unchanged = prepared.unchanged();
    let Prepared {
        target, text: out, ..
    } = prepared;
    if unchanged {
        return Ok(SaveReport {
            path: target,
            created,
            unchanged: true,
            backup: None,
        });
    }
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;

    let backup = if created {
        None
    } else {
        let b = backup_path(&target);
        fs::copy(&target, &b).map_err(io_err(&b))?;
        Some(b)
    };

    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(io_err(&dir))?;
    tmp.write_all(out.as_bytes()).map_err(io_err(&target))?;
    tmp.as_file().sync_all().map_err(io_err(&target))?;
    if let Ok(meta) = fs::metadata(&target) {
        let _ = fs::set_permissions(tmp.path(), meta.permissions());
    }
    tmp.persist(&target).map_err(|e| io_err(&target)(e.error))?;
    Ok(SaveReport {
        path: target,
        created,
        unchanged: false,
        backup,
    })
}

fn backup_path(target: &Path) -> PathBuf {
    let now = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".into());
    let mut candidate = target.with_file_name(format!("{name}.bak-{stamp}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = target.with_file_name(format!("{name}.bak-{stamp}-{n}"));
        n += 1;
    }
    candidate
}

// ---------------------------------------------------------------------------------------------
// Values

fn parent(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(p, _)| p)
}

fn lookup<'a>(table: &'a toml::Table, path: &str) -> Option<&'a toml::Value> {
    let mut parts = path.split('.');
    let mut cur = table.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?;
    }
    Some(cur)
}

/// Numbers compare by value, so `0` equals `0.0`.
fn same_json(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => x.as_f64() == y.as_f64(),
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q)),
        _ => a == b,
    }
}

fn is_default(v: &toml::Value, f: &Field) -> bool {
    let (Some(d), Ok(j)) = (&f.default, serde_json::to_value(v)) else {
        return false;
    };
    same_json(&j, d)
}

fn to_edit(v: &toml::Value) -> toml_edit::Value {
    match v {
        toml::Value::String(s) => s.as_str().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(x) => (*x).into(),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Datetime(d) => d
            .to_string()
            .parse()
            .unwrap_or_else(|_| d.to_string().as_str().into()),
        toml::Value::Array(a) => a.iter().map(to_edit).collect::<toml_edit::Array>().into(),
        toml::Value::Table(t) => {
            let mut it = toml_edit::InlineTable::new();
            for (k, x) in t {
                it.insert(k, to_edit(x));
            }
            it.into()
        }
    }
}

/// The value of an existing item, for comparison with the draft's.
fn old_value(item: &Item) -> Option<toml::Value> {
    let text = item.as_value()?.to_string();
    let t: toml::Table = format!("v = {}", text.trim()).parse().ok()?;
    t.get("v").cloned()
}

fn same_value(a: &toml::Value, b: &toml::Value) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(x), Ok(y)) => same_json(&x, &y),
        _ => false,
    }
}

fn new_section() -> Item {
    let mut t = Table::new();
    t.set_implicit(true);
    t.decor_mut().set_prefix("\n");
    Item::Table(t)
}

fn set_key(doc: &mut DocumentMut, path: &str, v: &toml::Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((leaf, parents)) = parts.split_last() else {
        return;
    };
    let mut cur: &mut dyn TableLike = doc.as_table_mut();
    for p in parents {
        if cur.get(p).is_none() {
            cur.insert(p, new_section());
        }
        match cur.get_mut(p).and_then(Item::as_table_like_mut) {
            Some(next) => cur = next,
            None => return,
        }
    }
    match cur.get_mut(leaf) {
        Some(item) => {
            if old_value(item).is_some_and(|o| same_value(&o, v)) {
                return;
            }
            let mut new = to_edit(v);
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            cur.insert(leaf, Item::Value(to_edit(v)));
        }
    }
    // A section that gained a key must print its header.
    if let Some(t) = doc.get_mut(parts[0]).and_then(Item::as_table_mut) {
        reveal(t);
    }
}

/// Un-implicits every table on the way that holds a key-value.
fn reveal(t: &mut Table) {
    if t.iter().any(|(_, i)| i.is_value()) {
        t.set_implicit(false);
    }
    for (_, i) in t.iter_mut() {
        if let Some(sub) = i.as_table_mut() {
            reveal(sub);
        }
    }
}

/// Removes `path`. The user's own comment lines above the key are not lost: they move to the
/// next entry of the table, or to the end of the document when it was the last one.
fn remove_key(doc: &mut DocumentMut, path: &str) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((leaf, parents)) = parts.split_last() else {
        return false;
    };
    let mut cur: &mut Table = doc.as_table_mut();
    for p in parents {
        match cur.get_mut(p).and_then(Item::as_table_mut) {
            Some(next) => cur = next,
            // Inline or dotted: nothing to keep, plain removal.
            None => return remove_plain(doc, path),
        }
    }
    let names: Vec<String> = cur.iter().map(|(k, _)| k.to_string()).collect();
    let Some(idx) = names.iter().position(|n| n == leaf) else {
        return false;
    };
    let notes: String = cur
        .key(leaf)
        .map(|k| prefix_of(Some(k.leaf_decor())))
        .unwrap_or_default()
        .split_inclusive('\n')
        .filter(|l| l.trim_start().starts_with('#') && !is_generated(l))
        .collect();
    cur.remove(leaf);
    if notes.is_empty() {
        return true;
    }
    match names.get(idx + 1) {
        Some(next) => {
            if cur.get(next).is_some_and(Item::is_table) {
                if let Some(t) = cur.get_mut(next).and_then(Item::as_table_mut) {
                    let old = prefix_of(Some(t.decor()));
                    t.decor_mut().set_prefix(format!("{notes}{old}"));
                }
            } else if let Some(mut k) = cur.key_mut(next) {
                let old = prefix_of(Some(k.leaf_decor()));
                k.leaf_decor_mut().set_prefix(format!("{notes}{old}"));
            }
        }
        None => {
            let old = doc.trailing().as_str().unwrap_or("").to_string();
            doc.set_trailing(format!("{old}{notes}"));
        }
    }
    true
}

fn remove_plain(doc: &mut DocumentMut, path: &str) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((leaf, parents)) = parts.split_last() else {
        return false;
    };
    let mut cur: &mut dyn TableLike = doc.as_table_mut();
    for p in parents {
        match cur.get_mut(p).and_then(Item::as_table_like_mut) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    cur.remove(leaf).is_some()
}

/// Drops tables the writer emptied, unless the user left their own lines above the header.
/// Deepest tables first, so a parent emptied by that also goes.
fn prune_empty(root: &mut Table, emptied: &BTreeSet<String>) {
    let mut paths: Vec<&String> = emptied.iter().filter(|p| !p.is_empty()).collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.matches('.').count()));
    for path in paths {
        let mut p = path.as_str();
        while !p.is_empty() {
            if !try_remove_table(root, p) {
                break;
            }
            p = parent(p);
        }
    }
}

fn try_remove_table(root: &mut Table, path: &str) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((leaf, parents)) = parts.split_last() else {
        return false;
    };
    let mut cur: &mut Table = root;
    for p in parents {
        match cur.get_mut(p).and_then(Item::as_table_mut) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    let removable = cur.get(leaf).and_then(Item::as_table).is_some_and(|t| {
        t.is_empty() && !has_user_lines(t.decor().prefix().and_then(|r| r.as_str()).unwrap_or(""))
    });
    if removable {
        cur.remove(leaf);
    }
    removable
}

// ---------------------------------------------------------------------------------------------
// Generated comments

/// Whether `line` is a generated comment: `##` followed by a space or the end of the line
/// (`###` banners belong to the user).
fn is_generated(line: &str) -> bool {
    let t = line.trim_end_matches(['\r', '\n']).trim_start();
    t == "##" || t.starts_with("## ")
}

fn has_user_lines(prefix: &str) -> bool {
    prefix
        .lines()
        .any(|l| !l.trim().is_empty() && !is_generated(l))
}

/// Rewrites a comment prefix: all generated lines (and the blank line directly above the first
/// run, when `blank_before` is set) are replaced by one fresh `block`, placed where the first run
/// was, or else directly above the item. `blank_before` makes the blank line above the block part
/// of the generated text (keys); table headers keep theirs as plain whitespace.
fn replace_generated(prefix: &str, block: &[String], blank_before: bool) -> String {
    let mut lines: Vec<&str> = prefix.split_inclusive('\n').collect();
    // A trailing piece without a newline is indentation of the item itself.
    let indent = match lines.last() {
        Some(l) if !l.ends_with('\n') => lines.pop().unwrap_or(""),
        _ => "",
    };
    let first_run = lines.iter().position(|l| is_generated(l));
    let (mut before, after): (Vec<&str>, Vec<&str>) = match first_run {
        Some(start) => (
            lines[..start].to_vec(),
            lines[start..]
                .iter()
                .copied()
                .filter(|l| !is_generated(l))
                .collect(),
        ),
        None => (lines, Vec::new()),
    };
    let mut absorbed = false;
    if blank_before && first_run.is_some() && before.last().is_some_and(|l| l.trim().is_empty()) {
        before.pop();
        absorbed = true;
    }
    let mut out: String = before.concat();
    if !block.is_empty() {
        if blank_before && (absorbed || (first_run.is_none() && before.is_empty())) {
            out.push('\n');
        }
        for l in block {
            if l.is_empty() {
                out.push_str("##\n");
            } else {
                out.push_str("## ");
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out.push_str(&after.concat());
    out.push_str(indent);
    out
}

fn prefix_of(d: Option<&toml_edit::Decor>) -> String {
    d.and_then(|d| d.prefix())
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string()
}

fn comments(doc: &mut DocumentMut, catalog: &Catalog, level: CommentLevel) {
    walk(doc.as_table_mut(), "", catalog, level);
}

fn walk(table: &mut Table, path: &str, catalog: &Catalog, level: CommentLevel) {
    let mut kv_index = 0usize;
    for (mut key, item) in table.iter_mut() {
        let name = key.get().to_string();
        let full = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}.{name}")
        };
        match item {
            Item::Value(_) => {
                let first = kv_index == 0;
                kv_index += 1;
                let block = if path.is_empty() && name == "schema_version" {
                    Some(if level == CommentLevel::None {
                        Vec::new()
                    } else {
                        text::notice()
                    })
                } else {
                    catalog
                        .field(&full)
                        .filter(|f| f.kind == FieldKind::Value)
                        .map(|f| text::key_block(f, level))
                };
                if let Some(block) = block {
                    let new = replace_generated(
                        &prefix_of(Some(key.leaf_decor())),
                        &block,
                        !first && !(path.is_empty()),
                    );
                    key.leaf_decor_mut().set_prefix(new);
                }
            }
            Item::Table(t) => {
                let has_kv = t.iter().any(|(_, i)| i.is_value());
                let shown = !t.is_dotted() && (has_kv || !t.is_implicit());
                if shown {
                    let block = if path.is_empty() {
                        catalog
                            .section(&name)
                            .map(|s| text::section_block(s, level))
                    } else {
                        catalog
                            .field(&full)
                            .filter(|f| f.kind == FieldKind::Group)
                            .map(|f| text::group_block(f, level))
                    };
                    if let Some(block) = block {
                        let new = replace_generated(&prefix_of(Some(t.decor())), &block, false);
                        t.decor_mut().set_prefix(new);
                    }
                }
                walk(t, &full, catalog, level);
            }
            _ => {}
        }
    }
}
