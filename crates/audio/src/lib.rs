//! Sound playback. Decodes WAV bytes (from `.snd` files) once per key and
//! plays them through kira. MUGEN channel rules: `channel >= 0` means one
//! voice per (player, channel), a new sound replaces the running one;
//! `-1` plays on any free voice.
//!
//! Samples are cached per *bank*: every loaded fighter (and each fight's
//! common and announcer sounds) gets a fresh [`BankId`] from [`new_bank`], so
//! a group/number never resolves to the `.snd` of a fighter from an earlier
//! fight. [`Audio::release_fight`] stops every fight voice and forgets those
//! banks when a fight ends or is replaced.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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

/// Owner of a set of samples: one loaded `.snd` (a fighter, a fight's
/// common.snd / fight.snd, or the screenpack's system.snd).
pub type BankId = u32;

/// The screenpack's menu sounds: outlive fights, never released.
pub const SYSTEM_BANK: BankId = u32::MAX;

static NEXT_BANK: AtomicU32 = AtomicU32::new(1);

/// A bank id no earlier load used (process-wide), never [`SYSTEM_BANK`].
pub fn new_bank() -> BankId {
    loop {
        let b = NEXT_BANK.fetch_add(1, Ordering::Relaxed);
        if b != SYSTEM_BANK && b != 0 {
            return b;
        }
    }
}

/// Identifies a decoded sample in the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoundKey {
    /// Which `.snd` the sample came from (see [`new_bank`]).
    pub bank: BankId,
    pub group: i32,
    pub number: i32,
}

/// Decoded samples by bank / group / number. Needs no audio device.
#[derive(Default)]
pub struct SampleCache {
    map: HashMap<SoundKey, Option<Arc<StaticSoundData>>>,
}

impl SampleCache {
    /// Decode and remember a sample. Returns false when the bytes are not playable.
    pub fn load(&mut self, key: SoundKey, wav: &[u8]) -> bool {
        if let Some(e) = self.map.get(&key) {
            return e.is_some();
        }
        let data = StaticSoundData::from_cursor(Cursor::new(wav.to_vec()))
            .ok()
            .map(Arc::new);
        let ok = data.is_some();
        self.map.insert(key, data);
        ok
    }

    pub fn is_loaded(&self, key: SoundKey) -> bool {
        self.map.get(&key).is_some_and(|d| d.is_some())
    }

    pub fn get(&self, key: SoundKey) -> Option<&Arc<StaticSoundData>> {
        self.map.get(&key).and_then(|d| d.as_ref())
    }

    /// Forget every sample of the banks `drop` picks.
    pub fn release(&mut self, drop: impl Fn(BankId) -> bool) {
        self.map.retain(|k, _| !drop(k.bank));
    }

    /// Banks with at least one cached entry.
    pub fn banks(&self) -> Vec<BankId> {
        let mut v: Vec<BankId> = self.map.keys().map(|k| k.bank).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// A playing sound and the bank it came from.
struct Voice {
    bank: BankId,
    handle: StaticSoundHandle,
}

pub struct Audio {
    manager: AudioManager<DefaultBackend>,
    cache: SampleCache,
    /// `channel >= 0` voices by (player, channel).
    channels: HashMap<(usize, i32), Voice>,
    /// `channel -1` voices, kept so a fight change can stop them.
    free: Vec<Voice>,
    master: f32,
    music: Option<(
        std::path::PathBuf,
        StreamingSoundHandle<kira::sound::FromFileError>,
    )>,
    music_volume: f32,
    /// Volume of the current track itself (system.def `bgm.volume`), 0..=2.
    music_track: f32,
}

impl Audio {
    pub fn new() -> Result<Self, Error> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(Self {
            manager,
            cache: SampleCache::default(),
            channels: HashMap::new(),
            free: Vec::new(),
            master: 1.0,
            music: None,
            music_volume: 1.0,
            music_track: 1.0,
        })
    }

    /// Music loudness relative to the master volume; applies to the playing track at once.
    pub fn set_music_volume(&mut self, v: f32) {
        self.music_volume = v.clamp(0.0, 1.0);
        self.refresh_music();
    }

    /// Linear music gain: master x music x track.
    fn music_gain(&self) -> f32 {
        self.master * self.music_volume * self.music_track
    }

    fn refresh_music(&mut self) {
        let db = Decibels::from(volume_db(self.music_gain()));
        if let Some((_, h)) = self.music.as_mut() {
            h.set_volume(db, Tween::default());
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
        self.music_track = volume.clamp(0.0, 2.0);
        // music inside a stage's .rgs streams from memory (its bytes are stored as they were)
        let data = if path.is_file() {
            StreamingSoundData::from_file(path)
        } else {
            match rugen_formats::vfs::read(path) {
                Ok(bytes) => StreamingSoundData::from_cursor(Cursor::new(bytes)),
                Err(e) => {
                    log::warn!("music {}: {e}", path.display());
                    return;
                }
            }
        };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                log::warn!("music {}: {e}", path.display());
                return;
            }
        };
        let vol = Decibels::from(volume_db(self.music_gain()));
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

    /// Overall loudness of sound effects and music; the playing track follows at once.
    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.clamp(0.0, 1.0);
        self.refresh_music();
    }

