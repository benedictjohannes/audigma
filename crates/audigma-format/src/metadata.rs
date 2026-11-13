//! Audio metadata abstraction and format-specific codecs (`Design.md §5`).
//!
//! Provides a container-agnostic representation for audio tags (Vorbis comment,
//! ID3v2, and RIFF INFO) and visual cover art, with bi-directional conversion
//! between FLAC, Sony DSF, and WAV container formats.

use id3::TagLike;
use serde::{Deserialize, Serialize};

/// Standardized picture / visual art types (matching FLAC and ID3v2 APIC specs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum PictureType {
    Other = 0,
    FileIcon = 1,
    OtherFileIcon = 2,
    #[default]
    CoverFront = 3,
    CoverBack = 4,
    LeafletPage = 5,
    Media = 6,
    LeadArtist = 7,
    Artist = 8,
    Conductor = 9,
    Band = 10,
    Composer = 11,
    Lyricist = 12,
    RecordingLocation = 13,
    DuringRecording = 14,
    DuringPerformance = 15,
    MovieScreenCapture = 16,
    ColouredFish = 17,
    Illustration = 18,
    BandLogo = 19,
    PublisherLogo = 20,
}

impl PictureType {
    /// Converts a raw u8 value to `PictureType`. Unknown values map to `Other`.
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::FileIcon,
            2 => Self::OtherFileIcon,
            3 => Self::CoverFront,
            4 => Self::CoverBack,
            5 => Self::LeafletPage,
            6 => Self::Media,
            7 => Self::LeadArtist,
            8 => Self::Artist,
            9 => Self::Conductor,
            10 => Self::Band,
            11 => Self::Composer,
            12 => Self::Lyricist,
            13 => Self::RecordingLocation,
            14 => Self::DuringRecording,
            15 => Self::DuringPerformance,
            16 => Self::MovieScreenCapture,
            17 => Self::ColouredFish,
            18 => Self::Illustration,
            19 => Self::BandLogo,
            20 => Self::PublisherLogo,
            _ => Self::Other,
        }
    }

    /// Converts to the standardized u8 code.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Embedded picture / visual metadata (e.g., album cover art).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Picture {
    pub picture_type: PictureType,
    pub mime_type: String,
    pub description: String,
    pub width: u32,
    pub height: u32,
    pub color_depth: u32,
    pub num_colors: u32,
    pub data: Vec<u8>,
}

/// Container-agnostic audio metadata container.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AudioMetadata {
    /// Track title.
    pub title: Option<String>,
    /// Track artist / performer.
    pub artist: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Album artist.
    pub album_artist: Option<String>,
    /// Composer.
    pub composer: Option<String>,
    /// Musical genre.
    pub genre: Option<String>,
    /// Release date / year.
    pub date: Option<String>,
    /// Track number.
    pub track_number: Option<u32>,
    /// Total tracks in album.
    pub track_total: Option<u32>,
    /// Disc number.
    pub disc_number: Option<u32>,
    /// Total discs in album.
    pub disc_total: Option<u32>,
    /// Comment / description.
    pub comment: Option<String>,
    /// Copyright notice.
    pub copyright: Option<String>,
    /// Additional custom / unmapped text tags (e.g. `ISRC`, `LABEL`, `DISCID`).
    pub custom_tags: Vec<(String, String)>,
    /// Embedded pictures / cover art.
    pub pictures: Vec<Picture>,
}

