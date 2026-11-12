//! Writer tests. Golden files live in `tests/golden/`; regenerate with `AUDIGMA_BLESS=1`.

use std::path::PathBuf;

use audigma_configurator::draft::Draft;
use audigma_configurator::writer::{CommentLevel, WriteOptions, render, save, verify};
use serde_json::json;

const BASE: &str = "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n";

fn draft_from(text: &str, edits: &[(&str, serde_json::Value)]) -> Draft {
    let mut d = Draft::from_toml_str(text).unwrap();
    for (k, v) in edits {
        d.set(k, v).unwrap();
    }
    d
}

fn opts(level: CommentLevel, only_changed: bool) -> WriteOptions {
    WriteOptions {
        only_changed,
        comment_level: level,
    }
}

/// Renders and checks the post-condition.
fn out(d: &Draft, existing: Option<&str>, o: &WriteOptions) -> String {
    let t = render(d, existing, o).unwrap();
    verify(d, &t).unwrap_or_else(|e| panic!("{e}\n---\n{t}"));
    t
}

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("AUDIGMA_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with AUDIGMA_BLESS=1"));
    assert_eq!(
        actual, want,
        "golden {name} differs (AUDIGMA_BLESS=1 to update)"
    );
}

fn sample() -> Draft {
    draft_from(
        BASE,
        &[
            ("resampler.phase", json!("minimum")),
            ("resampler.apodization", json!(18000)),
            ("runtime.dsp_threads", json!(4)),
            ("input.dsd.gain_db", json!(-1.0)),
            ("input.channel_positions", json!(["FR", "FL"])),
            ("gain.preamp_db", json!(-3.0)),
        ],
    )
}

// ---- new file ---------------------------------------------------------------------------------

#[test]
fn new_file_golden_per_level() {
    for (name, level) in [
        ("new_none.toml", CommentLevel::None),
        ("new_brief.toml", CommentLevel::Brief),
        ("new_full.toml", CommentLevel::Full),
    ] {
        golden(name, &out(&sample(), None, &opts(level, true)));
    }
}

#[test]
fn brief_and_full_text_for_enum_range_and_default_text_keys() {
    let d = draft_from(
        BASE,
        &[
            ("resampler.phase", json!("minimum")),
            ("gain.preamp_db", json!(-3.0)),
            ("input.channel_positions", json!(["FR", "FL"])),
        ],
    );
    let brief = out(&d, None, &opts(CommentLevel::Brief, true));
    assert!(brief.contains("## Filter phase response. Values: linear (default), minimum.\nphase"));
    assert!(brief.contains("Default: 0.0. Range: -120 to"));
    assert!(brief.contains("Default: synthesized from channels:"));
    let full = out(&d, None, &opts(CommentLevel::Full, true));
    assert!(full.contains("## - `linear`: "));
    assert!(full.contains("## Default: `linear`.\nphase"));
    assert!(full.contains("## Default: `0.0`. Range: -120 to 12.\npreamp_db"));
    assert!(full.contains("## Default: synthesized from channels:"));
    for line in full.lines().filter(|l| l.starts_with("##")) {
        assert!(line.chars().count() <= 80, "too wide: {line}");
    }
}

#[test]
fn only_changed_omits_defaults_and_explicit_writes_them() {
    let d = draft_from(
        BASE,
        &[
            ("gain.preamp_db", json!(0.0)),
            ("resampler.phase", json!("linear")),
            ("resampler.apodization", json!(18000)),
        ],
    );
    let lean = out(&d, None, &opts(CommentLevel::None, true));
    assert!(!lean.contains("preamp_db"), "{lean}");
    assert!(!lean.contains("phase"), "{lean}");
    assert!(lean.contains("apodization = 18000"));
    let explicit = out(&d, None, &opts(CommentLevel::None, false));
    assert!(explicit.contains("preamp_db = 0.0"), "{explicit}");
    assert!(explicit.contains("phase = \"linear\""));
}

