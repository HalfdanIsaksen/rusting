# ADR-020: Audio layers

**Status:** Proposed (feat/audio-layers branch, cut from feat/asset-library; merged with objects (ADR-018), motion tweens (ADR-019) and dockable panels (ADR-021) for 0.1.9).

## Context
Animators time drawings to sound: dialogue, music, effects. Users asked for a layer that holds an MP3 or WAV file, imported through File → Import and listed in the Library (ADR-017). The sound should play in sync with playback, play a short snippet while scrubbing, and show a waveform on the timeline. Exported video may stay silent for now.

Core crates must stay free of decoders, devices and threads, and every document change must stay an undoable command.

## Options
| | Layer kind + library sound (chosen) | Sound keys on a drawing track | Document-wide soundtrack |
|---|---|---|---|
| Several sounds, each with its own start | Yes, one per layer | Yes | No |
| Mute, lock, reorder, rename, group, delete | Existing layer commands | Mixed with drawing keys | New UI |
| Fits `Keyframe` (shared with the objects branch) | Untouched | New variant, merge conflict | Untouched |
| One file stored once however often it is used | Yes (asset pool) | Yes | Yes |

## Decision
- **Model.**
  - `AssetKind::Audio(AudioData)` (`document::assets`). `AudioData` holds the file bytes as imported (`Arc<[u8]>`, base64 in files), `AudioFormat` (`Mp3` or `Wav`), sample rate, channels, `duration` in seconds and `peaks`: the loudest sample of every 1/100 s, over all channels, as `u8` (255 = full scale). The peaks are computed once on import, so the core never decodes.
  - `scene::Layer` gets `kind: LayerKind { Drawing, Audio(AudioClip) }` (`#[serde(default)]`, left out of files for drawing layers). `AudioClip { asset: AssetId, start: FrameIndex, volume: f32 }`.
  - An audio layer keeps an empty track, so "every layer has a track" still holds. `evaluate` skips audio layers.
  - `Document::audio_at(frame)` lists the clips sounding at a frame (from `start` for `ceil(duration × fps)` frames) with the offset into the sound in seconds and the volume. A hidden layer, or a layer in a hidden group, is muted. `Document::used_assets` counts an audio layer's sound, so the library refuses to delete it.
- **Commands** (`editor::commands::audio`).
  - `AddAudioLayer` (asset must be a library sound) and `SetAudioClip` (start, volume or sound; refused on locked layers; edits in one gesture merge into one undo step).
  - Drawing commands (`editable_layer`) and `AddKeyframe` refuse audio layers with `CommandError::AudioLayer`.
  - `DuplicateFrame`, `InsertFrames`, `RemoveFrames` and `MoveFrames` move clip starts with the frames, with the same mapping as keys and tags, and restore the saved starts on undo. A clip starting inside removed frames starts where they were. A clip start never blocks `MoveFrames`.
- **Import.** `apps/desktop/src/import.rs` picks the file type by extension and decodes the whole file once with rodio's symphonia decoders to check it, measure it and build the peaks. `Session::import_audio` runs one `Batch`: `AddAsset` plus `AddAudioLayer` on top, starting at frame 1. The active layer stays as it was. `Session::place_audio` starts a library sound at a frame: dropping a library row (`ui::AssetDrag`) on the timeline frames, or the row's Add to Timeline menu item.
- **Playback** (`apps/desktop/src/audio.rs`).
  - `Session::playback_event` turns the playing flag and the current frame, sampled once per drawn frame, into `PlaybackEvent::{Started, Seeked, Stopped, Scrubbed}` (`ui::playback::PlaybackWatch`). Moving forward by more than a quarter second (at least 2 frames) between two drawn frames, or backwards, counts as a seek, which catches loops and playhead clicks.
  - `AudioPlayer` keeps one rodio `Player` per sounding clip while playing. Each drawn frame it compares `audio_at(frame)` with what plays (`plan`, unit-tested without a device): it starts new clips at their offset, stops ended, muted or removed ones, restarts moved ones and applies volume changes. Started and Seeked restart everything at the new frame.
  - Scrubbed plays a snippet of each clip at that frame: one frame long, at least 60 ms.
  - Seeking uses the decoder's `try_seek`, and falls back to decoding up to the offset.
  - The default output device opens on first use. If it can't be opened, a warning is logged once and sound stays off. Nothing panics.
- **Timeline.** An audio row shows its clip as a bar with the waveform drawn from the peaks, and a speaker icon in the layer column. Its eye button mutes. Dragging the clip moves its start live (one undo step). With an audio layer active, Tool Settings shows the clip's sound, volume and start frame.
- **Files.** Format version 13, after objects (11) and motion tweens (12). Library sounds are saved once, with bytes and peaks. Older builds must reject these files. v10 to v12 files load unchanged. Validation rejects an audio layer whose asset is missing (`MissingAudioAsset`) or not a sound (`NotAnAudioAsset`), an audio layer with keys (`AudioLayerHasKeys`), a volume outside 0..=1, and an empty sound (`EmptyAsset`).
- **Dependency.** `rodio` 0.23 (workspace dependency, used by `apps/desktop` only) with `default-features = false` and the features `playback` (cpal 0.18), `mp3` and `wav` (symphonia 0.5 MP3 bundle, RIFF and PCM), and `tracing` (rodio's errors go to our log). rodio decodes for both import and playback, so `symphonia` is not a direct dependency. Symphonia is MPL-2.0; it is used unmodified as a library.

## Consequences
- **Export is silent.** MP4, MOV and GIF exports have no sound. Adding it needs an AAC encoder and mixing the clips; it is left for later.
- The View → Preview window's own playback is silent.
- The sound is decoded again every time a player starts (playback start, seek, every scrub step). This is quick for in-memory MP3 and WAV, but long MP3s seek coarsely, so a clip started mid-way can be a few milliseconds off.
- Sync is per drawn frame. Playback timing (ADR-005) drops frames rather than slowing down, so sound and picture stay together. A seek is detected from a jump of more than a quarter second, so a very slow render may restart the sound needlessly.
- A clip plays to the end of its sound. Trimming, fades, offsets into the file and per-clip panning are not modelled. A layer holds one clip.
- Peaks at 100 per second take about 350 bytes per second of sound in the file, small next to the sound itself.
