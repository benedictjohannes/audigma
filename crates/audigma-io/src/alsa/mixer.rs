//! ALSA Hardware UAC2 mixer interface (`Design.md §1.8`, `Phase 5 Step 5.5`).
//!
//! Provides hardware-level volume attenuation and mute control on ALSA endpoints
//! (e.g. UAC2 DACs like JadeAudio JA11). Probes candidate controls in priority order
//! (`Master` > `PCM` > `Playback` > `Headphone` > `Speaker` > first match).

#[cfg(feature = "alsa")]
use alsa::Round;
#[cfg(feature = "alsa")]
use alsa::mixer::{MilliBel, Mixer, Selem, SelemChannelId, SelemId};
use audigma_engine::EngineError;

/// Standard candidate mixer controls probed in priority order (first match wins).
pub const CANDIDATE_MIXER_CONTROLS: &[&str] =
    &["Master", "PCM", "Playback", "Headphone", "Speaker"];

/// Resolves an ALSA device string (e.g. `hw:CARD=JA11,DEV=0` or `hw:1,0`)
/// to an ALSA card mixer identifier (e.g. `hw:1` or `hw:CARD=JA11`).
pub fn resolve_card_mixer_name(device_str: &str) -> String {
    if let Some(card_part) = device_str.strip_prefix("hw:CARD=") {
        let name = card_part.split(',').next().unwrap_or("").trim();
        format!("hw:CARD={name}")
    } else if let Some(stripped) = device_str.strip_prefix("hw:") {
        let card_id = stripped.split(',').next().unwrap_or("").trim();
        format!("hw:{card_id}")
    } else {
        device_str.to_string()
    }
}

/// Encapsulates an open ALSA hardware mixer bound to a specific playback control element.
pub struct AlsaHardwareMixer {
    #[cfg(feature = "alsa")]
    mixer: Mixer,
    element_name: String,
    element_index: u32,
    has_volume: bool,
    has_switch: bool,
    has_db: bool,
    min_mb: i64,
    max_mb: i64,
    min_vol: i64,
    max_vol: i64,
    #[cfg(feature = "alsa")]
    active_channel: SelemChannelId,
}

impl AlsaHardwareMixer {
    /// Opens the ALSA mixer for the specified device and binds to a playback volume control.
    ///
    /// If `configured_control` is `Some(name)`, attempts to bind strictly to that control element.
    /// Otherwise, probes `CANDIDATE_MIXER_CONTROLS` in priority order (`Master` > `PCM` >
    /// `Playback` > `Headphone` > `Speaker`), followed by the first available playback element.
    #[cfg(feature = "alsa")]
    pub fn open(
        device_or_card: &str,
        configured_control: Option<&str>,
    ) -> Result<Self, EngineError> {
        let mixer_card = resolve_card_mixer_name(device_or_card);
        let mixer = Mixer::new(&mixer_card, false)
            .or_else(|_| Mixer::new(device_or_card, false))
            .map_err(|e| {
                EngineError::Pipeline(format!(
                    "failed to open ALSA mixer on '{mixer_card}' (or '{device_or_card}'): {e}"
                ))
            })?;

        // 1. If explicit control name is configured, probe it directly
        let bound_elem_info = if let Some(custom) = configured_control {
            let id = SelemId::new(custom, 0);
            if let Some(selem) = mixer.find_selem(&id) {
                if selem.has_playback_volume() || selem.has_playback_switch() {
                    Some((custom.to_string(), 0))
                } else {
                    return Err(EngineError::Pipeline(format!(
                        "configured mixer control '{custom}' on '{mixer_card}' lacks playback capabilities"
                    )));
                }
            } else {
                return Err(EngineError::Pipeline(format!(
                    "configured mixer control '{custom}' not found on '{mixer_card}'"
                )));
            }
        } else {
            // 2. Candidate probe order: Master > PCM > Playback > Headphone > Speaker (first match wins)
            let mut matched = None;
            for &candidate in CANDIDATE_MIXER_CONTROLS {
                let id = SelemId::new(candidate, 0);
                if let Some(selem) = mixer.find_selem(&id) {
                    if selem.has_playback_volume() {
                        matched = Some((candidate.to_string(), 0));
                        break;
                    }
                }
            }

            // 3. Fallback: scan all selems on the card for any element with playback volume
            if matched.is_none() {
                for elem in mixer.iter() {
                    if let Some(selem) = Selem::new(elem) {
                        if selem.has_playback_volume() {
                            let id = selem.get_id();
                            if let Ok(name) = id.get_name() {
                                matched = Some((name.to_string(), id.get_index()));
                                break;
                            }
                        }
                    }
                }
            }

            matched
        };

        let (elem_name, elem_idx) = bound_elem_info.ok_or_else(|| {
            EngineError::Pipeline(format!(
                "no hardware playback volume mixer controls found on card '{mixer_card}'"
            ))
        })?;

        let id = SelemId::new(&elem_name, elem_idx);
        let selem = mixer.find_selem(&id).ok_or_else(|| {
            EngineError::Pipeline(format!(
                "internal error: element '{elem_name}' vanished during open"
            ))
        })?;

        let has_volume = selem.has_playback_volume();
        let has_switch = selem.has_playback_switch();
        let (min_mb_t, max_mb_t) = selem.get_playback_db_range();
        let (min_vol, max_vol) = selem.get_playback_volume_range();

        let has_db = max_mb_t.0 > min_mb_t.0;
        let min_mb = min_mb_t.0;
        let max_mb = max_mb_t.0;

        let mut active_channel = SelemChannelId::mono();
        for c in SelemChannelId::all().iter() {
            if selem.has_playback_channel(*c) {
                active_channel = *c;
                break;
            }
        }

        Ok(Self {
            mixer,
            element_name: elem_name,
            element_index: elem_idx,
            has_volume,
            has_switch,
            has_db,
            min_mb,
            max_mb,
            min_vol,
            max_vol,
            active_channel,
        })
    }

