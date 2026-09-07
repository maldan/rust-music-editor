use std::collections::HashMap;

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

use super::node::{SeqNote, SEQ_OCTAVE_MAX, SEQ_OCTAVE_MIN, SEQ_STEPS};

pub struct MidiTrack {
    pub name: String,
    pub notes: Vec<SeqNote>,
    pub bars: u32,
    pub octave: i32,
}

pub fn parse_midi(bytes: &[u8]) -> Result<Vec<MidiTrack>, String> {
    let smf = Smf::parse(bytes).map_err(|e| e.to_string())?;
    let tpq = match smf.header.timing {
        Timing::Metrical(t) => u16::from(t) as u32,
        Timing::Timecode(..) => return Err("SMPTE MIDI timing is not supported".into()),
    };
    if tpq == 0 {
        return Err("MIDI file has zero ticks per beat".into());
    }
    let mut out = Vec::new();
    for (i, track) in smf.tracks.iter().enumerate() {
        let Some(parsed) = parse_track(track, tpq, i) else {
            continue;
        };
        out.push(parsed);
    }
    if out.is_empty() {
        return Err("No notes in MIDI file".into());
    }
    Ok(out)
}

fn parse_track(track: &[midly::TrackEvent], tpq: u32, index: usize) -> Option<MidiTrack> {
    let mut abs = 0u64;
    let mut name = None;
    let mut held: HashMap<(u8, u8), u64> = HashMap::new();
    let mut raw: Vec<(u64, u64, u8)> = Vec::new();
    for ev in track {
        abs = abs.saturating_add(u32::from(ev.delta) as u64);
        match ev.kind {
            TrackEventKind::Meta(MetaMessage::TrackName(bytes) | MetaMessage::InstrumentName(bytes)) => {
                if name.is_none() {
                    let s = String::from_utf8_lossy(bytes).trim().replace('\0', "");
                    if !s.is_empty() {
                        name = Some(s);
                    }
                }
            }
            TrackEventKind::Midi { channel, message } => {
                let ch = u8::from(channel);
                match message {
                    MidiMessage::NoteOn { key, vel } if u8::from(vel) > 0 => {
                        let pitch = u8::from(key);
                        if let Some(start) = held.insert((ch, pitch), abs) {
                            raw.push((start, abs, pitch));
                        }
                    }
                    MidiMessage::NoteOn { key, vel } if u8::from(vel) == 0 => {
                        let pitch = u8::from(key);
                        if let Some(start) = held.remove(&(ch, pitch)) {
                            raw.push((start, abs, pitch));
                        }
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        let pitch = u8::from(key);
                        if let Some(start) = held.remove(&(ch, pitch)) {
                            raw.push((start, abs, pitch));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for ((_, pitch), start) in held {
        if abs > start {
            raw.push((start, abs, pitch));
        }
    }
    let mut notes = Vec::new();
    let mut last_end = 0u64;
    let mut min_pitch = u8::MAX;
    for (start_tick, end_tick, pitch) in raw {
        let start = tick_to_step(start_tick, tpq);
        if start > u32::MAX as u64 {
            continue;
        }
        let end = tick_to_step(end_tick.max(start_tick), tpq).max(start + 1);
        let step = start as u32;
        let len = (end - start).clamp(1, u32::MAX as u64) as u32;
        notes.push(SeqNote {
            step,
            pitch,
            len,
            group: 0,
        });
        last_end = last_end.max(start + u64::from(len));
        min_pitch = min_pitch.min(pitch);
    }
    if notes.is_empty() {
        return None;
    }
    notes.sort_by_key(|n| (n.step, n.pitch));
    let bars = last_end
        .div_ceil(u64::from(SEQ_STEPS))
        .clamp(1, u32::MAX as u64) as u32;
    let octave = if min_pitch == u8::MAX {
        4
    } else {
        ((min_pitch as i32 / 12) - 1).clamp(SEQ_OCTAVE_MIN, SEQ_OCTAVE_MAX)
    };
    Some(MidiTrack {
        name: name.unwrap_or_else(|| format!("Track {}", index + 1)),
        notes,
        bars,
        octave,
    })
}

fn tick_to_step(tick: u64, tpq: u32) -> u64 {
    ((tick as u128 * 4 + u128::from(tpq) / 2) / u128::from(tpq)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::{num::u15, Format, Header, MidiMessage, Smf, TrackEvent, TrackEventKind};

    fn note_on(delta: u32, key: u8, vel: u8) -> TrackEvent<'static> {
        TrackEvent {
            delta: delta.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOn {
                    key: key.into(),
                    vel: vel.into(),
                },
            },
        }
    }

    fn note_off(delta: u32, key: u8) -> TrackEvent<'static> {
        TrackEvent {
            delta: delta.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOff {
                    key: key.into(),
                    vel: 0.into(),
                },
            },
        }
    }

    fn write(tracks: Vec<Vec<TrackEvent<'static>>>) -> Vec<u8> {
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Metrical(u15::new(96))),
            tracks,
        };
        let mut buf = Vec::new();
        smf.write(&mut buf).unwrap();
        buf
    }

    #[test]
    fn extracts_notes_per_track() {
        let tpq = 96u32;
        let sixteenth = tpq / 4;
        let t0 = vec![
            TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Lead")),
            },
            note_on(0, 60, 100),
            note_off(sixteenth, 60),
        ];
        let t1 = vec![
            note_on(0, 64, 100),
            note_off(sixteenth * 2, 64),
        ];
        let tracks = parse_midi(&write(vec![t0, t1])).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].name, "Lead");
        assert_eq!(tracks[0].notes, vec![SeqNote { step: 0, pitch: 60, len: 1, group: 0 }]);
        assert_eq!(tracks[1].notes, vec![SeqNote { step: 0, pitch: 64, len: 2, group: 0 }]);
        assert_eq!(tracks[0].bars, 1);
    }

    #[test]
    fn skips_empty_tracks_and_keeps_notes_past_eight_bars() {
        let tpq = 96u32;
        let sixteenth = tpq / 4;
        let far = sixteenth * SEQ_STEPS * 9;
        let empty = vec![TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Tempo")),
        }];
        let long = vec![
            note_on(0, 72, 90),
            note_off(sixteenth, 72),
            note_on(far - sixteenth, 48, 90),
            note_off(sixteenth, 48),
        ];
        let tracks = parse_midi(&write(vec![empty, long])).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].notes.len(), 2);
        assert_eq!(tracks[0].notes[1].pitch, 48);
        assert_eq!(tracks[0].notes[1].step, SEQ_STEPS * 9);
        assert_eq!(tracks[0].bars, 10);
        assert_eq!(tracks[0].octave, 3);
    }

    #[test]
    fn rejects_file_with_no_notes() {
        let empty = vec![TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Empty")),
        }];
        assert!(parse_midi(&write(vec![empty])).is_err());
    }
}