#[test]
fn comment_levels_cycle_back_to_the_direct_rendering() {
    let d = sample();
    let direct = out(&d, None, &opts(CommentLevel::Full, true));
    let mut text = direct.clone();
    for level in [CommentLevel::Brief, CommentLevel::None, CommentLevel::Full] {
        text = out(&d, Some(&text), &opts(level, true));
    }
    assert_eq!(text, direct);
    // And `none` strips every generated line.
    let bare = out(&d, Some(&direct), &opts(CommentLevel::None, true));
    assert!(!bare.contains("##"));
    assert_eq!(bare, out(&d, None, &opts(CommentLevel::None, true)));
}

#[test]
fn saves_are_idempotent() {
    let d = sample();
    for level in [CommentLevel::None, CommentLevel::Brief, CommentLevel::Full] {
        let o = opts(level, true);
        let first = out(&d, None, &o);
        let second = out(&d, Some(&first), &o);
        assert_eq!(first, second, "{level:?}");
        // Touching nothing, then saving a loaded draft, also changes nothing.
        let loaded = Draft::from_toml_str(&first).unwrap();
        assert_eq!(out(&loaded, Some(&first), &o), first, "{level:?}");
    }
}

// ---- update in place --------------------------------------------------------------------------

const USER_FILE: &str = r#"# My DAC setup, edited by hand.
schema_version = 1

# Hand-written note about the target.
[target]
format = "pcm"   # trailing note stays
pcm_rate_hz = 192000
somethingunknown = 7

### Banner the user drew ###
[gain]
# why -2: matches the other setup
preamp_db   =   -2.0

[unknown_section]
## looks generated but the catalog does not know this table
x = 1
"#;

#[test]
fn update_leaves_untouched_keys_and_user_lines_alone() {
    // The unknown keys make `Settings` reject the file, so use a parseable variant for the
    // draft and the full text for the writer.
    let parseable = USER_FILE.replace("somethingunknown = 7\n", "").replace(
        "[unknown_section]\n## looks generated but the catalog does not know this table\nx = 1\n",
        "",
    );
    let d = draft_from(&parseable, &[("resampler.phase", json!("minimum"))]);
    let o = opts(CommentLevel::Brief, true);
    let t = render(&d, Some(USER_FILE), &o).unwrap();
    // Untouched key: byte-for-byte, odd spacing included.
    assert!(t.contains("preamp_db   =   -2.0\n"), "{t}");
    assert!(t.contains("format = \"pcm\"   # trailing note stays\n"));
    // User lines, banner, unknown keys and the unknown table's `##` line survive.
    for keep in [
        "# My DAC setup, edited by hand.\n",
        "# Hand-written note about the target.\n",
        "# why -2: matches the other setup\n",
        "### Banner the user drew ###\n",
        "somethingunknown = 7\n",
        "[unknown_section]\n## looks generated but the catalog does not know this table\nx = 1\n",
    ] {
        assert!(t.contains(keep), "lost {keep:?}\n{t}");
    }
    // The user's line stays above the generated block.
    assert!(
        t.contains("# Hand-written note about the target.\n## The format audigma produces"),
        "{t}"
    );
    // The new key is appended in its own section.
    assert!(t.contains("[resampler]\n## Filter phase response."), "{t}");
    assert!(t.contains("phase = \"minimum\""));
    // Order of the existing sections did not change.
    let pos = |s: &str| t.find(s).unwrap();
    assert!(pos("[target]") < pos("[gain]") && pos("[gain]") < pos("[resampler]"));
}

#[test]
fn touched_key_keeps_decor_and_new_value_wins() {
    let text = "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000  # keep me\n";
    let d = draft_from(text, &[("target.pcm_rate_hz", json!(384000))]);
    let t = out(&d, Some(text), &opts(CommentLevel::None, true));
    assert!(t.contains("pcm_rate_hz = 384000  # keep me\n"), "{t}");
}