    #[cfg(not(feature = "alsa"))]
    pub fn open(
        _device_or_card: &str,
        _configured_control: Option<&str>,
    ) -> Result<Self, EngineError> {
        Err(EngineError::Pipeline(
            "ALSA feature is disabled in this build".to_string(),
        ))
    }

    /// Returns the name of the bound ALSA mixer control element (e.g. `"Master"`, `"PCM"`).
    pub fn element_name(&self) -> &str {
        &self.element_name
    }

    /// Returns whether this hardware mixer element supports volume adjustment.
    pub fn has_volume(&self) -> bool {
        self.has_volume
    }

    /// Returns whether this hardware mixer element supports a mute switch.
    pub fn has_switch(&self) -> bool {
        self.has_switch
    }

    /// Sets the hardware playback volume in decibels (clamped to device limits).
    #[cfg(feature = "alsa")]
    pub fn set_volume_db(&mut self, db: f64) -> Result<(), EngineError> {
        if !self.has_volume {
            return Ok(());
        }

        let id = SelemId::new(&self.element_name, self.element_index);
        let selem = self.mixer.find_selem(&id).ok_or_else(|| {
            EngineError::Pipeline(format!("mixer element '{}' not found", self.element_name))
        })?;

        if self.has_db {
            let target_mb = MilliBel::from_db(db as f32);
            let clamped_mb = MilliBel(target_mb.0.clamp(self.min_mb, self.max_mb));
            selem
                .set_playback_db_all(clamped_mb, Round::Floor)
                .map_err(|e| {
                    EngineError::Pipeline(format!(
                        "failed to set ALSA playback dB on '{}': {e}",
                        self.element_name
                    ))
                })?;
        } else if self.max_vol > self.min_vol {
            let linear = 10.0_f64.powf(db / 20.0).clamp(0.0, 1.0);
            let raw = self.min_vol + (linear * (self.max_vol - self.min_vol) as f64).round() as i64;
            selem.set_playback_volume_all(raw).map_err(|e| {
                EngineError::Pipeline(format!(
                    "failed to set ALSA playback volume on '{}': {e}",
                    self.element_name
                ))
            })?;
        }

        Ok(())
    }

    #[cfg(not(feature = "alsa"))]
    pub fn set_volume_db(&mut self, _db: f64) -> Result<(), EngineError> {
        Ok(())
    }

