//! Sound playback. Decodes WAV bytes (from `.snd` files) once per key and
//! plays them through kira. MUGEN channel rules: `channel >= 0` means one
//! voice per (player, channel), a new sound replaces the running one;
//! `-1` plays on any free voice.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle, StreamingSoundSettings};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Panning, Tween};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("audio backend: {0}")]
    Backend(String),
    #[error("decode: {0}")]
    Decode(String),
}

/// Identifies a decoded sample in the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoundKey {
    /// Which `.snd` file: a player's data index, or `u32::MAX` for common/fight sounds.
    pub source: u32,
    pub group: i32,
    pub number: i32,
}

pub struct Audio {
    manager: AudioManager<DefaultBackend>,
    cache: HashMap<SoundKey, Option<Arc<StaticSoundData>>>,
    channels: HashMap<(usize, i32), StaticSoundHandle>,
    master: f32,
    music: Option<(
        std::path::PathBuf,
        StreamingSoundHandle<kira::sound::FromFileError>,
    )>,
    music_volume: f32,
}

impl Audio {
    pub fn new() -> Result<Self, Error> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(Self {
            manager,
            cache: HashMap::new(),
            channels: HashMap::new(),
            master: 1.0,
            music: None,
            music_volume: 1.0,
        })
    }

    pub fn set_music_volume(&mut self, v: f32) {
        self.music_volume = v.clamp(0.0, 1.0);
        if let Some((_, h)) = self.music.as_mut() {
            h.set_volume(
                Decibels::from(volume_db(self.music_volume)),
                Tween::default(),
            );
        }
    }

    /// Stream a music file (mp3 / ogg / flac / wav) in a loop. The same file
    /// keeps playing across screens; `None` stops music.
    pub fn play_music(&mut self, path: Option<&std::path::Path>, volume: f32, looping: bool) {
        let Some(path) = path else {
            self.stop_music();
            return;
        };
        if let Some((cur, h)) = self.music.as_ref()
            && cur == path
            && h.state() != PlaybackState::Stopped
        {
            return;
        }
        self.stop_music();
        let data = match StreamingSoundData::from_file(path) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("music {}: {e}", path.display());
                return;
            }
        };
        let vol = Decibels::from(volume_db(self.music_volume * volume.clamp(0.0, 2.0)));
        let mut data = data.with_settings(StreamingSoundSettings::new().volume(vol));
        if looping {
            data = data.loop_region(..);
        }
        match self.manager.play(data) {
            Ok(h) => self.music = Some((path.to_path_buf(), h)),
            Err(e) => log::warn!("music {}: {e}", path.display()),
        }
    }

    pub fn stop_music(&mut self) {
        if let Some((_, mut h)) = self.music.take() {
            h.stop(Tween::default());
        }
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.clamp(0.0, 1.0);
    }

    /// Decode and remember a sample. Returns false when the bytes are not playable.
    pub fn load(&mut self, key: SoundKey, wav: &[u8]) -> bool {
        if let Some(e) = self.cache.get(&key) {
            return e.is_some();
        }
        let data = StaticSoundData::from_cursor(Cursor::new(wav.to_vec()))
            .ok()
            .map(Arc::new);
        let ok = data.is_some();
        self.cache.insert(key, data);
        ok
    }

    pub fn is_loaded(&self, key: SoundKey) -> bool {
        self.cache.get(&key).is_some_and(|d| d.is_some())
    }

    /// Play a loaded sample. `volume` 0..=100 like PlaySnd, `pan` -1..=1, `channel` -1 = free voice.
    pub fn play(
        &mut self,
        key: SoundKey,
        player: usize,
        channel: i32,
        volume: i32,
        pan: f32,
        looping: bool,
    ) {
        let Some(Some(data)) = self.cache.get(&key).cloned() else {
            return;
        };
        let amp = (volume.clamp(0, 100) as f32 / 100.0) * self.master;
        let db = if amp <= 0.0001 {
            -60.0
        } else {
            20.0 * libm_log10(amp)
        };
        let mut settings = StaticSoundSettings::new()
            .volume(Decibels(db))
            .panning(Panning(pan.clamp(-1.0, 1.0)));
        if looping {
            settings = settings.loop_region(..);
        }
        let sound = data.as_ref().clone().with_settings(settings);
        if channel >= 0 {
            if let Some(mut h) = self.channels.remove(&(player, channel)) {
                h.stop(Tween::default());
            }
        }
        match self.manager.play(sound) {
            Ok(h) => {
                if channel >= 0 {
                    self.channels.insert((player, channel), h);
                }
            }
            Err(e) => log::warn!("play: {e}"),
        }
    }

    pub fn stop_channel(&mut self, player: usize, channel: i32) {
        if channel < 0 {
            let keys: Vec<(usize, i32)> = self
                .channels
                .keys()
                .filter(|k| k.0 == player)
                .copied()
                .collect();
            for k in keys {
                if let Some(mut h) = self.channels.remove(&k) {
                    h.stop(Tween::default());
                }
            }
        } else if let Some(mut h) = self.channels.remove(&(player, channel)) {
            h.stop(Tween::default());
        }
    }

    /// Drop handles of finished sounds.
    pub fn gc(&mut self) {
        self.channels
            .retain(|_, h| h.state() != PlaybackState::Stopped);
    }
}

fn libm_log10(x: f32) -> f32 {
    // audio is presentation-only; std is fine here but keep the sim's convention
    x.log10()
}

/// Linear 0..=1 (or above) volume to decibels; 0 is silent.
fn volume_db(v: f32) -> f32 {
    if v <= 0.0001 {
        -80.0
    } else {
        20.0 * libm::log10f(v)
    }
}