#[test]
fn key_reset_to_default_is_removed_but_user_lines_stay() {
    let text = "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n\n[resampler]\n# my phase note\nphase = \"minimum\"\n";
    let d = draft_from(text, &[("resampler.phase", json!("linear"))]);
    let t = out(&d, Some(text), &opts(CommentLevel::None, true));
    assert!(!t.contains("phase ="), "{t}");
    assert!(t.contains("# my phase note"), "{t}");
    // Explicit mode writes the default instead.
    let t2 = out(&d, Some(text), &opts(CommentLevel::None, false));
    assert!(t2.contains("phase = \"linear\""), "{t2}");
}

#[test]
fn table_emptied_by_the_writer_goes_unless_the_user_commented_it() {
    let plain = "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n\n[resampler]\nphase = \"minimum\"\n";
    let d = draft_from(plain, &[("resampler.phase", json!("linear"))]);
    let t = out(&d, Some(plain), &opts(CommentLevel::None, true));
    assert!(!t.contains("[resampler]"), "{t}");

    let noted = plain.replace("[resampler]", "# keep this heading\n[resampler]");
    let d = draft_from(&noted, &[("resampler.phase", json!("linear"))]);
    let t = out(&d, Some(&noted), &opts(CommentLevel::None, true));
    assert!(t.contains("# keep this heading\n[resampler]"), "{t}");
}

#[test]
fn hand_edited_generated_lines_are_overwritten() {
    let d = sample();
    let o = opts(CommentLevel::Brief, true);
    let first = out(&d, None, &o);
    let tampered = first.replace(
        "## Filter phase response. Values: linear (default), minimum.",
        "## my edit of a generated line",
    );
    assert_ne!(tampered, first);
    assert_eq!(out(&d, Some(&tampered), &o), first);
}

#[test]
fn comment_level_does_not_depend_on_the_existing_file() {
    // A file with full comments, saved at brief, ends up brief; at none, bare.
    let d = sample();
    let full = out(&d, None, &opts(CommentLevel::Full, true));
    let brief = out(&d, Some(&full), &opts(CommentLevel::Brief, true));
    assert_eq!(brief, out(&d, None, &opts(CommentLevel::Brief, true)));
}

// ---- save ---------------------------------------------------------------------------------

#[test]
fn save_creates_backs_up_and_replaces_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/config.toml");
    let o = opts(CommentLevel::Brief, true);

    let d = sample();
    let r = save(&d, &path, &o).unwrap();
    assert!(r.created && r.backup.is_none() && !r.unchanged);
    let first = std::fs::read_to_string(&path).unwrap();
    Draft::from_toml_str(&first).unwrap();

    // Same draft again: nothing written, no backup.
    let r = save(&d, &path, &o).unwrap();
    assert!(r.unchanged && r.backup.is_none());

    // A change: backup of the old text, new text in place, no temp files left over.
    let mut d2 = Draft::from_toml_str(&first).unwrap();
    d2.set("runtime.dsp_threads", &json!(2)).unwrap();
    let r = save(&d2, &path, &o).unwrap();
    let bak = r.backup.expect("backup made");
    assert!(
        bak.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("config.toml.bak-")
    );
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), first);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("dsp_threads = 2")
    );
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn save_rejects_a_draft_that_disagrees_with_the_file() {
    // The file changed after the draft was loaded: an untouched key now differs.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, BASE.replace("192000", "96000")).unwrap();
    let d = draft_from(BASE, &[("resampler.phase", json!("minimum"))]);
    let err = save(&d, &path, &opts(CommentLevel::None, true)).unwrap_err();
    assert!(err.to_string().contains("resolve differently"), "{err}");
    // The file was not touched.
    assert!(std::fs::read_to_string(&path).unwrap().contains("96000"));
}