    /// Reads the current hardware playback volume in decibels.
    #[cfg(feature = "alsa")]
    pub fn get_volume_db(&self) -> Result<f64, EngineError> {
        if !self.has_volume {
            return Ok(0.0);
        }

        let id = SelemId::new(&self.element_name, self.element_index);
        let selem = self.mixer.find_selem(&id).ok_or_else(|| {
            EngineError::Pipeline(format!("mixer element '{}' not found", self.element_name))
        })?;

        if self.has_db {
            let mb = selem
                .get_playback_vol_db(self.active_channel)
                .map_err(|e| {
                    EngineError::Pipeline(format!(
                        "failed to read ALSA playback dB on '{}': {e}",
                        self.element_name
                    ))
                })?;
            Ok(mb.to_db() as f64)
        } else if self.max_vol > self.min_vol {
            let raw = selem
                .get_playback_volume(self.active_channel)
                .map_err(|e| {
                    EngineError::Pipeline(format!(
                        "failed to read ALSA playback volume on '{}': {e}",
                        self.element_name
                    ))
                })?;
            let norm = (raw - self.min_vol) as f64 / (self.max_vol - self.min_vol) as f64;
            if norm <= 1e-6 {
                Ok(-120.0)
            } else {
                Ok(20.0 * norm.log10())
            }
        } else {
            Ok(0.0)
        }
    }

    #[cfg(not(feature = "alsa"))]
    pub fn get_volume_db(&self) -> Result<f64, EngineError> {
        Ok(0.0)
    }

    /// Sets the hardware mute state.
    #[cfg(feature = "alsa")]
    pub fn set_mute(&mut self, muted: bool) -> Result<(), EngineError> {
        let id = SelemId::new(&self.element_name, self.element_index);
        let selem = self.mixer.find_selem(&id).ok_or_else(|| {
            EngineError::Pipeline(format!("mixer element '{}' not found", self.element_name))
        })?;

        if self.has_switch {
            selem
                .set_playback_switch_all(if muted { 0 } else { 1 })
                .map_err(|e| {
                    EngineError::Pipeline(format!(
                        "failed to set ALSA playback switch on '{}': {e}",
                        self.element_name
                    ))
                })?;
        } else if self.has_volume && muted {
            // Fallback for hardware without a dedicated mute switch: set to minimum volume
            self.set_volume_db(-120.0)?;
        }

        Ok(())
    }

    #[cfg(not(feature = "alsa"))]
    pub fn set_mute(&mut self, _muted: bool) -> Result<(), EngineError> {
        Ok(())
    }

    /// Reads the current hardware mute state.
    #[cfg(feature = "alsa")]
    pub fn is_muted(&self) -> Result<bool, EngineError> {
        if self.has_switch {
            let id = SelemId::new(&self.element_name, self.element_index);
            let selem = self.mixer.find_selem(&id).ok_or_else(|| {
                EngineError::Pipeline(format!("mixer element '{}' not found", self.element_name))
            })?;
            let sw = selem
                .get_playback_switch(self.active_channel)
                .map_err(|e| {
                    EngineError::Pipeline(format!(
                        "failed to read ALSA playback switch on '{}': {e}",
                        self.element_name
                    ))
                })?;
            Ok(sw == 0)
        } else {
            Ok(false)
        }
    }

    #[cfg(not(feature = "alsa"))]
    pub fn is_muted(&self) -> Result<bool, EngineError> {
        Ok(false)
    }

    /// Returns the hardware volume dB range `(min_db, max_db)` if supported.
    pub fn get_db_range(&self) -> Option<(f64, f64)> {
        if self.has_db {
            Some((self.min_mb as f64 / 100.0, self.max_mb as f64 / 100.0))
        } else {
            None
        }
    }
}

impl audigma_engine::HardwareVolumeBridge for AlsaHardwareMixer {
    fn set_volume_db(&mut self, db: f64) -> Result<(), EngineError> {
        self.set_volume_db(db)
    }

    fn set_mute(&mut self, muted: bool) -> Result<(), EngineError> {
        self.set_mute(muted)
    }

    fn get_db_range(&self) -> Option<(f64, f64)> {
        self.get_db_range()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_card_mixer_name() {
        assert_eq!(
            resolve_card_mixer_name("hw:CARD=JA11,DEV=0"),
            "hw:CARD=JA11"
        );
        assert_eq!(resolve_card_mixer_name("hw:CARD=K17,DEV=0"), "hw:CARD=K17");
        assert_eq!(resolve_card_mixer_name("hw:2,0"), "hw:2");
        assert_eq!(resolve_card_mixer_name("default"), "default");
    }

    #[test]
    fn test_candidate_mixer_controls_priority() {
        assert_eq!(
            CANDIDATE_MIXER_CONTROLS,
            &["Master", "PCM", "Playback", "Headphone", "Speaker"]
        );
    }

    #[test]
    fn test_mixer_nonexistent_device_fails_gracefully() {
        let res = AlsaHardwareMixer::open("hw:CARD=NONEXISTENT_XYZ,DEV=0", None);
        assert!(res.is_err());
    }
}