    /// Decode and remember a sample. Returns false when the bytes are not playable.
    pub fn load(&mut self, key: SoundKey, wav: &[u8]) -> bool {
        self.cache.load(key, wav)
    }

    pub fn is_loaded(&self, key: SoundKey) -> bool {
        self.cache.is_loaded(key)
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
        let Some(data) = self.cache.get(key).cloned() else {
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
            if let Some(mut v) = self.channels.remove(&(player, channel)) {
                v.handle.stop(Tween::default());
            }
        }
        match self.manager.play(sound) {
            Ok(handle) => {
                let v = Voice {
                    bank: key.bank,
                    handle,
                };
                if channel >= 0 {
                    self.channels.insert((player, channel), v);
                } else {
                    self.free.push(v);
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
                if let Some(mut v) = self.channels.remove(&k) {
                    v.handle.stop(Tween::default());
                }
            }
        } else if let Some(mut v) = self.channels.remove(&(player, channel)) {
            v.handle.stop(Tween::default());
        }
    }

    /// Stop every voice of a fight bank (anything but [`SYSTEM_BANK`]),
    /// keeping the decoded samples (a restarted round plays them again).
    pub fn stop_fight_voices(&mut self) {
        let fight = |v: &Voice| v.bank != SYSTEM_BANK;
        self.channels.retain(|_, v| {
            if fight(v) {
                v.handle.stop(Tween::default());
                false
            } else {
                true
            }
        });
        self.free.retain_mut(|v| {
            if fight(v) {
                v.handle.stop(Tween::default());
                false
            } else {
                true
            }
        });
    }

    /// The fight ended or is being replaced: stop its voices and forget the
    /// samples of every fighter / common / announcer bank.
    pub fn release_fight(&mut self) {
        self.stop_fight_voices();
        self.cache.release(|b| b != SYSTEM_BANK);
    }

    /// Banks with cached samples (for tests and diagnostics).
    pub fn cached_banks(&self) -> Vec<BankId> {
        self.cache.banks()
    }

    /// Drop handles of finished sounds.
    pub fn gc(&mut self) {
        self.channels
            .retain(|_, v| v.handle.state() != PlaybackState::Stopped);
        self.free
            .retain(|v| v.handle.state() != PlaybackState::Stopped);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny mono 8-bit PCM WAV of `n` samples of `v`.
    fn wav(n: usize, v: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&11025u32.to_le_bytes());
        b.extend_from_slice(&11025u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(n as u32).to_le_bytes());
        b.extend(std::iter::repeat_n(v, n));
        b
    }

    #[test]
    fn banks_are_unique_and_never_system() {
        let a = new_bank();
        let b = new_bank();
        assert_ne!(a, b);
        assert_ne!(a, SYSTEM_BANK);
    }

    /// Same group/number from two fighters: two entries, and releasing the
    /// first fight leaves only the system bank.
    #[test]
    fn same_group_number_in_two_banks_stay_apart() {
        let mut c = SampleCache::default();
        let (a, b) = (new_bank(), new_bank());
        let ka = SoundKey {
            bank: a,
            group: 0,
            number: 0,
        };
        let kb = SoundKey { bank: b, ..ka };
        let ks = SoundKey {
            bank: SYSTEM_BANK,
            ..ka
        };
        assert!(c.load(ka, &wav(100, 10)));
        assert!(c.load(kb, &wav(300, 200)));
        assert!(c.load(ks, &wav(50, 128)));
        let frames = |k| c.get(k).map(|d: &Arc<StaticSoundData>| d.frames.len());
        assert_eq!(frames(ka), Some(100));
        assert_eq!(frames(kb), Some(300), "b's sample, not a's");
        c.release(|bank| bank != SYSTEM_BANK);
        assert!(!c.is_loaded(ka) && !c.is_loaded(kb));
        assert_eq!(c.banks(), vec![SYSTEM_BANK]);
    }
}