impl AudioMetadata {
    /// Returns true if this metadata container contains no tags and no pictures.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.album_artist.is_none()
            && self.composer.is_none()
            && self.genre.is_none()
            && self.date.is_none()
            && self.track_number.is_none()
            && self.track_total.is_none()
            && self.disc_number.is_none()
            && self.disc_total.is_none()
            && self.comment.is_none()
            && self.copyright.is_none()
            && self.custom_tags.is_empty()
            && self.pictures.is_empty()
    }

    /// Sets or updates a tag by case-insensitive key name.
    pub fn set_tag(&mut self, key: &str, value: String) {
        let trimmed_val = value.trim();
        if trimmed_val.is_empty() {
            return;
        }

        match key.to_ascii_uppercase().as_str() {
            "TITLE" => self.title = Some(trimmed_val.to_string()),
            "ARTIST" => self.artist = Some(trimmed_val.to_string()),
            "ALBUM" => self.album = Some(trimmed_val.to_string()),
            "ALBUMARTIST" | "ALBUM_ARTIST" | "ALBUM ARTIST" => {
                self.album_artist = Some(trimmed_val.to_string())
            }
            "COMPOSER" => self.composer = Some(trimmed_val.to_string()),
            "GENRE" => self.genre = Some(trimmed_val.to_string()),
            "DATE" | "YEAR" => self.date = Some(trimmed_val.to_string()),
            "TRACKNUMBER" | "TRACK" => {
                if let Some((n, total)) = parse_number_and_total(trimmed_val) {
                    self.track_number = Some(n);
                    if total.is_some() {
                        self.track_total = total;
                    }
                }
            }
            "TRACKTOTAL" | "TOTALTRACKS" => {
                if let Ok(n) = trimmed_val.parse::<u32>() {
                    self.track_total = Some(n);
                }
            }
            "DISCNUMBER" | "DISC" => {
                if let Some((n, total)) = parse_number_and_total(trimmed_val) {
                    self.disc_number = Some(n);
                    if total.is_some() {
                        self.disc_total = total;
                    }
                }
            }
            "DISCTOTAL" | "TOTALDISCS" => {
                if let Ok(n) = trimmed_val.parse::<u32>() {
                    self.disc_total = Some(n);
                }
            }
            "COMMENT" | "DESCRIPTION" => self.comment = Some(trimmed_val.to_string()),
            "COPYRIGHT" => self.copyright = Some(trimmed_val.to_string()),
            _ => {
                // Keep custom / unknown tags, deduplicating case-insensitively
                if let Some(pos) = self
                    .custom_tags
                    .iter()
                    .position(|(k, _)| k.eq_ignore_ascii_case(key))
                {
                    self.custom_tags[pos].1 = trimmed_val.to_string();
                } else {
                    self.custom_tags
                        .push((key.to_string(), trimmed_val.to_string()));
                }
            }
        }
    }

    /// Returns a list of `(KEY, VALUE)` tuples suitable for Vorbis comments in FLAC.
    pub fn vorbis_comments(&self) -> Vec<(String, String)> {
        let mut list = Vec::new();
        if let Some(ref t) = self.title {
            list.push(("TITLE".into(), t.clone()));
        }
        if let Some(ref a) = self.artist {
            list.push(("ARTIST".into(), a.clone()));
        }
        if let Some(ref alb) = self.album {
            list.push(("ALBUM".into(), alb.clone()));
        }
        if let Some(ref aa) = self.album_artist {
            list.push(("ALBUMARTIST".into(), aa.clone()));
        }
        if let Some(ref c) = self.composer {
            list.push(("COMPOSER".into(), c.clone()));
        }
        if let Some(ref g) = self.genre {
            list.push(("GENRE".into(), g.clone()));
        }
        if let Some(ref d) = self.date {
            list.push(("DATE".into(), d.clone()));
        }
        if let Some(tn) = self.track_number {
            list.push(("TRACKNUMBER".into(), tn.to_string()));
        }
        if let Some(tt) = self.track_total {
            list.push(("TRACKTOTAL".into(), tt.to_string()));
        }
        if let Some(dn) = self.disc_number {
            list.push(("DISCNUMBER".into(), dn.to_string()));
        }
        if let Some(dt) = self.disc_total {
            list.push(("DISCTOTAL".into(), dt.to_string()));
        }
        if let Some(ref cm) = self.comment {
            list.push(("COMMENT".into(), cm.clone()));
        }
        if let Some(ref cr) = self.copyright {
            list.push(("COPYRIGHT".into(), cr.clone()));
        }
        for (k, v) in &self.custom_tags {
            list.push((k.to_ascii_uppercase(), v.clone()));
        }
        list
    }

    /// Returns a list of `([u8; 4], String)` tuples suitable for WAV RIFF `LIST/INFO` chunk.
    pub fn riff_info_tags(&self) -> Vec<([u8; 4], String)> {
        let mut list = Vec::new();
        if let Some(ref t) = self.title {
            list.push((*b"INAM", t.clone()));
        }
        if let Some(ref a) = self.artist {
            list.push((*b"IART", a.clone()));
        }
        if let Some(ref alb) = self.album {
            list.push((*b"IPRD", alb.clone()));
        }
        if let Some(ref d) = self.date {
            list.push((*b"ICRD", d.clone()));
        }
        if let Some(ref g) = self.genre {
            list.push((*b"IGNR", g.clone()));
        }
        if let Some(ref cm) = self.comment {
            list.push((*b"ICMT", cm.clone()));
        }
        if let Some(tn) = self.track_number {
            if let Some(tt) = self.track_total {
                list.push((*b"IPRT", format!("{tn}/{tt}")));
            } else {
                list.push((*b"IPRT", tn.to_string()));
            }
        }
        if let Some(ref cr) = self.copyright {
            list.push((*b"ICOP", cr.clone()));
        }
        list.push((*b"ISFT", "audigma".to_string()));
        list
    }

    /// Converts this metadata into an `id3::Tag` (ID3v2.4).
    pub fn to_id3_tag(&self) -> id3::Tag {
        let mut tag = id3::Tag::new();
        if let Some(ref t) = self.title {
            tag.set_title(t);
        }
        if let Some(ref a) = self.artist {
            tag.set_artist(a);
        }
        if let Some(ref alb) = self.album {
            tag.set_album(alb);
        }
        if let Some(ref aa) = self.album_artist {
            tag.set_album_artist(aa);
        }
        if let Some(ref c) = self.composer {
            tag.add_frame(id3::frame::ExtendedText {
                description: "COMPOSER".into(),
                value: c.clone(),
            });
            // Also standard TCOM frame
            tag.set_text("TCOM", c);
        }
        if let Some(ref g) = self.genre {
            tag.set_genre(g);
        }
        if let Some(ref d) = self.date {
            tag.set_date_recorded(id3::Timestamp {
                year: d.get(..4).and_then(|y| y.parse::<i32>().ok()).unwrap_or(0),
                month: None,
                day: None,
                hour: None,
                minute: None,
                second: None,
            });
            tag.set_text("TDRC", d);
        }
        if let Some(tn) = self.track_number {
            tag.set_track(tn);
        }
        if let Some(tt) = self.track_total {
            tag.set_total_tracks(tt);
        }
        if let Some(dn) = self.disc_number {
            tag.set_disc(dn);
        }
        if let Some(dt) = self.disc_total {
            tag.set_total_discs(dt);
        }
        if let Some(ref cm) = self.comment {
            tag.add_frame(id3::frame::Comment {
                lang: "eng".into(),
                description: "".into(),
                text: cm.clone(),
            });
        }
        for (k, v) in &self.custom_tags {
            tag.add_frame(id3::frame::ExtendedText {
                description: k.clone(),
                value: v.clone(),
            });
        }
        for pic in &self.pictures {
            let id3_pt = match pic.picture_type {
                PictureType::Other => id3::frame::PictureType::Other,
                PictureType::FileIcon => id3::frame::PictureType::Icon,
                PictureType::OtherFileIcon => id3::frame::PictureType::OtherIcon,
                PictureType::CoverFront => id3::frame::PictureType::CoverFront,
                PictureType::CoverBack => id3::frame::PictureType::CoverBack,
                PictureType::LeafletPage => id3::frame::PictureType::Leaflet,
                PictureType::Media => id3::frame::PictureType::Media,
                PictureType::LeadArtist => id3::frame::PictureType::LeadArtist,
                PictureType::Artist => id3::frame::PictureType::Artist,
                PictureType::Conductor => id3::frame::PictureType::Conductor,
                PictureType::Band => id3::frame::PictureType::Band,
                PictureType::Composer => id3::frame::PictureType::Composer,
                PictureType::Lyricist => id3::frame::PictureType::Lyricist,
                PictureType::RecordingLocation => id3::frame::PictureType::RecordingLocation,
                PictureType::DuringRecording => id3::frame::PictureType::DuringRecording,
                PictureType::DuringPerformance => id3::frame::PictureType::DuringPerformance,
                PictureType::MovieScreenCapture => id3::frame::PictureType::ScreenCapture,
                PictureType::ColouredFish => id3::frame::PictureType::BrightFish,
                PictureType::Illustration => id3::frame::PictureType::Illustration,
                PictureType::BandLogo => id3::frame::PictureType::BandLogo,
                PictureType::PublisherLogo => id3::frame::PictureType::PublisherLogo,
            };
            tag.add_frame(id3::frame::Picture {
                mime_type: pic.mime_type.clone(),
                picture_type: id3_pt,
                description: pic.description.clone(),
                data: pic.data.clone(),
            });
        }
        tag
    }

    /// Constructs an `AudioMetadata` instance from an `id3::Tag`.
    pub fn from_id3_tag(tag: &id3::Tag) -> Self {
        let mut meta = Self::default();
        if let Some(t) = tag.title() {
            meta.title = Some(t.to_string());
        }
        if let Some(a) = tag.artist() {
            meta.artist = Some(a.to_string());
        }
        if let Some(alb) = tag.album() {
            meta.album = Some(alb.to_string());
        }
        if let Some(aa) = tag.album_artist() {
            meta.album_artist = Some(aa.to_string());
        }
        if let Some(g) = tag.genre() {
            meta.genre = Some(g.to_string());
        }
        if let Some(ts) = tag.date_recorded() {
            meta.date = Some(ts.year.to_string());
        } else if let Some(d) = tag.get("TDRC").and_then(|f| f.content().text()) {
            meta.date = Some(d.to_string());
        } else if let Some(d) = tag.get("TYER").and_then(|f| f.content().text()) {
            meta.date = Some(d.to_string());
        }
        if let Some(tn) = tag.track() {
            meta.track_number = Some(tn);
        }
        if let Some(tt) = tag.total_tracks() {
            meta.track_total = Some(tt);
        }
        if let Some(dn) = tag.disc() {
            meta.disc_number = Some(dn);
        }
        if let Some(dt) = tag.total_discs() {
            meta.disc_total = Some(dt);
        }
        if let Some(c) = tag.get("TCOM").and_then(|f| f.content().text()) {
            meta.composer = Some(c.to_string());
        }
        if let Some(cr) = tag.get("TCOP").and_then(|f| f.content().text()) {
            meta.copyright = Some(cr.to_string());
        }
        for cm in tag.comments() {
            if meta.comment.is_none() {
                meta.comment = Some(cm.text.clone());
            }
        }
        for ext in tag.extended_texts() {
            if ext.description.eq_ignore_ascii_case("COMPOSER") && meta.composer.is_none() {
                meta.composer = Some(ext.value.clone());
            } else {
                meta.set_tag(&ext.description, ext.value.clone());
            }
        }
        for pic in tag.pictures() {
            let pt_u8: u8 = pic.picture_type.into();
            meta.pictures.push(Picture {
                picture_type: PictureType::from_u8(pt_u8),
                mime_type: pic.mime_type.clone(),
                description: pic.description.clone(),
                width: 0,
                height: 0,
                color_depth: 0,
                num_colors: 0,
                data: pic.data.clone(),
            });
        }
        meta
    }

    /// Constructs an `AudioMetadata` instance from a Symphonia metadata revision.
    pub fn from_symphonia_revision(rev: &symphonia::core::meta::MetadataRevision) -> Self {
        let mut audio_meta = Self::default();

        let all_tags = rev
            .media
            .tags
            .iter()
            .chain(rev.per_track.iter().flat_map(|pt| &pt.metadata.tags));

        for tag in all_tags {
            let val_str = match &tag.raw.value {
                symphonia::core::meta::RawValue::String(s) => s.as_str().to_string(),
                symphonia::core::meta::RawValue::StringList(list) => list.join("; "),
                symphonia::core::meta::RawValue::UnsignedInt(u) => u.to_string(),
                symphonia::core::meta::RawValue::SignedInt(i) => i.to_string(),
                symphonia::core::meta::RawValue::Float(f) => f.to_string(),
                symphonia::core::meta::RawValue::Boolean(b) => b.to_string(),
                _ => continue,
            };

            if let Some(ref std_tag) = tag.std {
                match std_tag {
                    symphonia::core::meta::StandardTag::TrackTitle(s) => {
                        audio_meta.title = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::Artist(s) => {
                        audio_meta.artist = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::Album(s) => {
                        audio_meta.album = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::AlbumArtist(s) => {
                        audio_meta.album_artist = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::Composer(s) => {
                        audio_meta.composer = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::Genre(s) => {
                        audio_meta.genre = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::ReleaseDate(s)
                    | symphonia::core::meta::StandardTag::OriginalReleaseDate(s)
                    | symphonia::core::meta::StandardTag::RecordingDate(s) => {
                        audio_meta.date = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::ReleaseYear(y)
                    | symphonia::core::meta::StandardTag::OriginalReleaseYear(y)
                    | symphonia::core::meta::StandardTag::RecordingYear(y) => {
                        audio_meta.date = Some(y.to_string())
                    }
                    symphonia::core::meta::StandardTag::TrackNumber(n) => {
                        audio_meta.track_number = Some(*n as u32);
                    }
                    symphonia::core::meta::StandardTag::TrackTotal(n) => {
                        audio_meta.track_total = Some(*n as u32);
                    }
                    symphonia::core::meta::StandardTag::DiscNumber(n) => {
                        audio_meta.disc_number = Some(*n as u32);
                    }
                    symphonia::core::meta::StandardTag::DiscTotal(n) => {
                        audio_meta.disc_total = Some(*n as u32);
                    }
                    symphonia::core::meta::StandardTag::Comment(s) => {
                        audio_meta.comment = Some(s.as_str().to_string())
                    }
                    symphonia::core::meta::StandardTag::Copyright(s) => {
                        audio_meta.copyright = Some(s.as_str().to_string())
                    }
                    _ => audio_meta.set_tag(&tag.raw.key, val_str),
                }
            } else {
                audio_meta.set_tag(&tag.raw.key, val_str);
            }
        }

        let all_visuals = rev
            .media
            .visuals
            .iter()
            .chain(rev.per_track.iter().flat_map(|pt| &pt.metadata.visuals));

        for visual in all_visuals {
            let pt = visual
                .usage
                .map(|u| match u {
                    symphonia::core::meta::StandardVisualKey::FileIcon => PictureType::FileIcon,
                    symphonia::core::meta::StandardVisualKey::OtherIcon => {
                        PictureType::OtherFileIcon
                    }
                    symphonia::core::meta::StandardVisualKey::FrontCover => {
                        PictureType::CoverFront
                    }
                    symphonia::core::meta::StandardVisualKey::BackCover => {
                        PictureType::CoverBack
                    }
                    symphonia::core::meta::StandardVisualKey::Leaflet => {
                        PictureType::LeafletPage
                    }
                    symphonia::core::meta::StandardVisualKey::Media => PictureType::Media,
                    symphonia::core::meta::StandardVisualKey::LeadArtistPerformerSoloist => {
                        PictureType::LeadArtist
                    }
                    symphonia::core::meta::StandardVisualKey::ArtistPerformer => {
                        PictureType::Artist
                    }
                    symphonia::core::meta::StandardVisualKey::Conductor => {
                        PictureType::Conductor
                    }
                    symphonia::core::meta::StandardVisualKey::BandOrchestra => PictureType::Band,
                    symphonia::core::meta::StandardVisualKey::Composer => PictureType::Composer,
                    symphonia::core::meta::StandardVisualKey::Lyricist => PictureType::Lyricist,
                    symphonia::core::meta::StandardVisualKey::RecordingLocation => {
                        PictureType::RecordingLocation
                    }
                    symphonia::core::meta::StandardVisualKey::Performance => {
                        PictureType::DuringPerformance
                    }
                    symphonia::core::meta::StandardVisualKey::ScreenCapture => {
                        PictureType::MovieScreenCapture
                    }
                    symphonia::core::meta::StandardVisualKey::Illustration => {
                        PictureType::Illustration
                    }
                    symphonia::core::meta::StandardVisualKey::BandArtistLogo => {
                        PictureType::BandLogo
                    }
                    symphonia::core::meta::StandardVisualKey::PublisherStudioLogo => {
                        PictureType::PublisherLogo
                    }
                    _ => PictureType::Other,
                })
                .unwrap_or(PictureType::CoverFront);

            let (w, h) = visual
                .dimensions
                .map(|s| (s.width, s.height))
                .unwrap_or((0, 0));
            audio_meta.pictures.push(Picture {
                picture_type: pt,
                mime_type: visual.media_type.clone().unwrap_or_default(),
                description: "".into(),
                width: w,
                height: h,
                color_depth: 0,
                num_colors: 0,
                data: visual.data.to_vec(),
            });
        }
        audio_meta
    }

    /// Encodes a complete Vorbis Comment metadata block payload for FLAC (type 4).
    pub fn encode_flac_vorbis_comment_payload(&self, vendor: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        let vendor_bytes = vendor.as_bytes();
        buf.extend_from_slice(&(vendor_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(vendor_bytes);

        let comments = self.vorbis_comments();
        buf.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for (k, v) in comments {
            let entry = format!("{k}={v}");
            let entry_bytes = entry.as_bytes();
            buf.extend_from_slice(&(entry_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(entry_bytes);
        }
        buf
    }

    /// Encodes a Picture metadata block payload for FLAC (type 6).
    pub fn encode_flac_picture_payload(pic: &Picture) -> Vec<u8> {
        let mut buf = Vec::new();
        // 1. Picture type (32-bit BE)
        buf.extend_from_slice(&(pic.picture_type.as_u8() as u32).to_be_bytes());
        // 2. MIME type (32-bit BE length + bytes)
        let mime_bytes = pic.mime_type.as_bytes();
        buf.extend_from_slice(&(mime_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(mime_bytes);
        // 3. Description (32-bit BE length + bytes)
        let desc_bytes = pic.description.as_bytes();
        buf.extend_from_slice(&(desc_bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(desc_bytes);
        // 4. Width, height, color depth, num colors (32-bit BE each)
        buf.extend_from_slice(&pic.width.to_be_bytes());
        buf.extend_from_slice(&pic.height.to_be_bytes());
        buf.extend_from_slice(&pic.color_depth.to_be_bytes());
        buf.extend_from_slice(&pic.num_colors.to_be_bytes());
        // 5. Data (32-bit BE length + bytes)
        buf.extend_from_slice(&(pic.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&pic.data);
        buf
    }

    /// Encodes all FLAC metadata blocks (VORBIS_COMMENT + PICTUREs), with 4-byte block headers.
    /// `is_last` on the final block will be set to `true`.
    pub fn encode_flac_metadata_blocks(&self, vendor: &str) -> Vec<u8> {
        let mut blocks = Vec::new();

        // 1. VORBIS_COMMENT block (type 4)
        let vc_payload = self.encode_flac_vorbis_comment_payload(vendor);
        blocks.push((4u8, vc_payload));

        // 2. PICTURE blocks (type 6)
        for pic in &self.pictures {
            let pic_payload = Self::encode_flac_picture_payload(pic);
            blocks.push((6u8, pic_payload));
        }

        let mut output = Vec::new();
        let total = blocks.len();
        for (i, (block_type, payload)) in blocks.into_iter().enumerate() {
            let is_last = i == total - 1;
            let header_byte0 = (if is_last { 0x80 } else { 0x00 }) | (block_type & 0x7F);
            let len = payload.len() as u32;
            let len_bytes = [(len >> 16) as u8, (len >> 8) as u8, len as u8];

            output.push(header_byte0);
            output.extend_from_slice(&len_bytes);
            output.extend_from_slice(&payload);
        }
        output
    }

    /// Encodes a complete RIFF `LIST/INFO` chunk (including 8-byte chunk header and padding).
    pub fn encode_wav_riff_info_chunk(&self) -> Option<Vec<u8>> {
        let info_chunks = self.riff_info_tags();
        if info_chunks.is_empty() {
            return None;
        }

        let mut body = Vec::new();
        body.extend_from_slice(b"INFO");

        for (id, val) in info_chunks {
            // Null-terminated string
            let val_bytes = val.as_bytes();
            let subchunk_len = (val_bytes.len() + 1) as u32;

            body.extend_from_slice(&id);
            body.extend_from_slice(&subchunk_len.to_le_bytes());
            body.extend_from_slice(val_bytes);
            body.push(0x00); // null terminator

            // RIFF word alignment
            if subchunk_len % 2 != 0 {
                body.push(0x00);
            }
        }

        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"LIST");
        chunk.extend_from_slice(&(body.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&body);

        // RIFF word alignment for the entire LIST chunk
        if chunk.len() % 2 != 0 {
            chunk.push(0x00);
        }

        Some(chunk)
    }

    /// Encodes an ID3v2.4 tag as raw bytes to append to a Sony DSF file.
    pub fn encode_dsf_id3v2_tag(&self) -> Option<Vec<u8>> {
        if self.is_empty() {
            return None;
        }
        let tag = self.to_id3_tag();
        let mut buf = Vec::new();
        tag.write_to(&mut buf, id3::Version::Id3v24).ok()?;
        Some(buf)
    }
}

/// Parses RIFF `LIST/INFO` metadata from a seekable WAV stream.
pub fn parse_wav_riff_metadata<R: std::io::Read + std::io::Seek>(reader: &mut R) -> Option<AudioMetadata> {
    if reader.seek(std::io::SeekFrom::Start(12)).is_err() {
        return None;
    }
    let mut meta = AudioMetadata::default();
    let mut found = false;

    let mut chunk_hdr = [0u8; 8];
    while reader.read_exact(&mut chunk_hdr).is_ok() {
        let chunk_id = &chunk_hdr[0..4];
        let chunk_len = u32::from_le_bytes(chunk_hdr[4..8].try_into().unwrap()) as u64;

        if chunk_id == b"LIST" && chunk_len >= 4 {
            let mut form = [0u8; 4];
            if reader.read_exact(&mut form).is_ok() && &form == b"INFO" {
                let mut remaining = chunk_len - 4;
                let mut sub_hdr = [0u8; 8];
                while remaining >= 8 && reader.read_exact(&mut sub_hdr).is_ok() {
                    let sub_id = [sub_hdr[0], sub_hdr[1], sub_hdr[2], sub_hdr[3]];
                    let sub_len = u32::from_le_bytes(sub_hdr[4..8].try_into().unwrap()) as u64;
                    remaining -= 8;
                    if sub_len > remaining {
                        break;
                    }
                    let mut buf = vec![0u8; sub_len as usize];
                    if reader.read_exact(&mut buf).is_ok() {
                        let text = String::from_utf8_lossy(&buf).trim_matches('\0').trim().to_string();
                        match &sub_id {
                            b"INAM" => meta.title = Some(text),
                            b"IART" => meta.artist = Some(text),
                            b"IPRD" => meta.album = Some(text),
                            b"ICRD" => meta.date = Some(text),
                            b"IGNR" => meta.genre = Some(text),
                            b"ICMT" => meta.comment = Some(text),
                            b"IPRT" => {
                                if let Some((n, tot)) = parse_number_and_total(&text) {
                                    meta.track_number = Some(n);
                                    if tot.is_some() {
                                        meta.track_total = tot;
                                    }
                                }
                            }
                            b"ICOP" => meta.copyright = Some(text),
                            _ => {}
                        }
                        found = true;
                    }
                    remaining -= sub_len;
                    // 2-byte word alignment
                    if sub_len % 2 != 0 && remaining > 0 {
                        let mut pad = [0u8; 1];
                        let _ = reader.read_exact(&mut pad);
                        remaining -= 1;
                    }
                }
            } else {
                let _ = reader.seek(std::io::SeekFrom::Current((chunk_len - 4) as i64));
            }
        } else {
            let pad = if chunk_len % 2 != 0 { 1 } else { 0 };
            if reader.seek(std::io::SeekFrom::Current((chunk_len + pad) as i64)).is_err() {
                break;
            }
        }
    }

    if found && !meta.is_empty() {
        Some(meta)
    } else {
        None
    }
}

/// Helper parsing strings like "7", "07", "7/19".
fn parse_number_and_total(s: &str) -> Option<(u32, Option<u32>)> {
    let s = s.trim();
    if let Some((n_str, tot_str)) = s.split_once('/') {
        let n = n_str.trim().parse::<u32>().ok()?;
        let tot = tot_str.trim().parse::<u32>().ok();
        Some((n, tot))
    } else {
        let n = s.parse::<u32>().ok()?;
        Some((n, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_metadata_vorbis_and_riff() {
        let mut meta = AudioMetadata::default();
        meta.set_tag("TITLE", "Test Title".into());
        meta.set_tag("ARTIST", "Test Artist".into());
        meta.set_tag("ALBUM", "Test Album".into());
        meta.set_tag("TRACKNUMBER", "03/12".into());
        meta.set_tag("DATE", "2024".into());

        assert_eq!(meta.title.as_deref(), Some("Test Title"));
        assert_eq!(meta.artist.as_deref(), Some("Test Artist"));
        assert_eq!(meta.album.as_deref(), Some("Test Album"));
        assert_eq!(meta.track_number, Some(3));
        assert_eq!(meta.track_total, Some(12));
        assert_eq!(meta.date.as_deref(), Some("2024"));

        // Vorbis comments
        let vc = meta.vorbis_comments();
        assert!(vc.iter().any(|(k, v)| k == "TITLE" && v == "Test Title"));
        assert!(vc.iter().any(|(k, v)| k == "TRACKNUMBER" && v == "3"));
        assert!(vc.iter().any(|(k, v)| k == "TRACKTOTAL" && v == "12"));

        // RIFF INFO chunk
        let riff_bytes = meta.encode_wav_riff_info_chunk().unwrap();
        assert_eq!(&riff_bytes[0..4], b"LIST");
        assert_eq!(&riff_bytes[8..12], b"INFO");

        // FLAC metadata blocks
        let flac_blocks = meta.encode_flac_metadata_blocks("audigma");
        assert!(!flac_blocks.is_empty());
        // First byte has is_last bit (0x80) | block_type (4) => 0x84
        assert_eq!(flac_blocks[0], 0x84);
    }

    #[test]
    fn test_id3_roundtrip() {
        let mut meta = AudioMetadata::default();
        meta.title = Some("Cold Cold Heart".into());
        meta.artist = Some("Norah Jones".into());
        meta.album = Some("Come Away With Me".into());
        meta.track_number = Some(3);
        meta.date = Some("2012".into());
        meta.pictures.push(Picture {
            picture_type: PictureType::CoverFront,
            mime_type: "image/jpeg".into(),
            description: "Cover".into(),
            width: 500,
            height: 500,
            color_depth: 24,
            num_colors: 0,
            data: vec![0xFF, 0xD8, 0xFF, 0xE0, 0x01, 0x02],
        });

        let id3_bytes = meta.encode_dsf_id3v2_tag().unwrap();
        assert_eq!(&id3_bytes[0..3], b"ID3");

        let tag = id3::Tag::read_from2(&mut std::io::Cursor::new(&id3_bytes)).unwrap();
        let decoded = AudioMetadata::from_id3_tag(&tag);

        assert_eq!(decoded.title.as_deref(), Some("Cold Cold Heart"));
        assert_eq!(decoded.artist.as_deref(), Some("Norah Jones"));
        assert_eq!(decoded.album.as_deref(), Some("Come Away With Me"));
        assert_eq!(decoded.track_number, Some(3));
        assert_eq!(decoded.pictures.len(), 1);
        assert_eq!(decoded.pictures[0].picture_type, PictureType::CoverFront);
        assert_eq!(decoded.pictures[0].mime_type, "image/jpeg");
        assert_eq!(decoded.pictures[0].data, vec![0xFF, 0xD8, 0xFF, 0xE0, 0x01, 0x02]);
    }
}
