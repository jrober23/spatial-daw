//! MIDI input devices.

use std::sync::Arc;

use crossbeam_channel::Sender;
use midir::{MidiInput, MidiInputConnection};

use daw_core::TrackId;

use crate::engine::{EngineCommand, SharedState};

#[derive(Debug, Clone)]
pub struct MidiPort {
    pub index: usize,
    pub name: String,
}

pub fn input_ports() -> Vec<MidiPort> {
    let Ok(input) = MidiInput::new("Spatial DAW") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .enumerate()
        .map(|(index, port)| MidiPort {
            index,
            name: input
                .port_name(port)
                .unwrap_or_else(|_| format!("MIDI input {index}")),
        })
        .collect()
}

pub struct MidiHub {
    _connection: MidiInputConnection<()>,
    sender: Sender<EngineCommand>,
}

impl MidiHub {
    /// Connects to a MIDI input port and forwards notes to the selected track.
    pub fn connect(
        port_index: usize,
        sender: Sender<EngineCommand>,
        shared: Arc<SharedState>,
    ) -> Result<Self, String> {
        let input = MidiInput::new("Spatial DAW").map_err(|e| e.to_string())?;
        let ports = input.ports();
        let port = ports
            .get(port_index)
            .ok_or_else(|| format!("MIDI port {port_index} disappeared"))?;
        let name = input
            .port_name(port)
            .unwrap_or_else(|_| "MIDI input".to_string());
        let tx = sender.clone();
        let activity = shared.clone();
        let connection = input
            .connect(
                port,
                &format!("Spatial DAW — {name}"),
                move |_stamp, message, _| {
                    let Some(&status) = message.first() else {
                        return;
                    };
                    let kind = status & 0xF0;
                    if message.len() < 3 {
                        return;
                    }
                    let data1 = message[1] & 0x7F;
                    let data2 = message[2] & 0x7F;
                    let track = TrackId(
                        activity
                            .midi_target
                            .load(std::sync::atomic::Ordering::Relaxed),
                    );
                    match kind {
                        0x90 if data2 > 0 => {
                            activity
                                .midi_activity
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let _ = tx.send(EngineCommand::NoteOn {
                                track,
                                pitch: data1,
                                velocity: data2,
                            });
                        }
                        0x80 | 0x90 => {
                            let _ = tx.send(EngineCommand::NoteOff {
                                track,
                                pitch: data1,
                            });
                        }
                        0xB0 if data1 == 123 => {
                            let _ = tx.send(EngineCommand::AllNotesOff);
                        }
                        _ => {}
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _connection: connection,
            sender,
        })
    }
}

impl Drop for MidiHub {
    fn drop(&mut self) {
        let _ = self.sender.send(EngineCommand::AllNotesOff);
    }
}
